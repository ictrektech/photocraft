//! Finder 的 HTTP 约定：响应结构与平台的错误码。
//!
//! 每个 /v1000/finder_next/* 响应都是 ResultBody<T>：{"code": 0, "data": ..., "msg": ...}，
//! 业务失败时 HTTP 状态仍是 200，所以只有 code == 0 才算成功。下面这些错误码是文件浏览
//! 真正会遇到的；措辞就是给用户看的，因为一个裸数字说明不了任何问题。
//!
//! 本模块刻意不碰 web_sys，因此能在宿主机的测试里跑。

use serde::Deserialize;

/// 平台的响应信封。`data` 保持泛型，让每个调用方解码自己的载荷。
///
/// 平台对一部分自身错误（比如卷外的路径）会把 code 写成**字符串**，直接解码会让整个响应
/// 变成"无法解析"。因此 `code` 宽松读取：先按 `Value` 接住，再由 `code()` 解码。
#[derive(Debug, Deserialize)]
pub struct ResultBody<T> {
    /// 缺失时为 `0`；真实取值见 `code()`。
    #[serde(default, deserialize_with = "code_of")]
    pub code: i64,
    #[serde(default)]
    pub msg: String,
    /// 缺 `data` 时解成 `None`，不需要 `T: Default`（所以不加 `#[serde(default)]`）。
    pub data: Option<T>,
}

/// 响应里的 code，也能读平台表示自身错误时用的字符串形式。
pub fn code(value: &serde_json::Value) -> i64 {
    value.as_i64().or_else(|| value.as_str().and_then(|s| s.trim().parse().ok())).unwrap_or(0)
}

fn code_of<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
    Ok(code(&serde_json::Value::deserialize(deserializer)?))
}

/// 一个目录项，即 `index` 返回的形态。
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Entry {
    /// "dir" 或 "file"。
    #[serde(rename = "type")]
    pub kind: String,
    /// 完整路径，可直接交给任何 Finder 接口（<adapter>://<dir>/<name>）。
    pub path: String,
    /// 仅文件名。
    pub basename: String,
    /// 不带点的扩展名（目录或无扩展名文件为空）。
    #[serde(default)]
    pub extension: String,
    /// 字节数（目录为 0）。
    #[serde(default)]
    pub file_size: u64,
    /// 该项所在的 adapter（平台称 `storage`），如 `v_<uuid>`。
    /// 下载时它必须和路径一起给出，平台会拒绝空 adapter。
    #[serde(default)]
    pub storage: String,
}

impl Entry {
    /// 该项是不是目录。
    pub fn is_dir(&self) -> bool {
        self.kind == "dir"
    }

    /// 列表里显示的名字：`basename` 为空时退回路径的最后一段。
    pub fn label(&self) -> std::borrow::Cow<'_, str> {
        if self.basename.trim().is_empty() { super::path::display_name(&self.path).into() } else { self.basename.as_str().into() }
    }
}

/// `index` 的载荷：一个目录里的条目。
///
/// 平台还会回显 adapter 和所列的目录；选择器本来就知道这两者（是它请求的），故不解码。
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Index {
    #[serde(default)]
    pub files: Vec<Entry>,
}

/// 已登录用户可访问的一块卷（存储空间）。
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Volume {
    pub uuid: String,
    #[serde(default)]
    pub adapter: Option<String>,
    #[serde(default)]
    pub nick_name: Option<String>,
    #[serde(default)]
    pub name: String,
}

impl Volume {
    /// 选择器里显示的名字：用户的昵称，没有则用平台名。
    pub fn label(&self) -> &str {
        self.nick_name.as_deref().filter(|n| !n.trim().is_empty()).unwrap_or(&self.name)
    }

    /// 这块卷的 Finder adapter。平台前端在 uuid 前加了 v_。
    pub fn adapter(&self) -> String {
        self.adapter.clone().unwrap_or_else(|| format!("v_{}", self.uuid))
    }
}

/// `volumes/list` 载荷的一页。
///
/// 分页信封是 `{total, pages, page_num, page_size, data}`：数组在 `data` 里，同时接受 `items`
/// 作为旧版本响应的别名。
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct VolumePage {
    /// 用户拥有的卷总数，`page_size` 不一定覆盖得完。
    #[serde(default)]
    pub total: u64,
    #[serde(default, rename = "data", alias = "items")]
    pub volumes: Vec<Volume>,
}

