//! 把 GIF 处理接成 meme-generator-rs 的外部表情包。
//!
//! 注册方式沿用官方 contrib 包的做法：导出一个 `MEME_PACK_DECLARATION`
//! 静态符号，宿主 dlopen 之后调用 `register` 把每个表情塞进注册表。
//!
//! 与官方模板的差别：不依赖 `meme_generator_utils`（那会拖进 skia-safe 和
//! gifski，编译要几百 MB 的 C++ 工具链），直接实现 `Meme` trait。代价是
//! 要自己写 `info()` 的样板，收益是编译快、体积小、且能拿到未经框架改写的
//! 原始 GIF 字节（保住逐帧 delay）。
//!
//! 六个表情统一 `xagif` 前缀（前缀本身由宿主的 core_plugin_memes 配置决定，
//! 这里只保证关键词都以 `gif` 开头，让用户敲 `xagif倒放` 这样的命令）：
//!
//! | key          | 关键词        | 参数                        |
//! |--------------|--------------|-----------------------------|
//! | gif_reverse  | gif倒放      | 无                          |
//! | gif_speed_up | gif加速      | 倍数（如 2x），必填          |
//! | gif_slow     | gif减速      | 倍数（如 0.2x），必填        |
//! | gif_mirror   | gif镜像      | 方向 h/v，可选               |
//! | gif_text     | gif配字      | 文字必填 + 字号/底衬可选      |
//! | gif_help     | gif帮助      | 无（自己渲染帮助图）          |

use std::collections::HashMap;

use meme_generator_core::error::Error;
use meme_generator_core::meme::{
    Image, Meme, MemeInfo, MemeOption, MemeParams, OptionValue, ParserFlags,
};
use meme_generator_core::registry::MemeRegistry;

mod gifio;
mod ops;
mod text;

use crate::text::{TextStyle, overlay_text};

/// 带一个短别名的解析标记（`-f` / `--factor`）。
fn short_alias(alias: char) -> ParserFlags {
    ParserFlags {
        short: true,
        long: true,
        short_aliases: vec![alias],
        long_aliases: Vec::new(),
    }
}

/// 只有一个输入图、没有文字、没有选项的表情信息。
fn gif_params() -> MemeParams {
    MemeParams {
        min_images: 1,
        max_images: 1,
        min_texts: 0,
        max_texts: 0,
        default_texts: Vec::new(),
        options: Vec::new(),
    }
}

fn gif_info(key: &str, keywords: &[&str]) -> MemeInfo {
    MemeInfo {
        key: key.to_string(),
        params: gif_params(),
        keywords: keywords.iter().map(|s| s.to_string()).collect(),
        shortcuts: Vec::new(),
        tags: Default::default(),
        date_created: chrono::Local::now(),
        date_modified: chrono::Local::now(),
    }
}

/// 从 payload 里取唯一那张图。
fn take_single_image(images: Vec<Image>) -> Result<Image, Error> {
    images
        .into_iter()
        .next()
        .ok_or(Error::ImageNumberMismatch(1, 1, 0))
}

/// 取第一段非空文字。
fn take_text(texts: Vec<String>) -> Result<String, Error> {
    texts
        .into_iter()
        .find(|t| !t.trim().is_empty())
        .ok_or_else(|| Error::TextNumberMismatch(1, 1, 0))
}

/// 读一个 f32 选项，带默认值。
fn opt_f32(options: &HashMap<String, OptionValue>, name: &str, default: f32) -> f32 {
    match options.get(name) {
        Some(OptionValue::Float(v)) => *v,
        Some(OptionValue::Integer(v)) => *v as f32,
        Some(OptionValue::String(s)) => s.parse().unwrap_or(default),
        _ => default,
    }
}

// ── 倍数解析（加速/减速共用） ───────────────────────────────

/// 倍数的合法区间。
const FACTOR_MIN: f32 = 0.1;
const FACTOR_MAX: f32 = 20.0;

/// 解析倍数文字，接受 `2x` / `2X` / `2倍` / `2×` / `2`。
///
/// 只负责「把字符串变成数」；区间校验交给 `check_factor`，因为加速和减速
/// 对区间的解释不同（加速必须 > 1，减速必须 < 1）。
fn parse_factor_text(s: &str) -> Result<f32, String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err("没给倍数，请写成 2x 这样".to_string());
    }
    let core = trimmed.trim_end_matches(['x', 'X', '×', '倍']).trim();
    core.parse::<f32>()
        .map_err(|_| format!("看不懂倍数「{s}」，请写成 2x 这种形式"))
}

