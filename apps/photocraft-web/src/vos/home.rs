//! 首页的 VOS 区块：本插件打开或保存过的文件，数据来自 [`super::recent`]。
//!
//! 首页绘制它；一次点击以 [`PlatformRequest`] 回来，变成一个下载，字节交给插件的状态。
//!
//! 这里不持有 app：异步下载不能跨 await 拿着 `&mut PhotocraftApp`，所以字节落进插件的状态，由
//! [`super::install::ui`] 晚一帧打开 —— 与 `Services::inbox` 同一个形态。

use photocraft_ui_egui::i18n;
use photocraft_ui_egui::{PlatformFile, PlatformListing};
use wasm_bindgen_futures::spawn_local;

use super::http;
use super::model;
use super::path;

/// 列表持久化的位置：localStorage，和偏好设置一样能扛过刷新。
pub const RECENT_KEY: &str = "photocraft.vos.recent";

/// 首页的 VOS 区块：要列出的文件，以及平台浏览器那一行。
pub fn listing(paths: &[String]) -> PlatformListing {
    PlatformListing {
        heading: i18n::t("VOS storage").to_string(),
        browse: Some(i18n::t("Browse VOS storage…").to_string()),
        files: paths
            .iter()
            .map(|p| PlatformFile {
                id: p.clone(),
                name: path::display_name(p),
                // scheme 是存储空间的 adapter（v_<uuid>）；首页那一行很窄，所以它说明文件在哪个空间，
                // 而不是重复它的目录。
                detail: path::adapter_of(p).unwrap_or_default().to_string(),
            })
            .collect(),
    }
}

/// 打开列出的文件：取回它的字节，把字节（以及取不到的原因）交给插件的状态，由它晚一帧打开文件。
pub fn open(path: String, deliver: impl FnOnce(String, Result<Vec<u8>, model::Failure>) + 'static) {
    let Some(adapter) = path::adapter_of(&path).map(str::to_string) else {
        deliver(path.clone(), Err(model::Failure { code: model::BAD_PATH, message: i18n::t("not a VOS storage path").to_string() }));
        return;
    };
    spawn_local(async move {
        let result = http::download(&adapter, &path).await;
        deliver(path, result);
    });
}
