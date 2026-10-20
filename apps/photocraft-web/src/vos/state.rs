//! Finder 选择器的状态机，不依赖 egui 和 web_sys。
//!
//! `picker.rs` 里的窗口只绘制这份状态并转发点击；每个决定（列出什么、"打开"意味着什么、
//! "向上"去哪里）都在这里，宿主机的测试能触达。
//!
//! 生命周期：选择器从不自己取数据。它把需要的请求记在 [`Picker::take_request`] 里，帧循环取走
//! 一次并发起，再通过 `set_volumes`/`set_entries`/`fail` 把答案交回来。

use photocraft_ui_egui::theme::ThemeKind;

use super::model::{Entry, Volume};
use super::path::{self, FinderPath};

/// 保存输入框在空的时候显示的浅色提示。它只是提示，不是名字的一部分。
pub const FILE_NAME_HINT: &str = "untitled.psd";

/// 选择器默认打开的目录名，在卷根下。
pub const ASSETS_DIR: &str = "assets-psd";

/// 对话框用来做什么。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// File > Open：选一个或多个已有文件。
    Open { multiple: bool },
    /// 另存为：在当前目录里给文件起名。
    Save,
}

/// 选择器在生命周期里的位置。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    /// 正在选卷（还没有 adapter）。
    Volumes,
    /// 在等请求：有当前目录时是它的列表，否则是卷列表。
    Loading,
    /// 正在显示一个目录的条目。
    Browsing,
    /// 上一个请求失败了；消息和重试按钮一起显示。
    Error(String),
}

/// 一个选中的文件：它所在的 adapter 和完整路径。
///
/// adapter 跟着路径一起走，因为 Finder 下载要两者，并且拒绝空 adapter。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selected {
    pub adapter: String,
    pub path: String,
}

/// 用户确认过的选择，可以直接变成 `FileDialogAnswer`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    /// 选中的文件（File > Open）。
    Open(Vec<Selected>),
    /// 要写入的完整路径（另存为）。
    Save(String),
}

/// 选择器的全部状态。
#[derive(Clone, Debug)]
pub struct Picker {
    mode: Mode,
    phase: Phase,
    volumes: Vec<Volume>,
    /// 正在浏览的目录；选好卷之前为 `None`。
    current: Option<FinderPath>,
    /// 帧循环应当发起的请求：要列出的目录，或者 `None` 表示卷列表。在这里设置，由帧循环取走
    /// 一次，所以同一份列表不会被要两次。
    wanted: Option<Option<FinderPath>>,
    entries: Vec<Entry>,
    /// 选中的文件，按点击顺序。
    selected: Vec<Selected>,
    /// 保存输入框里的文件名。
    file_name: String,
    /// 窗口画在哪个主题上（配色跟着它）。
    theme: ThemeKind,
    /// 要列出的扩展名（小写、不带点）。为空则全部列出。
    filter: Vec<String>,
    /// 用户右键点了"移到回收站"、等着确认的那份文件。
    pending_delete: Option<Entry>,
    /// 正在改名的条目和新名字：右键"重命名"打开，确认或取消后清掉。
    renaming: Option<(Entry, String)>,
    /// 默认目录还没打开过；打不开就建它，只建一次。
    want_default: bool,
}

/// `path` 换成新名字后的路径：只换最后一段，目录和 scheme 不动。
fn renamed_path(path: &str, name: &str) -> String {
    format!("{}/{name}", path.rsplit_once('/').map_or(path, |(dir, _)| dir))
}

/// 文件名里能留的字符：路径分隔符会换到别的目录，控制字符会截断路径。
///
/// `egui::TextEdit` 直接写 [`Picker::file_name`]，不做过滤，所以确认时要滤掉。
fn clean_name(name: &str) -> String {
    name.chars().filter(|c| !c.is_control() && *c != '/' && *c != '\\').collect()
}

/// 名字没有扩展名时补上 `extension`。结尾的点不算扩展名。
fn with_extension(name: String, extension: &str) -> String {
    let has_extension = std::path::Path::new(&name).extension().is_some_and(|e| !e.is_empty());
    if extension.is_empty() || has_extension {
        return name;
    }
    format!("{}.{extension}", name.trim_end_matches('.'))
}