/// 从第一段文字里取倍数；没有文字就报错（加速/减速都要求必填）。
fn factor_from_texts(texts: &[String]) -> Result<f32, String> {
    let raw = texts
        .iter()
        .find(|t| !t.trim().is_empty())
        .ok_or_else(|| {
            format!(
                "需要倍数，例如 {} 2x。范围 {}x – {}x",
                "「", FACTOR_MIN, FACTOR_MAX
            )
        })?;
    parse_factor_text(raw)
}

/// 校验倍数落在全局区间内。
fn check_factor(raw: f32, what: &str) -> Result<f32, String> {
    if !raw.is_finite() {
        return Err(format!("{what}必须是数字"));
    }
    if !(FACTOR_MIN..=FACTOR_MAX).contains(&raw) {
        return Err(format!(
            "{what}范围 {FACTOR_MIN}x – {FACTOR_MAX}x，收到 {raw}x"
        ));
    }
    Ok(raw)
}

// ── gif_reverse ───────────────────────────────────────────

struct GifReverse;

impl Meme for GifReverse {
    fn key(&self) -> String {
        "gif_reverse".to_string()
    }

    fn info(&self) -> MemeInfo {
        gif_info("gif_reverse", &["gif倒放", "gif反转", "gif倒着放"])
    }

    fn generate(
        &self,
        images: Vec<Image>,
        _texts: Vec<String>,
        _options: HashMap<String, OptionValue>,
    ) -> Result<Vec<u8>, Error> {
        let image = take_single_image(images)?;
        let anim = gifio::decode(&image.data)?;
        ops::validate(&anim).map_err(Error::MemeFeedback)?;
        gifio::encode(&ops::reverse(&anim))
    }

    fn generate_preview(&self, _options: HashMap<String, OptionValue>) -> Result<Vec<u8>, Error> {
        Err(Error::MemeFeedback(
            "gif倒放 需要一张动图，无法生成纯参数预览".to_string(),
        ))
    }
}

// ── gif_speed_up（加速） ──────────────────────────────────

struct GifSpeedUp;

impl Meme for GifSpeedUp {
    fn key(&self) -> String {
        "gif_speed_up".to_string()
    }

    fn info(&self) -> MemeInfo {
        let mut info = gif_info("gif_speed_up", &["gif加速", "gif快进", "gif倍速"]);
        // 倍数走第一段文字：用户直接写 `xagif加速 2x`，不用记 -f。
        info.params.min_texts = 1;
        info.params.max_texts = 1;
        info.params.default_texts = Vec::new();
        info
    }

    fn generate(
        &self,
        images: Vec<Image>,
        texts: Vec<String>,
        _options: HashMap<String, OptionValue>,
    ) -> Result<Vec<u8>, Error> {
        let image = take_single_image(images)?;
        let factor = factor_from_texts(&texts).map_err(Error::MemeFeedback)?;
        let factor = check_factor(factor, "加速倍数").map_err(Error::MemeFeedback)?;
        if factor <= 1.0 {
            return Err(Error::MemeFeedback(format!(
                "加速倍数要大于 1，收到 {factor}x；想变慢请用 gif减速"
            )));
        }
        let anim = gifio::decode(&image.data)?;
        ops::validate(&anim).map_err(Error::MemeFeedback)?;
        gifio::encode(&ops::speed(&anim, factor))
    }

    fn generate_preview(&self, _options: HashMap<String, OptionValue>) -> Result<Vec<u8>, Error> {
        Err(Error::MemeFeedback(
            "gif加速 需要一张动图，无法生成纯参数预览".to_string(),
        ))
    }
}

// ── gif_slow（减速） ──────────────────────────────────────

struct GifSlow;

impl Meme for GifSlow {
    fn key(&self) -> String {
        "gif_slow".to_string()
    }

    fn info(&self) -> MemeInfo {
        let mut info = gif_info("gif_slow", &["gif减速", "gif慢放", "gif放慢"]);
        info.params.min_texts = 1;
        info.params.max_texts = 1;
        info.params.default_texts = Vec::new();
        info
    }

    fn generate(
        &self,
        images: Vec<Image>,
        texts: Vec<String>,
        _options: HashMap<String, OptionValue>,
    ) -> Result<Vec<u8>, Error> {
        let image = take_single_image(images)?;
        let slow = factor_from_texts(&texts).map_err(Error::MemeFeedback)?;
        let slow = check_factor(slow, "减速倍数").map_err(Error::MemeFeedback)?;
        if slow >= 1.0 {
            return Err(Error::MemeFeedback(format!(
                "减速倍数要小于 1（表示「慢到原来的几分之一」），收到 {slow}x；\
                 想变快请用 gif加速。例如 0.2x = 慢到五分之一"
            )));
        }
        // ops::speed 的 factor 是「播放速度倍数」：0.2 会让 delay ×5。
        // 减速这里用户给的就是这个倍率，直接透传，语义一致。
        let anim = gifio::decode(&image.data)?;
        ops::validate(&anim).map_err(Error::MemeFeedback)?;
        gifio::encode(&ops::speed(&anim, slow))
    }

