//! 首页列出的 VOS 文件，最新的在前。
//!
//! Finder 没有"最近"接口，条目也不带修改时间，所以插件自己维护一份列表：它打开的每个路径
//! （[`super::install`] 的选择器作答）或写入的每个路径（它的 `write` 包装器）都记在这里，并
//! 和偏好设置、笔刷预设一样持久化到浏览器的 localStorage。
//!
//! 纯 Rust：进去的是存储里的文本，出来的是字符串，所以宿主机测试能覆盖解析、去重和截断。
//!
//! 只有 `localStorage` 本身需要浏览器。

/// 列表保留多少个文件，与首页自己的最近文件上限一致。
pub const MAX: usize = 6;

/// 最近打开或保存过的 VOS 路径，最新的在前。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Recent {
    paths: Vec<String>,
}

impl Recent {
    /// 读取存储里的列表。脏数据直接丢弃而不往外传：被手改或截断的存储项不能让首页无法工作。
    ///
    /// 按宽松 JSON 解码，这样一个坏元素（手改出来的）不会把旁边有效的路径一起丢掉。存储的
    /// 文本是新的在前，而 `record` 插到最前面，所以按最旧到最新回放，才能重建同样的顺序。
    pub fn load(text: Option<&str>) -> Self {
        let Some(parsed) = text.and_then(|t| serde_json::from_str::<Vec<serde_json::Value>>(t).ok()) else {
            return Self::default();
        };
        let mut recent = Self::default();
        for path in parsed.into_iter().rev().filter_map(|v| v.as_str().map(str::to_string)) {
            recent.record(&path);
        }
        recent
    }

    /// 把路径记为最新的，会去重并截断。不是 Finder 路径的会被忽略，所以本地路径（或测试里的
    /// `/tmp/x.psd`）进不了这个列表。
    pub fn record(&mut self, path: &str) {
        if super::path::adapter_of(path).is_none() {
            return;
        }
        self.paths.retain(|p| p != path);
        self.paths.insert(0, path.to_string());
        self.paths.truncate(MAX);
    }

    /// 从列表里删掉一个路径（文件被改名、移动，或太大打不开）。
    pub fn forget(&mut self, path: &str) {
        self.paths.retain(|p| p != path);
    }

    /// 这些路径，最新的在前。
    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    /// 列表的存储形态：一个 JSON 数组，最新的在前。
    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.paths).unwrap_or_else(|_| "[]".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_path_comes_first() {
        let mut r = Recent::default();
        r.record("v_abc://a.psd");
        r.record("v_abc://b.psd");
        assert_eq!(r.paths(), ["v_abc://b.psd", "v_abc://a.psd"]);
    }

    #[test]
    fn recording_the_same_path_again_moves_it_up_without_duplicating_it() {
        let mut r = Recent::default();
        r.record("v_abc://a.psd");
        r.record("v_abc://b.psd");
        r.record("v_abc://a.psd");
        assert_eq!(r.paths(), ["v_abc://a.psd", "v_abc://b.psd"]);
    }

    #[test]
    fn the_list_is_capped() {
        let mut r = Recent::default();
        for i in 0..(MAX + 4) {
            r.record(&format!("v_abc://{i}.psd"));
        }
        assert_eq!(r.paths().len(), MAX);
        assert_eq!(r.paths()[0], format!("v_abc://{}.psd", MAX + 3));
    }

    #[test]
    fn only_finder_paths_are_recorded() {
        let mut r = Recent::default();
        r.record("/tmp/a.psd");
        r.record("a.psd");
        r.record("https://example.com/a.psd");
        assert!(r.paths().is_empty(), "本地或网络路径不是 VOS 存储");
    }

    #[test]
    fn forgetting_a_path_drops_just_that_one() {
        let mut r = Recent::default();
        r.record("v_abc://a.psd");
        r.record("v_abc://b.psd");
        r.forget("v_abc://a.psd");
        assert_eq!(r.paths(), ["v_abc://b.psd"]);
        r.forget("v_abc://gone.psd");
        assert_eq!(r.paths(), ["v_abc://b.psd"], "不在列表里的路径什么也不改变");
    }

    #[test]
    fn a_stored_list_round_trips() {
        let mut r = Recent::default();
        r.record("v_abc://pics/a.psd");
        r.record("p_docs://b.png");
        let back = Recent::load(Some(&r.to_json()));
        assert_eq!(back, r);
    }

    #[test]
    fn junk_in_storage_leaves_a_usable_list() {
        assert!(Recent::load(None).paths().is_empty());
        assert!(Recent::load(Some("")).paths().is_empty());
        assert!(Recent::load(Some("not json")).paths().is_empty());
        assert!(Recent::load(Some("{\"paths\":1}")).paths().is_empty(), "旧的 object 形态不做猜测");
        let kept = Recent::load(Some("[\"v_abc://a.psd\",\"/tmp/x\",42,null]"));
        assert_eq!(kept.paths(), ["v_abc://a.psd"], "只有有效的 Finder 路径留了下来");
    }
}
