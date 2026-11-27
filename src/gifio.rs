//! GIF 编解码：把 GIF 拆成「RGBA 帧 + 每帧自己的延迟」，处理完再按原延迟写回去。
//!
//! 为什么不用 `image` crate 的 `GifEncoder`：它的编码器只接受一个全局 delay，
//! 会把每帧原本不同的时长抹平成统一间隔。这里直接用底层的 `gif` crate，
//! 逐帧写自己的 delay，保住原动画的节奏。

use gif::{DisposalMethod, Encoder, Frame, Repeat};
use meme_generator_core::error::Error;

/// 一帧已经展开成整张画布的 RGBA 图（未预乘 alpha）。
pub struct RgbaFrame {
    pub width: u16,
    pub height: u16,
    /// 长度 = width * height * 4，行优先。
    pub pixels: Vec<u8>,
    /// 这一帧在播放时停留的毫秒数。
    pub delay_ms: u32,
}

/// 一个 GIF 的完整内容。
pub struct Animation {
    pub frames: Vec<RgbaFrame>,
    /// NETSCAPE2.0 的循环次数。0 = 无限循环。
    pub repeat: u16,
}

impl Animation {
    pub fn width(&self) -> u16 {
        self.frames.first().map_or(0, |f| f.width)
    }

    pub fn height(&self) -> u16 {
        self.frames.first().map_or(0, |f| f.height)
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

fn decode_err(msg: impl std::fmt::Display) -> Error {
    Error::ImageDecodeError(msg.to_string())
}

fn encode_err(msg: impl std::fmt::Display) -> Error {
    Error::ImageEncodeError(msg.to_string())
}

/// 解码 GIF。子帧（delta frame）会按 disposal 规则叠加到画布上，
/// 每一帧拿到的都是「此刻屏幕上完整的样子」，与浏览器渲染结果一致。
///
/// 之所以必须展开：优化过的 GIF 里第 N 帧常常只是画布上的一小块补丁，
/// 直接对它的矩形做镜像/旋转会算错位置，出现错位或黑块。
pub fn decode(data: &[u8]) -> Result<Animation, Error> {
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = options.read_info(data).map_err(decode_err)?;

    let canvas_w = decoder.width() as usize;
    let canvas_h = decoder.height() as usize;
    if canvas_w == 0 || canvas_h == 0 {
        return Err(decode_err("GIF 画布尺寸为 0"));
    }

    // gif 0.13 的 repeat() 直接返回 Repeat（无穷 / 有限 N 次），
    // 没有 NETSCAPE 扩展时它按「只播一次」处理，也就是 Finite(0)。
    let repeat = match decoder.repeat() {
        Repeat::Finite(n) => n,
        Repeat::Infinite => 0,
    };

    // 整张画布的合成缓冲，初始全透明。
    let mut canvas = vec![0u8; canvas_w * canvas_h * 4];
    // disposal=Previous（恢复上一帧）需要一份绘制前的备份。
    let mut saved: Option<Vec<u8>> = None;
    // 上一帧的矩形与 disposal，决定「下一帧绘制前要清哪块」。
    let mut pending: Option<(usize, usize, usize, usize, DisposalMethod)> = None;

    let mut frames: Vec<RgbaFrame> = Vec::new();

    while let Some(frame) = decoder.read_next_frame().map_err(decode_err)? {
        // 1) 先执行上一帧的 disposal。
        if let Some((px, py, pw, ph, dispose)) = pending.take() {
            match dispose {
                DisposalMethod::Background => clear_rect(&mut canvas, canvas_w, px, py, pw, ph),
                DisposalMethod::Previous => {
                    if let Some(backup) = saved.take() {
                        canvas.copy_from_slice(&backup);
                    }
                }
                _ => {}
            }
        }

        let frame_w = frame.width as usize;
        let frame_h = frame.height as usize;
        let fx = frame.left as usize;
        let fy = frame.top as usize;
        let dispose = frame.dispose;

        // 本帧画完后若要恢复上一帧，此刻先备份。
        if dispose == DisposalMethod::Previous {
            saved = Some(canvas.clone());
        }

        // 2) 把这一帧的子矩形贴到画布上（透明像素保留画布原值）。
        let (copy_w, copy_h) = (
            frame_w.min(canvas_w.saturating_sub(fx)),
            frame_h.min(canvas_h.saturating_sub(fy)),
        );
        let src = frame.buffer.as_ref();
        for row in 0..copy_h {
            let src_row = row * frame_w * 4;
            let dst_row = ((fy + row) * canvas_w + fx) * 4;
            for col in 0..copy_w {
                let s = src_row + col * 4;
                let d = dst_row + col * 4;
                if s + 4 > src.len() || d + 4 > canvas.len() {
                    continue;
                }
                let alpha = src[s + 3];
                match alpha {
                    0 => {}
                    255 => canvas[d..d + 4].copy_from_slice(&src[s..s + 4]),
                    a => {
                        // 半透明：source-over。GIF 很少出现，但合成器给的
                        // RGBA 在缩放/圆角素材里会有边缘半透明像素。
                        let af = a as u32;
                        for c in 0..3 {
                            let fg = src[s + c] as u32;
                            let bg = canvas[d + c] as u32;
                            canvas[d + c] = ((fg * af + bg * (255 - af)) / 255) as u8;
                        }
                        canvas[d + 3] = a.max(canvas[d + 3]);
                    }
                }
            }
        }

        pending = Some((fx, fy, frame_w, frame_h, dispose));

        // 3) 记录一张完整画布快照。delay 单位是 1/100 秒。
        frames.push(RgbaFrame {
            width: canvas_w as u16,
            height: canvas_h as u16,
            pixels: canvas.clone(),
            delay_ms: frame.delay as u32 * 10,
        });
    }

    if frames.is_empty() {
        return Err(decode_err("GIF 里没有任何帧"));
    }

    Ok(Animation { frames, repeat })
}

fn clear_rect(canvas: &mut [u8], canvas_w: usize, x: usize, y: usize, w: usize, h: usize) {
    let w = w.min(canvas_w.saturating_sub(x));
    for row in 0..h {
        let start = ((y + row) * canvas_w + x) * 4;
        let end = start + w * 4;
        if end <= canvas.len() {
            canvas[start..end].fill(0);
        }
    }
}

/// 编码成 GIF。每帧写自己的 delay，透明像素交给 gif crate 映射成透明索引。
///
/// 输出统一用 disposal=Background + 每帧完整画布：这样播放器不需要理解
/// delta 帧，也就不会因为处置方式不同而出现残影。
pub fn encode(anim: &Animation) -> Result<Vec<u8>, Error> {
    if anim.frames.is_empty() {
        return Err(encode_err("没有可编码的帧"));
    }
    let width = anim.width();
    let height = anim.height();

    let mut out = Vec::new();
    {
        let mut encoder = Encoder::new(&mut out, width, height, &[]).map_err(encode_err)?;
        encoder
            .set_repeat(if anim.repeat == 0 {
                Repeat::Infinite
            } else {
                Repeat::Finite(anim.repeat)
            })
            .map_err(encode_err)?;

        for f in &anim.frames {
            let mut pixels = f.pixels.clone();
            // gif crate 的 speed 是 NeuQuant 的采样因子，范围 1–30，
            // **越大越快但画质越差**（文档原话）。1 是最高质量档。
            // 表情包尺寸小、帧数少，用 1 换清晰度完全值得——静图（帮助图）
            // 更是必须用 1，否则渐变底会出现明显色带。
            let mut frame = Frame::from_rgba_speed(f.width, f.height, &mut pixels, 1);
            // 单位 1/100 秒。允许 1cs（10ms）：不少播放器会把 0/1cs 当 100ms
            // 处理，但那是老浏览器的历史行为，现代浏览器（Chrome/Firefox/
            // QQ/微信）都按实际值播。下限放到 1cs 才能让高倍速真正生效。
            let cs = ((f.delay_ms + 9) / 10).clamp(1, u16::MAX as u32);
            frame.delay = cs as u16;
            frame.dispose = DisposalMethod::Background;
            encoder.write_frame(&frame).map_err(encode_err)?;
        }
    }
    Ok(out)
}