impl Picker {
    /// `mode` 对应的选择器。`filter` 是要提供的扩展名（为空表示全部），`suggested` 预先填好保存
    /// 输入框里的文件名。
    ///
    /// 它先要卷列表：Finder 路径需要 adapter，而 adapter 正来自用户的卷。
    pub fn new(mode: Mode, filter: Vec<String>, suggested: Option<&str>) -> Self {
        let file_name = suggested.map(path::display_name).unwrap_or_default();
        Self {
            mode,
            phase: Phase::Loading,
            volumes: Vec::new(),
            current: None,
            wanted: Some(None),
            entries: Vec::new(),
            selected: Vec::new(),
            file_name,
            theme: ThemeKind::default(),
            filter: filter.into_iter().map(|e| e.trim_start_matches('.').to_ascii_lowercase()).collect(),
            pending_delete: None,
            renaming: None,
            want_default: false,
        }
    }

    pub fn mode(&self) -> &Mode {
        &self.mode
    }

    /// 窗口和它画的东西的颜色跟着它走；外壳每帧把当前主题推过来（[`Self::set_theme`]）。
    pub fn theme(&self) -> ThemeKind {
        self.theme
    }

    /// 按外壳的主题配色。
    pub fn set_theme(&mut self, theme: ThemeKind) {
        self.theme = theme;
    }

    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    pub fn volumes(&self) -> &[Volume] {
        &self.volumes
    }

    pub fn current(&self) -> Option<&FinderPath> {
        self.current.as_ref()
    }

    /// 现在要发起的请求（如果有）：`Some(None)` 要卷列表，`Some(Some(dir))` 要一个目录的列表。
    pub fn take_request(&mut self) -> Option<Option<FinderPath>> {
        self.wanted.take()
    }

    /// 输入框里要画的名字（也是要保存的名字）。空字段时是空：提示文字（[`FILE_NAME_HINT`]）
    /// 只画成浅色占位符。
    #[cfg(test)]
    pub fn file_name_text(&self) -> &str {
        &self.file_name
    }

    /// 交给 `egui::TextEdit` 直接编辑的缓冲区。
    ///
    /// 必须是持久的那一个 `String`：副本会丢掉选中范围和光标位置。
    pub fn file_name_mut(&mut self) -> &mut String {
        &mut self.file_name
    }

