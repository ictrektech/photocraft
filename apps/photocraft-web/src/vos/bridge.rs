//! 平台桥：找到 `window.vos_platform` 并取得应用访问令牌。
//!
//! 平台把 `window.vos_platform` 注入同源 iframe（OIDC fastpath）。我们用它的
//! `api.v1000.oauth2` 跑授权码 + PKCE 流程，并把得到的访问令牌放在 `sessionStorage` 里，所以
//! Finder 调用是**以已登录用户的身份**发出的，由平台执行 RBAC。平台自己的 JWT 从不读取：桥
//! 不会暴露它，我们也不去要。
//!
//! 一切尽力而为、不会致命：没有桥时（本地开发服务器，或直接打开的页面）[`available`] 为 false，
//! 调用方继续用普通的浏览器选择器。
//!
//! PKCE 的摘要用 Rust 计算（[`super::crypto`]），而不是 `crypto.subtle`：后者只存在于安全上下文中，
//! 而用普通 `http://` 访问的 VOS 盒子有 `window.crypto` 却没有 `subtle`。

use wasm_bindgen::JsCast as _;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;

/// manifest 声明的 OAuth 客户端（`ictrek.app/src/manifest.yml` 的 `oauth2.client.id`）。
const CLIENT_ID: &str = "com.ictrek.photocraft";

/// manifest 授予本客户端的范围：`openid` 用于登录，`read` 用于 Finder。访问令牌有效期一小
/// 时，平台拒绝它时 [`renew`] 会重新登录。
const SCOPE: &str = "openid read";
/// 令牌缓存的位置：按标签页，关闭标签页即清（fastpath 指南的建议）。
const TOKEN_KEY: &str = "photocraft.vos.access_token";

/// 页面注入了桥时返回它。
pub fn bridge() -> Option<JsValue> {
    let window = web_sys::window()?;
    let value = js_sys::Reflect::get(&window, &JsValue::from_str("vos_platform")).ok()?;
    (!value.is_undefined() && !value.is_null()).then_some(value)
}

/// app 是否运行在 VOS 里并且桥可用。
pub fn available() -> bool {
    bridge().is_some()
}

/// 桥的 api.v1000.oauth2 对象。
pub fn oauth2() -> Result<JsValue, String> {
    let mut node = bridge().ok_or("not running inside VOS")?;
    for key in ["api", "v1000", "oauth2"] {
        node = get(&node, key)?;
    }
    Ok(node)
}

/// 读取一个 JS 对象的必需属性。
pub fn get(object: &JsValue, key: &str) -> Result<JsValue, String> {
    let value = js_sys::Reflect::get(object, &JsValue::from_str(key)).map_err(|e| describe(&e))?;
    (!value.is_undefined() && !value.is_null()).then_some(value).ok_or_else(|| format!("bridge has no {key}"))
}

/// 调用桥的一个方法，并把它返回的 promise 变成**本 realm** 的 promise。
///
/// 桥的函数创建在父 frame 里、只被注入到我们的 window，所以它们返回的 promise 过不了
/// `instanceof Promise` 检查（`dyn_into` 用的就是它），尽管它是个完全可用的 thenable。
/// `Promise::resolve` 会接纳 thenable 并返回一个本 realm 的 promise，正是 [`JsFuture`] 需要的：
/// 直接强转原始结果会让每个调用都以 "returned no promise" 被拒。
pub fn call(object: &JsValue, method: &str, argument: &JsValue) -> Result<js_sys::Promise<JsValue>, String> {
    let function: js_sys::Function = get(object, method)?.dyn_into().map_err(|_| format!("bridge {method} is not callable"))?;
    let returned = function.call1(object, argument).map_err(|e| format!("bridge {method} failed: {}", describe(&e)))?;
    // 不是 thenable 的值会解析成它自己，所以同步方法照样能用。
    Ok(js_sys::Promise::resolve(&returned))
}

/// 本标签页已经持有的令牌（如果有）。
fn cached_token() -> Option<String> {
    web_sys::window()?.session_storage().ok().flatten()?.get_item(TOKEN_KEY).ok().flatten().filter(|t| !t.is_empty())
}

/// 丢弃令牌（10001 表示它已不被接受）。
pub fn forget_token() {
    if let Some(storage) = web_sys::window().and_then(|w| w.session_storage().ok().flatten()) {
        let _ = storage.remove_item(TOKEN_KEY);
    }
}

/// 存下本标签页刚拿到的令牌，这样在标签页内刷新不用再登录。
fn cache_token(token: &str) {
    if let Some(storage) = web_sys::window().and_then(|w| w.session_storage().ok().flatten()) {
        let _ = storage.set_item(TOKEN_KEY, token);
    }
}