    fn generate_preview(&self, _options: HashMap<String, OptionValue>) -> Result<Vec<u8>, Error> {
        Err(Error::MemeFeedback(
            "gif减速 需要一张动图，无法生成纯参数预览".to_string(),
        ))
    }
}

// ── gif_mirror ───────────────────────────────────────────

/// 镜像方向的取值表：左右 / 上下各认几种写法。
const MIRROR_HORIZONTAL: &[&str] = &["h", "horizontal", "左右", "横", "水平"];
const MIRROR_VERTICAL: &[&str] = &["v", "vertical", "上下", "竖", "垂直"];

impl GifMirror {
    fn parse_axis(raw: &str) -> Result<bool, String> {
        let s = raw.trim().to_lowercase();
        if s.is_empty() {
            return Ok(true);
        }
        if MIRROR_HORIZONTAL.contains(&s.as_str()) {
            return Ok(true);
        }
        if MIRROR_VERTICAL.contains(&s.as_str()) {
            return Ok(false);
        }
        Err(format!(
            "看不懂镜像方向「{raw}」，请用 左右 或 上下（也可写 h / v）"
        ))
    }
}

struct GifMirror;

impl Meme for GifMirror {
    fn key(&self) -> String {
        "gif_mirror".to_string()
    }

    fn info(&self) -> MemeInfo {
        let mut info = gif_info("gif_mirror", &["gif镜像", "gif翻转", "gif左右翻转"]);
        // 方向也走文字：`xagif镜像 上下`。默认左右。
        info.params.min_texts = 0;
        info.params.max_texts = 1;
        info.params.default_texts = Vec::new();
        info
    }

    fn generate(
        &self,
        images: Vec<Image>,
        texts: Vec<String>,
        _options: HashMap<String, OptionValue>,
    ) -> Result<Vec<u8>, Error> {
        let image = take_single_image(images)?;
        let horizontal = match texts.iter().find(|t| !t.trim().is_empty()) {
            Some(raw) => GifMirror::parse_axis(raw).map_err(Error::MemeFeedback)?,
            None => true,
        };
        let anim = gifio::decode(&image.data)?;
        ops::validate(&anim).map_err(Error::MemeFeedback)?;
        gifio::encode(&ops::mirror(&anim, horizontal))
    }

    fn generate_preview(&self, _options: HashMap<String, OptionValue>) -> Result<Vec<u8>, Error> {
        Err(Error::MemeFeedback(
            "gif镜像 需要一张动图，无法生成纯参数预览".to_string(),
        ))
    }
}

// ── gif_text ─────────────────────────────────────────────

struct GifText;

impl Meme for GifText {
    fn key(&self) -> String {
        "gif_text".to_string()
    }

