//! Finder 选择器窗口：绘制 [`state::Picker`] 并把点击转发回去。
//!
//! 所有决定都在 `state.rs` 里；本文件只负责表现。它跑在 web 外壳的帧里，所以直接用 egui 绘制，
//! 而不是走 app 的面板。

use egui::{Context, RichText, Ui};
use photocraft_ui_egui::i18n;
use photocraft_ui_egui::theme::Tokens;

use super::model::Entry;
use super::state::{Choice, Mode, Phase, Picker};

/// 绘制完之后这一帧该做什么。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// 保持窗口打开。
    None,
    /// 用户确认了；把选择交给调用方。
    Confirm(Choice),
    /// 用户取消了（窗口关闭，对话框回取消）。
    Cancel,
}

/// 把选择器画成一个类模态窗口。`busy` 在有在途请求时禁用控件。
pub fn show(ctx: &Context, picker: &mut Picker, busy: bool) -> Action {
    let mut action = Action::None;
    egui::Window::new(title(picker.mode()))
        .id(id())
        .collapsible(false)
        .resizable(true)
        .default_size(egui::vec2(640.0, 460.0))
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            action = body(ui, picker, busy);
        });
    action
}

/// 选择器窗口的 id：键盘焦点记在它上面（见 [`super::install::own_keyboard`]）。
pub fn id() -> egui::Id {
    egui::Id::new("vos-finder-picker")
}

/// 保存输入框的 id（见 [`super::install::own_keyboard`]）。
pub fn name_field_id() -> egui::Id {
    egui::Id::new("vos-finder-name")
}

/// 窗口标题。
fn title(mode: &Mode) -> &'static str {
    match mode {
        Mode::Open { .. } => i18n::t("Open from VOS"),
        Mode::Save => i18n::t("Save to VOS"),
    }
}

/// 下角的主按钮：打开对话框是"Open"，保存对话框是"Save"。
fn primary(mode: &Mode) -> &'static str {
    match mode {
        Mode::Open { .. } => i18n::t("Open"),
        Mode::Save => i18n::t("Save"),
    }
}

