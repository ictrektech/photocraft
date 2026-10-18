//! Finder 选择器窗口：绘制 [`state::Picker`] 并把点击转发回去。
//!
//! 所有决定都在 `state.rs` 里；本文件只负责表现。它跑在 web 外壳的帧里，所以直接用 egui 绘制，
//! 而不是走 app 的面板。

use egui::{Context, RichText, Ui};
use photocraft_ui_egui::i18n;
use photocraft_ui_egui::theme::{ThemeKind, Tokens};

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
pub fn show(ctx: &Context, picker: &mut Picker, theme: ThemeKind, busy: bool) -> Action {
    let tokens = Tokens::for_kind(theme);
    let mut action = Action::None;
    let (title, primary) = match picker.mode() {
        Mode::Open { .. } => (i18n::t("Open from VOS"), i18n::t("Open")),
        Mode::Save => (i18n::t("Save to VOS"), i18n::t("Save")),
    };
    egui::Window::new(title)
        .collapsible(false)
        .resizable(true)
        .default_size(egui::vec2(640.0, 460.0))
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            action = body(ui, picker, &tokens, busy, primary);
        });
    action
}

fn body(ui: &mut Ui, picker: &mut Picker, tokens: &Tokens, busy: bool, primary: &str) -> Action {
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
                let entries: Vec<Entry> = picker.visible_entries().cloned().collect();
                if entries.is_empty() {
                    ui.label(RichText::new(i18n::t("This folder is empty")).color(tokens.text_dim));
                }
                for entry in entries {
                    ui.horizontal(|ui| {
                        if entry.is_dir() {
                            if ui.add_enabled(!busy, egui::Button::new(format!("📁 {}", entry.basename))).clicked() {
                                picker.activate(&entry);
                            }
                        } else {
                            let selected = picker.is_selected(&entry.path);
                            if ui.add_enabled(!busy, egui::Button::new(format!("🖼 {}", entry.basename)).selected(selected)).clicked() {
                                picker.activate(&entry);
                            }
                            ui.label(RichText::new(human_size(entry.file_size)).color(tokens.text_dim).small());
                        }
                    });
                }
            });
        }
    }

    ui.separator();
    if matches!(picker.mode(), Mode::Save) {
        ui.horizontal(|ui| {
            ui.label(i18n::t("File name"));
            let mut name = picker.file_name().to_string();
            if ui.add_enabled(!busy, egui::TextEdit::singleline(&mut name)).changed() {
                picker.set_file_name(&name);
            }
        });
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

    #[test]
    fn sizes_read_as_short_units() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(1024 * 1024), "1.0 MB");
        assert_eq!(human_size(3 * 1024 * 1024 * 1024), "3.0 GB");
        // 离谱的大小仍然留在单位表内。
        assert!(human_size(u64::MAX).ends_with(" EB"), "{}", human_size(u64::MAX));
    }
}
