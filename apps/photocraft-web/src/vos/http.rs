//! Finder 的 HTTP 调用：对同源平台 API 发 `fetch`。
//!
//! 每个调用都带 [`super::bridge`] 取来的应用访问令牌。`code` 非零的 `ResultBody` 就是业务
//! 失败（这种情况下 HTTP 仍是 200），所以每个调用都解码信封并把坏码转成消息。
//!
//! `10001` 表示平台拒绝了令牌（已过期，或是本标签页没有刷新凭证的旧令牌）。此时先丢弃令牌
//! 再取一个新的，然后把调用重做一次：用户看到的是一次登录，而不是"请重新登录后再试"的
//! 死路。

use wasm_bindgen::JsCast as _;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;

use super::bridge;
use super::model::{self, Failure, Index, Volume, VolumePage, error_message};
use super::path::FinderPath;

/// 平台 Finder 的基础路径（同源：页面就由平台主机提供）。
const FINDER: &str = "/v1000/finder_next";
/// 按用户列出的卷表，一块卷的 Finder adapter 就是从这里发现的。
const USER_VOLUMES: &str = "/v1000/nas/users/volumes/list";
/// 已登录用户自己的档案，里面带着卷表用作键的 id。
const USERINFO: &str = "/v1000/oauth2/userinfo";

/// 本插件在浏览器里能打开的最大文件。
///
/// wasm 构建只有一个 2-4 GB 的堆，要装下所有东西（页面、文件、合成器的缓冲区），而字节在
/// 读入途中会被存两份（先 `Blob` 再 `Uint8Array`），所以一个文件最多占下限的四分之一。
pub const MAX_DOWNLOAD: u64 = 256 * 1024 * 1024;

/// 浏览器打不开 `bytes` 字节的文件时的原因（[`model::BAD_PATH`] 失败），能打开则为 `None`。
pub fn too_big(bytes: u64) -> Option<Failure> {
    (bytes > MAX_DOWNLOAD).then(|| Failure {
        code: model::BAD_PATH,
        message: format!("This document is {} and the browser can open at most {}.", super::picker::human_size(bytes), super::picker::human_size(MAX_DOWNLOAD)),
    })
}

/// 把浏览器异常转成消息。
fn js(e: JsValue) -> String {
    // 用 `name: message` 而不是 `{e:?}`，后者的调用栈会占满选择器的一行标签。
    bridge::describe(&e)
}

/// 把桥自身的失败（没有桥、登录被取消）转成 [`Failure`]。
async fn token() -> Result<String, Failure> {
    bridge::token().await.map_err(|e| broke(JsValue::from_str(&e)))
}

async fn renew() -> Result<String, Failure> {
    bridge::renew().await.map_err(|e| broke(JsValue::from_str(&e)))
}

/// 把浏览器异常转成 [`Failure`]：code 0 表示"根本不是 VOS 的回答"（见 [`hopeless`]）。
fn broke(e: JsValue) -> Failure {
    Failure { code: 0, message: js(e) }
}

/// 通过 [`model::decode`] 解码响应，会话过期（`10001` 不能拿同一个令牌重试）时丢弃缓存的令牌。
fn body<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, Failure> {
    model::decode(text).inspect_err(|failure| {
        if failure.is_unauthorized() {
            bridge::forget_token();
        }
    })
}

/// 响应是否报告令牌被拒（`10001`）。是的话，每个调用都会换新令牌重试一次（见模块说明）。
fn refused(text: &str) -> bool {
    model::failure(text).is_some_and(|failure| failure.is_unauthorized())
}

async fn fetch_with(init: &web_sys::RequestInit, url: &str) -> Result<web_sys::Response, Failure> {
    let request = web_sys::Request::new_with_str_and_init(url, init).map_err(broke)?;
    let window = web_sys::window().ok_or_else(|| broke(JsValue::from_str("no window")))?;
    let response = JsFuture::from(window.fetch_with_request(&request)).await.map_err(broke)?;
    response.dyn_into().map_err(|_| broke(JsValue::from_str("not a Response")))
}

