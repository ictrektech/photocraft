//! 插件的对外表面：把 Finder 对话框装进 `Services`、绘制选择器，并维护首页的 VOS 列表。
//!
//! 外壳在启动时把 `Services` 交给我们一次。我们替换 `file_dialog`（打开/保存）、包装 `write`
//! （保存的字节），并填好 `platform_files`/`platform_open`（首页的列表和它的点击），其余服务
//! 不动。插件状态全放在 `thread_local!` 里，因为 wasm 是单线程，外壳也没有地方放它。
//!
//! 异步形态：对话框的答案必须晚几帧才回来，所以一个请求把它的 [`FileDialogReply`] 停在这里，
//! 由每帧的 [`ui`] 完成其余部分：它发起选择器在等的 Finder 请求、把答案交回去，并在用户选定后
//! 结束对话。所有异步操作都由这一个帧钩子驱动，所以在途请求永远不会丢掉停放的回复。应用自己
//! 要用的字节（首页打开）在同一份状态里等着，由 [`ui`] 打开 —— 这是唯一持有
//! `&mut PhotocraftApp` 的地方。没有桥时，原样的浏览器对话框继续使用。

use std::cell::RefCell;
use std::collections::HashMap;

#[cfg(target_arch = "wasm32")]
use photocraft_ui_egui::{FileDialogAnswer, FileDialogRequest, PhotocraftApp, PlatformRequest, Services};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_futures::spawn_local;

use super::open;
#[cfg(target_arch = "wasm32")]
use super::path::{self, FinderPath};
#[cfg(any(target_arch = "wasm32", test))]
use super::picker;
#[cfg(target_arch = "wasm32")]
use super::picker::Action;
use super::recent::Recent;
use super::state::Picker;
#[cfg(target_arch = "wasm32")]
use super::state::{Choice, Mode, Phase};
#[cfg(target_arch = "wasm32")]
use super::{bridge, home, http, model};