    /// 要绘制的条目：目录总是显示，文件只在匹配过滤条件时显示。
    pub fn visible_entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.is_dir() || self.matches_filter(e))
    }

    fn matches_filter(&self, entry: &Entry) -> bool {
        self.filter.is_empty() || self.filter.iter().any(|f| f == &entry.extension.to_ascii_lowercase())
    }

    /// 该条目当前是否被选中。
    pub fn is_selected(&self, entry_path: &str) -> bool {
        self.selected.iter().any(|s| s.path == entry_path)
    }

    /// 当前目录是否可以退回上一级（或回到卷选择）。
    pub fn can_go_up(&self) -> bool {
        matches!(self.phase, Phase::Browsing) && self.current.is_some()
    }

    /// 卷到了。只有一个时直接进入它的默认目录，这样单卷环境不用点一次。
    pub fn set_volumes(&mut self, volumes: Vec<Volume>) {
        let only = if let [only] = volumes.as_slice() { Some(only.adapter()) } else { None };
        self.volumes = volumes;
        match only {
            Some(adapter) => self.enter_default(&adapter),
            None => self.phase = Phase::Volumes,
        }
    }

    /// 进入 `adapter` 的默认目录（[`ASSETS_DIR`]）。它不存在时由 [`Self::enter_failed`]
    /// 记下要建它。
    pub fn enter_default(&mut self, adapter: &str) {
        let dir = FinderPath::root(adapter).child(ASSETS_DIR);
        self.want_default = true;
        self.enter(&dir);
    }

    /// 打开默认目录失败：路径不存在就在卷根下建它，返回要建的目录；其他失败照常报告。
    pub fn enter_failed(&mut self, failure: &super::model::Failure) -> Option<FinderPath> {
        let dir = self.current.clone()?;
        if !self.want_default || !failure.is_not_found() {
            self.fail(failure.to_string());
            return None;
        }
        let root = FinderPath::root(&dir.adapter);
        Some(root)
    }

    /// 默认目录建好了：再打开一次。
    pub fn default_created(&mut self, adapter: &str) {
        let dir = FinderPath::root(adapter).child(ASSETS_DIR);
        self.enter(&dir);
    }

    /// 开始浏览 `dir`（Finder 路径）。会清空选择：它属于旧目录。
    pub fn enter(&mut self, dir: &str) {
        let Some(parsed) = FinderPath::parse_dir(dir) else {
            self.fail(format!("Not a VOS storage path: {dir}"));
            return;
        };
        // 这些条目属于正要离开的目录，不是正要请求的那个。
        self.current = Some(parsed.clone());
        self.entries.clear();
        self.selected.clear();
        self.wanted = Some(Some(parsed));
        self.phase = Phase::Loading;
    }

    /// 列表到了。
    pub fn set_entries(&mut self, entries: Vec<Entry>) {
        self.want_default = false;
        self.entries = entries;
        self.phase = Phase::Browsing;
    }

    /// 上一个请求失败了。
    pub fn fail(&mut self, message: impl Into<String>) {
        self.phase = Phase::Error(message.into());
    }

    /// 离开错误状态并重新要失败的那个：当前目录的列表，或者还没打开目录时的卷列表。
    pub fn retry(&mut self) {
        match self.current.clone() {
            Some(dir) => self.enter(&dir.api_path()),
            None => {
                self.wanted = Some(None);
                self.phase = Phase::Loading;
            }
        }
    }

    /// 退上一级；在卷根目录时回到卷选择。
    pub fn up(&mut self) {
        let Some(current) = self.current.clone() else { return };
        match current.parent() {
            Some(parent) => self.enter(&parent.api_path()),
            // 回到选择界面，那正好就是一个只知道自己的卷的选择器。
            None => {
                self.current = None;
                self.entries.clear();
                self.selected.clear();
                self.wanted = None;
                self.phase = Phase::Volumes;
            }
        }
    }

    /// 点击一个条目：目录就进入，文件就选中。
    pub fn activate(&mut self, entry: &Entry) {
        if entry.is_dir() {
            self.enter(&entry.path);
        } else {
            self.toggle(entry);
        }
    }

    /// 选中或取消选中一个文件。单选打开时，选中一个会清掉其他的。
    ///
    /// adapter 和路径一起记住：下载两者都要，而 adapter 只出现在条目里。
    pub fn toggle(&mut self, entry: &Entry) {
        if entry.is_dir() {
            return;
        }
        let single = matches!(self.mode, Mode::Open { multiple: false });
        if let Some(i) = self.selected.iter().position(|c| c.path == entry.path) {
            self.selected.remove(i);
            return;
        }
        if single {
            self.selected.clear();
        }
        self.selected.push(Selected { adapter: entry.storage.clone(), path: entry.path.clone() });
    }

    /// 右键点了删除、等着用户确认的那份文件（没有就是 `None`）。
    pub fn pending_delete(&self) -> Option<&Entry> {
        self.pending_delete.as_ref()
    }

    /// 记下（或清掉）要确认删除的文件。
    pub fn set_pending_delete(&mut self, entry: Option<Entry>) {
        self.pending_delete = entry;
    }

    /// 正在改名的条目。
    pub fn renaming(&self) -> Option<&(Entry, String)> {
        self.renaming.as_ref()
    }

    /// 改名输入框编辑的那个名字。
    pub fn rename_name_mut(&mut self) -> Option<&mut String> {
        self.renaming.as_mut().map(|(_, name)| name)
    }

    /// 打开改名输入框，初始值是条目现在的名字。
    pub fn start_rename(&mut self, entry: Entry) {
        let name = entry.label().into_owned();
        self.renaming = Some((entry, name));
    }

    /// 关掉改名输入框。确认且名字真的变了时返回（原条目, 新名字, 新路径），本地列表也同步
    /// 改名；空名字和没变的名字返回 `None`。
    ///
    /// 名字和 `path` 一起改，`path` 是下次请求的 `target`。
    pub fn finish_rename(&mut self, confirmed: bool) -> Option<(Entry, String, String)> {
        let (entry, name) = self.renaming.take()?;
        let name = clean_name(name.trim());
        if !confirmed || name.is_empty() || name == entry.label() {
            return None;
        }
        let name = with_extension(name, &entry.extension);
        let renamed = renamed_path(&entry.path, &name);
        // 扩展名一起换掉。
        let extension = std::path::Path::new(&name).extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
        for e in self.entries.iter_mut().filter(|e| e.path == entry.path) {
            e.basename = name.clone();
            e.path = renamed.clone();
            e.extension = extension.clone();
        }
        for s in self.selected.iter_mut().filter(|s| s.path == entry.path) {
            s.path = renamed.clone();
        }
        Some((entry, name, renamed))
    }

    /// 删掉 `path` 的条目和它的选中状态。
    #[cfg(any(target_arch = "wasm32", test))]
    pub fn drop_item(&mut self, path: &str) {
        self.entries.retain(|e| e.path != path);
        self.selected.retain(|s| s.path != path);
    }

    /// 确认后的选择；对话框还不能确认时为 None（主按钮变灰也是因此）。
    pub fn confirm(&self) -> Option<Choice> {
        match self.mode {
            Mode::Open { .. } => (!self.selected.is_empty()).then(|| Choice::Open(self.selected.clone())),
            Mode::Save => {
                let name = clean_name(self.file_name.trim());
                // 保存需要一个要写进去的目录和一个要写的名字。
                let dir = self.current.as_ref().filter(|_| !name.is_empty())?;
                Some(Choice::Save(dir.child(&name)))
            }
        }
    }
}

