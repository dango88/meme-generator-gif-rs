//! GIF 表情的纯处理逻辑，与 meme 框架解耦，方便单测。
//!
//! 所有函数都吃 `Animation`、吐 `Animation`，不碰 HTTP、不碰框架类型。

use meme_generator_core::error::Error;

use crate::gifio::{Animation, RgbaFrame};
use crate::text::{Align, draw_line, load_font_data, measure_text};

/// GIF 的 delay 单位是 1/100 秒，文件层面最小能写 1cs（10ms）。
///
/// 但浏览器对过小的延时另有处理：多个实测项目（ruozao/gifSpeed 等）报告
/// <20ms 会被抬到 100ms 播放。写 10ms 在文件里合法，播出来却可能比原始
/// 还慢——那才是真的翻车。所以这里用 20ms 作为**输出下限**，保证写进去的
/// 值在各家播放器里都按原意播。
pub const MIN_FRAME_MS: u32 = 20;

/// 文件层面绝对不能写的值：0cs 会被当 100ms。仅在测试里作为断言基准。
#[allow(dead_code)]
pub const ABSOLUTE_MIN_MS: u32 = 10;

/// 在累积时间轴上找到 `t` 落在哪一帧，返回该帧下标。
///
/// `bounds` 长 frames.len()+1，第 i 帧覆盖 [bounds[i], bounds[i+1])。
fn frame_index_at(bounds: &[f32], t: f32) -> usize {
    let n = bounds.len().saturating_sub(1);
    if n == 0 {
        return 0;
    }
    if t <= 0.0 {
        return 0;
    }
    let mut lo = 0usize;
    let mut hi = n;
    while lo < hi {
        let mid = (lo + hi) / 2;
        if bounds[mid + 1] <= t {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo.min(n - 1)
}

/// 帮助图画布宽度。
///
/// 用 1240 而不是 620：手机 QQ 会把图缩到屏幕宽度（约 400–430px）显示，
/// 620 宽的图缩掉 1/3 后文字就糊了；1240 宽缩到 400 还有 3 倍采样余量，
/// 看上去是清晰的小字而不是模糊的大字。字号与间距同步按 2 倍放大。
const HELP_W: u16 = 1240;
/// 标题区高度。
const HELP_HEADER_H: f32 = 214.0;
/// 每条指令占的行高。
const HELP_ROW_H: f32 = 78.0;
/// 页脚区高度。
const HELP_FOOTER_H: f32 = 132.0;
/// 左右内边距。
const HELP_PAD: f32 = 80.0;

/// 帮助图的配色：深色底 + 米白字 + 每个命令用亮青高亮。
const HELP_BG: [u8; 3] = [26, 28, 34];
const HELP_TITLE: [u8; 3] = [255, 236, 179];
const HELP_CMD: [u8; 3] = [122, 214, 255];
const HELP_DESC: [u8; 3] = [176, 182, 194];
const HELP_SEP: [u8; 3] = [58, 62, 72];

/// 渲染一张帮助图：标题 + 每行「命令  说明」+ 分隔线。
///
/// 刻意做成单帧 GIF：`gif_help` 的 min_images=0，用户在群里发
/// `xagif帮助` 就能拿到这张图，不需要先准备素材。
pub fn render_help(lines: &[(&str, &str)]) -> Result<Animation, Error> {
    let font_data = load_font_data()?;
    // 先按内容算出需要多高，再分配画布——写死高度会让页脚被裁掉。
    let help_h = (HELP_HEADER_H + HELP_ROW_H * lines.len() as f32 + HELP_FOOTER_H) as u16;
    let mut pixels = vec![0u8; HELP_W as usize * help_h as usize * 4];
    // 铺深色底（不透明）
    for px in pixels.chunks_exact_mut(4) {
        px[0] = HELP_BG[0];
        px[1] = HELP_BG[1];
        px[2] = HELP_BG[2];
        px[3] = 255;
    }

    // 标题
    draw_line(
        &font_data, &mut pixels, HELP_W, help_h,
        "GIF 表情指令", 68.0,
        HELP_TITLE, HELP_BG, 4.0,
        0.0, 124.0, Align::Center,
    )?;
    draw_line(
        &font_data, &mut pixels, HELP_W, help_h,
        "发图时/@引用图片后跟指令", 34.0,
        HELP_DESC, HELP_BG, 2.0,
        0.0, 184.0, Align::Center,
    )?;

    // 分隔线
    let mut y = HELP_HEADER_H;
    fill_hline(&mut pixels, HELP_W, HELP_PAD as i32, HELP_W as i32 - HELP_PAD as i32, y as i32, HELP_SEP);

    // 每条指令：左侧命令（青色） + 右侧说明（灰）
    
    let cmd_x = HELP_PAD;

    // 描述列的起始位置按「最宽的那条命令」动态算。中文字形的实际像素
    // 比 advance 累加更宽，写死 300 会让两列挤在一起。
    let widest = lines
        .iter()
        .map(|(cmd, _)| measure_text(&font_data, cmd, 42.0, HELP_W, help_h))
        .fold(0.0f32, f32::max);
    let desc_x = (cmd_x + widest + 52.0).min(HELP_W as f32 * 0.56);

    for (cmd, desc) in lines {
        y += HELP_ROW_H;
        draw_line(
            &font_data, &mut pixels, HELP_W, help_h,
            cmd, 42.0, HELP_CMD, HELP_BG, 0.0,
            cmd_x, y, Align::Left,
        )?;
        draw_line(
            &font_data, &mut pixels, HELP_W, help_h,
            desc, 34.0, HELP_DESC, HELP_BG, 0.0,
            desc_x, y, Align::Left,
        )?;
    }

    // 页脚
    let footer_y = y + 88.0;
    fill_hline(&mut pixels, HELP_W, HELP_PAD as i32, HELP_W as i32 - HELP_PAD as i32, (footer_y - 52.0) as i32, HELP_SEP);
    draw_line(
        &font_data, &mut pixels, HELP_W, help_h,
        "加速 >1，减速 <1；配字支持 -s 字号 -b 底衬", 32.0,
        HELP_DESC, HELP_BG, 0.0,
        0.0, footer_y, Align::Center,
    )?;

    Ok(Animation {
        frames: vec![RgbaFrame {
            width: HELP_W,
            height: help_h,
            pixels,
            // 静态图给个较长延时，避免播放器一闪而过
            delay_ms: 1000,
        }],
        repeat: 0,
    })
}

/// 画一条水平线。
fn fill_hline(pixels: &mut [u8], width: u16, x0: i32, x1: i32, y: i32, color: [u8; 3]) {
    let h = pixels.len() / (width as usize * 4);
    if y < 0 || y as usize >= h {
        return;
    }
    let start = x0.max(0);
    let end = x1.min(width as i32);
    for x in start..end {
        let idx = (y as usize * width as usize + x as usize) * 4;
        if idx + 4 <= pixels.len() {
            pixels[idx] = color[0];
            pixels[idx + 1] = color[1];
            pixels[idx + 2] = color[2];
            pixels[idx + 3] = 255;
        }
    }
}

/// 倒放：帧序反转。delay 跟着帧一起走，所以节奏会整体镜像——
/// 快进的地方变成快退，这是倒放该有的样子。
pub fn reverse(anim: &Animation) -> Animation {
    let mut frames: Vec<RgbaFrame> = anim
        .frames
        .iter()
        .rev()
        .map(|f| RgbaFrame {
            width: f.width,
            height: f.height,
            pixels: f.pixels.clone(),
            delay_ms: f.delay_ms,
        })
        .collect();
    // 倒放后第一帧是原来最后一帧。若原 GIF 用差分帧，末帧可能只覆盖一小块，
    // 但这里 decode 已经把所有帧展开成完整画布，所以没有这个问题。
    // 唯一要处理的是：原首帧常常是「空/淡入」帧，倒放后跑到了末尾。
    // 这是倒放的自然结果，不做特殊处理。
    frames.shrink_to_fit();
    Animation {
        frames,
        repeat: anim.repeat,
    }
}

/// 变速。`factor` > 1 变快（delay 变小），< 1 变慢。
/// 只改时长不改帧内容，所以帧数不变。
pub fn speed(anim: &Animation, factor: f32) -> Animation {
    let factor = if factor.is_finite() && factor > 0.0 {
        factor
    } else {
        1.0
    };

    // 减速：每帧变长，没有精度问题，直接按比例放大。
    if factor <= 1.0 {
        let frames = anim
            .frames
            .iter()
            .map(|f| RgbaFrame {
                width: f.width,
                height: f.height,
                pixels: f.pixels.clone(),
                delay_ms: ((f.delay_ms as f32 / factor).round() as u32).max(MIN_FRAME_MS),
            })
            .collect();
        return Animation {
            frames,
            repeat: anim.repeat,
        };
    }

    // 加速：GIF 的 delay 精度是 10ms，且 <20ms 会被浏览器抬到 100ms。
    // 倍数一大单帧就不够表示，必须丢帧——钳住延时会让 5x 和 10x 一样快。
    //
    // 采样方式采用「时间片中点」而非顺序累积：把输出时间轴等分成 n 帧，
    // 每帧取自己时间片的中点映射回源时间轴去挑源帧。这样丢掉的帧均匀
    // 散布在整段动画里，而不是集中在某处（顺序累积容易在开头/结尾堆积）。
    // 这个做法与 ruozao/gifSpeed 的 drop 模式一致。
    let floor_ms = MIN_FRAME_MS;
    let src_total: f32 = anim.frames.iter().map(|f| f.delay_ms as f32).sum();
    let target_ms = src_total / factor;

    // 输出帧数：向下取整，至少 1 帧，且不超过源帧数（不复制帧）。
    let n_out = ((target_ms / floor_ms as f32) as usize)
        .max(1)
        .min(anim.frames.len());
    let step = target_ms / n_out as f32;

    // 源时间轴的累积边界，用于把时间点映射回源帧下标。
    let mut bounds: Vec<f32> = Vec::with_capacity(anim.frames.len() + 1);
    bounds.push(0.0);
    let mut acc_src = 0.0f32;
    for f in &anim.frames {
        acc_src += f.delay_ms as f32;
        bounds.push(acc_src);
    }

    let mut frames: Vec<RgbaFrame> = Vec::with_capacity(n_out);
    // 误差扩散：把浮点步长量化成 10ms 整数倍时，把余数带到下一帧，
    // 保证总时长不漂移（否则 30 帧下来能差出几十毫秒）。
    let mut err_ms: f32 = 0.0;
    for j in 0..n_out {
        // 取本时间片中点，换算回源时间轴
        let src_t = (j as f32 * step + step * 0.5) * factor;
        let idx = frame_index_at(&bounds, src_t);
        let f = &anim.frames[idx];

        // 目标步长 + 上一帧余数，量化到 10ms
        let want = step + err_ms;
        let cs = (want / 10.0).round().max(floor_ms as f32 / 10.0);
        let out_ms = (cs * 10.0) as u32;
        err_ms = want - out_ms as f32;

        frames.push(RgbaFrame {
            width: f.width,
            height: f.height,
            pixels: f.pixels.clone(),
            delay_ms: out_ms.max(MIN_FRAME_MS),
        });
    }

    Animation {
        frames,
        repeat: anim.repeat,
    }
}

/// 镜像。`horizontal` 为真时左右翻，否则上下翻。
pub fn mirror(anim: &Animation, horizontal: bool) -> Animation {
    let frames = anim
        .frames
        .iter()
        .map(|f| RgbaFrame {
            width: f.width,
            height: f.height,
            pixels: flip_pixels(&f.pixels, f.width, f.height, horizontal),
            delay_ms: f.delay_ms,
        })
        .collect();
    Animation {
        frames,
        repeat: anim.repeat,
    }
}

fn flip_pixels(pixels: &[u8], width: u16, height: u16, horizontal: bool) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let mut out = vec![0u8; pixels.len()];
    for y in 0..h {
        let src_row = y * w;
        for x in 0..w {
            let (dx, dy) = if horizontal { (w - 1 - x, y) } else { (x, h - 1 - y) };
            let s = (src_row + x) * 4;
            let d = (dy * w + dx) * 4;
            if s + 4 <= pixels.len() && d + 4 <= out.len() {
                out[d..d + 4].copy_from_slice(&pixels[s..s + 4]);
            }
        }
    }
    out
}

/// 帧数上限：防止有人传个几百帧的 GIF 把服务打满。
pub const MAX_FRAMES: usize = 400;

/// 统一做一次入口校验，四个表情共用。
pub fn validate(anim: &Animation) -> Result<(), String> {
    if anim.is_empty() {
        return Err("GIF 里没有任何帧".to_string());
    }
    if anim.frames.len() > MAX_FRAMES {
        return Err(format!(
            "GIF 有 {} 帧，超过上限 {} 帧；先用「变速」或剪辑缩小再试",
            anim.frames.len(),
            MAX_FRAMES
        ));
    }
    let px = anim.width() as u64 * anim.height() as u64;
    if px > 4096 * 4096 {
        return Err("GIF 单帧像素过多，请先缩小尺寸".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: u16, h: u16, color: [u8; 4], delay_ms: u32) -> RgbaFrame {
        RgbaFrame {
            width: w,
            height: h,
            pixels: color.iter().copied().cycle().take(w as usize * h as usize * 4).collect(),
            delay_ms,
        }
    }

    #[test]
    fn reverse_flips_order_and_keeps_delays() {
        let anim = Animation {
            frames: vec![
                frame(2, 2, [255, 0, 0, 255], 100),
                frame(2, 2, [0, 255, 0, 255], 200),
                frame(2, 2, [0, 0, 255, 255], 300),
            ],
            repeat: 0,
        };
        let out = reverse(&anim);
        assert_eq!(out.frames[0].delay_ms, 300);
        assert_eq!(out.frames[2].delay_ms, 100);
        assert_eq!(out.frames[0].pixels[0..4], [0, 0, 255, 255]);
    }

    #[test]
    fn speed_halves_delay() {
        let anim = Animation {
            frames: vec![frame(2, 2, [1, 2, 3, 255], 100)],
            repeat: 0,
        };
        assert_eq!(speed(&anim, 2.0).frames[0].delay_ms, 50);
    }

    #[test]
    fn fast_speed_stays_monotonic() {
        // 加速倍率越大，总时长必须越短。曾经把 delay 钳在 20ms，
        // 导致 5x 和 10x 看起来一样快（用户反馈「加速有点慢」）。
        let anim = Animation {
            frames: vec![
                frame(2, 2, [1, 2, 3, 255], 100),
                frame(2, 2, [1, 2, 3, 255], 200),
                frame(2, 2, [1, 2, 3, 255], 100),
                frame(2, 2, [1, 2, 3, 255], 300),
            ],
            repeat: 0,
        };
        let total = |f: f32| speed(&anim, f).frames.iter().map(|x| x.delay_ms).sum::<u32>();
        let t1 = total(1.0);
        let t2 = total(2.0);
        let t5 = total(5.0);
        let t10 = total(10.0);
        assert_eq!(t1, 700);
        assert!(t2 < t1 && t5 < t2 && t10 < t5, "{t1} {t2} {t5} {t10}");
        // 20ms 下限会限制极端加速的实际效果，但仍须明显快于 5x
        assert!(t10 >= MIN_FRAME_MS * 2, "10x 总时长 {t10}ms 过小");
    }

    #[test]
    fn high_speed_drops_frames_instead_of_flattening() {
        // 20x 时每帧只剩 5ms，低于 20ms 下限，必须丢帧。
        // 关键是不能因为钳制导致「比低倍速还慢」。
        let anim = Animation {
            frames: (0..10)
                .map(|_| frame(2, 2, [1, 2, 3, 255], 100))
                .collect(),
            repeat: 0,
        };
        let r20 = speed(&anim, 20.0);
        assert!(
            r20.frames.len() < 10,
            "20x 应当丢帧，实际 {} 帧",
            r20.frames.len()
        );
        assert!(!r20.frames.is_empty(), "不能把帧全丢光");
        // 每帧都不低于输出下限
        for f in &r20.frames {
            assert!(
                f.delay_ms >= MIN_FRAME_MS,
                "延时 {}ms 低于下限 {}ms",
                f.delay_ms,
                MIN_FRAME_MS
            );
        }
        // 绝不能写出 0cs（会被当 100ms）
        for f in &r20.frames {
            assert!(f.delay_ms >= ABSOLUTE_MIN_MS);
        }
    }

    #[test]
    fn speed_picks_frames_across_whole_animation() {
        // 中点采样：丢帧应均匀分布，而不是集中在开头或结尾。
        // 用 100 帧、每帧不同亮度做标记。
        let anim = Animation {
            frames: (0..100u8)
                .map(|i| {
                    let v = i.wrapping_mul(2);
                    frame(2, 2, [v, v, v, 255], 100)
                })
                .collect(),
            repeat: 0,
        };
        let out = speed(&anim, 10.0);
        assert!(out.frames.len() >= 2 && out.frames.len() < 100);
        // 第一帧不该来自最开头几个（中点采样会跳过开头一小段）
        let first = out.frames[0].pixels[0];
        let last = out.frames.last().unwrap().pixels[0];
        assert!(last > first, "输出帧应覆盖到动画后段");
        // 输出帧的首像素应当递增（证明是按时间顺序抽取，没有乱序）
        let mut prev = out.frames[0].pixels[0];
        for f in out.frames.iter().skip(1) {
            assert!(f.pixels[0] >= prev, "帧顺序乱了");
            prev = f.pixels[0];
        }
    }

    #[test]
    fn slow_speed_lengthens_delay() {
        let anim = Animation {
            frames: vec![frame(2, 2, [1, 2, 3, 255], 100)],
            repeat: 0,
        };
        assert_eq!(speed(&anim, 0.5).frames[0].delay_ms, 200);
        assert_eq!(speed(&anim, 0.2).frames[0].delay_ms, 500);
    }

    #[test]
    fn mirror_horizontal_swaps_columns() {
        // 1x2 像素：左黑右白 → 镜像后左白右黑
        let anim = Animation {
            frames: vec![RgbaFrame {
                width: 2,
                height: 1,
                pixels: vec![0, 0, 0, 255, 255, 255, 255, 255],
                delay_ms: 100,
            }],
            repeat: 0,
        };
        let out = mirror(&anim, true);
        assert_eq!(out.frames[0].pixels, vec![255, 255, 255, 255, 0, 0, 0, 255]);
    }

    #[test]
    fn mirror_vertical_swaps_rows() {
        let anim = Animation {
            frames: vec![RgbaFrame {
                width: 1,
                height: 2,
                pixels: vec![0, 0, 0, 255, 255, 255, 255, 255],
                delay_ms: 100,
            }],
            repeat: 0,
        };
        let out = mirror(&anim, false);
        assert_eq!(out.frames[0].pixels, vec![255, 255, 255, 255, 0, 0, 0, 255]);
    }
}