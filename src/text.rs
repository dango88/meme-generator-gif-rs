//! 往每一帧上叠一行文字（居中 + 描边 + 半透明底衬）。
//!
//! 不用 skia：`ab_glyph` 把字形拆成轮廓，再由回调逐像素喂给我们，
//! 这样能自己做描边（把同一字形在 8 个方向偏移画一遍）和 alpha 混合。
//!
//! 字体从容器里找：meme-generator-rs 官方镜像把 resources/fonts 装到了
//! /usr/share/fonts/meme-fonts/，其中 NotoSansSC-Bold.ttf 覆盖中英文。

use ab_glyph::{Font, FontRef, PxScale, PxScaleFont, ScaleFont, point};
use meme_generator_core::error::Error;

use crate::gifio::{Animation, RgbaFrame};

/// 依次尝试的字体路径。第一个存在的就用。
const FONT_CANDIDATES: &[&str] = &[
    "/usr/share/fonts/meme-fonts/NotoSansSC-Bold.ttf",
    "/usr/share/fonts/meme-fonts/NotoSansSC-Regular.ttf",
    "/usr/share/fonts/meme-fonts/GlowSansSC-Normal-Heavy.otf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
];

/// 文字叠加的样式。
pub struct TextStyle {
    /// 字号（像素）。实际会再按画布宽度自适应收缩。
    pub size: f32,
    /// 文字颜色 RGB。
    pub color: [u8; 3],
    /// 描边颜色 RGB。
    pub stroke_color: [u8; 3],
    /// 描边宽度（像素），0 = 不描边。
    pub stroke_width: f32,
    /// 底部底衬的不透明度（0-100），0 = 不画底衬。
    pub banner_opacity: u8,
}

impl Default for TextStyle {
    fn default() -> Self {
        TextStyle {
            size: 48.0,
            color: [255, 255, 255],
            stroke_color: [0, 0, 0],
            stroke_width: 3.0,
            banner_opacity: 55,
        }
    }
}

fn load_font() -> Result<Vec<u8>, Error> {
    for path in FONT_CANDIDATES {
        if let Ok(data) = std::fs::read(path) {
            return Ok(data);
        }
    }
    Err(Error::ImageAssetMissing(
        "找不到可用字体；请确认容器内 /usr/share/fonts/meme-fonts/ 存在".to_string(),
    ))
}

/// 加载字体，供需要多次绘制（例如帮助图）的调用方复用，
/// 避免每行都读一次文件。
pub fn load_font_data() -> Result<Vec<u8>, Error> {
    load_font()
}

/// 量一行文字的像素宽度（用字形包围盒，比 advance 累加更贴近视觉边界）。
///
/// 帮助图这类自己排版的场景需要事先知道宽度才能决定列位置，所以单独暴露。
pub fn measure_text(
    font_data: &[u8],
    text: &str,
    size: f32,
    _width: u16,
    _height: u16,
) -> f32 {
    let Ok(font) = FontRef::try_from_slice(font_data) else {
        return 0.0;
    };
    let scale = PxScale::from(size.max(6.0));
    let scaled = font.as_scaled(scale);

    let mut pen_x = 0.0f32;
    let mut prev: Option<ab_glyph::GlyphId> = None;
    let mut min_x = f32::MAX;
    let mut max_x = f32::MIN;
    for ch in text.chars() {
        let id = font.glyph_id(ch);
        if let Some(p) = prev {
            pen_x += scaled.kern(p, id);
        }
        if ch != ' ' {
            let glyph = id.with_scale_and_position(scale, point(pen_x, 0.0));
            if let Some(outlined) = font.outline_glyph(glyph) {
                let b = outlined.px_bounds();
                min_x = min_x.min(b.min.x);
                max_x = max_x.max(b.max.x);
            }
        }
        pen_x += scaled.h_advance(id);
        prev = Some(id);
    }
    if min_x > max_x {
        0.0
    } else {
        (max_x - min_x).max(1.0)
    }
}

