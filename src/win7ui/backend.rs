//! Win32 backend implementation of cross-platform UI traits.

use std::sync::mpsc;

use windows_sys::Win32::Foundation::HWND;

use super::{
    choose_file, code_editor::CodeEditor, controls, event_channel, get_window_text,
    log_view::LogView, set_window_text, BuiltTree, UiEventSender,
};
use crate::ui::{
    app_common, BuiltUi, Button, CheckBox, Editor, EventSender, LogView as LogViewTrait,
    ProgressBar, SearchEdit, StatusBar, TabControl, UiBackend,
};

// ─── Zero-sized backend marker ──────────────────────────────

/// Win32 UI backend (uses windows-sys + DTT/BTT).
pub struct Win32Backend;

// ─── Widget handle wrappers ─────────────────────────────────

#[derive(Clone)]
pub struct HwndButton(HWND);
#[derive(Clone)]
pub struct HwndCheckBox(HWND);
#[derive(Clone)]
pub struct HwndStatusBar(HWND);
#[derive(Clone)]
pub struct HwndProgressBar(HWND);
#[derive(Clone)]
pub struct HwndSearchEdit(HWND);
#[derive(Clone)]
pub struct HwndTabControl(HWND);

// ─── UiBackend implementation ───────────────────────────────

impl UiBackend for Win32Backend {
    type BuiltUi = Win32BuiltUi;
    type Editor = CodeEditor;
    type LogView = LogView;
    type TabControl = HwndTabControl;
    type StatusBar = HwndStatusBar;
    type Button = HwndButton;
    type CheckBox = HwndCheckBox;
    type ProgressBar = HwndProgressBar;
    type SearchEdit = HwndSearchEdit;

    type EventSender<T: Send + 'static> = UiEventSender<T>;

    fn build_from_tree(tree: &crate::ui::dtt::UiTree) -> Result<Self::BuiltUi, String> {
        // Win32 BTT needs an HWND parent — use the desktop window.
        // In practice, the main window is created first via create_main_window(),
        // then BTT builds into it. This entry point exists for future use.
        Err("Win32 requires HWND parent; use BuiltTree::from_tree(tree, hwnd) instead".into())
    }

    fn build_ui(toml: &str) -> Result<Self::BuiltUi, String> {
        // BTT has its own TOML→BuiltTree path with HWND creation.
        // Use build_from_tree after creating the tree manually.
        let tree: crate::ui::dtt::UiTree = toml::from_str(toml).map_err(|e| format!("TOML: {e}"))?;
        Self::build_from_tree(&tree)
    }

    fn run_message_loop() {
        unsafe {
            let mut msg = std::mem::zeroed();
            while windows_sys::Win32::UI::WindowsAndMessaging::GetMessageW(
                &mut msg,
                std::ptr::null_mut(),
                0,
                0,
            ) > 0
            {
                windows_sys::Win32::UI::WindowsAndMessaging::TranslateMessage(&msg);
                windows_sys::Win32::UI::WindowsAndMessaging::DispatchMessageW(&msg);
            }
        }
    }

    fn event_channel<T: Send + 'static>() -> (Self::EventSender<T>, mpsc::Receiver<T>) {
        // Win32 needs HWND + wake_msg; use stub values for now.
        // Real callers should use win7ui::event_channel(hwnd, msg) directly.
        panic!("Win32Backend::event_channel() requires HWND; use win7ui::event_channel() directly")
    }

    fn choose_file(
        save: bool,
        filter: &str,
        title: &str,
        default_ext: &str,
    ) -> Option<std::path::PathBuf> {
        unsafe { choose_file(std::ptr::null_mut(), save, filter, title, default_ext) }
    }
}

// ─── BuiltUi implementation ─────────────────────────────────

/// Win32 built widget tree, wrapping BuiltTree<HWND>.
pub struct Win32BuiltUi {
    inner: BuiltTree,
}

impl Win32BuiltUi {
    pub fn from_built_tree(tree: BuiltTree) -> Self {
        Self { inner: tree }
    }

    /// Access the underlying BuiltTree (for Win32-specific operations).
    pub fn inner(&self) -> &BuiltTree {
        &self.inner
    }

    /// Access the underlying BuiltTree mutably.
    pub fn inner_mut(&mut self) -> &mut BuiltTree {
        &mut self.inner
    }
}

impl BuiltUi for Win32BuiltUi {
    type Backend = Win32Backend;

    fn editor_by_id(&self, id: i32) -> Option<CodeEditor> {
        self.inner.code_editor_by_id(id).cloned()
    }

    fn log_view_by_id(&self, id: i32) -> Option<LogView> {
        self.inner.log_view_by_id(id).cloned()
    }

    fn tab_by_id(&self, id: i32) -> Option<HwndTabControl> {
        self.inner.hwnd_by_id(id).map(HwndTabControl)
    }

    fn status_by_id(&self, id: i32) -> Option<HwndStatusBar> {
        self.inner.hwnd_by_id(id).map(HwndStatusBar)
    }

    fn button_by_id(&self, id: i32) -> Option<HwndButton> {
        self.inner.hwnd_by_id(id).map(HwndButton)
    }