impl std::fmt::Display for Failure {
    /// 给用户看的文本：平台给的原因加上它的码，便于售后凭它查询。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, code_tag(self.code))
    }
}

/// 消息里书写平台码的格式，让展示给用户的失败带上它的码。
pub fn code_tag(code: i64) -> String {
    format!("VOS-{code}")
}

/// 把失败的响应转成值得展示的文案。code == 0 不会走到这里。
///
/// 这里只覆盖浏览会碰到的 Finder 码；未知码保留平台自己的 msg，这样新码也仍然说得出东西。
pub fn error_message(code: i64, msg: &str) -> String {
    let known = match code {
        10001 => "Your VOS session has expired; sign in again and retry",
        10002 => "You do not have permission to access this VOS folder",
        10017 => "VOS rejected the request (bad path or parameters)",
        10045 => "VOS does not recognise this storage location",
        10046 => "You do not have read permission for this VOS folder",
        10047 => "You do not have write permission for this VOS folder",
        10048 => "The VOS storage volume is not available",
        10051 => "A file with this name already exists in VOS",
        10055 => "There is not enough space left in VOS storage",
        _ => "",
    };
    if !known.is_empty() {
        return known.to_string();
    }
    if msg.trim().is_empty() { format!("VOS storage request failed (code {code})") } else { format!("VOS storage request failed: {msg} (code {code})") }
}

/// 一次失败的响应，把平台的码和消息放在一起。
///
/// 调用方根据码行事（`10001` 表示缓存的 token 已失效，必须丢弃），消息只给用户看。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub code: i64,
    pub message: String,
}

/// 插件给"从未到达平台"的失败用的码：浏览器异常、响应无法解析、文件太大无法打开、
/// 不是 Finder 的路径。
/// [`Failure::is_missing`] 据此把它排除在"稍后重试"那一类之外。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))] // only the browser refuses a path
pub const BAD_PATH: i64 = 0;

impl Failure {
    /// 重试前是否必须先刷新会话。
    pub fn is_unauthorized(&self) -> bool {
        self.code == 10001
    }

    /// 平台说路径本身已不存在（在别处被移动或删除）时置位，首页据此忘掉它，而不是继续摆一个
    /// 永远打不开的条目。
    pub fn is_missing(&self) -> bool {
        self.code == 10017
    }

    /// 平台说这个路径不存在（`ItemNotExist`）。
    pub fn is_not_found(&self) -> bool {
        self.code == 10007
    }
}

