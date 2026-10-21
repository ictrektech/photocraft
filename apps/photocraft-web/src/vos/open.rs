//! `?open=`：另一个 VOS 应用把一份文件交给 PhotoCraft 打开。
//!
//! 契约（`docs/vos-integration.md` › Handoff）：
//! `?open=<路径>&name=<显示名>&source=<app id>`。`open` 只接受**根相对路径**
//! （`/api/<app>/…` 或 `/app/<app>/…`，因此必然同源）或 Finder 文件路径（`v_…://…`）；
//! 带 scheme 或 host 的完整 URL、平台内部路径、目录形式的 Finder 路径、`..`、反斜杠与控制
//! 字符一律拒绝。
//!
//! 纯函数，不碰 web_sys：宿主机测试直接驱动它（与 `vos::path` 同样的理由）。

use super::path;

/// 路径上限：正常图片 URL 远小于它，超出的当作坏链接。
const MAX_URL: usize = 2048;
/// 显示名上限（文档标题）。
const MAX_NAME: usize = 128;
/// `source` 上限（app id）。
const MAX_SOURCE: usize = 100;
/// 允许的同源前缀：应用的 API 与挂载路径。后面必须紧跟应用 id。
const PREFIXES: [&str; 2] = ["/api/", "/app/"];

/// 一次交接：要打开什么、文档叫什么、谁发起的。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    /// 同源路径，或 Finder 路径。
    pub url: String,
    /// 文档标题（`?name`，缺省时取路径最后一段）。
    pub name: String,
    /// 发起方 app id，仅用于展示与日志，不参与鉴权。
    pub source: Option<String>,
}

/// 解析页面的查询串。`Ok(None)` 表示查询串里没有 `?open=`。
pub fn parse(search: &str) -> Result<Option<Launch>, String> {
    let mut open = None;
    let mut name = None;
    let mut source = None;
    for pair in search.trim_start_matches('?').split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match decode(key).as_str() {
            // 重复参数取第一个。
            "open" if open.is_none() => open = Some(decode(value)),
            "name" if name.is_none() => name = Some(decode(value)),
            "source" if source.is_none() => source = Some(decode(value)),
            _ => {}
        }
    }
    let Some(open) = open else { return Ok(None) };
    let url = validate(&open)?;
    let name = name.map(|name| sanitize_name(&name)).filter(|name| !name.is_empty()).unwrap_or_else(|| name_from(&url));
    let source = source.map(|source| sanitize_source(&source)).filter(|source| !source.is_empty());
    Ok(Some(Launch { url, name, source }))
}

/// 校验并返回可请求的目标：平台内的根相对路径，或 Finder 路径。
fn validate(raw: &str) -> Result<String, String> {
    let url = raw.trim();
    if url.is_empty() {
        return Err("the link does not name a file to open".into());
    }
    if url.len() > MAX_URL {
        return Err("the link's file path is too long".into());
    }
    if url.contains('\\') || url.chars().any(char::is_control) {
        return Err("the link's file path is not a URL path".into());
    }
    if path::adapter_of(url).is_none() {
        if !url.starts_with('/') || url.starts_with("//") {
            return Err("the link must stay inside this VOS: pass a path, not a full URL".into());
        }
        if app_namespace(url).is_none() {
            return Err("the link must name an app file (`/api/<app>/…` or `/app/<app>/…`)".into());
        }
    } else if path::FinderPath::split_file(url).is_none() {
        return Err("the link must name a file inside a VOS volume".into());
    }
    // 只检查路径部分；查询串里的 `..` 不影响目录。反复解到稳定（有条数上限），挡住
    // `%2e%2e`、`%252e%252e` 这类多重转义。
    let target = url.split(['?', '#']).next().unwrap_or(url);
    let mut decoded = target.to_string();
    for _ in 0..3 {
        let next = decode_path(&decoded);
        if next == decoded {
            break;
        }
        decoded = next;
    }
    for segment in decoded.split('/') {
        if segment == ".." || segment == "." {
            return Err("the link's file path escapes its directory".into());
        }
        if segment.contains('\\') || segment.chars().any(char::is_control) {
            return Err("the link's file path is not a URL path".into());
        }
    }
    Ok(url.to_string())
}

/// 去掉交接参数后的查询串（不含 `?`），其它键值原样保留。
pub fn without_handoff(search: &str) -> String {
    search.trim_start_matches('?').split('&').filter(|pair| !pair.is_empty() && !is_handoff_key(pair)).collect::<Vec<_>>().join("&")
}

fn is_handoff_key(pair: &str) -> bool {
    let key = pair.split_once('=').map_or(pair, |(key, _)| key);
    matches!(decode(key).as_str(), "open" | "name" | "source")
}

/// 路径里的应用命名空间：`/api/<app id>/…` 或 `/app/<app id>/…`，应用 id 含点。
///
/// 平台自身的端点（如 `/api/v1/…`）没有应用 id，因此不会被交接链接触及。
fn app_namespace(url: &str) -> Option<&str> {
    let rest = PREFIXES.iter().find_map(|prefix| url.strip_prefix(*prefix))?;
    let app = rest.split('/').next()?;
    let looks_like_id = app.contains('.') && app.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'));
    looks_like_id.then_some(app)
}

/// 路径最后一段（去掉查询串与片段），作为没有 `?name` 时的标题。
fn name_from(url: &str) -> String {
    let target = url.split(['?', '#']).next().unwrap_or(url);
    let last = target.rsplit('/').next().unwrap_or(target);
    let name = sanitize_name(last);
    if name.is_empty() { "image".to_string() } else { name }
}

