//! Browser/VOS/AI Album choices share the existing asynchronous file-dialog reply.
use super::bridge;
use photocraft_ui_egui::{FileDialogAnswer, FileDialogFn, FileDialogReply, FileDialogRequest, i18n};
use std::cell::RefCell;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::{JsFuture, spawn_local};
#[derive(Default)]
struct State {
    browser: Option<FileDialogFn>,
    pending: Option<(FileDialogRequest, FileDialogReply)>,
    album: bool,
    loading: bool,
    error: Option<String>,
    page: u64,
    has_next: bool,
    token: Option<String>,
    items: Vec<Asset>,
    thumbnails: std::collections::HashMap<String, egui::TextureHandle>,
    generation: u64,
}
#[derive(Clone, serde::Deserialize)]
struct Asset {
    id: String,
    #[serde(rename = "originalFileName")]
    name: String,
}
thread_local! {static STATE:RefCell<State>=RefCell::new(State::default());}
pub fn install(browser: Option<FileDialogFn>) {
    STATE.with(|s| s.borrow_mut().browser = browser);
}
pub fn choose(request: FileDialogRequest, reply: FileDialogReply) {
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.pending = Some((request, reply));
        s.album = false;
        s.error = None;
        s.items.clear();
        s.thumbnails.clear();
        s.page = 1;
        s.generation += 1;
    });
}
pub fn text<'a>(en: &'a str, zh: &'a str) -> &'a str {
    if i18n::current().code().starts_with("zh") { zh } else { en }
}
pub const ALBUM_PREFIX: &str = "ai-album://";
const API: &str = "/app/com.ictrek.ai-album/api";
async fn request(method: &str, url: &str, token: Option<&str>, body: Option<String>) -> Result<web_sys::Response, String> {
    let init = web_sys::RequestInit::new();
    init.set_method(method);
    // Do not depend on or replace the AI Album tab's cookies.
    init.set_credentials(web_sys::RequestCredentials::Omit);
    let headers = web_sys::Headers::new().map_err(|_| "Cannot prepare AI Album request")?;
    if let Some(token) = token {
        headers.set("Authorization", &format!("Bearer {token}")).map_err(|_| "Cannot authorize AI Album request")?;
    }
    if let Some(body) = body {
        headers.set("Content-Type", "application/json").map_err(|_| "Cannot prepare AI Album request")?;
        init.set_body(&JsValue::from_str(&body));
    }
    init.set_headers(&headers);
    let window = web_sys::window().ok_or("No browser window")?;
    let response = JsFuture::from(window.fetch_with_str_and_init(url, &init))
        .await
        .map_err(|_| "AI Album is unreachable")?
        .dyn_into::<web_sys::Response>()
        .map_err(|_| "Invalid AI Album reply")?;
    if !response.ok() {
        if response.status() == 401 {
            STATE.with(|s| s.borrow_mut().token = None);
        }
        return Err(format!(
            "{} (HTTP {})",
            text("AI Album request failed. Retry or check the app status.", "AI 相册请求失败，请重试或检查应用状态。"),
            response.status()
        ));
    }
    Ok(response)
}
async fn json(response: web_sys::Response) -> Result<serde_json::Value, String> {
    let value = JsFuture::from(response.text().map_err(|_| "Cannot read AI Album reply")?).await.map_err(|_| "Cannot read AI Album reply")?;
    serde_json::from_str(&value.as_string().unwrap_or_default()).map_err(|_| "Invalid AI Album reply".into())
}
async fn session() -> Result<String, String> {
    if let Some(token) = STATE.with(|s| s.borrow().token.clone()) {
        return Ok(token);
    }
    let token = bridge::album_token().await?;
    let origin = web_sys::window().ok_or("No browser window")?.location().origin().map_err(|_| "No VOS origin")?;
    let value =
        json(request("POST", "/app/com.ictrek.photocraft/storage/album/session", Some(&token), Some(serde_json::json!({"origin":origin}).to_string())).await?)
            .await?;
    let token = value["accessToken"].as_str().filter(|v| !v.is_empty()).ok_or("AI Album returned no session")?.to_owned();
    STATE.with(|s| s.borrow_mut().token = Some(token.clone()));
    Ok(token)
}
fn load(ctx: &egui::Context) {
    let (page, generation) = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.loading = true;
        s.error = None;
        (s.page, s.generation)
    });
    let ctx = ctx.clone();
    spawn_local(async move {
        let outcome = async {
            let token = session().await?;
            let value = json(
                request(
                    "POST",
                    &format!("{API}/search/metadata"),
                    Some(&token),
                    Some(serde_json::json!({"type":"IMAGE","page":page,"size":40,"order":"desc"}).to_string()),
                )
                .await?,
            )
            .await?;
            let items: Vec<Asset> = serde_json::from_value(value["assets"]["items"].clone()).map_err(|_| "AI Album returned an invalid image list")?;
            let next = value["assets"]["nextPage"].as_str().is_some() || value["assets"]["nextPage"].as_u64().is_some();
            Ok::<_, String>((items, next))
        }
        .await;
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            if s.generation != generation {
                return;
            }
            s.loading = false;
            match outcome {
                Ok((items, next)) => {
                    s.items = items;
                    s.thumbnails.clear();
                    s.has_next = next;
                }
                Err(error) => {
                    s.token = None;
                    s.error = Some(error);
                }
            }
        });
        ctx.request_repaint();
        let items = STATE.with(|s| s.borrow().items.clone());
        if let Ok(token) = session().await {
            for asset in items {
                if STATE.with(|s| s.borrow().generation != generation || s.borrow().page != page || s.borrow().pending.is_none()) {
                    break;
                }
                if let Ok(response) = request("GET", &format!("{API}/assets/{}/thumbnail?size=thumbnail", asset.id), Some(&token), None).await {
                    let decoded = async {
                        let blob = JsFuture::from(response.blob().map_err(|_| ())?).await.map_err(|_| ())?.dyn_into::<web_sys::Blob>().map_err(|_| ())?;
                        if blob.size() > 4.0 * 1024.0 * 1024.0 {
                            return Err(());
                        }
                        let raw = JsFuture::from(blob.array_buffer()).await.map_err(|_| ())?;
                        let bytes = js_sys::Uint8Array::new(&raw).to_vec();
                        let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().map_err(|_| ())?;
                        let mut limits = image::Limits::default();
                        limits.max_image_width = Some(2048);
                        limits.max_image_height = Some(2048);
                        limits.max_alloc = Some(32 * 1024 * 1024);
                        reader.limits(limits);
                        let image = reader.decode().map_err(|_| ())?.thumbnail(120, 90).to_rgba8();
                        Ok::<_, ()>(egui::ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], image.as_raw()))
                    }
                    .await;
                    if let Ok(image) = decoded {
                        STATE.with(|s| {
                            let mut s = s.borrow_mut();
                            if s.generation == generation && s.page == page {
                                s.thumbnails.insert(asset.id.clone(), ctx.load_texture(format!("album-{}", asset.id), image, egui::TextureOptions::LINEAR));
                            }
                        });
                        ctx.request_repaint();
                    }
                }
            }
        }
    });
}
fn open(asset: Asset, ctx: &egui::Context) {
    let reply = STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.pending.take().map(|(_, reply)| reply)
    });
    let Some(reply) = reply else {
        return;
    };
    let ctx = ctx.clone();
    spawn_local(async move {
        let outcome = async {
            let token = session().await?;
            let response = request("GET", &format!("{API}/assets/{}/original", asset.id), Some(&token), None).await?;
            let blob = JsFuture::from(response.blob().map_err(|_| "Cannot read original image")?)
                .await
                .map_err(|_| "Cannot read original image")?
                .dyn_into::<web_sys::Blob>()
                .map_err(|_| "Invalid original image")?;
            if blob.size() > super::http::MAX_DOWNLOAD as f64 {
                return Err("Image exceeds the browser file limit".into());
            }
            let bytes = JsFuture::from(blob.array_buffer()).await.map_err(|_| "Cannot read original image")?;
            Ok::<_, String>(js_sys::Uint8Array::new(&bytes).to_vec())
        }
        .await;
        reply.send(Some(match outcome {
            Ok(bytes) => FileDialogAnswer::Contents(asset.name, bytes),
            Err(e) => FileDialogAnswer::Unavailable(e),
        }));
        ctx.request_repaint();
    });
}
pub fn ui(ctx: &egui::Context) {
    if !STATE.with(|s| s.borrow().pending.is_some()) {
        return;
    }
    let album = STATE.with(|s| s.borrow().album);
    let save = STATE.with(|s| matches!(s.borrow().pending.as_ref().map(|p| &p.0), Some(FileDialogRequest::Save { .. })));
    let album_export = STATE.with(|s| match s.borrow().pending.as_ref().map(|p| &p.0) {
        Some(FileDialogRequest::Save { suggested }) => super::model::album_export_name(suggested),
        _ => None,
    });
    let mut action = 0;
    let mut selected = None;
    egui::Window::new(if save { text("Save image", "保存图片") } else { text("Open image", "打开图片") })
        .id(egui::Id::new("photocraft-open-source"))
        .collapsible(false)
        .resizable(true)
        .default_width(560.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            if !album {
                ui.vertical(|ui| {
                    if ui
                        .button(if save {
                            text("Save to this computer…", "保存到本机…")
                        } else {
                            text("Open from this computer…", "从本机打开…")
                        })
                        .clicked()
                    {
                        action = 1;
                    }
                    if ui.button(if save { text("Save to VOS…", "保存到 VOS…") } else { text("Open from VOS…", "从 VOS 打开…") }).clicked() {
                        action = 2;
                    }
                    if !save && ui.button(text("Choose from AI Album…", "从 AI 相册选择…")).clicked() {
                        action = 3;
                    }
                    if save {
                        if album_export.is_some() {
                            if ui.button(text("Export image to AI Album", "导出成品图片到 AI 相册")).clicked() {
                                action = 8;
                            }
                        } else {
                            ui.label(text(
                                "To send a finished image to AI Album, use Export as PNG, JPEG or WebP.",
                                "要发送成品到 AI 相册，请使用“导出”为 PNG、JPEG 或 WebP。",
                            ));
                        }
                    }
                });
            } else {
                ui.label(text("AI Album · Your images", "AI 相册 · 我的图片"));
                ui.label(text(
                    "Edit a copy; save to this computer or VOS, or export a new image to AI Album. The original stays in AI Album.",
                    "编辑副本，可保存到本机、VOS，或导出新图片到相册；原图保留。",
                ));
                let (loading, error, items, page, has_next) = STATE.with(|s| {
                    let s = s.borrow();
                    (s.loading, s.error.clone(), s.items.clone(), s.page, s.has_next)
                });
                if loading {
                    ui.spinner();
                }
                let failed = error.is_some();
                if let Some(error) = error {
                    ui.label(error);
                    if ui.button(text("Retry", "重试")).clicked() {
                        action = 4;
                    }
                }
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    if items.is_empty() && !loading && !failed {
                        ui.label(text("No images in this account", "当前账户没有图片"));
                    }
                    for asset in items {
                        ui.horizontal(|ui| {
                            if let Some(texture) = STATE.with(|s| s.borrow().thumbnails.get(&asset.id).cloned())
                                && ui.add(egui::Button::image(egui::Image::new((texture.id(), texture.size_vec2())))).clicked()
                            {
                                selected = Some(asset.clone());
                            }
                            if ui.add_enabled(!loading, egui::Button::new(&asset.name)).clicked() {
                                selected = Some(asset);
                            }
                        });
                    }
                });
                ui.horizontal(|ui| {
                    if ui.add_enabled(page > 1 && !loading, egui::Button::new(text("Previous", "上一页"))).clicked() {
                        action = 5;
                    }
                    ui.label(page.to_string());
                    if ui.add_enabled(has_next && !loading, egui::Button::new(text("Next", "下一页"))).clicked() {
                        action = 6;
                    }
                });
            }
            ui.separator();
            if ui.button(text("Cancel", "取消")).clicked() {
                action = 7;
            }
        });
    if let Some(asset) = selected {
        open(asset, ctx);
        return;
    }
    match action {
        1 => {
            let pending = STATE.with(|s| s.borrow_mut().pending.take());
            if let Some((request, reply)) = pending {
                STATE.with(|s| {
                    if let Some(browser) = s.borrow_mut().browser.as_mut() {
                        browser(request, None, reply);
                    } else {
                        reply.send(Some(FileDialogAnswer::Unavailable("Browser file picker unavailable".into())));
                    }
                });
            }
        }
        2 => {
            if let Some((request, reply)) = STATE.with(|s| s.borrow_mut().pending.take()) {
                super::install::show_dialog(request, reply);
            }
        }
        3 => {
            STATE.with(|s| s.borrow_mut().album = true);
            load(ctx);
        }
        4 => load(ctx),
        5 | 6 => {
            STATE.with(|s| {
                let mut s = s.borrow_mut();
                if action == 5 {
                    s.page = s.page.saturating_sub(1).max(1);
                } else {
                    s.page += 1;
                }
            });
            load(ctx);
        }
        8 => {
            if let Some(name) = album_export
                && let Some((_, reply)) = STATE.with(|s| s.borrow_mut().pending.take())
            {
                reply.send(Some(FileDialogAnswer::SaveTo(format!("{ALBUM_PREFIX}{name}"))));
            }
        }
        7 => {
            STATE.with(|s| {
                let mut s = s.borrow_mut();
                s.pending.take();
                s.generation += 1;
            });
        }
        _ => {}
    }
}