/// 对查询参数值做百分号编码（路径里带 `/`、空格和非 ASCII 名字）。
///
/// 可在宿主机测试，所以放在这里而不是仅限 wasm 的 `http` 模块。
pub fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            // RFC 3986 的非保留字符，加上在查询里可以原样出现的子分隔符。
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'!' | b'*' | b'/' | b':' => out.push(*byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// 响应正文里的 JSON 信封，返回**第一个完整的 JSON 值**。
///
/// 认两种形状：
///
/// - **SSE**：`GET /v1000/finder_next/index` 与 `progress/*` 的正文以事件行开头
///   （`data: {...}`，前面可能有 `event:` / `id:` 行）。
/// - **网关切分的普通 JSON**：gzip 后解码出来可能是 `{...}\t\nDATA:{...}`（每段一份压缩
///   流），后续段落是网关分片，不是业务数据。
///
/// 首尾空白和 BOM 在拆包前就去掉：`data:` 只认行首。
///
/// 每个响应都走这里（`http.rs` 也在这里拆包），这样在解码任何载荷之前就能认出被拒的 token。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))] // the browser's replies only
pub fn sse_payload(body: &str) -> String {
    // SSE 允许 CRLF 换行；先统一，之后按哪种都切得对。
    let normalised = body.replace("\r\n", "\n");
    let normalised = normalised.trim_start_matches(|ch: char| ch.is_whitespace() || ch == '\u{feff}');
    let normalised = normalised.trim_end();
    // 正文以 `{` 开头：就是（可能被网关切分的）普通 JSON，取第一个完整的值。
    if normalised.starts_with('{') {
        return json_value(normalised).unwrap_or(normalised).to_string();
    }
    // 否则只有以事件行开头的正文才是 SSE；`data:` 出现在正文中间的是纯文本或 JSON 字符串。
    let Some(data) = sse_data(normalised) else {
        return normalised.to_string();
    };
    // 取不到完整值也把 `data:` 前缀去掉再交出去：否则 serde 只会报在 column 1，而那是
    // `data:` 的 `d`。
    json_value(data).unwrap_or(data).to_string()
}

/// `body`（已去掉前导空白）里第一个 `data:` 字段的内容，前提是它是 SSE 事件帧。
///
/// 只看**行首**的字段：事件帧以字段行开头（`data:`、`event:`、`id:`、`retry:` 或注释行），
/// 正文中间出现的 `data:` 属于纯文本或 JSON 字符串，不是事件。
fn sse_data(body: &str) -> Option<&str> {
    let mut lines = body.lines();
    let mut seen_field = false;
    for line in lines.by_ref() {
        let field = line.trim_start_matches([' ', '\t']);
        if let Some(payload) = field.strip_prefix("data:") {
            return Some(payload.trim_start());
        }
        // 事件帧可以以别的字段或注释行开头；数据行之前出现空行说明这不是事件流。
        if field.starts_with("event:") || field.starts_with("id:") || field.starts_with("retry:") || field.starts_with(':') && !field.is_empty() {
            seen_field = true;
            continue;
        }
        if field.is_empty() && seen_field {
            continue;
        }
        return None;
    }
    None
}

/// `rest`（`data:` 之后的正文）里第一个完整的 JSON 对象。
///
/// 按括号配对而不是按行或按空行：字符串里的花括号和转义不算数。
fn json_value(rest: &str) -> Option<&str> {
    let data = rest.trim_start();
    let mut depth = 0i64;
    let mut in_string = false;
    let mut escaped = false;
    let mut end = None;
    for (offset, ch) in data.char_indices() {
        if in_string {
            match ch {
                '\\' if !escaped => escaped = true,
                '"' if !escaped => in_string = false,
                _ => escaped = false,
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' | '[' => depth += 1,
            '}' | ']' => {
                depth -= 1;
                if depth <= 0 {
                    end = Some(offset + ch.len_utf8());
                    break;
                }
            }
            _ => {}
        }
    }
    data.get(..end?)
}

/// 一次响应的正文，拆掉 SSE/网关封装，并给读不懂的纯文本标上出处。
///
/// 顺序不能反：先给非 `{` 开头的内容加前缀，`data: {...}` 就会变成 `VOS: data: {...}`，
/// [`sse_payload`] 再也认不出事件。
///
/// `what` 是正文不是 JSON 时的出处提示（如 `"VOS upload"`，平台对上传报的是纯文本）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))] // the browser's replies only
pub fn reply_body(body: &str, what: &str) -> String {
    let unwrapped = sse_payload(body);
    let text = unwrapped.trim();
    if text.is_empty() || text.starts_with('{') {
        return unwrapped;
    }
    // 拆包后仍不是 JSON：事件帧不能加前缀（读不懂的原因在帧里，不在“平台回了纯文本”上）。
    // 在**原始**正文上看，因为拆包结果已经把 `data:` 剥掉了。
    if sse_data(raw_trimmed(body)).is_some() {
        return unwrapped;
    }
    format!("{what}: {text}")
}

/// 去掉前导空白和 BOM，与 [`sse_payload`] 的入口一致。
fn raw_trimmed(body: &str) -> &str {
    body.trim_start_matches(|ch: char| ch.is_whitespace() || ch == '\u{feff}')
}

/// `userinfo` 回复里的 `sub`，或者平台拒绝它的原因。
///
/// `GET /v1000/oauth2/userinfo` 成功时回的是纯 OIDC claims，失败时则是 `ResultBody` 错误信封，
/// 而且 HTTP 状态始终是 200，`fetch` 两种情况都算成功。
/// token 过期时回 `{"code":10001,...,"msg":"RS256 JWT 校验失败: ExpiredSignature"}`，完全没有
/// `sub`；正常时回 `{"sub":"01M31...","preferred_username":"..."}`。
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub fn userinfo_sub(text: &str) -> Result<String, Failure> {
    #[derive(Deserialize)]
    struct Userinfo {
        /// 数字或字符串；缺失为 0。
        #[serde(default)]
        code: serde_json::Value,
        #[serde(default)]
        msg: String,
        #[serde(default)]
        sub: Option<String>,
    }
    let info: Userinfo = json(text).map_err(|failure| Failure { code: 0, message: failure.message.replace("reply", "user profile") })?;
    // 部分平台版本的整个档案响应里 code 是字符串而不是数字。
    let code = code(&info.code);
    if code != 0 {
        return Err(Failure { code, message: error_message(code, &info.msg) });
    }
    match info.sub.filter(|sub| !sub.is_empty()) {
        Some(sub) => Ok(sub),
        None => Err(Failure { code: 0, message: "VOS returned a user profile with no user id".to_string() }),
    }
}

/// 把响应信封解码成载荷，业务码则转成 [`Failure`]。
///
/// 业务失败时 HTTP 仍是 200，所以这里是唯一能发现调用失败的地方；它可在宿主机测试，
/// 不像调用它的浏览器模块。
pub fn decode<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, Failure> {
    let body: ResultBody<T> = json(text)?;
    check(body.code, &body.msg)?;
    body.data.ok_or_else(|| Failure { code: 0, message: "VOS returned an empty reply".to_string() })
}

/// 响应报告的失败，不解码载荷：足以分清被拒的 token 和其他失败，而无须解码那些因同一原因
/// 本来就会解码失败的载荷。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))] // the retry path is the browser's
pub fn failure(text: &str) -> Option<Failure> {
    json::<Envelope>(text).ok().and_then(Envelope::failure)
}

/// 解析响应的 JSON，不管该接口用哪种封装。
fn json<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, Failure> {
    serde_json::from_str(&sse_payload(text)).map_err(|e| Failure { code: 0, message: format!("VOS returned an unreadable reply: {e} | {}", snippet(text)) })
}

/// 响应的开头，用于错误消息：足以看出到底收到了什么，又短到能塞进选择器的一行标签。
/// 不会是整个文件，也不会是 token（响应不会回显请求头）。
fn snippet(text: &str) -> String {
    let text = text.trim_start_matches(|ch: char| ch.is_whitespace() || ch == '\u{feff}');
    let escaped = text.escape_debug().to_string();
    let cut = escaped.char_indices().nth(80).map_or(escaped.len(), |(at, _)| at);
    let cut = escaped.get(..cut).unwrap_or("");
    format!("{cut} ({} bytes)", text.len())
}

/// 响应里的 `{code, msg}` 部分（每个接口都是这个形状）。
#[derive(Deserialize)]
struct Envelope {
    #[serde(default)]
    code: serde_json::Value,
    #[serde(default)]
    msg: String,
}

impl Envelope {
    fn failure(self) -> Option<Failure> {
        match code(&self.code) {
            0 => None,
            code => Some(Failure { code, message: error_message(code, &self.msg) }),
        }
    }
}

fn check(code: i64, msg: &str) -> Result<(), Failure> {
    match code {
        0 => Ok(()),
        code => Err(Failure { code, message: error_message(code, msg) }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_real_index_reply() {
        // 形状与平台自己的 Finder 响应完全一致（endpoints/finder/index.md）。
        let json = r#"{
            "code": 0,
            "msg": "",
            "data": {
                "adapter": "v_01KYS",
                "storages": [],
                "dirname": "v_01KYS://pics",
                "files": [
                    {"type": "dir", "path": "v_01KYS://pics/raw", "basename": "raw", "extension": "", "file_size": 4096},
                    {"type": "file", "path": "v_01KYS://pics/a.psd", "basename": "a.psd", "extension": "psd", "file_size": 1234}
                ],
                "sort_by": "name",
                "sort_order": "asc"
            }
        }"#;
        let body: ResultBody<Index> = serde_json::from_str(json).expect("a valid reply");
        assert_eq!(body.code, 0);
        let index = body.data.expect("data");
        assert_eq!(index.files.len(), 2);
        assert!(index.files[0].is_dir());
        assert!(!index.files[1].is_dir());
        assert_eq!(index.files[1].file_size, 1234);
    }

    #[test]
    fn an_error_reply_carries_no_data() {
        let json = r#"{"code": 10046, "data": null, "msg": "no access"}"#;
        let body: ResultBody<Index> = serde_json::from_str(json).expect("a valid reply");
        assert_eq!(body.code, 10046);
        assert!(body.data.is_none());
    }

    /// 线上平台 `POST /v1000/nas/users/volumes/list` 的原始响应，裁到一页一个卷：分页信封和
    /// 里面的数组都叫 `data`。
    #[test]
    fn decodes_a_volume_page_and_builds_adapters() {
        let json = r#"{"code":0,"category":"ok","data":{"total":1,"pages":1,"page_num":1,"page_size":64,"data":[{"uuid":"01M31B5XDFHPXSKCBSXAZVZA83","name":"01M31B5XDFHPXSKCBSXAZVZA83","nick_name":"vos_workspace","volume_type":"local_dir","state":"Running","alias":[],"quota_enabled":false,"quota_enabled_desired":false,"backend":{"source_type":"local_path","source":"/data/vos_workspace","pool_id":null},"mount":{"target":"/share/01M31B5XDFHPXSKCBSXAZVZA83","fs":"Ext4","options":"defaults"},"extra":{"description":""}}]},"msg":"","version":"1.1.0"}"#;
        let body: ResultBody<VolumePage> = serde_json::from_str(json).expect("a valid reply");
        let page = body.data.expect("data");
        assert_eq!(page.volumes.len(), 1);
        assert_eq!(page.volumes[0].adapter(), "v_01M31B5XDFHPXSKCBSXAZVZA83");
        assert_eq!(page.volumes[0].label(), "vos_workspace");
    }

    /// 线上平台 `GET /v1000/finder_next/index` 的原始正文，含 SSE 封装。
    #[test]
    fn an_sse_index_reply_is_unwrapped_and_decoded() {
        let body = "data: {\"code\":0,\"category\":\"ok\",\"data\":{\"adapter\":\"v_01M31\",\"storages\":[],\"dirname\":\"v_01M31://\",\"files\":[{\"type\":\"dir\",\"path\":\"v_01M31://probe_folder\",\"basename\":\"probe_folder\",\"extension\":\"\",\"storage\":\"v_01M31\",\"file_size\":4096}],\"sort_by\":\"name\",\"sort_order\":\"asc\"},\"msg\":\"\",\"version\":\"1.1.0\"}\n\n";
        let index: Index = decode(&sse_payload(body)).expect("a decodable payload");
        assert_eq!(index.files.len(), 1);
        assert_eq!(index.files[0].basename, "probe_folder");
        assert!(index.files[0].is_dir());
        assert_eq!(index.files[0].storage, "v_01M31");
    }

    /// 被压缩成一行的两个 SSE 事件：中间只有一个 tab，没有空行。
    #[test]
    fn a_compressed_sse_reply_stops_at_the_end_of_the_payload() {
        let body =
            "data: {\"code\":0,\"category\":\"ok\",\"data\":{\"adapter\":\"v_01M31\",\"path\":\"v_01M31://a\\\"b\"},\"msg\":\"\"}\there:data: {\"code\":0}\n\n";
        assert_eq!(sse_payload(body), "{\"code\":0,\"category\":\"ok\",\"data\":{\"adapter\":\"v_01M31\",\"path\":\"v_01M31://a\\\"b\"},\"msg\":\"\"}");
    }

    /// 普通 JSON 被网关 gzip 后解码出来的拼接正文（压缩段 + 明文段）。
    #[test]
    fn a_json_reply_split_by_a_gateway_keeps_its_first_value() {
        let payload = "{\"code\":0,\"category\":\"ok\",\"data\":\"\",\"msg\":\"\",\"version\":\"0.153.0\"}";
        let body = format!("{payload}\t\nDATA:{payload}");
        assert_eq!(sse_payload(&body), payload);
        let envelope: Envelope = serde_json::from_str(&sse_payload(&body)).expect("the reply decodes");
        assert_eq!(envelope.failure().map(|failure| failure.code), None);
    }

    /// 六种真实响应形状都走完 `reply_body` 后能解码。
    #[test]
    fn live_reply_shapes_decode_end_to_end() {
        // `GET /v1000/finder_next/index` 的信封，以及网关可能加上的前缀。
        let index = r#"{"code":0,"category":"ok","data":{"adapter":"v_01M31B5XDFHPXSKCBSXAZVZA83","storages":[],"dirname":"v_01M31B5XDFHPXSKCBSXAZVZA83://","files":[{"type":"dir","path":"v_01M31B5XDFHPXSKCBSXAZVZA83://probe_folder","basename":"probe_folder","extension":"","storage":"v_01M31B5XDFHPXSKCBSXAZVZA83","file_size":4096}],"sort_by":"name","sort_order":"asc"},"msg":"","version":"1.1.0-20261011.1.f0232367"}"#;
        for body in [
            format!("data: {index}\n\n"),
            format!(" data: {index}\n\n"),
            format!("\tdata: {index}\n\n"),
            format!("\u{feff}data: {index}\n\n"),
            format!("data: {index}"),
            index.to_string(),
        ] {
            let decoded = decode::<Index>(&reply_body(&body, "VOS"));
            assert!(decoded.is_ok(), "{body:.90} must decode: {:?}", decoded.err().map(|e| e.message));
            assert_eq!(decoded.expect("decoded").files.len(), 1);
        }
    }

    /// 事件帧只在正文以字段行**开头**时成立；正文中间的 `data:` 不算。
    #[test]
    fn a_data_field_inside_a_body_is_not_an_event_frame() {
        assert!(sse_data(raw_trimmed("data: {\"code\":0}")).is_some());
        assert!(sse_data(raw_trimmed("  \tevent: message\ndata: {}")).is_some());
        assert!(sse_data(raw_trimmed("\u{feff}data:{}")).is_some());
        assert!(sse_data(raw_trimmed("upstream said no\ndata: reset")).is_none());
        // 一段纯文本错误，中间恰好有 `\ndata:`：原样返回，并带上出处。
        let body = "upstream said no\ndata: connection reset";
        assert_eq!(reply_body(body, "VOS upload"), format!("VOS upload: {body}"));
    }

    /// 纯文本错误带上出处；空正文、信封和 SSE 帧都不加。
    #[test]
    fn a_plain_text_reply_is_still_attributed() {
        let body = "Invalid 'boundary' for 'multipart/form-data' request";
        assert_eq!(reply_body(body, "VOS upload"), format!("VOS upload: {body}"));
        assert_eq!(reply_body("", "VOS"), "");
        assert_eq!(reply_body("{\"code\":0}", "VOS"), "{\"code\":0}");
        // SSE 帧：`data:` 剥掉、不加前缀，报错才落得进 JSON 内部。
        assert_eq!(reply_body("data: {truncated", "VOS"), "{truncated");
        assert_eq!(reply_body("data: broken", "VOS"), "broken");
    }

    #[test]
    fn a_crlf_sse_reply_is_unwrapped_too() {
        assert_eq!(sse_payload("data:{\"code\":0}\r\n\r\n"), "{\"code\":0}");
    }

    /// 截断的 SSE 帧：`data:` 前缀去掉，错误位置落在 JSON 内部而不是 `data:` 的 `d`。
    #[test]
    fn an_unwrapable_sse_frame_is_not_dressed_up_as_plain_text() {
        let body = "data: {\"code\":0,\"data\":{\"files\":[";
        let unwrapped = sse_payload(body);
        assert!(unwrapped.starts_with('{'), "the `data:` prefix must go: {unwrapped:?}");
        let failure = decode::<Index>(&unwrapped).expect_err("a truncated frame must not decode");
        assert!(!failure.message.contains("line 1 column 1"), "{}", failure.message);
    }

    /// 合法的信封即使 `data:` 前有空格或 BOM 也能解码。
    #[test]
    fn the_reported_unreadable_reply_can_no_longer_be_produced() {
        let payload = "{\"code\":0,\"category\":\"ok\",\"data\":{\"adapter\":\"v_01M31B5XDFHPXSKCBSXAZVZA83\",\"files\":[]},\"msg\":\"\"}";
        for body in [format!("data: {payload}\n\n"), format!("  data: {payload}\n\n"), format!("\u{feff}data: {payload}\n\n")] {
            let unwrapped = sse_payload(&body);
            let decoded = decode::<Index>(&unwrapped);
            assert!(decoded.is_ok(), "{body:?} must decode, got {:?}", decoded.err().map(|e| e.message));
        }
    }

    #[test]
    fn a_plain_json_reply_is_left_alone() {
        let body = "{\"code\":0,\"data\":{}}";
        assert_eq!(sse_payload(body), body);
    }

    /// `basename` 为空或纯空白时，名字取路径的最后一段。
    #[test]
    fn an_entry_without_a_basename_falls_back_to_its_path() {
        let mut entry = Entry {
            kind: "file".into(),
            path: "v_vol1://pics/poster.psd".into(),
            basename: String::new(),
            extension: "psd".into(),
            file_size: 573_900,
            storage: "v_vol1".into(),
        };
        assert_eq!(entry.label(), "poster.psd");
        entry.basename = "   ".into();
        assert_eq!(entry.label(), "poster.psd", "whitespace is not a name either");
        entry.basename = "num.psd".into();
        assert_eq!(entry.label(), "num.psd");
    }

    #[test]
    fn a_page_with_no_data_decodes_to_an_empty_list() {
        let json = r#"{"code": 0, "data": {"total": 0, "pages": 1, "page_num": 1, "page_size": 64, "data": []}}"#;
        let body: ResultBody<VolumePage> = serde_json::from_str(json).expect("a valid reply");
        assert!(body.data.expect("data").volumes.is_empty());
    }

    #[test]
    fn a_volume_without_a_nickname_falls_back_to_its_name() {
        let v = Volume { uuid: "u".into(), adapter: None, nick_name: Some("   ".into()), name: "Volume 1".into() };
        assert_eq!(v.label(), "Volume 1");
        let v = Volume { uuid: "u".into(), adapter: None, nick_name: None, name: "Volume 2".into() };
        assert_eq!(v.label(), "Volume 2");
    }

    #[test]
    fn known_error_codes_read_as_sentences() {
        for code in [10001, 10002, 10017, 10045, 10046, 10047, 10048, 10051, 10055] {
            let text = error_message(code, "");
            assert!(!text.contains("code"), "{code} should be a plain sentence, got {text}");
        }
        assert!(error_message(10001, "").contains("session"));
    }

    #[test]
    fn an_unknown_code_still_says_something_useful() {
        let text = error_message(10999, "server error");
        assert!(text.contains("10999"), "{text}");
        assert!(text.contains("server error"), "{text}");
        let bare = error_message(10999, "");
        assert!(bare.contains("10999"), "{bare}");
    }

    /// 百分号编码在宿主机上也会编译并覆盖测试。
    #[test]
    fn query_values_are_percent_encoded() {
        assert_eq!(encode("v_abc://pics/a.psd"), "v_abc://pics/a.psd", "the ones the query accepts unescaped stay");
        assert_eq!(encode("a b"), "a%20b");
        assert_eq!(encode("A-Z_a.z~0"), "A-Z_a.z~0", "unreserved characters stay");
        assert_eq!(encode("a&b=c+d"), "a%26b%3Dc%2Bd", "the query's own separators do not");
        assert_eq!(encode("中文"), "%E4%B8%AD%E6%96%87");
    }

    /// 用户看到的文本带码，售后报告才有可查的东西。
    #[test]
    fn a_failure_shows_its_message_and_its_code() {
        let gone = Failure { code: 10017, message: error_message(10017, "") }.to_string();
        assert!(gone.ends_with("(VOS-10017)"), "{gone}");
        assert_eq!(code_tag(10001), "VOS-10001");
    }

    #[test]
    fn a_gone_path_is_told_apart_from_another_failure() {
        assert!(Failure { code: 10017, message: String::new() }.is_missing());
        assert!(!Failure { code: 10046, message: String::new() }.is_missing());
    }

    /// 线上平台原样：有效 token 回的是纯 OIDC claims。
    #[test]
    fn userinfo_yields_the_sub_of_a_valid_profile() {
        let json = r#"{"sub":"01M31NHQT4W6HXABAYWM7T4JSE","preferred_username":"wangrui","nickname":"wangrui","email":"","email_verified":false,"phone_number":"","role_code":"normal_user","enable":1}"#;
        assert_eq!(userinfo_sub(json).expect("a sub"), "01M31NHQT4W6HXABAYWM7T4JSE");
    }

    /// 线上平台原样：被拒的 token 回的是 HTTP 200 的错误信封，里面根本没有 sub。
    #[test]
    fn userinfo_reports_the_platform_reason_for_a_rejected_token() {
        let json = r#"{"code":10001,"category":"system","data":{"backtrace":"at src/auth/verifier.rs:164"},"msg":"RS256 JWT 校验失败: ExpiredSignature"}"#;
        let failure = userinfo_sub(json).expect_err("a failure");
        assert_eq!(failure.code, 10001);
        assert!(failure.is_unauthorized(), "the cached token must be dropped");
        assert_eq!(failure.message, "Your VOS session has expired; sign in again and retry");
    }

    #[test]
    fn userinfo_without_a_sub_is_reported_plainly() {
        let failure = userinfo_sub(r#"{"preferred_username":"wangrui"}"#).expect_err("a failure");
        assert!(failure.message.contains("no user id"), "{}", failure.message);
        assert!(!failure.is_unauthorized());
    }

    #[test]
    fn userinfo_that_is_not_json_is_reported_as_such() {
        let failure = userinfo_sub("<html>login</html>").expect_err("a failure");
        assert!(failure.message.contains("unreadable"), "{}", failure.message);
    }

    #[test]
    fn decode_returns_the_payload_on_success() {
        let json = r#"{"code": 0, "data": {"adapter": "v_x", "files": []}}"#;
        let index: Index = decode(json).expect("a successful reply");
        assert!(index.files.is_empty());
    }

    #[test]
    fn decode_turns_a_business_code_into_a_message() {
        let json = r#"{"code": 10046, "data": null, "msg": "no access"}"#;
        let failure = decode::<Index>(json).expect_err("a failure");
        assert_eq!(failure.code, 10046);
        assert!(failure.message.contains("read permission"), "{}", failure.message);
        assert!(!failure.is_unauthorized());
    }

    #[test]
    fn an_expired_session_is_recognisable_from_the_failure_alone() {
        let json = r#"{"code": 10001, "data": null, "msg": ""}"#;
        let failure = decode::<Index>(json).expect_err("a failure");
        assert!(failure.is_unauthorized(), "the caller must know to drop the token");
    }

    #[test]
    fn decode_keeps_the_platform_message_for_an_unknown_code() {
        let json = r#"{"code": 10999, "data": null, "msg": "server exploded"}"#;
        let failure = decode::<Index>(json).expect_err("a failure");
        assert_eq!(failure.code, 10999);
        assert!(failure.message.contains("server exploded") && failure.message.contains("10999"), "{}", failure.message);
    }

    #[test]
    fn decode_reports_a_reply_with_no_data() {
        let json = r#"{"code": 0, "msg": "ok"}"#;
        assert!(decode::<Index>(json).is_err());
    }

    #[test]
    fn decode_reports_unreadable_json() {
        // 响应本身被原样引回。
        let failure = decode::<Index>("<html>not json</html>").expect_err("a failure");
        assert!(failure.message.contains("<html>"), "{}", failure.message);
    }

    #[test]
    fn an_unreadable_reply_is_quoted_shortly() {
        let failure = decode::<Index>(&"x".repeat(400)).expect_err("a failure");
        assert!(failure.message.contains("400 bytes") && failure.message.len() < 200, "{}", failure.message);
    }

    /// 错误消息里引用的正文不带 BOM 和前后空白。
    #[test]
    fn an_error_quote_drops_a_leading_byte_order_mark() {
        let failure = decode::<Index>("\u{feff}<html>login</html>").expect_err("a failure");
        assert!(!failure.message.contains('\u{feff}'), "{}", failure.message);
        assert!(failure.message.contains("<html>"), "{}", failure.message);
    }

    #[test]
    fn missing_optional_fields_still_decode() {
        // 部分平台版本会省略 storages/sort_by；缺字段不能让整个目录列表失败。

        let json = r#"{"code": 0, "data": {"adapter": "v_x", "files": []}}"#;
        let body: ResultBody<Index> = serde_json::from_str(json).expect("a valid reply");
        assert!(body.data.expect("data").files.is_empty());
    }
}

/// Translate VOS locale tags to PhotoCraft's existing catalog names.
pub fn locale_code(raw: &str) -> Option<&'static str> {
    match raw.to_ascii_lowercase().replace('_', "-").as_str() {
        "zh" | "zh-cn" | "zh-hans" => Some("zh-hans"),
        "zh-tw" | "zh-hk" | "zh-hant" => Some("zh-hant"),
        "en" | "en-us" | "en-gb" => Some("en"),
        _ => None,
    }
}
#[cfg(test)]
mod source_tests {
    #[test]
    fn vos_locales_use_existing_catalogs() {
        for (input, expected) in [("zh-CN", "zh-hans"), ("zh_TW", "zh-hant"), ("en-US", "en")] {
            assert_eq!(super::locale_code(input), Some(expected));
        }
        assert_eq!(super::locale_code("unknown"), None);
    }
    #[test]
    fn exposed_directory_keeps_its_own_adapter() {
        let volume: super::Volume = serde_json::from_str(r#"{"uuid":"grant","adapter":"x_grant","name":"Public"}"#).unwrap();
        assert_eq!(volume.adapter(), "x_grant");
    }
}

/// AI Album accepts rendered images, not layered project documents.
pub fn album_export_name(suggested: &str) -> Option<String> {
    let name = std::path::Path::new(suggested).file_name()?.to_str()?;
    let extension = std::path::Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "webp").then(|| name.to_owned())
}
#[cfg(test)]
mod export_tests {
    #[test]
    fn album_destination_accepts_renders_and_preserves_projects() {
        assert_eq!(super::album_export_name("x_grant://folder/result.PNG"), Some("result.PNG".into()));
        assert!(super::album_export_name("layers.psd").is_none());
        assert!(super::album_export_name("layers.pcraft").is_none());
        assert!(super::album_export_name("result.svg").is_none());
    }
}