/// 设置 JS 请求对象的一个字符串属性。
fn set(object: &js_sys::Object, key: &str, value: &JsValue) -> Result<(), String> {
    js_sys::Reflect::set(object, &JsValue::from_str(key), value).map(|_| ()).map_err(|e| format!("could not build the VOS request: {}", describe(&e)))
}

/// 带 `bytes` 字节熵的随机 base64url 字符串（PKCE verifier，以及 CSRF 的 `state`）。
///
/// `crypto.getRandomValues` 在任何上下文都能用（只有 `crypto.subtle` 需要 HTTPS），所以 verifier
/// 仍是浏览器的 CSPRNG；对它求摘要是 Rust 做的部分。
fn random_b64url(bytes: usize) -> Result<String, String> {
    let crypto = web_sys::window().ok_or("no window")?.crypto().map_err(|e| describe(&e))?;
    let mut buf = vec![0u8; bytes];
    crypto.get_random_values_with_u8_array(&mut buf).map_err(|e| describe(&e))?;
    Ok(super::crypto::b64url(&buf))
}

/// JavaScript 抛出或拒绝的值的一段简短可读描述。
///
/// 用 `name: message` 而不是 `{value:?}`，后者的调用栈在提示里没法看；取不到就退回值本身，
/// 再退回一个占位串。
pub fn describe(value: &JsValue) -> String {
    let field = |key: &str| js_sys::Reflect::get(value, &JsValue::from_str(key)).ok().and_then(|v| v.as_string());
    let name = field("name").filter(|name| !name.is_empty());
    let message = field("message").filter(|message| !message.is_empty());
    match (name, message) {
        (Some(name), Some(message)) => format!("{name}: {message}"),
        (None, Some(message)) => message,
        (Some(name), None) => name,
        (None, None) => value.as_string().filter(|text| !text.is_empty()).unwrap_or_else(|| "unknown error".to_string()),
    }
}

/// 给 Finder 调用用的访问令牌：缓存的，或者一次新的 PKCE 交换。
pub async fn token() -> Result<String, String> {
    let verifier = random_b64url(32)?;
    authorized(&verifier).await
}

/// 同 [`token`]，但平台已经拒绝过的令牌不会再用。
pub async fn renew() -> Result<String, String> {
    forget_token();
    token().await
}

/// 给这个 verifier 取令牌：缓存的，或者一次证明该 verifier 的 PKCE 交换。
async fn authorized(verifier: &str) -> Result<String, String> {
    if let Some(token) = cached_token() {
        return Ok(token);
    }
    let state = random_b64url(16)?;
    let authorize = js_sys::Object::new();
    set(&authorize, "client_id", &JsValue::from_str(CLIENT_ID))?;
    set(&authorize, "response_type", &JsValue::from_str("code"))?;
    set(&authorize, "scope", &JsValue::from_str(SCOPE))?;
    set(&authorize, "state", &JsValue::from_str(&state))?;
    // S256：base64url(SHA-256(verifier))，本地计算，这样纯 http 页面也能登录。
    let challenge = super::crypto::b64url(&super::crypto::digest(verifier.as_bytes()));
    set(&authorize, "code_challenge", &JsValue::from_str(&challenge))?;
    set(&authorize, "code_challenge_method", &JsValue::from_str("S256"))?;
    let promise = call(&oauth2()?, "authorize", &authorize)?;
    let answer = JsFuture::from(promise).await.map_err(|e| format!("VOS sign-in failed: {}", describe(&e)))?;
    let code = get(&answer, "code")?.as_string().ok_or("VOS returned no authorization code")?;
    let returned = get(&answer, "state").ok().and_then(|s| s.as_string());
    if returned.as_deref() != Some(state.as_str()) {
        return Err("VOS sign-in returned a mismatched state".into());
    }
    exchange(&code, verifier).await
}

/// 用授权码换访问令牌（并存下来）。
async fn exchange(code: &str, verifier: &str) -> Result<String, String> {
    let request = js_sys::Object::new();
    set(&request, "grant_type", &JsValue::from_str("authorization_code"))?;
    set(&request, "code", &JsValue::from_str(code))?;
    set(&request, "client_id", &JsValue::from_str(CLIENT_ID))?;
    set(&request, "code_verifier", &JsValue::from_str(verifier))?;
    let promise = call(&oauth2()?, "token", &request)?;
    let answer = JsFuture::from(promise).await.map_err(|e| format!("VOS token exchange failed: {}", describe(&e)))?;
    let token = get(&answer, "access_token")?.as_string().filter(|t| !t.is_empty()).ok_or("VOS returned no access token")?;
    cache_token(&token);
    Ok(token)
}