async fn send(init: &web_sys::RequestInit, url: &str) -> Result<web_sys::Response, Failure> {
    let response = fetch_with(init, url).await?;
    // 在读取正文之前就判断：SSE 接口把 10001 发成 `error` 事件，而那永远不会变成响应文本，
    // 交给解码器只会把 403 报成空响应或无法解析的响应，并且永远不换新令牌重试。

    if matches!(response.status(), 401 | 403) {
        bridge::forget_token();
        return Err(Failure { code: 10001, message: error_message(10001, "") });
    }
    Ok(response)
}

/// 响应的 JSON，交给解码器（先拆掉 SSE 接口的封装）。
///
/// `what` 是正文不是 JSON 时的出处提示（如 `"VOS upload"`）。
/// 拆包与出处标注都在 [`model::reply_body`] 里，所以能在宿主机测试。
async fn reply(init: &web_sys::RequestInit, url: &str, what: &str) -> Result<String, Failure> {
    let response = send(init, url).await?;
    let text = finish(response.text()).await.map_err(|e| broke(JsValue::from_str(&e)))?;
    let body = text.as_string().ok_or_else(|| Failure { code: model::BAD_PATH, message: format!("{what} returned no body (HTTP {})", response.status()) })?;
    Ok(model::reply_body(&body, what))
}

/// 带上 `token` 的请求初始化参数。
fn authorized(method: &str, token: &str) -> Result<web_sys::RequestInit, Failure> {
    let init = web_sys::RequestInit::new();
    init.set_method(method);
    let headers = web_sys::Headers::new().map_err(broke)?;
    headers.set("Authorization", &format!("Bearer {token}")).map_err(broke)?;
    // 没有这个头时，平台对 POST 会回一段 54 字节的纯文本报错
    // （"Expected request with `Content-Type: application/json`"），接下来就被读成无法解析的
    // 响应。GET 会忽略它。
    headers.set("Content-Type", "application/json").map_err(broke)?;
    init.set_headers(&headers);
    Ok(init)
}

/// 一次 GET；平台报告令牌被拒时换新令牌重试一次。
async fn get(url: &str) -> Result<String, Failure> {
    let token = token().await?;
    let text = reply(&authorized("GET", &token)?, url, "VOS").await?;
    if !refused(&text) {
        return Ok(text);
    }
    let token = renew().await?;
    reply(&authorized("GET", &token)?, url, "VOS").await
}

async fn post(url: &str, body: &str) -> Result<String, Failure> {
    let body = JsValue::from_str(body);
    let token = token().await?;
    let text = send_body(url, &body, &token).await?;
    if !refused(&text) {
        return Ok(text);
    }
    let token = renew().await?;
    send_body(url, &body, &token).await
}

/// 用 `token` POST 一份 `body`，检查响应并拆掉 SSE 封装。
async fn send_body(url: &str, body: &JsValue, token: &str) -> Result<String, Failure> {
    let init = authorized("POST", token)?;
    init.set_body(body);
    reply(&init, url, "VOS").await
}

/// 列出一个目录。平台分开接收 adapter 和路径：路径是完整的 Finder 路径，平台自己的
/// `fs_path_from_raw` 会把它再剥回去。
pub async fn index(dir: &FinderPath) -> Result<Index, Failure> {
    let url = format!("{FINDER}/index?adapter={}&path={}", model::encode(&dir.adapter), model::encode(&dir.api_path()));
    body(&get(&url).await?)
}

/// `volumes/list` 每页要求返回多少块卷。
const VOLUME_PAGE: u64 = 64;

/// 已登录用户能访问的卷，各自带上 adapter。
///
/// 逐页读到某一页没有新东西为止（这样忽略 `page_num` 的平台不会让它空转），或者达到 `total`。
pub async fn volumes() -> Result<Vec<Volume>, Failure> {
    let sub = user_sub().await?;
    let mut volumes: Vec<Volume> = Vec::new();
    let mut page_num = 1;
    loop {
        let request = serde_json::json!({ "user_id": sub, "page_num": page_num, "page_size": VOLUME_PAGE }).to_string();
        let page: VolumePage = body(&post(USER_VOLUMES, &request).await?)?;
        let before = volumes.len();
        volumes.extend(page.volumes);
        if volumes.len() == before || volumes.len() as u64 >= page.total {
            return Ok(volumes);
        }
        page_num += 1;
    }
}