/// 标题只保留可打印字符；路径分隔符与控制字符去掉，长度截到 [`MAX_NAME`]。
fn sanitize_name(raw: &str) -> String {
    raw.chars().filter(|c| !c.is_control() && *c != '/' && *c != '\\').take(MAX_NAME).collect::<String>().trim().to_string()
}

/// app id 只保留 `[A-Za-z0-9._-]`，长度截到 [`MAX_SOURCE`]。
fn sanitize_source(raw: &str) -> String {
    raw.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')).take(MAX_SOURCE).collect()
}

/// 表单值的百分号解码（`+` 是空格）。坏转义原样保留，不让一个 `%` 让整条链接失效。
fn decode(raw: &str) -> String {
    decode_with(raw, true)
}

/// 路径段的百分号解码：`+` 是普通字符，重解时不能把它变成空格。
fn decode_path(raw: &str) -> String {
    decode_with(raw, false)
}

fn decode_with(raw: &str, plus: bool) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'+' if plus => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => match (hex(bytes.get(index + 1)), hex(bytes.get(index + 2))) {
                (Some(high), Some(low)) => {
                    out.push(high << 4 | low);
                    index += 3;
                }
                _ => {
                    out.push(b'%');
                    index += 1;
                }
            },
            other => {
                out.push(other);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(byte: Option<&u8>) -> Option<u8> {
    match byte.copied()? {
        digit @ b'0'..=b'9' => Some(digit - b'0'),
        digit @ b'a'..=b'f' => Some(digit - b'a' + 10),
        digit @ b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_handoff() {
        let launch = parse("?cpu&open=%2Fapi%2Fcom.ictrek.image-studio%2Fapi%2Fv1%2Foutputs%2Fjob%2Fa+b.png&name=%E5%9B%BE.png&source=com.ictrek.image-studio")
            .unwrap()
            .expect("a launch");
        assert_eq!(launch.url, "/api/com.ictrek.image-studio/api/v1/outputs/job/a b.png");
        assert_eq!(launch.name, "图.png");
        assert_eq!(launch.source.as_deref(), Some("com.ictrek.image-studio"));
    }

    #[test]
    fn without_open_there_is_nothing_to_do() {
        assert_eq!(parse("").unwrap(), None);
        assert_eq!(parse("?cpu").unwrap(), None);
        assert_eq!(parse("?webgl&name=x").unwrap(), None);
    }

    #[test]
    fn the_name_falls_back_to_the_file_name() {
        let launch = parse("?open=/app/com.ictrek.image-studio/a.png").unwrap().expect("a launch");
        assert_eq!(launch.name, "a.png");
        // 查询串与片段不属于名字。
        let launch = parse("?open=/api/com.ictrek.image-studio/photo%20one.psd%3Fv%3D2").unwrap().expect("a launch");
        assert_eq!(launch.name, "photo one.psd");
    }

    #[test]
    fn a_name_cannot_carry_paths_or_control_characters() {
        let launch = parse("?open=/api/com.ictrek.image-studio/x.png&name=..%2F..%2Fetc%2Fpasswd%00").unwrap().expect("a launch");
        assert_eq!(launch.name, "....etcpasswd");
        let launch = parse("?open=/api/com.ictrek.image-studio/x.png&name=%2F%2F").unwrap().expect("a launch");
        assert_eq!(launch.name, "x.png", "an empty name falls back to the path");
    }

    #[test]
    fn a_source_keeps_only_identifier_characters() {
        let launch = parse("?open=/app/com.ictrek.image-studio/a.png&source=com.ictrek.image-studio%20%3Cscript%3E").unwrap().expect("a launch");
        assert_eq!(launch.source.as_deref(), Some("com.ictrek.image-studioscript"));
    }

    #[test]
    fn accepts_a_finder_path() {
        let launch = parse("?open=v_v1%3A%2F%2Fdir%2Fa.psd").unwrap().expect("a launch");
        assert_eq!(launch.url, "v_v1://dir/a.psd");
        assert_eq!(launch.name, "a.psd");
    }

    #[test]
    fn rejects_anything_that_is_not_an_inside_the_platform_path() {
        for bad in [
            "https://evil.example/x.png",
            "//evil.example/x.png",
            "javascript:alert(1)",
            "data:image/png;base64,AA",
            "/etc/passwd",
            "api/x.png",
            "C:/x.png",
            "/api/../secret.png",
            "/app/%2e%2e/api/x.png",
            "/api/%252e%252e/x.png",
            "/api/com.ictrek.x/../secret.png",
            "/api/com.ictrek.x/%252e%252e/x.png",
            "v_v1://dir/../../etc/passwd",
            "/api/com.ictrek.x/a\\b.png",
            "/api/com.ictrek.x/a%00b.png",
            // 平台自身的端点不是应用文件。
            "/api/v1/users",
            "/app/healthz",
            // Finder 目录不是文件。
            "v_v1://dir/",
        ] {
            let encoded: String = bad.bytes().map(|byte| format!("%{byte:02X}")).collect();
            assert!(parse(&format!("?open={encoded}")).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn unrelated_query_parameters_survive() {
        assert_eq!(without_handoff("?cpu&open=%2Fapi%2Fx&webgl&name=n&source=s"), "cpu&webgl");
        assert_eq!(without_handoff("?open=x"), "");
        assert_eq!(without_handoff(""), "");
        assert_eq!(without_handoff("?a=1&name=n&b=2"), "a=1&b=2");
        assert_eq!(without_handoff("?%6Fpen=x&a=1"), "a=1", "a percent-encoded key is still the handoff key");
    }

    #[test]
    fn a_broken_escape_is_kept_verbatim() {
        assert_eq!(decode("a%zzb"), "a%zzb");
        assert_eq!(decode("a%2Gb"), "a%2Gb");
        assert_eq!(decode("%e5%9b%be"), "图");
    }
}