/// 一行文字的水平对齐方式。
#[derive(Clone, Copy, PartialEq)]
pub enum Align {
    Left,
    Center,
}

/// 把一行文字画到 RGBA 画布的指定位置上，供帮助图这类多行排版使用。
///
/// - `left` 是文字左边缘的 x 坐标（`Align::Center` 时忽略，按画布居中）
/// - `baseline` 是这行文字的基线 y 坐标
/// - 返回这行文字的实际像素宽度，方便调用方算下一行或画分隔线
///
/// 与 `overlay_text` 共用 `stamp_glyph`，所以描边/混合行为完全一致。
pub fn draw_line(
    font_data: &[u8],
    pixels: &mut [u8],
    width: u16,
    height: u16,
    text: &str,
    size: f32,
    color: [u8; 3],
    stroke_color: [u8; 3],
    stroke_width: f32,
    left: f32,
    baseline: f32,
    align: Align,
) -> Result<f32, Error> {
    let font = FontRef::try_from_slice(font_data)
        .map_err(|e| Error::ImageDecodeError(format!("字体解析失败: {e}")))?;
    let scale = PxScale::from(size.max(6.0));
    let scaled = font.as_scaled(scale);

    // 先把字形排好（位置用 y=baseline），再按对齐方式整体平移。
    let mut placed: Vec<ab_glyph::OutlinedGlyph> = Vec::new();
    let mut pen_x = 0.0f32;
    let mut prev: Option<ab_glyph::GlyphId> = None;
    for ch in text.chars() {
        let id = font.glyph_id(ch);
        if let Some(p) = prev {
            pen_x += scaled.kern(p, id);
        }
        if ch != ' ' {
            let glyph = id.with_scale_and_position(scale, point(pen_x, baseline));
            if let Some(outlined) = font.outline_glyph(glyph) {
                placed.push(outlined);
            }
        }
        pen_x += scaled.h_advance(id);
        prev = Some(id);
    }
    if placed.is_empty() {
        return Ok(0.0);
    }

    // 用包围盒算真实宽度，比 h_advance 累加更贴近视觉边界。
    let mut min_x = f32::MAX;
    let mut max_x = f32::MIN;
    for g in &placed {
        let b = g.px_bounds();
        min_x = min_x.min(b.min.x);
        max_x = max_x.max(b.max.x);
    }
    let text_w = (max_x - min_x).max(1.0);

    let shift = match align {
        Align::Left => left - min_x,
        Align::Center => (width as f32 - text_w) / 2.0 - min_x,
    };

    for g in &placed {
        if stroke_width > 0.0 {
            let steps = ((stroke_width * 2.5).ceil() as usize).clamp(8, 16);
            for i in 0..steps {
                let angle = std::f32::consts::TAU * i as f32 / steps as f32;
                stamp_glyph(
                    g,
                    pixels,
                    width,
                    height,
                    shift + angle.cos() * stroke_width,
                    angle.sin() * stroke_width,
                    stroke_color,
                );
            }
        }
        stamp_glyph(g, pixels, width, height, shift, 0.0, color);
    }

    Ok(text_w)
}

/// 在每一帧底部叠同一行文字。
pub fn overlay_text(anim: &Animation, text: &str, style: &TextStyle) -> Result<Animation, Error> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(Animation {
            frames: anim
                .frames
                .iter()
                .map(|f| RgbaFrame {
                    width: f.width,
                    height: f.height,
                    pixels: f.pixels.clone(),
                    delay_ms: f.delay_ms,
                })
                .collect(),
            repeat: anim.repeat,
        });
    }

    let font_data = load_font()?;
    let font = FontRef::try_from_slice(&font_data)
        .map_err(|e| Error::ImageDecodeError(format!("字体解析失败: {e}")))?;

    let width = anim.width();
    let height = anim.height();

    // 字号自适应：文字宽度不超过画布的 88%，同时高度不超过画布的 1/4。
    let scale = fit_scale(&font, text, style.size, width, height);

    let frames = anim
        .frames
        .iter()
        .map(|f| {
            let mut pixels = f.pixels.clone();
            draw_on_frame(&font, &mut pixels, f.width, f.height, text, scale, style);
            RgbaFrame {
                width: f.width,
                height: f.height,
                pixels,
                delay_ms: f.delay_ms,
            }
        })
        .collect();

    Ok(Animation {
        frames,
        repeat: anim.repeat,
    })
}