#[cfg(test)]
mod tests {

    /// 测试里填名字：只写缓冲区，过滤留给 `confirm()`。
    fn set_name(p: &mut Picker, name: &str) {
        *p.file_name_mut() = name.to_string();
    }
    use super::*;

    fn entry(kind: &str, path: &str, name: &str, ext: &str) -> Entry {
        Entry { kind: kind.into(), path: path.into(), basename: name.into(), extension: ext.into(), file_size: 1, storage: "v_vol1".into() }
    }

    fn picked(adapter: &str, path: &str) -> Selected {
        Selected { adapter: adapter.into(), path: path.into() }
    }

    fn volume(uuid: &str, name: &str) -> Volume {
        Volume { uuid: uuid.into(), nick_name: None, name: name.into() }
    }

    fn open_picker() -> Picker {
        Picker::new(Mode::Open { multiple: true }, vec!["psd".into(), "png".into()], None)
    }

    /// 帧循环现在会发起的请求，以路径表示（卷列表为 `None`）。
    fn requested(p: &mut Picker) -> Option<Option<String>> {
        p.take_request().map(|r| r.map(|d| d.api_path()))
    }

    /// 跑一帧帧循环：取走请求并作答，与 `install::load` 一样。
    fn frame(p: &mut Picker) -> Option<Option<String>> {
        let asked = requested(p);
        match &asked {
            Some(None) => p.set_volumes(vec![volume("vol1", "Space")]),
            Some(Some(_)) => p.set_entries(Vec::new()),
            None => {}
        }
        asked
    }

    #[test]
    fn a_single_volume_is_entered_without_a_choice() {
        let mut p = open_picker();
        p.set_volumes(vec![volume("vol1", "Space")]);
        assert_eq!(p.phase(), &Phase::Loading);
        assert_eq!(p.current().map(|c| c.api_path()), Some("v_vol1://assets-psd".to_string()));
    }

    #[test]
    fn several_volumes_wait_for_a_choice() {
        let mut p = open_picker();
        p.set_volumes(vec![volume("a", "A"), volume("b", "B")]);
        assert_eq!(p.phase(), &Phase::Volumes);
        assert!(p.current().is_none());
        assert_eq!(p.volumes().len(), 2);
    }

    #[test]
    fn no_volumes_leaves_the_chooser_empty() {
        let mut p = open_picker();
        p.set_volumes(Vec::new());
        assert_eq!(p.phase(), &Phase::Volumes);
        assert!(p.volumes().is_empty());
    }

    #[test]
    fn entering_a_directory_loads_and_clears_the_selection() {
        let mut p = open_picker();
        p.set_volumes(vec![volume("vol1", "Space")]);
        p.set_entries(vec![entry("file", "v_vol1://a.psd", "a.psd", "psd")]);
        p.toggle(&entry("file", "v_vol1://a.psd", "a.psd", "psd"));
        assert!(p.confirm().is_some());
        p.enter("v_vol1://pics");
        assert_eq!(p.phase(), &Phase::Loading);
        assert!(p.confirm().is_none(), "a new directory starts with nothing selected");
    }

    #[test]
    fn going_up_walks_to_the_parent_then_to_the_volumes() {
        let mut p = open_picker();
        p.set_volumes(vec![volume("vol1", "Space")]);
        p.enter("v_vol1://a/b");
        p.set_entries(Vec::new());
        p.up();
        assert_eq!(p.current().map(|c| c.api_path()), Some("v_vol1://a".to_string()));
        p.up();
        assert_eq!(p.current().map(|c| c.api_path()), Some("v_vol1://".to_string()));
        p.up();
        assert_eq!(p.phase(), &Phase::Volumes);
        assert!(p.current().is_none());
    }

    #[test]
    fn clicking_a_directory_enters_it_and_a_file_selects_it() {
        let mut p = open_picker();
        p.set_volumes(vec![volume("vol1", "Space")]);
        p.set_entries(vec![entry("dir", "v_vol1://pics", "pics", ""), entry("file", "v_vol1://a.psd", "a.psd", "psd")]);
        p.activate(&entry("dir", "v_vol1://pics", "pics", ""));
        assert_eq!(p.current().map(|c| c.api_path()), Some("v_vol1://pics".to_string()));
        p.set_entries(vec![entry("file", "v_vol1://pics/b.psd", "b.psd", "psd")]);
        p.activate(&entry("file", "v_vol1://pics/b.psd", "b.psd", "psd"));
        assert!(p.is_selected("v_vol1://pics/b.psd"));
    }