    fn checkbox_by_id(&self, id: i32) -> Option<HwndCheckBox> {
        self.inner.hwnd_by_id(id).map(HwndCheckBox)
    }

    fn progress_by_id(&self, id: i32) -> Option<HwndProgressBar> {
        self.inner.hwnd_by_id(id).map(HwndProgressBar)
    }

    fn search_by_id(&self, id: i32) -> Option<HwndSearchEdit> {
        self.inner.hwnd_by_id(id).map(HwndSearchEdit)
    }

    fn has_widget(&self, id: i32) -> bool {
        self.inner.hwnd_by_id(id).is_some()
            || self.inner.code_editor_by_id(id).is_some()
            || self.inner.log_view_by_id(id).is_some()
    }

    fn on_resize(&mut self, width: i32, height: i32) {
        self.inner.on_resize(width, height);
    }

    fn switch_tab(&mut self, tab_id: i32, page_index: usize) {
        self.inner.switch_tab(tab_id, page_index);
    }
}

// ─── Widget trait implementations ───────────────────────────

impl Button for HwndButton {
    fn set_enabled(&self, enabled: bool) {
        unsafe { controls::enable_window(self.0, enabled) }
    }
}

impl CheckBox for HwndCheckBox {
    fn is_checked(&self) -> bool {
        unsafe { controls::checkbox_is_checked(self.0) }
    }

    fn set_checked(&self, checked: bool) {
        unsafe { controls::checkbox_set_checked(self.0, checked) }
    }
}

impl StatusBar for HwndStatusBar {
    fn set_text(&self, text: &str) {
        unsafe { set_window_text(self.0, text) }
    }
}

impl ProgressBar for HwndProgressBar {
    fn set_value(&self, value: i32) {
        unsafe { controls::progress_set_value(self.0, value) }
    }
}

impl SearchEdit for HwndSearchEdit {
    fn text(&self) -> String {
        unsafe { get_window_text(self.0) }
    }

    fn set_text(&self, text: &str) {
        unsafe { set_window_text(self.0, text) }
    }
}

impl TabControl for HwndTabControl {
    fn insert_item(&self, index: i32, label: &str) {
        unsafe { controls::tab_insert_item(self.0, index, label); }
    }

    fn delete_item(&self, index: i32) {
        unsafe { controls::tab_delete_item(self.0, index) }
    }

    fn selected(&self) -> i32 {
        unsafe { controls::tab_get_selected(self.0) }
    }

    fn set_selected(&self, index: i32) {
        unsafe { controls::tab_set_selected(self.0, index) }
    }

    fn set_item_text(&self, index: i32, label: &str) {
        unsafe { controls::tab_set_item_text(self.0, index, label) }
    }

    fn count(&self) -> i32 {
        unsafe { controls::tab_get_count(self.0) }
    }
}

impl Editor for CodeEditor {
    fn set_text(&self, text: &str) {
        unsafe { (*self).set_text(text) }
    }

    fn text(&self) -> String {
        unsafe { (*self).text() }
    }

    fn is_dirty(&self) -> bool {
        unsafe { self.is_dirty() }
    }

    fn mark_clean(&self) {
        unsafe { self.mark_clean() }
    }

    fn clear_error_line(&self) {
        unsafe { (*self).clear_error_line() }
    }

    fn set_error_line(&self, line: usize) {
        unsafe { self.set_error_line(line) }
    }

    fn refresh_view(&self) {
        unsafe {
            use windows_sys::Win32::Graphics::Gdi::{
                InvalidateRect, UpdateWindow,
            };
            InvalidateRect(self.script_hwnd(), std::ptr::null_mut(), 1);
            UpdateWindow(self.script_hwnd());
        }
    }

    fn refresh_gutter(&self) {
        unsafe {
            use windows_sys::Win32::Graphics::Gdi::{
                RedrawWindow, RDW_ERASE, RDW_INVALIDATE, RDW_NOCHILDREN, RDW_UPDATENOW,
            };
            let gutter = self.gutter_hwnd();
            if !gutter.is_null() {
                RedrawWindow(
                    gutter,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    RDW_INVALIDATE | RDW_ERASE | RDW_UPDATENOW | RDW_NOCHILDREN,
                );
            }
        }
    }

    fn refresh_marks(&self) {
        unsafe { (*self).refresh_marks() }
    }
}

impl LogViewTrait for LogView {
    fn append(&self, text: &str) {
        unsafe { (*self).append_text(text) }
    }

    fn replace(&self, lines: &[String], total_lines: usize) {
        unsafe { (*self).replace_snapshot(lines, total_lines, 500) }
    }

    fn clear(&self) {
        unsafe { (*self).clear() }
    }

    fn set_max_chars(&self, max: i32) {
        let _ = max;
    }
}

// ─── EventSender impl for UiEventSender ─────────────────────

impl<T: Send + 'static> EventSender<T> for UiEventSender<T> {
    fn send(&self, event: T) -> Result<(), mpsc::SendError<T>> {
        UiEventSender::send(self, event)
    }

    fn wake(&self) {
        unsafe { UiEventSender::wake(self) }
    }
}