/// 已登录用户的 id（`sub`），卷表以它为键。
///
/// 在这里取而不是走桥的 `userinfo`（它在 JS 里解析响应）：该接口同源、接受普通 bearer
/// 令牌，并且把拒绝回成 HTTP 200 的错误信封，所以正文在 Rust 里读取并解码（见
/// `model::userinfo_sub`）。
async fn user_sub() -> Result<String, Failure> {
    let text = get(USERINFO).await?;
    // 成功是纯 OIDC 档案，失败是 `ResultBody` 错误信封。
    let failure = match model::userinfo_sub(&text) {
        Ok(sub) => return Ok(sub),
        Err(failure) => failure,
    };
    if failure.is_unauthorized() {
        bridge::forget_token();
    }
    Err(failure)
}

/// 这个失败是不是重试也不会好，从而首页应当忘掉那一行：路径已不存在（[`Failure::is_missing`]），
/// 或者插件自己拒绝（[`model::BAD_PATH`]：浏览器打不开这么大，或不是 Finder 路径）。令牌被拒
/// 或平台繁忙都不算绝望。
pub fn hopeless(error: &Failure) -> bool {
    error.is_missing() || error.code == model::BAD_PATH
}

/// 下载一个文件的字节。
///
/// `adapter` 必填且必须是真实的 adapter：平台 `prepare` 会以 10045 拒绝空 adapter，所以
/// 既不能省略，也不能从 `path` 里推断。
pub async fn download(adapter: &str, path: &str) -> Result<Vec<u8>, Failure> {
    let url = format!("{FINDER}/download?adapter={}&path={}", model::encode(adapter), model::encode(path));
    // 先按 index 报出的大小拒绝，一个字节都不取：2 GB 的下载会填满堆，把整个标签页拖垮。

    if let Some(size) = listed(adapter, path).await
        && let Some(too_big) = too_big(size)
    {
        return Err(too_big);
    }
    // 正文以 Blob 到达，然后整体读出：分块读取需要 `ReadableStream` 绑定，而真正起约束作用
    // 的是 index 报出的大小。
    let fetch = || async {
        let token = token().await?;
        let response = send(&authorized("GET", &token)?, &url).await?;
        if !response.ok() {
            return Err(http_error(&response).await);
        }
        let blob = response.blob().map_err(broke)?;
        let blob = finish(Ok(blob)).await.map_err(|e| broke(JsValue::from_str(&e)))?;
        blob.dyn_into().map_err(|_| broke(JsValue::from_str("not a Blob")))
    };
    // 和 `reply` 一样重试，也包括 `send` 报告的无正文 SSE `error` 事件。
    let blob: web_sys::Blob = match fetch().await {
        Err(e) if refused_error(&e.to_string()) => fetch().await.map_err(|_| e)?,
        answer => answer?,
    };
    // Blob 已经装着整个正文，此时拦下超大文件省不了内存；它只是不让下一步（在 wasm 内存里
    // 再复制一份完整数据）发生。
    if let Some(too_big) = too_big(blob.size() as u64) {
        return Err(too_big);
    }
    let bytes = finish(Ok(blob.array_buffer())).await.map_err(|e| broke(JsValue::from_str(&e)))?;
    Ok(js_sys::Uint8Array::new(&bytes).to_vec())
}

/// 平台为 `path` 列出的大小，前提是它的目录能列出并且里面有它。
async fn listed(adapter: &str, path: &str) -> Option<u64> {
    let dir = FinderPath::parse_dir(path)?;
    let index: Index = body(&get(&format!("{FINDER}/index?adapter={}&path={}", model::encode(adapter), model::encode(&dir.api_path()))).await.ok()?).ok()?;
    index.files.into_iter().find(|entry| entry.path == path).map(|entry| entry.file_size)
}