/// 量出文字排版尺寸，必要时把字号整体压小。
fn fit_scale(font: &FontRef<'_>, text: &str, want: f32, canvas_w: u16, canvas_h: u16) -> PxScale {
    let max_w = canvas_w as f32 * 0.88;
    let max_h = canvas_h as f32 * 0.25;
    let mut size = want.max(8.0).min(max_h);
    for _ in 0..32 {
        let scaled = font.as_scaled(PxScale::from(size));
        if measure_width(&scaled, font, text) <= max_w {
            break;
        }
        size *= 0.92;
        if size <= 8.0 {
            size = 8.0;
            break;
        }
    }
    PxScale::from(size)
}

/// 量一行文字的像素宽度。`scaled` 提供缩放后的 advance/kern，
/// `font` 用来做 glyph 查找。
fn measure_width(scaled: &PxScaleFont<&FontRef<'_>>, font: &FontRef<'_>, text: &str) -> f32 {
    let mut w = 0.0;
    let mut prev: Option<ab_glyph::GlyphId> = None;
    for ch in text.chars() {
        let id = font.glyph_id(ch);
        if let Some(p) = prev {
            w += scaled.kern(p, id);
        }
        w += scaled.h_advance(id);
        prev = Some(id);
    }
    w
}

fn draw_on_frame(
    font: &FontRef<'_>,
    pixels: &mut [u8],
    width: u16,
    height: u16,
    text: &str,
    scale: PxScale,
    style: &TextStyle,
) {
    let scaled = font.as_scaled(scale);

    // 先把每个字形排好位置（只算 x，y 用 0 占位），量出它们合起来的
    // 像素包围盒。不依赖 ascent/descent 这些排版指标，因为不同字体的
    // 度量差异很大，而且 ab_glyph 的 y 轴向上、位图 y 轴向下，直接套
    // ascent 很容易把字画到画布外面去。
    let mut placed: Vec<(ab_glyph::OutlinedGlyph, f32, f32)> = Vec::new();
    let mut pen_x = 0.0f32;
    let mut prev: Option<ab_glyph::GlyphId> = None;
    for ch in text.chars() {
        let id = font.glyph_id(ch);
        if let Some(p) = prev {
            pen_x += scaled.kern(p, id);
        }
        if ch != ' ' {
            let glyph = id.with_scale_and_position(scale, point(pen_x, 0.0));
            if let Some(outlined) = font.outline_glyph(glyph) {
                let b = outlined.px_bounds();
                placed.push((outlined, b.min.x, b.min.y));
            }
        }
        pen_x += scaled.h_advance(id);
        prev = Some(id);
    }

    if placed.is_empty() {
        return;
    }

    // 合并包围盒。px_bounds 已经包含字形在该位置的实际像素范围。
    let mut min_x = f32::MAX;
    let mut max_x = f32::MIN;
    let mut min_y = f32::MAX;
    let mut max_y = f32::MIN;
    for (g, _, _) in &placed {
        let b = g.px_bounds();
        min_x = min_x.min(b.min.x);
        max_x = max_x.max(b.max.x);
        min_y = min_y.min(b.min.y);
        max_y = max_y.max(b.max.y);
    }
    let text_w = (max_x - min_x).max(1.0);
    let text_h = (max_y - min_y).max(1.0);

    let cw = width as f32;
    let ch_ = height as f32;

    // 目标：水平居中，底部留出边距。用一个整体位移把包围盒挪到位。
    let margin = (ch_ * 0.04).max(2.0);
    let target_bottom = ch_ - margin;
    let target_left = (cw - text_w) / 2.0;

    let shift_x = target_left - min_x;
    let shift_y = target_bottom - max_y;

    // 底衬条：覆盖文字包围盒 + 描边余量，横向铺满画布。
    if style.banner_opacity > 0 {
        let pad = style.stroke_width + text_h * 0.18;
        let top = (min_y + shift_y - pad).max(0.0);
        let bottom = (max_y + shift_y + pad).min(ch_);
        let alpha = (style.banner_opacity as u32 * 255 / 100) as u8;
        fill_rect_rgba(
            pixels,
            width,
            height,
            0,
            top as i32,
            width as i32,
            (bottom - top) as i32,
            [0, 0, 0, alpha],
        );
    }

    // 真正画：描边（圆周偏移多份）+ 本体。
    for (outlined, _, _) in &placed {
        if style.stroke_width > 0.0 {
            let r = style.stroke_width;
            let steps = ((r * 2.5).ceil() as usize).clamp(8, 16);
            for i in 0..steps {
                let angle = std::f32::consts::TAU * i as f32 / steps as f32;
                stamp_glyph(
                    outlined,
                    pixels,
                    width,
                    height,
                    shift_x + angle.cos() * r,
                    shift_y + angle.sin() * r,
                    style.stroke_color,
                );
            }
        }
        stamp_glyph(outlined, pixels, width, height, shift_x, shift_y, style.color);
    }
}