    fn info(&self) -> MemeInfo {
        let mut info = MemeInfo {
            key: "gif_text".to_string(),
            params: MemeParams {
                min_images: 1,
                max_images: 1,
                min_texts: 1,
                max_texts: 1,
                default_texts: vec!["配字".to_string()],
                options: Vec::new(),
            },
            keywords: ["gif配字", "gif加字", "gif字幕"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            shortcuts: Vec::new(),
            tags: Default::default(),
            date_created: chrono::Local::now(),
            date_modified: chrono::Local::now(),
        };
        info.params.options = vec![
            MemeOption::Float {
                name: "size".to_string(),
                default: Some(0.0),
                minimum: Some(8.0),
                maximum: Some(200.0),
                description: Some("字号（像素）；0 = 按画布高度自动推算（约 18%）".to_string()),
                parser_flags: short_alias('s'),
            },
            MemeOption::Integer {
                name: "banner".to_string(),
                default: Some(55),
                minimum: Some(0),
                maximum: Some(100),
                description: Some("底部黑色底衬的不透明度百分比，0 = 不画底衬".to_string()),
                parser_flags: short_alias('b'),
            },
        ];
        info
    }

    fn generate(
        &self,
        images: Vec<Image>,
        texts: Vec<String>,
        options: HashMap<String, OptionValue>,
    ) -> Result<Vec<u8>, Error> {
        let image = take_single_image(images)?;
        let text = take_text(texts)?;
        let anim = gifio::decode(&image.data)?;
        ops::validate(&anim).map_err(Error::MemeFeedback)?;

        // 字号：options 里给了正数就用，否则按画布高度推（约 18%）。
        // 小图上固定 48px 会占掉半屏，所以默认值随画布走。
        let auto_size = (anim.height() as f32 * 0.18).clamp(10.0, 64.0);
        let requested = opt_f32(&options, "size", 0.0);
        let style = TextStyle {
            size: if requested > 0.0 { requested } else { auto_size },
            banner_opacity: match options.get("banner") {
                Some(OptionValue::Integer(v)) => (*v).clamp(0, 100) as u8,
                Some(OptionValue::Float(v)) => (*v as i32).clamp(0, 100) as u8,
                _ => 55,
            },
            ..Default::default()
        };

        let out = overlay_text(&anim, &text, &style)?;
        gifio::encode(&out)
    }

    fn generate_preview(&self, _options: HashMap<String, OptionValue>) -> Result<Vec<u8>, Error> {
        Err(Error::MemeFeedback(
            "gif配字 需要一张动图和一行文字，无法生成纯参数预览".to_string(),
        ))
    }
}

// ── gif_help（帮助图） ────────────────────────────────────

/// 帮助文案。每行一条指令，格式对齐方便扫读。
const HELP_LINES: &[(&str, &str)] = &[
    ("xagif倒放", "把动图倒着播"),
    ("xagif加速 2x", "加速到 2 倍（必须大于 1）"),
    ("xagif减速 0.2x", "减速到 0.2 倍（必须小于 1）"),
    ("xagif镜像", "左右镜像（默认）"),
    ("xagif镜像 上下", "上下镜像"),
    ("xagif配字 文字", "逐帧底部配字，字号自适应"),
    ("xagif配字 文字 -s 30", "指定字号（像素）"),
    ("xagif配字 文字 -b 0", "不画底衬"),
    ("xagif帮助", "显示这张帮助图"),
];

struct GifHelp;

impl Meme for GifHelp {
    fn key(&self) -> String {
        "gif_help".to_string()
    }

    fn info(&self) -> MemeInfo {
        // 关键词刻意留空：群聊里的 `xagif帮助` 由 core_plugin_memes 侧的
        // 文字版拦截器处理（外部表情包只能返回图片字节，发不出文字）。
        // 这里保留表情本体，让 /memes/gif_help 这个 HTTP 接口仍可用，
        // 供 WebUI、其他客户端或调试时调用。
        let mut info = gif_info("gif_help", &[]);
        // min_images=0：纯参数即可生成，不需要用户上传图。
        // 宿主对 min_images=1 会自动补发送者头像，这里刻意避开。
        info.params.min_images = 0;
        info.params.max_images = 0;
        info
    }

    fn generate(
        &self,
        _images: Vec<Image>,
        _texts: Vec<String>,
        _options: HashMap<String, OptionValue>,
    ) -> Result<Vec<u8>, Error> {
        let anim = ops::render_help(HELP_LINES)?;
        gifio::encode(&anim)
    }

    fn generate_preview(&self, _options: HashMap<String, OptionValue>) -> Result<Vec<u8>, Error> {
        let anim = ops::render_help(HELP_LINES)?;
        gifio::encode(&anim)
    }
}

// ── 注册 ─────────────────────────────────────────────────

/// 宿主 dlopen 后调它，把本包的表情全部登记进去。
///
/// `catch_unwind` 是必须的：宿主是常驻服务进程，panic 一旦跨过 FFI 边界
/// 就是未定义行为，现实里表现为整个 2233 进程 abort。这里把每个表情的
/// generate 都套一层，出问题只让当次请求失败。
///
/// `improper_ctypes_definitions` 是 ABI 本身的约定：宿主与本包约定传
/// `&mut dyn MemeRegistry`，而 dyn trait 在 C 里没有对应类型。官方
/// contrib 包同样这么写、同样 allow。
#[allow(improper_ctypes_definitions)]
extern "C" fn register_memes(registry: &mut dyn MemeRegistry) {
    let memes: Vec<Box<dyn Meme>> = vec![
        Box::new(Guarded(GifReverse)),
        Box::new(Guarded(GifSpeedUp)),
        Box::new(Guarded(GifSlow)),
        Box::new(Guarded(GifMirror)),
        Box::new(Guarded(GifText)),
        Box::new(Guarded(GifHelp)),
    ];
    for meme in memes {
        registry.register_meme(&meme.key(), meme);
    }
}

/// 给任意 `Meme` 的 generate 套上 catch_unwind 的包装。
struct Guarded<M: Meme>(M);

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "未知内部错误".to_string()
    }
}