/// 下载遇到该 HTTP 状态时的错误；正文是 `ResultBody` 时用平台自己的原因（令牌被拒就回的是
/// 这种）。
async fn http_error(response: &web_sys::Response) -> Failure {
    let line = Failure { code: 0, message: "VOS could not download this document".to_string() };
    match finish(response.text()).await.map(|text| text.as_string()).ok().flatten() {
        Some(text) => model::failure(&text).unwrap_or(line),
        None => line,
    }
}

/// 这个错误是否表示平台拒绝了令牌，因此下载值得重试。
fn refused_error(text: &str) -> bool {
    text.contains("10001") || text.contains("Expired") || text.contains("401")
}

/// 等待一个 promise，拒绝时像 [`js`] 那样描述原因。
async fn finish(promise: Result<js_sys::Promise<JsValue>, JsValue>) -> Result<JsValue, String> {
    JsFuture::from(js_sys::Promise::resolve(&promise.map_err(js)?)).await.map_err(js)
}

/// 把 `target` 移进回收站：平台的删除只是挪到卷里的 `.recycle/` 下，不是抹掉。
///
/// `adapter` 必填，空的话平台以 10045 拒绝。`target` 是卷内的路径：完整 Finder 路径去掉
/// adapter 和 `//` 之后的那段。
pub async fn delete(adapter: &str, target: &str) -> Result<(), Failure> {
    let request = serde_json::json!({ "adapter": adapter, "path": "", "targets": [target] }).to_string();
    let url = format!("{FINDER}/delete");
    let request = JsValue::from_str(&request);
    let token = token().await?;
    let mut text = send_body(&url, &request, &token).await?;
    if refused(&text) {
        let token = renew().await?;
        text = send_body(&url, &request, &token).await?;
    }
    body::<String>(&text).map(|_| ())
}

/// 把 `bytes` 以 `name` 上传到目录 `dir`，替换同名文件。
///
/// `conflict_strategy=overwrite` 必填：平台默认是 `reject`，那样一次普通的 Ctrl+S 就会变成
/// "文件已存在"错误。
pub async fn upload(dir: &FinderPath, name: &str, bytes: &[u8]) -> Result<(), Failure> {
    let array = js_sys::Uint8Array::from(bytes);
    let blob = web_sys::Blob::new_with_u8_array_sequence(&js_sys::Array::of1(&array)).map_err(broke)?;
    let url = format!("{FINDER}/upload");
    let upload = |token: String| {
        let dir = dir.clone();
        let blob = blob.clone();
        let url = url.clone();
        let name = name.to_string();
        async move {
            // 表单每次新建：`RequestInit` 的正文只能用一次，而重试要重发同一份数据。
            let form = web_sys::FormData::new().map_err(broke)?;
            form.append_with_str("adapter", &dir.adapter).map_err(broke)?;
            form.append_with_str("path", &dir.api_path()).map_err(broke)?;
            form.append_with_str("name", &name).map_err(broke)?;
            form.append_with_str("conflict_strategy", "overwrite").map_err(broke)?;
            form.append_with_blob_and_filename("file", &blob, &name).map_err(broke)?;
            send_form(&url, &JsValue::from(form), "VOS upload", &token).await
        }
    };
    // 平台对上传（和别的 POST 一样）报的错可能是纯文本，见 [`model::reply_body`]。
    let mut text = upload(token().await?).await?;
    if refused(&text) {
        text = upload(renew().await?).await?;
    }
    body::<String>(&text).map(|_| ())
}

/// 和 [`send_body`] 一样，但按**表单**发（没有 JSON 的 `Content-Type`：浏览器要自己写带
/// boundary 的那个），并让正文用 `what` 解释读不懂的响应。
async fn send_form(url: &str, form: &JsValue, what: &str, token: &str) -> Result<String, Failure> {
    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    let headers = web_sys::Headers::new().map_err(broke)?;
    headers.set("Authorization", &format!("Bearer {token}")).map_err(broke)?;
    init.set_headers(&headers);
    init.set_body(form);
    reply(&init, url, what).await
}