    #[test]
    fn multi_select_keeps_several_and_single_select_keeps_one() {
        let (a, b) = (entry("file", "v_vol1://a.psd", "a.psd", "psd"), entry("file", "v_vol1://b.psd", "b.psd", "psd"));
        let mut many = open_picker();
        many.set_volumes(vec![volume("vol1", "Space")]);
        many.toggle(&a);
        many.toggle(&b);
        assert_eq!(many.confirm(), Some(Choice::Open(vec![picked("v_vol1", "v_vol1://a.psd"), picked("v_vol1", "v_vol1://b.psd")])));

        let mut one = Picker::new(Mode::Open { multiple: false }, Vec::new(), None);
        one.set_volumes(vec![volume("vol1", "Space")]);
        one.toggle(&a);
        one.toggle(&b);
        assert_eq!(one.confirm(), Some(Choice::Open(vec![picked("v_vol1", "v_vol1://b.psd")])));
    }

    /// 单卷时默认打开 `assets-psd`，不是卷根。
    #[test]
    fn a_single_volume_opens_the_default_directory() {
        let mut p = open_picker();
        p.set_volumes(vec![volume("vol1", "Space")]);
        assert_eq!(requested(&mut p), Some(Some("v_vol1://assets-psd".to_string())), "要的是默认目录");
    }

    /// 默认目录不存在时，在卷根下建它；其他失败照常报告。
    #[test]
    fn a_missing_default_directory_is_created_and_other_failures_are_reported() {
        let mut p = open_picker();
        p.set_volumes(vec![volume("vol1", "Space")]);
        // 平台说路径不存在。
        let missing = crate::vos::model::Failure { code: 10007, message: "ItemNotExist".into() };
        assert_eq!(p.enter_failed(&missing).map(|d| d.api_path()), Some("v_vol1://".to_string()), "在卷根下建目录");
        assert_eq!(p.phase(), &Phase::Loading, "建目录时还在等");

        // 建好后再打开默认目录。
        p.default_created("v_vol1");
        assert_eq!(requested(&mut p), Some(Some("v_vol1://assets-psd".to_string())));
        p.set_entries(vec![entry("file", "v_vol1://assets-psd/a.psd", "a.psd", "psd")]);
        assert_eq!(p.phase(), &Phase::Browsing);

        // 别的失败不进建目录流程。
        let mut q = open_picker();
        q.set_volumes(vec![volume("vol1", "Space")]);
        let denied = crate::vos::model::Failure { code: 10046, message: "no permission".into() };
        assert_eq!(q.enter_failed(&denied), None, "权限失败不建目录");
        assert!(matches!(q.phase(), Phase::Error(_)), "照常报告");
    }

    /// 用户从默认目录向上回到卷根后，不会再被拉回去。
    #[test]
    fn going_up_from_the_default_directory_stays_at_the_root() {
        let mut p = open_picker();
        p.set_volumes(vec![volume("vol1", "Space")]);
        p.set_entries(vec![entry("dir", "v_vol1://assets-psd", "assets-psd", "")]);
        p.up();
        assert_eq!(requested(&mut p), Some(Some("v_vol1://".to_string())), "回卷根");
        p.set_entries(vec![entry("dir", "v_vol1://assets-psd", "assets-psd", ""), entry("file", "v_vol1://notes.psd", "notes.psd", "psd")]);
        assert_eq!(p.current().map(|c| c.api_path()), Some("v_vol1://".to_string()), "留在卷根");
        assert_eq!(p.visible_entries().count(), 2, "卷根的文件都看得到");
    }

    #[test]
    fn clicking_a_volume_enters_its_default_directory() {
        let mut p = open_picker();
        p.set_volumes(vec![volume("vol1", "Space")]);
        p.enter_default("v_vol1");
        assert_eq!(p.phase(), &Phase::Loading);
        assert_eq!(p.current().map(|c| c.api_path()), Some("v_vol1://assets-psd".to_string()));
    }

    #[test]
    fn the_mode_is_reported_so_the_window_can_label_its_buttons() {
        assert_eq!(*open_picker().mode(), Mode::Open { multiple: true });
        assert_eq!(*Picker::new(Mode::Save, Vec::new(), None).mode(), Mode::Save);
    }

    #[test]
    fn up_is_only_offered_while_browsing() {
        let mut p = open_picker();
        assert!(!p.can_go_up(), "nothing to go up from before a volume is chosen");
        p.set_volumes(vec![volume("vol1", "Space")]);
        assert!(!p.can_go_up(), "the listing has not arrived yet");
        p.set_entries(Vec::new());
        assert!(p.can_go_up());
        p.fail("network");
        assert!(!p.can_go_up(), "the error state offers Retry instead");
    }