#[derive(Default)]
#[cfg_attr(test, expect(dead_code, reason = "host tests drive the upload scheduling only"))]
struct State {
    picker: Option<Picker>,
    /// 正在等选择器作答的对话框。停在这里而不是选择器里，这样在途请求永远不会丢掉它
    /// （丢掉的回复会被读成取消）。
    reply: Option<photocraft_ui_egui::FileDialogReply>,
    /// 保存交来的字节，等待上传，按路径存。
    writes: HashMap<String, Vec<u8>>,
    /// 下一次保存领的序号（见 `saving`）。
    next_save: u64,
    /// 每条路径上，已交给上传的最新保存（那个上传结束后为 `None`）。
    /// 同一文件的旧上传还在路上时落地的保存，与它之间没有先后保证，所以它在这里等那次上传
    /// 结束，而不是和它抢。
    saving: HashMap<String, (u64, Option<u64>)>,
    /// 首页列出的 VOS 文件，最新的在前。
    recent: Recent,
    /// 为首页取回的文件，晚一帧再打开（路径, 字节）。
    opened: Vec<(String, Vec<u8>)>,
    /// 晚一帧报告的错误，那时才有 app 可报。
    errors: Vec<String>,
    /// 平台说已不存在的最近路径，晚一帧从列表里删掉（`Recent` 不是共享的，取回操作碰不到它）。
    forgotten: Vec<String>,
    /// 首页请求打开平台自己的浏览器（由持有 app 的 [`ui`] 执行）。
    browse: bool,
    /// 另一个应用交接过来、还没取回的文件（`?open=`，见 [`open`]）。
    launch: Option<open::Launch>,
    /// 已取回的交接文件：Finder 路径（URL 来源为 `None`）、标题、字节。
    launched: Vec<(Option<String>, String, Vec<u8>)>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

/// 插件是否应当接管文件对话框（桥存在时）。
#[cfg(target_arch = "wasm32")]
pub fn active() -> bool {
    bridge::available()
}

///
/// 必须在 app 运行前调用一次。没有桥时什么都不改。
#[cfg(target_arch = "wasm32")]
pub fn install(services: &mut Services) {
    if !active() {
        log::info!("photocraft-web: no VOS bridge; keeping the browser file dialogs");
        return;
    }
    // 首页的列表通过 localStorage 在刷新后保留。
    let stored = web_sys::window().and_then(|w| w.local_storage().ok().flatten()).and_then(|s| s.get_item(home::RECENT_KEY).ok().flatten());
    with(|s| s.recent = Recent::load(stored.as_deref()));
    // 另一个应用交接过来的文件（`?open=`，见 `docs/vos-integration.md` › Handoff）。
    let search = web_sys::window().and_then(|w| w.location().search().ok()).unwrap_or_default();
    match open::parse(&search) {
        // 取字节与打开在 `ui` 里做，成功后在那里清掉查询串。
        Ok(Some(launch)) => {
            log::info!("photocraft-web: opening {} handed over by {}", launch.url, launch.source.as_deref().unwrap_or("another app"));
            with(|s| s.launch = Some(launch));
        }
        Ok(None) => {}
        Err(e) => {
            log::warn!("photocraft-web: ignored ?open=: {e}");
            with(|s| s.errors.push(e));
            // 坏链接同样清掉。
            clear_query();
        }
    }
    services.file_dialog = Some(Box::new(|request, _parent, reply| show_dialog(request, reply)));
    services.platform_files = Some(Box::new(|| with(|s| home::listing(s.recent.paths()))));
    services.platform_open = Some(Box::new(|request| match request {
        PlatformRequest::Open(path) => home::open(path, |path, result| {
            with(|s| match result {
                Ok(bytes) => {
                    // 记下它：正是这个列表让刷新后能回到这份文件。
                    s.recent.record(&path);
                    s.opened.push((path, bytes));
                }
                Err(e) => {
                    // 永远打不开的行从首页删掉。
                    if http::hopeless(&e) {
                        s.forgotten.push(path);
                    }
                    s.errors.push(e.to_string());
                }
            });
        }),
        // 选择器需要 app，所以晚一帧由 `ui` 执行。
        PlatformRequest::Browse => with(|s| s.browse = true),
        PlatformRequest::Forget(path) => with(|s| s.forgotten.push(path)),
    }));
    // 包装 `write`：Finder 路径的保存先暂存待上传，其他路径交给 `web.rs` 装好的写入器
    // （浏览器下载）。`WriteFn` 是 `FnMut`，所以那个写入器要多持有着、经它转发调用。

    let mut save_to_disk = services.write.take();
    services.write = Some(Box::new(move |path: &str, bytes: &[u8]| {
        if path::adapter_of(path).is_none() {
            return match save_to_disk.as_mut() {
                Some(write) => write(path, bytes),
                // `web.rs` 在本插件之前就装好了下载写入器，所以这里走不到。
                None => Ok(()),
            };
        }
        // 只暂存：写入器一应答外壳就把文件标为已保存，所以上传本身的失败由 `ui` 报告。
        stage(path, bytes);
        Ok(())
    }));
}

/// 去掉交接参数，保留其它查询参数、路径与片段。
#[cfg(target_arch = "wasm32")]
fn clear_query() {
    let Some(window) = web_sys::window() else { return };
    let location = window.location();
    let Ok(path) = location.pathname() else { return };
    let hash = location.hash().unwrap_or_default();
    let rest = open::without_handoff(&location.search().unwrap_or_default());
    let query = if rest.is_empty() { String::new() } else { format!("?{rest}") };
    let Ok(history) = window.history() else { return };
    if let Err(e) = history.replace_state_with_url(&wasm_bindgen::JsValue::UNDEFINED, "", Some(&format!("{path}{query}{hash}"))) {
        log::warn!("photocraft-web: could not clear the ?open= query: {}", bridge::describe(&e));
    }
}

/// 暂存一次保存的字节，直到它的上传开始。
fn stage(path: &str, bytes: &[u8]) {
    with(|s| {
        s.next_save += 1;
        // 第一次保存到这条路径时要占一个位置：`due_uploads` 只上传 `saving` 里已有的路径。
        // 在途的上传要留着，否则第二次保存会和它同时上传同一个文件。
        s.saving.entry(path.to_string()).and_modify(|slot| slot.0 = s.next_save).or_insert((s.next_save, None));
        s.writes.insert(path.to_string(), bytes.to_vec());
    });
}

/// 现在可以开始的上传：没有在途请求的路径，以及上传刚结束的路径。每条路径只发射最新的
/// 那次保存，且绝不同时发两次。
#[cfg(target_arch = "wasm32")]
fn start_due() -> Vec<(String, Vec<u8>, u64)> {
    with(due_uploads)
}

/// [`start_due`] 的正文，接受任意状态（测试也要用它）。
fn due_uploads(s: &mut State) -> Vec<(String, Vec<u8>, u64)> {
    let mut due = Vec::new();
    for (path, (newest, running)) in &mut s.saving {
        if running.is_some() {
            continue;
        }
        let Some(bytes) = s.writes.remove(path) else { continue };
        *running = Some(*newest);
        due.push((path.clone(), bytes, *newest));
    }
    due
}

/// 为 `request` 打开选择器，把 `reply` 停放起来等用户作答。
#[cfg(target_arch = "wasm32")]
fn show_dialog(request: FileDialogRequest, reply: photocraft_ui_egui::FileDialogReply) {
    let picker = match request {
        FileDialogRequest::Open { multiple, .. } => {
            Picker::new(Mode::Open { multiple }, crate::web::OPEN_EXTS.iter().map(|e| (*e).to_string()).collect(), None)
        }
        FileDialogRequest::Save { suggested } => Picker::new(Mode::Save, Vec::new(), Some(&suggested)),
        // 文件夹对话框在这里对 Finder 没有意义；回取消，让命令自己报出来。
        FileDialogRequest::Folder { .. } => {
            reply.send(None);
            return;
        }
    };
    // 这里不启动任何异步任务：选择器已把卷列表请求记下，由 `ui` 发起，所以停放的回复不会
    // 被一个在帧结束前就完成的请求弄丢。
    with(|s| {
        s.picker = Some(picker);
        s.reply = Some(reply);
    });
}

/// 作答停放的对话框并关闭选择器。
#[cfg(target_arch = "wasm32")]
fn answer(choice: Option<Choice>) {
    let (reply, picker) = with(|s| (s.reply.take(), s.picker.take()));
    let Some(reply) = reply else { return };
    match (choice, picker) {
        (Some(Choice::Open(chosen)), _) => {
            spawn_local(async move {
                let mut files = Vec::new();
                let mut failure = String::new();
                for file in chosen {
                    let name = path::display_name(&file.path);
                    match http::download(&file.adapter, &file.path).await {
                        Ok(bytes) => {
                            // 选中的文件加入首页列表。
                            with(|s| s.recent.record(&file.path));
                            files.push((name, bytes));
                        }
                        Err(e) => {
                            log::error!("VOS download failed for {}: {e}", file.path);
                            failure = e.to_string();
                            with(|s| s.errors.push(failure.clone()));
                        }
                    }
                }
                store_recent();
                if files.is_empty() {
                    // 每一次选取都失败了，每个原因都已经显示在屏幕上。`None` 是取消的答案，所以改回
                    // 一个失败。
                    reply.send(Some(FileDialogAnswer::Unavailable(failure)));
                } else {
                    reply.send(Some(FileDialogAnswer::ContentsMany(files)));
                }
            });
        }
        (Some(Choice::Save(path)), _) => reply.send(Some(FileDialogAnswer::SaveTo(path))),
        (None, _) => reply.send(None),
    }
}

/// 每帧：打开已到齐的数据、报告错误、上传保存、绘制选择器，并完成上一帧开始的事。
#[cfg(target_arch = "wasm32")]
pub fn ui(app: &mut PhotocraftApp, ctx: &egui::Context) {
    // 首页的"浏览"行：就是普通的 File > Open，由我们自己的对话框作答。
    if with(|s| std::mem::take(&mut s.browse))
        && let Err(e) = app.open_dialog_file()
    {
        report(app, e);
    }
    // 交接过来的文件：取回字节后留到下一帧打开。
    if let Some(launch) = with(|s| s.launch.take()) {
        let ctx = ctx.clone();
        spawn_local(async move {
            let adapter = FinderPath::split_file(&launch.url).map(|(dir, _)| dir.adapter);
            let (path, result) = match adapter {
                Some(adapter) => (Some(launch.url.clone()), http::download(&adapter, &launch.url).await),
                None => (None, http::download_url(&launch.url).await),
            };
            match result {
                Ok(bytes) => with(|s| s.launched.push((path, launch.name, bytes))),
                Err(e) => with(|s| s.errors.push(format!("{}: {e}", launch.name))),
            }
            ctx.request_repaint();
        });
    }
    for (path, name, bytes) in with(|s| std::mem::take(&mut s.launched)) {
        let opened = match &path {
            // Finder 路径成为文档路径：Ctrl+S 写回同一个文件。
            Some(path) => app.open_file(path, &bytes).map(|_| ()),
            // URL 只是来源，不是保存目标：Ctrl+S 走另存（浏览器下载或 Finder 选择器）。
            None => app.open_bytes(&name, &bytes).map(|_| ()),
        };
        if let Err(e) = opened {
            app.open_failed(&name, &e);
        } else {
            // 只消费一次：打开成功后清掉查询串。
            clear_query();
        }
        if let Some(path) = path {
            app.remove_recent(&path);
        }
    }
    // 首页打开的文件。传入完整的 Finder 路径，文件就记住它（Ctrl+S 写回 VOS），而标签页里只
    // 显示文件名。
    for (path, bytes) in with(|s| std::mem::take(&mut s.opened)) {
        if let Err(e) = app.open_file(&path, &bytes) {
            app.open_failed(&path::display_name(&path), &e);
        }
        // `open_file` 还会把路径加进 File › Open Recent，而 web 构建对它只会答"不可用"（没有
        // 可重开的路径）。首页的 VOS 列表就是 web 的最近列表，所以把它取回来，别留一条死菜单项。

        app.remove_recent(&path);
    }
    for message in with(|s| std::mem::take(&mut s.errors)) {
        report(app, message);
    }
    let forgotten = with(|s| std::mem::take(&mut s.forgotten));
    if !forgotten.is_empty() {
        with(|s| {
            for path in &forgotten {
                s.recent.forget(path);
            }
        });
        store_recent();
    }

    // 保存的字节：发射可以发的上传，然后报告已经发出去的那些的结果。
    for (path, bytes, ticket) in start_due() {
        let ctx = ctx.clone();
        spawn_local(async move {
            let result = match FinderPath::split_file(&path) {
                Some((dir, name)) => http::upload(&dir, &name, &bytes).await,
                None => Err(model::Failure { code: model::BAD_PATH, message: format!("{path} is not a VOS storage path") }),
            };
            finish_save(&path, ticket, result);
            ctx.request_repaint();
        });
    }

    let Some(mut picker) = with(|s| s.picker.take()) else { return };
    // 选择器要的卷列表或目录列表，每次请求至多一个。
    let wanted = picker.take_request();
    // 在途请求必须显得有动静：持续重绘让转圈在取数据时一直转，而不是一个看起来被无视的
    // 冻住的窗口。答案一到重绘就停。
    let busy = matches!(picker.phase(), Phase::Loading);
    if busy {
        ctx.request_repaint();
    }
    picker.set_theme(app.ui.theme);
    // 键盘要先定下来：`TextEdit` 只在有焦点的那一帧决定初始光标位置。
    own_keyboard(ctx);
    let action = picker::show(ctx, &mut picker, busy);
    let keep = !matches!(action, Action::Cancel);
    with(|s| s.picker = keep.then_some(picker));
    match action {
        Action::Confirm(choice) => answer(Some(choice)),
        Action::Cancel => answer(None),
        Action::Delete(entry) => remove(&entry.path, ctx),
        Action::Rename { entry, name, path } => rename(&entry.path, &name, &path, ctx),
        Action::None => {}
    }
    load(ctx, wanted);
}

/// 键盘焦点归谁：指针在选择器里时归选择器，出去时归 app。
///
/// 指针离开时交还窗口的焦点；指针在里面而焦点空着时把键盘交给窗口。已有焦点时不动：
/// 反复 `request_focus` 会清掉 egui 记的光标位置。
pub(crate) fn own_keyboard(ctx: &egui::Context) {
    let over = ctx.pointer_hover_pos().filter(|pos| ctx.layer_id_at(*pos).is_some_and(|layer| layer.order == egui::Order::Middle));
    let picker_id = picker::id();
    let inside = over.is_some();
    if ctx.data(|d| d.get_temp::<bool>(keyboard_id())) == Some(true) && !inside {
        ctx.memory_mut(|m| m.surrender_focus(picker_id));
    }
    ctx.data_mut(|d| d.insert_temp(keyboard_id(), inside));
    if inside && ctx.memory(|m| m.focused()).is_none() {
        ctx.memory_mut(|m| m.request_focus(picker_id));
    }
}

/// [`own_keyboard`] 存的“键盘是否归选择器”标记。
#[cfg(any(target_arch = "wasm32", test))]
fn keyboard_id() -> egui::Id {
    egui::Id::new("vos-finder-keyboard")
}

/// 把 `path` 改成 `name`，成功后就把它记成新路径 `renamed`；失败就报告，列表里的名字保持
/// 原样。
#[cfg(target_arch = "wasm32")]
fn rename(path: &str, name: &str, renamed: &str, ctx: &egui::Context) {
    let Some(file) = parse_or_report(path) else { return };
    // rename 的 `path` 是**文件所在目录**的全路径（带 adapter scheme），与 index 回复里的
    // dirname 同形；delete 的 `targets` 才是卷内相对路径。
    let Some(dir) = file.parent() else {
        with(|s| s.errors.push(format!("{path} is at a volume root, nothing to rename")));
        return;
    };
    let adapter = dir.adapter.clone();
    let directory = dir.api_path();
    let target = path.to_string();
    let name = name.to_string();
    let renamed = renamed.to_string();
    let ctx = ctx.clone();
    spawn_local(async move {
        match http::rename(&adapter, &directory, &target, &name).await {
            Ok(()) => {
                with(|s| {
                    // 首页的"VOS storage"就是最近列表。
                    s.recent.rename(&target, &renamed);
                });
                store_recent();
            }
            Err(e) => with(|s| s.errors.push(e.to_string())),
        }
        ctx.request_repaint();
    });
}

/// 解析 Finder 路径；不是的话把原因记进错误列表并返回 `None`。
#[cfg(target_arch = "wasm32")]
fn parse_or_report(path: &str) -> Option<FinderPath> {
    match FinderPath::parse_dir(path) {
        Some(dir) => Some(dir),
        None => {
            with(|s| s.errors.push(format!("{path} is not a VOS storage path")));
            None
        }
    }
}

/// 把 `path` 移到平台的回收站，并从选择器和首页的最近列表里去掉；失败就留在原处并报告。
#[cfg(target_arch = "wasm32")]
fn remove(path: &str, ctx: &egui::Context) {
    let Some(dir) = parse_or_report(path) else { return };
    // 删除接口的 `targets` 是卷内相对路径。
    let adapter = dir.adapter.clone();
    let target = dir.dir.trim_start_matches('/').to_string();
    let path = path.to_string();
    let ctx = ctx.clone();
    spawn_local(async move {
        match http::delete(&adapter, &target).await {
            Ok(()) => {
                with(|s| {
                    // 选择器可能正开着：删掉的条目要立刻从它的列表里消失。
                    if let Some(picker) = s.picker.as_mut() {
                        picker.drop_item(&path);
                    }
                    s.recent.forget(&path);
                });
                store_recent();
            }
            Err(e) => with(|s| s.errors.push(e.to_string())),
        }
        ctx.request_repaint();
    });
}

/// 取回选择器要的东西 —— `Some(None)` 是卷列表，`Some(Some(dir))` 是该目录的条目 —— 并把答案
/// 交回去。`None`（没要东西）什么都不做。
///
/// 失败进入 `Phase::Error`，屏幕上因此出现重试按钮；`retry` 随后再要一次同样的东西。
#[cfg(target_arch = "wasm32")]
fn load(ctx: &egui::Context, wanted: Option<Option<FinderPath>>) {
    let Some(dir) = wanted else { return };
    let ctx = ctx.clone();
    spawn_local(async move {
        // 进入唯一的卷会留下一个待取的列表，由下一帧取走。
        let outcome: Result<Prepare, model::Failure> = match &dir {
            None => http::volumes().await.map(Prepare::Volumes),
            Some(dir) => http::index(dir).await.map(|index| Prepare::Entries(index.files)),
        };
        answer_locally(&ctx, outcome);
    });
}

/// 一次完成的请求交回的东西：要的卷，或一个目录的条目。
#[cfg(target_arch = "wasm32")]
enum Prepare {
    Volumes(Vec<super::model::Volume>),
    Entries(Vec<super::model::Entry>),
}

/// 把完成的请求交给选择器（它可能已经不在了：用户中途取消了）。
#[cfg(target_arch = "wasm32")]
fn answer_locally(ctx: &egui::Context, outcome: Result<Prepare, model::Failure>) {
    let mut mkdir = None;
    with(|s| {
        let Some(picker) = s.picker.as_mut() else { return };
        match outcome {
            Ok(Prepare::Volumes(volumes)) => picker.set_volumes(volumes),
            Ok(Prepare::Entries(entries)) => picker.set_entries(entries),
            Err(e) => mkdir = picker.enter_failed(&e),
        }
    });
    if let Some(root) = mkdir {
        make_default_dir(ctx, root);
    }
    ctx.request_repaint();
}

/// 在卷根下建默认目录，建好后再打开它。
#[cfg(target_arch = "wasm32")]
fn make_default_dir(ctx: &egui::Context, root: FinderPath) {
    let ctx = ctx.clone();
    spawn_local(async move {
        match http::new_folder(&root, super::state::ASSETS_DIR).await {
            Ok(()) => with(|s| {
                if let Some(picker) = s.picker.as_mut() {
                    picker.default_created(&root.adapter);
                }
            }),
            Err(e) => with(|s| {
                if let Some(picker) = s.picker.as_mut() {
                    picker.fail(e.to_string());
                }
            }),
        }
        ctx.request_repaint();
    });
}

/// 记录一次保存上传的真实结果。写入器接受字节时外壳就把文件标为已保存，所以失败的
/// 上传在这里报告。
#[cfg(target_arch = "wasm32")]
fn finish_save(path: &str, ticket: u64, result: Result<(), model::Failure>) {
    // 这次上传在途时落地的保存才是更新的副本：不作声，等它。
    let waiting = with(|s| {
        let Some(slot) = s.saving.get_mut(path) else { return false };
        slot.1 = None;
        let waiting = slot.0 > ticket;
        s.saving.retain(|_, (_, running)| running.is_some());
        waiting
    });
    match result {
        // 只有到达 VOS 的保存才属于首页。
        Ok(()) => with(|s| s.recent.record(path)),
        Err(e) if !waiting => {
            // 外壳已经宣布“已保存”，所以失败必须说出来：说清是哪个文件，且文件没丢。
            let name = path::display_name(path);
            with(|s| s.errors.push(format!("{name} was not saved to VOS: {e} (the document is still in PhotoCraft, unsaved)")));
        }
        Err(e) => log::warn!("photocraft-web: save of {path} failed, a newer one is queued: {e}"),
    }
    store_recent();
}

/// 持久化首页的列表。被拒（隐私模式、配额）时内存里的列表照常工作，所以本次会话的文件
/// 仍然显示。
#[cfg(target_arch = "wasm32")]
fn store_recent() {
    with(|s| {
        let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) else { return };
        if let Err(e) = storage.set_item(home::RECENT_KEY, &s.recent.to_json()) {
            log::warn!("photocraft-web: could not store the VOS recent list: {e:?}");
        }
    });
}

