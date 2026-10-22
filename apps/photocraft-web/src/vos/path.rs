//! Finder 路径的语法：<adapter>://<dir>/<name>。
//!
//! 平台自己生成这种字符串（它的 `build_full_path`）：adapter、`://`，然后是相对卷的目录。
//!
//! 所以卷根是 <adapter>://，下一层的文件是 <adapter>://folder/a.psd。每个 Finder 接口都把
//! 这种字符串当作 `path` 参数并剥掉 scheme，所以插件把条目的路径原样传下去，只自己拼接交
//! 给 `Services::write` 的那一个路径。
//!
//! 只有当 scheme 是 `<prefix>_<id>`、且前缀是平台认识的（`v`/`vr` 卷、`p`/`pr` 公共目录、`s` 分享、
//! `d` 外部设备）时，它才是 Finder adapter。正是这一点让普通的 `https://...` URL 不会被误认成
//! Finder 路径。
//!
//! 本模块刻意不碰 web_sys，因此能在宿主机的测试里跑。

/// 平台的 `parse_adapter` 接受的 scheme 前缀。
const ADAPTER_PREFIXES: &[&str] = &["v", "vr", "p", "pr", "s", "d", "x"];

/// 当 `raw` 确实是 Finder 路径时，把它拆成 adapter 和 `:/` 之后的部分。
fn adapter_and_rest(raw: &str) -> Option<(&str, &str)> {
    let (adapter, rest) = raw.split_once(":/")?;
    let (prefix, id) = adapter.split_once('_')?;
    if id.is_empty() || !ADAPTER_PREFIXES.contains(&prefix) {
        return None;
    }
    Some((adapter, rest))
}

/// 某块 VOS 卷里的一个位置：Finder adapter 加上相对卷的目录。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FinderPath {
    /// Finder adapter，如 v_01KYS...（一块卷）或 p_...（一个公共目录）。
    pub adapter: String,
    /// 卷内的目录：卷根为空，否则形如 /a/b（不带结尾斜杠）。
    pub dir: String,
}

impl FinderPath {
    /// `adapter` 的根目录。
    pub fn root(adapter: impl Into<String>) -> Self {
        Self { adapter: adapter.into(), dir: String::new() }
    }

    /// 每个 Finder 接口为这个目录期望的 `path` 参数。
    pub fn api_path(&self) -> String {
        format!("{}://{}", self.adapter, self.dir.trim_start_matches('/'))
    }

    /// 这个目录下 `name` 的完整路径，可作条目路径或保存目标。
    pub fn child(&self, name: &str) -> String {
        let dir = self.dir.trim_start_matches('/');
        if dir.is_empty() { format!("{}://{}", self.adapter, name) } else { format!("{}://{dir}/{name}", self.adapter) }
    }

    /// 上一级目录；在卷根时为 None。
    pub fn parent(&self) -> Option<Self> {
        if self.dir.is_empty() {
            return None;
        }
        let dir = self.dir.rsplit_once('/').map(|(parent, _)| parent).unwrap_or("");
        Some(Self { adapter: self.adapter.clone(), dir: dir.to_string() })
    }

    /// 文件路径所在的目录，以及文件名。
    ///
    /// 带结尾斜杠表示目录，返回 None。不带斜杠的路径按文件读，因为唯一的调用方是写入路径，
    /// 它拿到的是本插件用 [`Self::child`] 拼出来的路径。对于直接来自 API 的路径，请改用条目的
    /// `type` 字段：光看字符串分不出文件和同名目录。
    pub fn split_file(raw: &str) -> Option<(Self, String)> {
        if raw.ends_with('/') {
            return None;
        }
        let (adapter, rest) = adapter_and_rest(raw)?;
        let (dir, name) = rest.rsplit_once('/')?;
        if name.is_empty() {
            return None;
        }
        Some((Self { adapter: adapter.to_string(), dir: dir.to_string() }, name.to_string()))
    }

    /// 解析目录路径。没有可识别的 scheme 时为 None；结尾斜杠会被忽略。
    pub fn parse_dir(raw: &str) -> Option<Self> {
        let (adapter, rest) = adapter_and_rest(raw)?;
        Some(Self { adapter: adapter.to_string(), dir: rest.trim_end_matches('/').to_string() })
    }
}

/// `raw` 的 adapter，不是 Finder 路径时为 None。
///
/// 这也是"这到底是不是 Finder 路径"的判断（可识别的 adapter scheme 是它与普通 URL 的唯一区别）。
/// Finder 分开接收 adapter 和路径，空 adapter 的下载会被拒绝（10045）。
pub fn adapter_of(raw: &str) -> Option<&str> {
    adapter_and_rest(raw).map(|(adapter, _)| adapter)
}

