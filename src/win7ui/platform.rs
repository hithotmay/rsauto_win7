//! Win32 backend implementation of the cross-platform UI traits.
//!
//! Wraps HWND-based controls to implement the `UiWidget` trait defined
//! in `crate::ui`, allowing application logic to be platform-agnostic.

use windows_sys::Win32::{
    Foundation::HWND,
    UI::Input::KeyboardAndMouse::IsWindowEnabled,
};

use crate::ui::UiWidget;
use super::controls::{
    checkbox_is_checked, checkbox_set_checked, enable_window, progress_set_value,
    tab_delete_item, tab_get_count, tab_get_selected, tab_insert_item, tab_set_item_text,
    tab_set_selected,
};
use super::text::{get_window_text, set_window_text};

/// A Win32 widget handle that wraps an HWND.
#[derive(Clone, Copy)]
pub struct WinWidget {
    hwnd: HWND,
}

impl WinWidget {
    pub fn new(hwnd: HWND) -> Self {
        Self { hwnd }
    }

    pub fn raw_hwnd(&self) -> HWND {
        self.hwnd
    }
}

impl UiWidget for WinWidget {
    fn text(&self) -> String {
        unsafe { get_window_text(self.hwnd) }
    }

    fn set_text(&self, text: &str) {
        unsafe { set_window_text(self.hwnd, text) }
    }

    fn set_enabled(&self, enabled: bool) {
        unsafe { enable_window(self.hwnd, enabled) }
    }

    fn is_enabled(&self) -> bool {
        unsafe { IsWindowEnabled(self.hwnd) != 0 }
    }

    fn tab_insert(&self, index: i32, label: &str) {
        unsafe { tab_insert_item(self.hwnd, index, label) };
    }

    fn tab_remove(&self, index: i32) {
        unsafe { tab_delete_item(self.hwnd, index) }
    }

    fn tab_selected(&self) -> i32 {
        unsafe { tab_get_selected(self.hwnd) }
    }

    fn tab_set_selected(&self, index: i32) {
        unsafe { tab_set_selected(self.hwnd, index) }
    }

    fn tab_set_label(&self, index: i32, label: &str) {
        unsafe { tab_set_item_text(self.hwnd, index, label) }
    }

    fn tab_count(&self) -> i32 {
        unsafe { tab_get_count(self.hwnd) }
    }

    fn progress_set(&self, value: i32) {
        unsafe { progress_set_value(self.hwnd, value) }
    }

    fn checkbox_is_checked(&self) -> bool {
        unsafe { checkbox_is_checked(self.hwnd) }
    }

    fn checkbox_set_checked(&self, checked: bool) {
        unsafe { checkbox_set_checked(self.hwnd, checked) }
    }

    // ── CodeEditor / LogView: use directly via AppState, not through trait ──

    fn editor_set_text(&self, _text: &str) {
        unimplemented!("Use CodeEditor::set_text() directly")
    }

    fn editor_get_text(&self) -> String {
        unimplemented!("Use CodeEditor::get_text() directly")
    }

    fn editor_set_error_line(&self, _line: i32) {
        unimplemented!("Use CodeEditor methods directly")
    }

    fn editor_clear_marks(&self) {
        unimplemented!("Use CodeEditor methods directly")
    }

    fn editor_undo(&self) {
        unimplemented!("Use CodeEditor methods directly")
    }

    fn editor_redo(&self) {
        unimplemented!("Use CodeEditor methods directly")
    }

    fn log_append(&self, _text: &str) {
        unimplemented!("Use LogView::append() directly")
    }
}