    #[test]
    fn clicking_a_selected_file_again_deselects_it() {
        let a = entry("file", "v_vol1://a.psd", "a.psd", "psd");
        let mut p = open_picker();
        p.set_volumes(vec![volume("vol1", "Space")]);
        p.toggle(&a);
        p.toggle(&a);
        assert_eq!(p.confirm(), None, "deselecting the last file leaves nothing to open");
    }

    #[test]
    fn the_filter_hides_other_files_but_never_directories() {
        let mut p = open_picker();
        p.set_volumes(vec![volume("vol1", "Space")]);
        p.set_entries(vec![
            entry("file", "v_vol1://a.psd", "a.psd", "psd"),
            entry("file", "v_vol1://notes.txt", "notes.txt", "txt"),
            entry("dir", "v_vol1://pics", "pics", ""),
        ]);
        let shown: Vec<&str> = p.visible_entries().map(|e| e.basename.as_str()).collect();
        assert_eq!(shown, vec!["a.psd", "pics"], "a directory is always reachable");
    }

    #[test]
    fn an_empty_filter_lists_everything() {
        let mut p = Picker::new(Mode::Open { multiple: true }, Vec::new(), None);
        p.set_volumes(vec![volume("vol1", "Space")]);
        p.set_entries(vec![entry("file", "v_vol1://a.xyz", "a.xyz", "xyz")]);
        assert_eq!(p.visible_entries().count(), 1);
    }

    #[test]
    fn the_filter_accepts_extensions_written_with_a_dot() {
        let mut p = Picker::new(Mode::Open { multiple: true }, vec![".PSD".into()], None);
        p.set_volumes(vec![volume("vol1", "Space")]);
        p.set_entries(vec![entry("file", "v_vol1://a.PSD", "a.PSD", "PSD")]);
        assert_eq!(p.visible_entries().count(), 1);
    }

    /// 空字段里没有名字（提示文字只是占位），敲进去的第一个字符不会丢。
    #[test]
    fn an_empty_field_is_not_a_name_and_loses_no_keystroke() {
        let mut p = Picker::new(Mode::Save, Vec::new(), None);
        assert_eq!(p.file_name_text(), "", "the placeholder is display only");
        assert!(p.confirm().is_none(), "an empty field cannot be saved");
        set_name(&mut p, "a");
        assert_eq!(p.file_name_text(), "a", "the first keystroke must survive");
        set_name(&mut p, "art.psd");
        assert_eq!(p.file_name_text(), "art.psd", "no stub of the hint survives");
    }

    #[test]
    fn the_name_field_text_rebuilds_from_the_picker() {
        // 输入框显示的文本必须能从选择器重建（`file_name_text`）。空格、全角字符、点号
        // 都要原样回来。
        let mut p = Picker::new(Mode::Save, Vec::new(), None);
        for typed in [" art.psd", "art .psd ", "未命名.psd", "中文.psd", "a b.psd", "..psd"] {
            set_name(&mut p, typed);
            assert_eq!(p.file_name_text(), typed, "{typed} comes back unchanged");
        }
    }

    /// 名字里的 `/` 不能让保存的位置换到别的目录。
    #[test]
    fn a_save_name_cannot_escape_the_directory() {
        let mut p = Picker::new(Mode::Save, Vec::new(), Some("art.psd"));
        p.set_volumes(vec![crate::vos::model::Volume { uuid: "v1".into(), nick_name: None, name: "Space".into() }]);
        p.enter("v_v1://assets-psd");
        // `TextEdit` 直接写这个字符串，不做过滤。
        *p.file_name_mut() = "../../etc/passwd".into();
        let Some(crate::vos::state::Choice::Save(path)) = p.confirm() else { panic!("saving needs a name") };
        assert_eq!(path, "v_v1://assets-psd/....etcpasswd", "分隔符不能把保存的位置换到别的目录");
    }

    #[test]
    fn typing_replaces_the_suggested_name_the_way_a_save_dialog_does() {
        // 建议的名字在打开时全选，敲字就换掉它。
        let mut p = Picker::new(Mode::Save, Vec::new(), Some("/home/me/art.psd"));
        assert_eq!(p.file_name_text(), "art.psd");
        set_name(&mut p, "poster.psd");
        assert_eq!(p.file_name_text(), "poster.psd");
        p.set_volumes(vec![volume("vol1", "Space")]);
        p.set_entries(Vec::new());
        assert_eq!(p.confirm(), Some(Choice::Save("v_vol1://assets-psd/poster.psd".into())));
    }

