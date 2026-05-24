//! Cross-platform UI abstraction layer.
//!
//! Defines traits that each platform backend (win7ui, linux-egui, etc.)
//! must implement. The application logic only depends on these traits,
//! never on platform-specific types like HWND.

pub mod dtt;

// ─── Platform-agnostic handle types ────────────────────────

/// Opaque handle to any widget. Each backend provides its own concrete type.
pub trait WidgetHandle: Clone {}

/// Opaque handle to a window. Each backend provides its own concrete type.
pub trait WindowHandle: Clone {}

// ─── Application lifecycle ─────────────────────────────────

/// Initialize the UI framework. Call once at startup.
pub trait UiApp {
    type Window: UiWindow;

    fn run<F>(self, main_proc: F)
    where
        F: FnOnce(Self::Window) + Send + 'static;
}

// ─── Main window ───────────────────────────────────────────

/// The application's main window, built from a DTT UiTree.
pub trait UiWindow {
    type Widget: UiWidget;

    /// Get a widget by its DTT node id.
    fn widget(&self, id: i32) -> Option<Self::Widget>;

    /// Get the primary code editor widget (if any).
    fn code_editor(&self) -> Option<Self::Widget>;

    /// Get the primary log view widget (if any).
    fn log_view(&self) -> Option<Self::Widget>;

    /// Show a file-open dialog and return the selected path.
    fn choose_file(&self, title: &str, filter: &str) -> Option<std::path::PathBuf>;

    /// Set the window title.
    fn set_title(&self, title: &str);
}

// ─── Generic widget operations ─────────────────────────────

/// Operations available on any widget.
pub trait UiWidget: Clone {
    /// Get/set text content (for labels, edits, buttons).
    fn text(&self) -> String;
    fn set_text(&self, text: &str);

    /// Enable/disable the widget.
    fn set_enabled(&self, enabled: bool);

    /// Is this widget currently enabled?
    fn is_enabled(&self) -> bool;

    // ── Tab control operations ──

    /// Insert a tab at the given index with the given label.
    fn tab_insert(&self, index: i32, label: &str);

    /// Remove a tab at the given index.
    fn tab_remove(&self, index: i32);

    /// Get the index of the currently selected tab.
    fn tab_selected(&self) -> i32;

    /// Select a tab by index.
    fn tab_set_selected(&self, index: i32);

    /// Change a tab's label text.
    fn tab_set_label(&self, index: i32, label: &str);

    /// Get the number of tabs.
    fn tab_count(&self) -> i32;

    // ── Progress bar ──

    /// Set progress bar value (0..=100).
    fn progress_set(&self, value: i32);

    // ── Checkbox ──

    fn checkbox_is_checked(&self) -> bool;
    fn checkbox_set_checked(&self, checked: bool);

    // ── CodeEditor-specific ──

    /// Set the entire text content of the code editor.
    fn editor_set_text(&self, text: &str);

    /// Get the entire text content.
    fn editor_get_text(&self) -> String;

    /// Set error mark on a line (0-based).
    fn editor_set_error_line(&self, line: i32);

    /// Clear all error marks.
    fn editor_clear_marks(&self);

    /// Undo / Redo.
    fn editor_undo(&self);
    fn editor_redo(&self);

    // ── LogView-specific ──

    /// Append a line to the log view.
    fn log_append(&self, text: &str);
}