/// 把 `message` 当错误显示：状态栏和通知各一份。
#[cfg(target_arch = "wasm32")]
fn report(app: &mut PhotocraftApp, message: String) {
    log::warn!("photocraft-web: {message}");
    app.ui.status = message.clone();
    app.ui.status_error = true;
    photocraft_ui_egui::notices::error(app, message);
}
#[cfg(test)]
mod tests {
    use super::{STATE, State, due_uploads, stage};

    /// 借出插件状态跑一段测试。`stage`/`due_uploads` 自己会借它，所以这里必须放手。
    ///
    /// 并行的测试共用这一个 `thread_local!`，所以每个测试只碰自己的路径。
    fn state<R>(f: impl FnOnce() -> R) -> R {
        STATE.with(|s| *s.borrow_mut() = State::default());
        let out = f();
        STATE.with(|s| *s.borrow_mut() = State::default());
        out
    }

    #[test]
    fn the_first_save_to_a_new_path_is_uploaded() {
        state(|| {
            stage("v_vol1://pics/art.psd", b"psd");
            let due = STATE.with(|s| due_uploads(&mut s.borrow_mut()));
            assert_eq!(due.len(), 1, "the staged bytes go to the uploader");
            assert_eq!(due[0].0, "v_vol1://pics/art.psd");
            assert_eq!(due[0].1, b"psd");
        });
    }