fn body(ui: &mut Ui, picker: &mut Picker, busy: bool) -> Action {
    let tokens = Tokens::for_kind(picker.theme());
    let primary = primary(picker.mode());
    let mut action = Action::None;
    // 当前位置：卷选择界面，或正在浏览的路径。
    ui.horizontal(|ui| {
        let up = picker.can_go_up() && !busy;
        if ui.add_enabled(up, egui::Button::new(i18n::t("Up"))).clicked() {
            picker.up();
        }
        let location = picker.current().map(|c| c.api_path()).unwrap_or_else(|| i18n::t("VOS storage").to_string());
        ui.label(RichText::new(location).color(tokens.text_dim));
    });
    ui.separator();

    match picker.phase().clone() {
        Phase::Error(message) => {
            // 失败信息可能很长（异常的消息），所以它换行并滚动，而不是把窗口撑开：一整行不断开
            // 会把按钮挤出屏幕。
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(&message).color(tokens.warning)).wrap());
            });
            if ui.add_enabled(!busy, egui::Button::new(i18n::t("Retry"))).clicked() {
                picker.retry();
            }
        }
        Phase::Loading => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(i18n::t("Loading…"));
            });
        }
        Phase::Volumes => {
            ui.horizontal(|ui| {
                ui.label(i18n::t("Choose a storage space"));
                // 刷新和重试发起的是同一个请求：卷列表。没有它，空的卷选择界面（比如在另一个
                // 标签页里加了卷）就没有回来的路。
                if ui.add_enabled(!busy, egui::Button::new(i18n::t("Refresh"))).clicked() {
                    picker.retry();
                }
            });
            egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                if picker.volumes().is_empty() {
                    ui.label(RichText::new(i18n::t("No storage space is available")).color(tokens.text_dim));
                }
                // `enter_root` 需要可变借用 `picker`，所以先把点击的结果拷出借用。
                let clicked = picker.volumes().iter().find(|v| ui.add_enabled(!busy, egui::Button::new(v.label())).clicked()).map(|v| v.adapter());
                if let Some(adapter) = clicked {
                    picker.enter_root(&adapter);
                }
            });
        }
        Phase::Browsing => {
            egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                let mut clicked = None;
                let entries: Vec<Entry> = picker.visible_entries().cloned().collect();
                if entries.is_empty() {
                    ui.label(RichText::new(i18n::t("This folder is empty")).color(tokens.text_dim));
                }
                // 删除这类决定由 `picker` 记下，所以本帧只收集点击，循环结束后再执行。
                for entry in entries {
                    let (icon, size) = if entry.is_dir() { ("📁", None) } else { ("🖼", Some(human_size(entry.file_size))) };
                    ui.horizontal(|ui| {
                        let name = format!("{icon} {}", entry.label());
                        let button = egui::Button::new(name).selected(!entry.is_dir() && picker.is_selected(&entry.path)).min_size(egui::vec2(240.0, 0.0));
                        let response = ui.add_enabled(!busy, button);
                        if response.clicked() {
                            clicked = Some(entry.clone());
                        }
                        if let Some(size) = size {
                            ui.label(RichText::new(size).color(tokens.text_dim).small());
                        }
                    });
                }
                if let Some(file) = clicked {
                    picker.activate(&file);
                }
            });
        }
    }

    ui.separator();
    if matches!(picker.mode(), Mode::Save) {
        ui.horizontal(|ui| {
            ui.label(i18n::t("File name"));
            // 直接编辑 `picker` 持有的名字；在途请求时只让看不让改。
            let enter = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
            let response = ui.add_enabled(
                !busy,
                egui::TextEdit::singleline(picker.file_name_mut())
                    .id(name_field_id())
                    .hint_text(super::state::FILE_NAME_HINT)
                    .desired_width(260.0)
                    .min_size(egui::vec2(260.0, 20.0)),
            );
            // 回车就是主按钮；在途请求时主按钮是灰的，回车也不确认。
            if enter && !busy && response.has_focus() {
                action = picker.confirm().map_or(Action::None, Action::Confirm);
            }
        });
    }
    if !busy && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        action = Action::Cancel;
    }
    ui.horizontal(|ui| {
        if ui.add_enabled(!busy && picker.confirm().is_some(), egui::Button::new(primary)).clicked()
            && let Some(choice) = picker.confirm()
        {
            action = Action::Confirm(choice);
        }
        if ui.add_enabled(!busy, egui::Button::new(i18n::t("Cancel"))).clicked() {
            action = Action::Cancel;
        }
    });
    action
}