impl<M: Meme> Meme for Guarded<M> {
    fn key(&self) -> String {
        self.0.key()
    }

    fn info(&self) -> MemeInfo {
        self.0.info()
    }

    fn generate(
        &self,
        images: Vec<Image>,
        texts: Vec<String>,
        options: HashMap<String, OptionValue>,
    ) -> Result<Vec<u8>, Error> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.0.generate(images, texts, options)
        }))
        .unwrap_or_else(|payload| {
            Err(Error::MemeFeedback(format!(
                "GIF 处理出错：{}",
                panic_message(payload)
            )))
        })
    }

    fn generate_preview(&self, options: HashMap<String, OptionValue>) -> Result<Vec<u8>, Error> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.0.generate_preview(options)
        }))
        .unwrap_or_else(|payload| {
            Err(Error::MemeFeedback(format!(
                "GIF 预览出错：{}",
                panic_message(payload)
            )))
        })
    }
}

#[allow(improper_ctypes_definitions)]
#[unsafe(no_mangle)]
pub static MEME_PACK_DECLARATION: meme_generator_core::registry::MemePackDeclaration =
    meme_generator_core::registry::MemePackDeclaration {
        rustc_version: meme_generator_core::registry::RUSTC_VERSION,
        core_version: meme_generator_core::registry::CORE_VERSION,
        register: register_memes,
    };

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn factor_text_accepts_suffixes() {
        assert_eq!(parse_factor_text("2x").unwrap(), 2.0);
        assert_eq!(parse_factor_text("2X").unwrap(), 2.0);
        assert_eq!(parse_factor_text("2倍").unwrap(), 2.0);
        assert_eq!(parse_factor_text("2×").unwrap(), 2.0);
        assert_eq!(parse_factor_text(" 1.5 ").unwrap(), 1.5);
        assert_eq!(parse_factor_text("0.2x").unwrap(), 0.2);
    }

    #[test]
    fn factor_text_rejects_garbage() {
        assert!(parse_factor_text("").is_err());
        assert!(parse_factor_text("   ").is_err());
        assert!(parse_factor_text("快一点").is_err());
    }

    #[test]
    fn factor_from_texts_reads_first_non_empty() {
        assert_eq!(factor_from_texts(&texts(&["2x"])).unwrap(), 2.0);
        assert_eq!(factor_from_texts(&texts(&["", "3x"])).unwrap(), 3.0);
        assert!(factor_from_texts(&texts(&[])).is_err());
    }

    #[test]
    fn factor_range_is_enforced() {
        assert!(check_factor(0.5, "加速").is_ok());
        assert!(check_factor(20.0, "加速").is_ok());
        assert!(check_factor(0.0, "加速").is_err());
        assert!(check_factor(0.05, "加速").is_err());
        assert!(check_factor(100.0, "加速").is_err());
    }

    #[test]
    fn mirror_axis_parsing() {
        assert_eq!(GifMirror::parse_axis("h").unwrap(), true);
        assert_eq!(GifMirror::parse_axis("左右").unwrap(), true);
        assert_eq!(GifMirror::parse_axis("horizontal").unwrap(), true);
        assert_eq!(GifMirror::parse_axis("v").unwrap(), false);
        assert_eq!(GifMirror::parse_axis("上下").unwrap(), false);
        assert_eq!(GifMirror::parse_axis("vertical").unwrap(), false);
        assert_eq!(GifMirror::parse_axis("").unwrap(), true);
        assert!(GifMirror::parse_axis("斜着").is_err());
    }

    #[test]
    fn help_lines_are_well_formed() {
        assert!(!HELP_LINES.is_empty());
        for (cmd, desc) in HELP_LINES {
            assert!(cmd.starts_with("xagif"), "帮助里的命令要以 xagif 开头: {cmd}");
            assert!(!desc.is_empty());
        }
    }

    #[test]
    fn all_meme_keys_start_with_gif() {
        // 关键词统一以 gif 开头，配合宿主前缀 xa 就是 xagifXXX
        for info in [
            GifReverse.info(),
            GifSpeedUp.info(),
            GifSlow.info(),
            GifMirror.info(),
            GifText.info(),
            GifHelp.info(),
        ] {
            assert!(
                info.key.starts_with("gif_"),
                "key 应为 gif_ 开头: {}",
                info.key
            );
            for kw in &info.keywords {
                assert!(
                    kw.starts_with("gif"),
                    "key={} 的关键词 {} 没有以 gif 开头",
                    info.key,
                    kw
                );
            }
        }
    }
}