    #[test]
    fn nothing_waits_behind_an_upload_that_is_still_running() {
        state(|| {
            stage("v_vol1://pics/art.psd", b"one");
            assert_eq!(STATE.with(|s| due_uploads(&mut s.borrow_mut()).len()), 1);
            // 上传还在路上：第二次保存只排队，等它结束。
            stage("v_vol1://pics/art.psd", b"two");
            assert!(STATE.with(|s| due_uploads(&mut s.borrow_mut()).is_empty()), "never two uploads of one file at once");
        });
    }

    #[test]
    fn the_upload_after_a_finished_one_carries_the_newest_save() {
        state(|| {
            stage("v_vol1://pics/art.psd", b"one");
            let ticket = STATE.with(|s| due_uploads(&mut s.borrow_mut())[0].2);
            stage("v_vol1://pics/art.psd", b"two");
            stage("v_vol1://pics/art.psd", b"three");
            // 第一次上传结束：`finish_save` 把在途标记清掉。
            STATE.with(|s| {
                let mut s = s.borrow_mut();
                let Some(slot) = s.saving.get_mut("v_vol1://pics/art.psd") else { return };
                slot.1 = None;
                assert!(slot.0 > ticket, "a newer save is queued");
            });
            let due = STATE.with(|s| due_uploads(&mut s.borrow_mut()));
            assert_eq!(due.len(), 1);
            assert_eq!(due[0].1, b"three", "only the newest copy is sent");
        });
    }
}