    #[test]
    fn save_names_a_file_in_the_current_directory() {
        let mut p = Picker::new(Mode::Save, Vec::new(), Some("/home/me/art.psd"));
        assert_eq!(p.file_name_text(), "art.psd", "only the file name is kept");
        p.set_volumes(vec![volume("vol1", "Space")]);
        p.enter("v_vol1://pics");
        p.set_entries(Vec::new());
        assert_eq!(p.confirm(), Some(Choice::Save("v_vol1://pics/art.psd".into())));
    }

    #[test]
    fn save_at_the_volume_root_has_no_extra_slash() {
        let mut p = Picker::new(Mode::Save, Vec::new(), Some("art.psd"));
        p.set_volumes(vec![volume("vol1", "Space")]);
        p.set_entries(Vec::new());
        assert_eq!(p.confirm(), Some(Choice::Save("v_vol1://assets-psd/art.psd".into())));
    }

    #[test]
    fn save_needs_a_name() {
        let mut p = Picker::new(Mode::Save, Vec::new(), None);
        p.set_volumes(vec![volume("vol1", "Space")]);
        p.set_entries(Vec::new());
        assert!(p.confirm().is_none());
        set_name(&mut p, "   ");
        assert!(p.confirm().is_none(), "whitespace is not a name");
        set_name(&mut p, "art.psd");
        assert!(p.confirm().is_some());
    }

    #[test]
    fn save_is_impossible_before_a_volume_is_chosen() {
        let p = Picker::new(Mode::Save, Vec::new(), Some("art.psd"));
        assert_eq!(p.confirm(), None);
    }

    #[test]
    fn a_failure_is_shown_and_can_be_retried() {
        let mut p = open_picker();
        p.set_volumes(vec![volume("vol1", "Space")]);
        p.enter("v_vol1://pics");
        p.fail("no access");
        assert_eq!(p.phase(), &Phase::Error("no access".into()));
        p.retry();
        assert_eq!(p.phase(), &Phase::Loading, "retry re-asks for the same directory");
        assert_eq!(p.current().map(|c| c.api_path()), Some("v_vol1://pics".to_string()));
        assert_eq!(requested(&mut p), Some(Some("v_vol1://pics".into())));
    }

    #[test]
    fn retrying_before_any_directory_asks_for_the_volumes_again() {
        let mut p = open_picker();
        p.fail("volume list failed");
        p.retry();
        assert_eq!(p.phase(), &Phase::Loading);
        assert!(p.current().is_none());
        assert_eq!(requested(&mut p), Some(None), "the volume list (a directory would mean a listing)");
    }

    #[test]
    fn a_finished_listing_is_not_asked_for_again() {
        // 已完成的列表不能看起来像待处理的请求：那样会每帧重新要卷列表，在转圈和文件之间闪现。
        let mut p = open_picker();
        assert_eq!(frame(&mut p), Some(None), "the first frame asks for the volumes");
        assert_eq!(frame(&mut p), Some(Some("v_vol1://assets-psd".into())), "and then for the only volume's default directory");
        assert_eq!(p.phase(), &Phase::Browsing);
        for _ in 0..3 {
            assert_eq!(frame(&mut p), None, "nothing is left to ask for");
            assert_eq!(p.phase(), &Phase::Browsing, "so the files stay on screen");
        }
    }

    #[test]
    fn a_new_picker_asks_for_the_volumes() {
        let mut p = Picker::new(Mode::Open { multiple: false }, Vec::new(), None);
        assert_eq!(p.phase(), &Phase::Loading);
        assert!(p.current().is_none(), "no directory yet, so the request is the volume list");
        assert_eq!(requested(&mut p), Some(None));
        assert_eq!(requested(&mut p), None, "a request is started once, not every frame");
    }

    #[test]
    fn a_non_finder_path_fails_instead_of_browsing_somewhere_wrong() {
        let mut p = open_picker();
        p.enter("/home/me/pics");
        assert!(matches!(p.phase(), Phase::Error(_)));
        assert!(p.current().is_none());
    }

    /// 补扩展名的边界：结尾的点不算扩展名，已有的扩展名不动。
    #[test]
    fn a_missing_extension_is_added_back() {
        assert_eq!(with_extension("poster".into(), "psd"), "poster.psd");
        assert_eq!(with_extension("poster.".into(), "psd"), "poster.psd", "结尾的点不算扩展名");
        assert_eq!(with_extension("poster..".into(), "psd"), "poster.psd");
        assert_eq!(with_extension("poster.tif".into(), "psd"), "poster.tif", "已有的扩展名不动");
        assert_eq!(with_extension(".hidden".into(), "psd"), ".hidden.psd", "只有名字的文件也要有扩展名");
        assert_eq!(with_extension("poster".into(), ""), "poster", "条目没有扩展名时不补");
    }