/// Upload the encoded export through the same per-user album session, then verify the new asset.
pub async fn upload(path: &str, bytes: &[u8]) -> Result<(), super::model::Failure> {
    async fn send(path: &str, bytes: &[u8]) -> Result<(), String> {
        let name = path.strip_prefix(ALBUM_PREFIX).ok_or("Invalid AI Album destination")?;
        let token = session().await?;
        let form = web_sys::FormData::new().map_err(|_| "Cannot prepare AI Album upload")?;
        let now = js_sys::Date::new_0().to_iso_string().as_string().ok_or("Cannot timestamp image")?;
        let id = format!("photocraft-{}", bridge::random_b64url(16)?);
        for (key, value) in [("deviceAssetId", id.as_str()), ("deviceId", "photocraft"), ("fileCreatedAt", now.as_str()), ("fileModifiedAt", now.as_str())] {
            form.append_with_str(key, value).map_err(|_| "Cannot prepare AI Album upload")?;
        }
        let mime = if name.to_ascii_lowercase().ends_with(".png") {
            "image/png"
        } else if name.to_ascii_lowercase().ends_with(".webp") {
            "image/webp"
        } else {
            "image/jpeg"
        };
        let options = web_sys::BlobPropertyBag::new();
        options.set_type(mime);
        let array = js_sys::Uint8Array::from(bytes);
        let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&js_sys::Array::of1(&array), &options).map_err(|_| "Cannot prepare image")?;
        form.append_with_blob_and_filename("assetData", &blob, name).map_err(|_| "Cannot prepare image")?;
        let headers = web_sys::Headers::new().map_err(|_| "Cannot authorize upload")?;
        headers.set("Authorization", &format!("Bearer {token}")).map_err(|_| "Cannot authorize upload")?;
        let init = web_sys::RequestInit::new();
        init.set_method("POST");
        init.set_headers(&headers);
        init.set_credentials(web_sys::RequestCredentials::Omit);
        init.set_body(&form);
        let window = web_sys::window().ok_or("No browser window")?;
        let response = JsFuture::from(window.fetch_with_str_and_init(&format!("{API}/assets"), &init))
            .await
            .map_err(|_| "AI Album upload failed")?
            .dyn_into::<web_sys::Response>()
            .map_err(|_| "Invalid upload reply")?;
        if !response.ok() {
            STATE.with(|s| s.borrow_mut().token = None);
            return Err(format!("AI Album upload failed (HTTP {}); retry the export", response.status()));
        }
        let value = json(response).await?;
        let id = value["id"].as_str().filter(|id| id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')).ok_or("AI Album did not confirm an image ID")?;
        request("GET", &format!("{API}/assets/{id}"), Some(&token), None).await?;
        Ok(())
    }
    send(path, bytes).await.map_err(|message| super::model::Failure { code: 0, message })
}