/// `raw` 的最后一段（文件的显示名），没有时返回 `raw` 本身。
///
/// 先剥掉 scheme，所以带结尾斜杠（目录路径）时名字就是该目录，而裸卷根（`v_abc://`）退回
/// `raw`，而不是 scheme 的残余。
pub fn display_name(raw: &str) -> String {
    let tail = raw.split_once(":/").map_or(raw, |(_, rest)| rest);
    tail.trim_end_matches('/').rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or(raw).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_paths_match_the_platform_format() {
        let root = FinderPath::root("v_abc");
        assert_eq!(root.api_path(), "v_abc://");
        assert_eq!(root.child("test"), "v_abc://test");
    }

    #[test]
    fn nested_paths_match_the_platform_format() {
        let dir = FinderPath { adapter: "v_abc".into(), dir: "/pics/raw".into() };
        assert_eq!(dir.api_path(), "v_abc://pics/raw");
        assert_eq!(dir.child("a.psd"), "v_abc://pics/raw/a.psd");
    }

    /// 两个接口的 `path` 形式不同：`rename` 要带 scheme 的目录全路径（与 index 回复里的
    /// `dirname` 同形），`delete` 的 `targets` 要卷内相对路径。
    #[test]
    fn rename_takes_the_directory_full_path_and_delete_the_relative_one() {
        let file = FinderPath::parse_dir("v_abc://pics/a.psd").expect("a finder path");
        assert_eq!(file.parent().map(|d| d.api_path()), Some("v_abc://pics".into()), "rename 的 path 是文件所在目录");
        assert_eq!(file.dir.trim_start_matches('/'), "pics/a.psd", "delete 的 targets 是文件本身");
    }

    #[test]
    fn parse_dir_reads_both_the_root_and_a_nested_directory() {
        assert_eq!(FinderPath::parse_dir("v_abc://"), Some(FinderPath::root("v_abc")));
        assert_eq!(FinderPath::parse_dir("v_abc://pics"), Some(FinderPath { adapter: "v_abc".into(), dir: "/pics".into() }));
        assert_eq!(FinderPath::parse_dir("v_abc://pics/"), Some(FinderPath { adapter: "v_abc".into(), dir: "/pics".into() }));
    }

    #[test]
    fn parse_dir_rejects_strings_without_an_adapter_scheme() {
        assert_eq!(FinderPath::parse_dir("/pics/a.psd"), None);
        assert_eq!(FinderPath::parse_dir("https://example.com/a.psd"), None);
        assert_eq!(FinderPath::parse_dir("://x"), None);
        assert_eq!(FinderPath::parse_dir("v_://x"), None);
        assert_eq!(FinderPath::parse_dir("unknown_abc://pics"), None);
    }

    #[test]
    fn split_file_separates_the_directory_from_the_name() {
        let (dir, name) = FinderPath::split_file("v_abc://pics/a.psd").expect("a file path");
        assert_eq!(dir, FinderPath { adapter: "v_abc".into(), dir: "/pics".into() });
        assert_eq!(name, "a.psd");
    }

    #[test]
    fn split_file_reads_a_file_at_the_volume_root() {
        let (dir, name) = FinderPath::split_file("v_abc://a.psd").expect("a file path");
        assert_eq!(dir, FinderPath::root("v_abc"));
        assert_eq!(name, "a.psd");
    }

    #[test]
    fn split_file_rejects_a_directory_and_schemeless_paths() {
        assert_eq!(FinderPath::split_file("v_abc://pics/"), None);
        assert_eq!(FinderPath::split_file("v_abc://a/b/"), None, "结尾的斜杠表示目录");
        assert_eq!(FinderPath::split_file("v_abc://"), None);
        assert_eq!(FinderPath::split_file("plain.psd"), None);
        assert_eq!(FinderPath::split_file("https://example.com/a.psd"), None);
    }

    #[test]
    fn a_save_path_round_trips_through_child_and_split_file() {
        let dir = FinderPath { adapter: "v_abc".into(), dir: "/pics".into() };
        let raw = dir.child("a.psd");
        let (back, name) = FinderPath::split_file(&raw).expect("round trip");
        assert_eq!(back, dir);
        assert_eq!(name, "a.psd");
    }

    #[test]
    fn a_root_level_save_path_round_trips() {
        let dir = FinderPath::root("v_abc");
        let (back, name) = FinderPath::split_file(&dir.child("a.psd")).expect("round trip");
        assert_eq!(back, dir);
        assert_eq!(name, "a.psd");
    }

    #[test]
    fn parent_walks_up_and_stops_at_the_root() {
        let nested = FinderPath { adapter: "v_abc".into(), dir: "/a/b".into() };
        assert_eq!(nested.parent(), Some(FinderPath { adapter: "v_abc".into(), dir: "/a".into() }));
        assert_eq!(FinderPath::root("v_abc").parent(), None);
    }

    #[test]
    fn adapter_of_distinguishes_adapters_from_urls_and_names() {
        assert_eq!(adapter_of("v_abc://pics/a.psd"), Some("v_abc"));
        assert_eq!(adapter_of("p_docs://"), Some("p_docs"));
        assert_eq!(adapter_of("s_share1://sub"), Some("s_share1"));
        assert_eq!(adapter_of("d_dev-1://"), Some("d_dev-1"));
        assert_eq!(adapter_of("a.psd"), None);
        assert_eq!(adapter_of("/home/user/a.psd"), None);
        assert_eq!(adapter_of("https://example.com/a.psd"), None, "普通 URL 不是 Finder 路径");
        assert_eq!(adapter_of("http://host/x"), None);
    }

    #[test]
    fn display_name_reads_the_last_component() {
        assert_eq!(display_name("v_abc://pics/a.psd"), "a.psd");
        assert_eq!(display_name("v_abc://pics/"), "pics", "结尾斜杠仍然给出目录名");
        assert_eq!(display_name("v_abc://"), "v_abc://", "裸 scheme 没有可取的段");
    }
}