    /// 改名只换最后一段，`path` 也一起改。
    #[test]
    fn renaming_moves_the_path_and_skips_no_ops() {
        let mut p = open_picker();
        p.set_volumes(vec![volume("vol1", "Space")]);
        let root = entry("file", "v_vol1://hello.psd", "hello.psd", "psd");
        let nested = entry("file", "v_vol1://pics/a.psd", "a.psd", "psd");
        p.set_entries(vec![root.clone(), nested.clone()]);
        let paths = |p: &Picker| p.visible_entries().map(|e| e.path.clone()).collect::<Vec<_>>();
        let rename = |p: &mut Picker, entry: Entry, to: &str| {
            p.start_rename(entry);
            *p.rename_name_mut().expect("renaming") = to.to_string();
            p.finish_rename(true)
        };
        let names = |p: &Picker| p.visible_entries().map(|e| e.basename.clone()).collect::<Vec<_>>();

        // 没变的名字和空名字都不发请求，输入框照样关掉。
        p.start_rename(root.clone());
        assert_eq!(p.renaming().map(|(_, n)| n.as_str()), Some("hello.psd"), "输入框预填现在的名字");
        assert_eq!(p.finish_rename(true), None, "名字没变不需要请求");
        assert!(p.renaming().is_none(), "输入框已关掉");
        assert_eq!(rename(&mut p, root.clone(), "  "), None, "空名字不需要请求");

        // 卷根：scheme 后面直接是名字。
        assert!(rename(&mut p, root.clone(), "hello1234.psd").is_some());
        assert_eq!(paths(&p), ["v_vol1://hello1234.psd", "v_vol1://pics/a.psd"]);

        // 子目录：只有最后一段换掉。
        assert!(rename(&mut p, nested.clone(), "poster.psd").is_some());
        assert_eq!(paths(&p), ["v_vol1://hello1234.psd", "v_vol1://pics/poster.psd"]);

        // 没有扩展名时补回原来的。
        let current = p.visible_entries().find(|e| e.basename == "poster.psd").cloned().expect("the renamed file");
        assert!(rename(&mut p, current, "poster").is_some());
        assert_eq!(names(&p), ["hello1234.psd", "poster.psd"], "补回扩展名");
        let current = p.visible_entries().find(|e| e.basename == "poster.psd").cloned().expect("the renamed file");
        assert!(rename(&mut p, current, "poster.tif").is_some());
        // 过滤器只留 psd，改成 tif 后这一行就该消失。
        assert_eq!(names(&p), ["hello1234.psd"], "扩展名跟着改，不再匹配过滤器");

        // 取消：什么都不改。
        let current = p.entries.iter().find(|e| e.basename == "poster.tif").cloned().expect("the renamed file");
        p.start_rename(current);
        *p.rename_name_mut().expect("renaming") = "other.psd".into();
        assert_eq!(p.finish_rename(false), None);
        assert_eq!(paths(&p), ["v_vol1://hello1234.psd"], "取消后仍然是 tif，仍然被过滤");
    }

    #[test]
    fn dropping_a_deleted_file_forgets_its_selection() {
        let a = entry("file", "v_vol1://a.psd", "a.psd", "psd");
        let b = entry("file", "v_vol1://b.psd", "b.psd", "psd");
        let mut p = open_picker();
        p.set_volumes(vec![volume("vol1", "Space")]);
        p.set_entries(vec![a.clone(), b.clone()]);
        p.toggle(&a);
        p.toggle(&b);
        p.drop_item("v_vol1://a.psd");
        assert_eq!(p.visible_entries().map(|e| e.path.as_str()).collect::<Vec<_>>(), ["v_vol1://b.psd"]);
        assert_eq!(p.confirm(), Some(Choice::Open(vec![picked("v_vol1", "v_vol1://b.psd")])), "a deleted file is not handed back");
    }

    #[test]
    fn the_selection_survives_a_failed_reload_of_the_same_directory() {
        let mut p = open_picker();
        p.set_volumes(vec![volume("vol1", "Space")]);
        p.set_entries(vec![entry("file", "v_vol1://a.psd", "a.psd", "psd")]);
        p.toggle(&entry("file", "v_vol1://a.psd", "a.psd", "psd"));
        p.fail("network");
        assert!(p.confirm().is_some(), "the selection is still the user's choice");
    }
}
