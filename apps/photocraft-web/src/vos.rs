//! VOS 集成：在平台存储里打开和保存文件。
//!
//! 平台把本构建嵌入同源 iframe，并注入 `window.vos_platform` 桥（见 `docs/vos-integration.md`）。
//! 桥存在时，File > Open 和保存都走平台的 Finder（`/v1000/finder_next/*`），而不是浏览器选择器加
//! 下载，这样文件就住在用户的 VOS 存储里，刷新也不丢。
//!
//! 插件填上外壳的 `Services` 接缝（`file_dialog` + `write`），自己绘制窗口，并通过
//! `platform_files`/`platform_open` 列出它打开或保存过的文件，让首页可以显示它们。这两个字段和
//! `PlatformListing`/`PlatformRequest` 类型是仅有的核心新增（`crates/ui-egui/src/lib.rs`，由
//! `canvas.rs::home_platform` 绘制）；没有桥时它们全部不设置，app 的行为与之前完全一样，所以分叉
//! 在 VOS 之外照样能用。

#[cfg(any(target_arch = "wasm32", test))]
pub mod crypto;
#[cfg(any(target_arch = "wasm32", test))]
pub mod model;
#[cfg(any(target_arch = "wasm32", test))]
pub mod open;
#[cfg(any(target_arch = "wasm32", test))]
pub mod path;
#[cfg(any(target_arch = "wasm32", test))]
pub mod recent;
#[cfg(any(target_arch = "wasm32", test))]
pub mod state;

#[cfg(target_arch = "wasm32")]
mod bridge;
#[cfg(target_arch = "wasm32")]
mod home;
#[cfg(target_arch = "wasm32")]
mod http;
#[cfg(any(target_arch = "wasm32", test))]
mod install;
#[cfg(any(target_arch = "wasm32", test))]
mod picker;

#[cfg(target_arch = "wasm32")]
pub use install::{install, ui};