/// 字节数转成简短易读的字符串。
pub fn human_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB", "PB", "EB"];
    // 每十位一个单位，所以不用循环：二进制大小每进一位就是 log2(1024) = 10 位。位数少时也
    // 更小，所以一位数的情况 `B` 不必单独判断。封顶到最后一个单位，这样离谱的大小也不会索引
    // 出表。
    let unit = (((63 - (bytes | 1).leading_zeros()) / 10) as usize).min(UNITS.len() - 1);
    let value = bytes as f64 / 1024f64.powi(unit as i32);
    format!("{value:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::human_size;
    use crate::vos::state::Mode;

    /// 装 egui 的字体，并把主题点名的 `medium`/`semibold` 家族绑到比例字体上。
    ///
    /// 无头渲染没有随包字体：不装时文字宽度算成 0，`cursor_from_pos` 永远返回 0。
    fn install_test_fonts(ctx: &egui::Context) {
        let mut fonts = photocraft_ui_egui::egui_fonts::definitions();
        let proportional = fonts.families.get(&egui::FontFamily::Proportional).cloned().unwrap_or_default();
        for family in ["medium", "semibold"] {
            fonts.families.insert(egui::FontFamily::Name(family.into()), proportional.clone());
        }
        ctx.set_fonts(fonts);
        photocraft_ui_egui::theme::apply(ctx, photocraft_ui_egui::theme::ThemeKind::ProMedium);
    }

    #[test]
    fn sizes_read_as_short_units() {
        assert_eq!(human_size(0), "0.0 B");
        assert_eq!(human_size(512), "512.0 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(1024 * 1024), "1.0 MB");
        assert_eq!(human_size(3 * 1024 * 1024 * 1024), "3.0 GB");
        // 离谱的大小仍然留在单位表内。
        assert!(human_size(u64::MAX).ends_with(" EB"), "{}", human_size(u64::MAX));
    }

    /// 保存对话框里，敲进名字的字符要进到字段里，回车要当成保存。
    #[test]
    fn a_typed_name_reaches_the_field_and_enter_saves() {
        use egui_kittest::Harness;

        let picker = std::cell::RefCell::new(super::Picker::new(Mode::Save, Vec::new(), Some("Untitled-1.psd")));
        picker.borrow_mut().set_volumes(vec![crate::vos::model::Volume { uuid: "v1".into(), nick_name: None, name: "Space".into() }]);
        let actions = std::cell::RefCell::new(Vec::new());
        picker.borrow_mut().set_theme(photocraft_ui_egui::theme::ThemeKind::ProMedium);
        let mut h = Harness::builder().with_size(egui::vec2(640.0, 460.0)).build_ui(|ui| {
            let mut picker = picker.borrow_mut();
            let action = super::show(ui.ctx(), &mut picker, false);
            super::super::install::own_keyboard(ui.ctx());
            if action != super::Action::None {
                actions.borrow_mut().push(action);
            }
        });
        install_test_fonts(&h.ctx);
        h.step();
        h.step();
        // 指针挪进对话框：这一帧起按键归对话框。
        h.event(egui::Event::PointerMoved(egui::pos2(320.0, 220.0)));
        h.step();
        h.step();
        h.step();
        h.step();
        // 点进输入框，把光标放到名字末尾。
        let rect = h.ctx.read_response(super::name_field_id()).expect("the field was drawn").rect;
        let click = rect.right_center() - egui::vec2(3.0, 0.0);
        h.event(egui::Event::PointerMoved(click));
        h.step();
        h.event(egui::Event::PointerButton { pos: click, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
        h.step();
        h.event(egui::Event::PointerButton { pos: click, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
        h.step();
        h.step();
        h.event(egui::Event::Text("co".into()));
        h.step();
        h.step();
        assert_eq!(picker.borrow().file_name_text(), "Untitled-1.psdco");
        // 退格删掉的是最后一个字（默认名的最后一个字也得删得掉）。
        h.key_press(egui::Key::Backspace);
        h.step();
        h.step();
        assert_eq!(picker.borrow().file_name_text(), "Untitled-1.psdc");

        // 回车就是保存。
        actions.borrow_mut().clear();
        h.key_press(egui::Key::Enter);
        h.step();
        h.step();
        let confirmed = actions.borrow().iter().find_map(|action| match action {
            super::Action::Confirm(choice) => Some(choice.clone()),
            _ => None,
        });
        assert!(confirmed.is_some(), "回车就是保存：{:?}", actions.borrow());

        // 全删光、重新打一个名字：存下去的就是这个新名字。
        actions.borrow_mut().clear();
        let text = || picker.borrow().file_name_text().to_string();
        // 点进输入框再打字：用户不会先把光标摆到哪，就在框里打。
        // 再点一次输入框，把光标放回末尾。
        let rect = h.ctx.read_response(super::name_field_id()).expect("the field was drawn").rect;
        let again = rect.right_center() - egui::vec2(3.0, 0.0);
        h.event(egui::Event::PointerMoved(again));
        h.step();
        h.event(egui::Event::PointerButton { pos: again, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
        h.step();
        h.event(egui::Event::PointerButton { pos: again, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
        h.step();
        h.step();
        let mut backspaces = 0;
        while !text().is_empty() && backspaces < 40 {
            h.key_press(egui::Key::Backspace);
            h.step();
            h.step();
            backspaces += 1;
        }
        assert_eq!(text(), "", "退格能把名字删光（{backspaces} 次）");
        // 空框里按退格不能把键盘按丢：接着打的字还得进得来。
        h.key_press(egui::Key::Backspace);
        h.step();
        h.step();
        h.event(egui::Event::Text("new.psd".into()));
        h.step();
        h.step();
        assert_eq!(text(), "new.psd", "空框里按过退格，接着打的字还在");
        h.key_press(egui::Key::Enter);
        h.step();
        h.step();
        let saved = actions.borrow().iter().find_map(|action| match action {
            super::Action::Confirm(crate::vos::state::Choice::Save(path)) => Some(path.clone()),
            _ => None,
        });
        assert_eq!(saved.as_deref(), Some("v_v1://new.psd"), "输入的完整文件名就是保存路径");
    }

    /// 键盘不归选择器时，按的键也不能进到文件名里。
    ///
    /// 焦点交还 app 后（指针离开窗口），用户在文本工具里打的字不能被同时追进文件名。
    #[test]
    fn keystrokes_do_not_reach_the_name_when_the_picker_lacks_the_keyboard() {
        use egui_kittest::Harness;

        let picker = std::cell::RefCell::new(super::Picker::new(Mode::Save, Vec::new(), Some("art.psd")));
        picker.borrow_mut().set_volumes(vec![crate::vos::model::Volume { uuid: "v1".into(), nick_name: None, name: "Space".into() }]);
        let mut h = Harness::builder().with_size(egui::vec2(640.0, 460.0)).build_ui(|ui| {
            let mut picker = picker.borrow_mut();
            let _ = super::show(ui.ctx(), &mut picker, false);
        });
        h.step();
        h.event(egui::Event::Text("xyz".into()));
        h.step();
        h.step();
        assert_eq!(picker.borrow().file_name_text(), "art.psd", "键盘不在选择器手里，名字不能被改");
    }

    /// 有请求在途时按回车不能确认：主按钮此时是灰的。
    #[test]
    fn enter_does_not_confirm_while_a_request_is_in_flight() {
        use egui_kittest::Harness;

        let picker = std::cell::RefCell::new(super::Picker::new(Mode::Save, Vec::new(), Some("art.psd")));
        picker.borrow_mut().set_volumes(vec![crate::vos::model::Volume { uuid: "v1".into(), nick_name: None, name: "Space".into() }]);
        let actions = std::cell::RefCell::new(Vec::new());
        // `busy = true`
        let mut h = Harness::builder().with_size(egui::vec2(640.0, 460.0)).build_ui(|ui| {
            let mut picker = picker.borrow_mut();
            let action = super::show(ui.ctx(), &mut picker, true);
            if action != super::Action::None {
                actions.borrow_mut().push(action);
            }
        });
        h.step();
        h.key_press(egui::Key::Enter);
        h.step();
        h.step();
        assert!(actions.borrow().is_empty(), "在途请求时回车不能确认：{:?}", actions.borrow());
    }

    /// 把选择器渲染成 PNG 供人眼检查（不是断言）；行为断言在 `vos::model` 里。
    ///
    /// ```sh
    /// cargo test -p photocraft-web --bin photocraft-web render_the_picker -- --ignored --nocapture
    /// ```
    ///
    /// 图片写在 `target/vos-picker-*.png`。
    #[test]
    #[ignore = "renders PNGs for a human to look at; needs a GPU"]
    fn render_the_picker_for_the_reported_reply() {
        use egui_kittest::Harness;

        // `GET /v1000/finder_next/index` 的信封，文件名换成看得见的。
        let payload = r#"{"code":0,"category":"ok","data":{"adapter":"v_01M31B5XDFHPXSKCBSXAZVZA83","storages":[],"dirname":"v_01M31B5XDFHPXSKCBSXAZVZA83://","files":[{"type":"dir","path":"v_01M31B5XDFHPXSKCBSXAZVZA83://probe_folder","basename":"probe_folder","extension":"","storage":"v_01M31B5XDFHPXSKCBSXAZVZA83","file_size":4096},{"type":"file","path":"v_01M31B5XDFHPXSKCBSXAZVZA83://poster.psd","basename":"poster.psd","extension":"psd","storage":"v_01M31B5XDFHPXSKCBSXAZVZA83","file_size":2482184}],"sort_by":"name","sort_order":"asc"},"msg":"","version":"1.1.0-20261011.1.f0232367"}"#;
        // `data:` 前有一个空格。
        let body = format!(" data: {payload}\n\n");
        let adapter = "v_01M31B5XDFHPXSKCBSXAZVZA83";

        // 一对对比图：同一个正文分别当作错误消息和当作已解码的目录列表。
        let reported = "VOS returned an unreadable reply: expected value at line 1 column 1 | VOS: data: {\"code\":0,\"category\":\"ok\",\"data\":{\"adapter\":\"v_01M31B5XDFH (840 bytes) (VOS-0)";
        for (name, fail, files) in [
            ("reported-message", Some(reported.to_string()), None),
            (
                "decoded-listing",
                None,
                crate::vos::model::decode::<crate::vos::model::Index>(&crate::vos::model::reply_body(&body, "VOS")).ok().map(|index| index.files),
            ),
        ] {
            let picker = std::cell::RefCell::new(super::Picker::new(Mode::Open { multiple: false }, Vec::new(), None));
            {
                let mut p = picker.borrow_mut();
                p.set_volumes(vec![crate::vos::model::Volume { uuid: adapter.into(), nick_name: Some("vos_workspace".into()), name: adapter.into() }]);
                p.enter_root(adapter);
                match (fail, files) {
                    (Some(message), _) => p.fail(message),
                    (None, Some(files)) => p.set_entries(files),
                    (None, None) => p.fail("reply_body could not decode the envelope".to_string()),
                }
            }
            let mut h = Harness::builder().with_size(egui::vec2(700.0, 420.0)).build_ui(|ui| {
                let mut picker = picker.borrow_mut();
                let _ = super::show(ui.ctx(), &mut picker, false);
            });
            install_test_fonts(&h.ctx);
            h.run_steps(3);
            let image = h.render().expect("the GPU renderer");
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
            std::fs::create_dir_all(&dir).ok();
            let path = dir.join(format!("vos-picker-{name}.png"));
            image.save(&path).expect("write the PNG");
            eprintln!("wrote {}", path.display());
        }
    }

    /// 渲染选择器的几个常见状态，供人眼检查。
    ///
    /// ```sh
    /// cargo test -p photocraft-web --bin photocraft-web render_picker_states -- --ignored --nocapture
    /// ```
    ///
    /// 图片写在 `target/vos-picker-state-*.png`。
    #[test]
    #[ignore = "renders PNGs for a human to look at; needs a GPU"]
    fn render_picker_states() {
        use egui_kittest::Harness;

        let adapter = "v_01M31B5XDFHPXSKCBSXAZVZA83";
        let files = vec![
            crate::vos::model::Entry {
                kind: "dir".into(),
                path: format!("{adapter}://probe_folder"),
                basename: "probe_folder".into(),
                extension: String::new(),
                file_size: 4096,
                storage: adapter.into(),
            },
            crate::vos::model::Entry {
                kind: "file".into(),
                path: format!("{adapter}://poster.psd"),
                basename: "poster.psd".into(),
                extension: "psd".into(),
                file_size: 2_482_184,
                storage: adapter.into(),
            },
        ];

        // (名字, 模式, 填充, busy)
        let states: [(&str, bool, Option<&str>, bool); 5] = [
            ("open-listing", false, None, false),
            ("open-loading", false, Some("loading"), true),
            ("open-error", false, Some("error"), false),
            ("save-empty-name", true, None, false),
            ("save-with-name", true, Some("num.psd"), false),
        ];

        for (name, save, fill, busy) in states {
            let mode = if save { Mode::Save } else { Mode::Open { multiple: false } };
            let suggested = fill.filter(|f| f.ends_with(".psd"));
            let picker = std::cell::RefCell::new(super::Picker::new(mode, Vec::new(), suggested));
            {
                let mut p = picker.borrow_mut();
                p.set_volumes(vec![crate::vos::model::Volume { uuid: adapter.into(), nick_name: Some("vos_workspace".into()), name: adapter.into() }]);
                p.enter_root(adapter);
                match fill {
                    Some(name) if name.ends_with(".psd") => p.set_entries(files.clone()),
                    Some("loading") => p.retry(),
                    Some("error") => p.fail("VOS storage request failed: no permission (VOS-10046)".to_string()),
                    _ => p.set_entries(files.clone()),
                }
            }
            let mut h = Harness::builder().with_size(egui::vec2(700.0, 460.0)).build_ui(|ui| {
                let mut picker = picker.borrow_mut();
                let _ = super::show(ui.ctx(), &mut picker, busy);
            });
            install_test_fonts(&h.ctx);
            h.run_steps(3);
            let image = h.render().expect("the GPU renderer");
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target");
            std::fs::create_dir_all(&dir).ok();
            let path = dir.join(format!("vos-picker-state-{name}.png"));
            image.save(&path).expect("write the PNG");
            eprintln!("wrote {}", path.display());
        }
    }

    /// 输入框里的名字能选中、选中后能删掉；`Backspace` 删末尾一个字符。
    #[test]
    fn the_name_can_be_selected_and_deleted() {
        use egui_kittest::Harness;

        let picker = std::cell::RefCell::new(super::Picker::new(Mode::Save, Vec::new(), Some("Untitled-1.psd")));
        picker.borrow_mut().set_volumes(vec![crate::vos::model::Volume { uuid: "v1".into(), nick_name: None, name: "Space".into() }]);
        let mut h = Harness::builder().with_size(egui::vec2(640.0, 460.0)).build_ui(|ui| {
            let mut picker = picker.borrow_mut();
            super::super::install::own_keyboard(ui.ctx());
            let _ = super::show(ui.ctx(), &mut picker, false);
        });
        install_test_fonts(&h.ctx);
        h.step();
        h.event(egui::Event::PointerMoved(egui::pos2(320.0, 220.0)));
        h.step();
        h.step();
        let rect = h.ctx.read_response(super::name_field_id()).expect("the field was drawn").rect;
        let click = rect.right_center() - egui::vec2(3.0, 0.0);
        h.event(egui::Event::PointerMoved(click));
        h.step();
        h.event(egui::Event::PointerButton { pos: click, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
        h.step();
        h.event(egui::Event::PointerButton { pos: click, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
        h.step();
        h.step();

        // 全选：真的选中了才能删。
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        h.step();
        h.step();
        let selected = h
            .ctx
            .data(|d| d.get_temp::<egui::text_edit::TextEditState>(super::name_field_id()))
            .and_then(|s| s.cursor.char_range())
            .is_some_and(|r| r.primary.index.0 != r.secondary.index.0);
        assert!(selected, "Cmd+A 要真的选中整段名字");
        // Delete 清掉选中的内容。
        h.key_press(egui::Key::Delete);
        h.step();
        h.step();
        assert_eq!(picker.borrow().file_name_text(), "", "Delete 清掉选中的名字");

        // 空框里打名字，Backspace 删末尾一个字符。
        h.event(egui::Event::Text("poster.psd".into()));
        h.step();
        h.step();
        assert_eq!(picker.borrow().file_name_text(), "poster.psd");
        h.key_press(egui::Key::Backspace);
        h.step();
        h.step();
        assert_eq!(picker.borrow().file_name_text(), "poster.ps", "Backspace 删末尾");
    }
}