/// 把一个字形按覆盖率混合到画布上。(dx, dy) 是先前提到的整体位移
/// （把包围盒挪到目标位置），描边时再叠加圆周偏移。
fn stamp_glyph(
    outlined: &ab_glyph::OutlinedGlyph,
    pixels: &mut [u8],
    width: u16,
    height: u16,
    dx: f32,
    dy: f32,
    color: [u8; 3],
) {
    let bounds = outlined.px_bounds();
    let w = width as i32;
    let h = height as i32;

    outlined.draw(|gx, gy, coverage| {
        let x = bounds.min.x as i32 + gx as i32 + dx.round() as i32;
        let y = bounds.min.y as i32 + gy as i32 + dy.round() as i32;
        if x < 0 || y < 0 || x >= w || y >= h {
            return;
        }
        let a = (coverage * 255.0) as u32;
        if a == 0 {
            return;
        }
        let idx = ((y as usize) * width as usize + x as usize) * 4;
        if idx + 4 > pixels.len() {
            return;
        }
        for c in 0..3 {
            let fg = color[c] as u32;
            let bg = pixels[idx + c] as u32;
            pixels[idx + c] = ((fg * a + bg * (255 - a)) / 255) as u8;
        }
        pixels[idx + 3] = (a + pixels[idx + 3] as u32 * (255 - a) / 255).min(255) as u8;
    });
}

/// 填充一个 RGBA 矩形（带 alpha 混合）。
fn fill_rect_rgba(
    pixels: &mut [u8],
    width: u16,
    height: u16,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    rgba: [u8; 4],
) {
    let cw = width as i32;
    let ch = height as i32;
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w).min(cw);
    let y1 = (y + h).min(ch);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let a = rgba[3] as u32;
    for yy in y0..y1 {
        for xx in x0..x1 {
            let idx = (yy as usize * width as usize + xx as usize) * 4;
            if idx + 4 > pixels.len() {
                continue;
            }
            for c in 0..3 {
                let fg = rgba[c] as u32;
                let bg = pixels[idx + c] as u32;
                pixels[idx + c] = ((fg * a + bg * (255 - a)) / 255) as u8;
            }
            pixels[idx + 3] = (a + pixels[idx + 3] as u32 * (255 - a) / 255).min(255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_is_passthrough() {
        let anim = Animation {
            frames: vec![RgbaFrame {
                width: 2,
                height: 2,
                pixels: vec![9; 16],
                delay_ms: 100,
            }],
            repeat: 0,
        };
        let out = overlay_text(&anim, "   ", &TextStyle::default()).unwrap();
        assert_eq!(out.frames[0].pixels, anim.frames[0].pixels);
    }
}