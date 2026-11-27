# `meme-generator-gif-rs`

此仓库为表情包生成器 [meme-generator-rs](https://github.com/MemeCrafters/meme-generator-rs) 的 GIF 处理额外表情仓库

## 表情列表

| key | 表情 | 参数 |
| --- | --- | --- |
| `gif_reverse` | 倒放 | 无 |
| `gif_speed_up` | 加速 | 倍数，必填，大于 1，如 `2x` |
| `gif_slow` | 减速 | 倍数，必填，小于 1，如 `0.2x` |
| `gif_mirror` | 镜像 | 方向，可选：`左右`（默认）/ `上下` |
| `gif_text` | 配字 | 文字必填，`-s` 字号 / `-b` 底衬可选 |
| `gif_help` | 帮助 | 无 |

倍数与镜像方向填在文字里，配字的字号与底衬走选项。

## 用法

在 [Actions](../../actions) 中下载对应平台的动态链接库，并参考
[`meme-generator-rs` 加载其他表情的方式](https://github.com/MemeCrafters/meme-generator-rs/wiki/%E5%8A%A0%E8%BD%BD%E5%85%B6%E4%BB%96%E8%A1%A8%E6%83%85)
进行加载。

## 构建

```bash
cargo build --release
```

推送到 `main` 或手动触发 `Build` workflow，会在 Actions 中产出 `linux-x86_64`、`linux-aarch64`、
`windows-x86_64`、`macos-x86_64`、`macos-aarch64` 五个平台的文件。

> workflow 中的 `RUST_VERSION` 需与宿主二进制使用的 rustc 一致，可用
> `readelf -p .comment <宿主二进制>` 查看。版本不一致时宿主不会拒绝加载，而是在调用时崩溃。

## 声明

本仓库代码以 MIT 协议开源。