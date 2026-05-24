//! Cross-platform UI abstraction layer.
//!
//! Defines traits that each platform backend (win7ui, linux-egui, etc.)
//! must implement. The application logic only depends on these traits,
//! never on platform-specific types like HWND.
//!
//! The entry point is `UiBackend::build_ui(&UiTree)` — every backend
//! consumes the same DTT description and produces its own widget tree.

use std::path::PathBuf;

use self::dtt::UiTree;

pub mod app_common;
pub mod app_logic;
pub mod dtt;

// ─── Event sender ──────────────────────────────────────────

/// Abstraction for sending events from background threads to the UI thread.
pub trait EventSender<T>: Clone + Send + Sync {
    fn send(&self, event: T) -> Result<(), std::sync::mpsc::SendError<T>>;
    /// Wake the UI thread (post a message / request redraw).
    fn wake(&self);
}

// ─── UiBackend ─────────────────────────────────────────────

/// A complete UI backend. Each platform implements this.
///
/// Call `build_ui(&UiTree)` to construct the widget tree from a
/// platform-neutral DTT description. The returned `BuiltUi` provides
/// access to individual widgets by their declared `id`.
pub trait UiBackend: Sized + 'static {
    type BuiltUi: BuiltUi;
    type Editor: Editor;
    type LogView: LogView;
    type TabControl: TabControl;
    type StatusBar: StatusBar;
    type Button: Button;
    type CheckBox: CheckBox;
    type ProgressBar: ProgressBar;
    type SearchEdit: SearchEdit;

    /// Event sender type for cross-thread communication.
    type EventSender<T: Send + 'static>: EventSender<T>;

    /// Build the UI from a parsed `UiTree`.
    ///
    /// Each backend interprets the same DTT tree with its own widgets:
    /// - Win32: creates HWND controls via BTT
    /// - egui: creates egui widgets
    /// - iced: creates iced widgets
    fn build_from_tree(tree: &UiTree) -> Result<Self::BuiltUi, String>;

    /// Parse a TOML string and build the UI.
    ///
    /// Convenience wrapper that parses TOML → UiTree → build_from_tree.
    fn build_ui(toml: &str) -> Result<Self::BuiltUi, String> {
        let tree: UiTree = toml::from_str(toml).map_err(|e| format!("TOML parse error: {e}"))?;
        tree.validate().map_err(|e| format!("DTT validation: {e}"))?;
        Self::build_from_tree(&tree)
    }

    /// Run the platform message loop. Blocks until the app exits.
    fn run_message_loop();

    /// Create a cross-thread event channel that posts events to the main window.
    fn event_channel<T: Send + 'static>() -> (Self::EventSender<T>, std::sync::mpsc::Receiver<T>);

    /// Show a file open/save dialog.
    fn choose_file(save: bool, filter: &str, title: &str, default_ext: &str) -> Option<PathBuf>;
}

// ─── Built UI ──────────────────────────────────────────────

/// The tree of built widgets produced by `UiBackend::build_ui`.
///
/// Provides widget lookup by the `id` declared in DTT TOML.
pub trait BuiltUi {
    type Backend: UiBackend;

    fn editor_by_id(&self, id: i32) -> Option<<Self::Backend as UiBackend>::Editor>;
    fn log_view_by_id(&self, id: i32) -> Option<<Self::Backend as UiBackend>::LogView>;
    fn tab_by_id(&self, id: i32) -> Option<<Self::Backend as UiBackend>::TabControl>;
    fn status_by_id(&self, id: i32) -> Option<<Self::Backend as UiBackend>::StatusBar>;
    fn button_by_id(&self, id: i32) -> Option<<Self::Backend as UiBackend>::Button>;
    fn checkbox_by_id(&self, id: i32) -> Option<<Self::Backend as UiBackend>::CheckBox>;
    fn progress_by_id(&self, id: i32) -> Option<<Self::Backend as UiBackend>::ProgressBar>;
    fn search_by_id(&self, id: i32) -> Option<<Self::Backend as UiBackend>::SearchEdit>;

    /// Generic widget lookup — returns true if any widget exists with this id.
    fn has_widget(&self, id: i32) -> bool;

    /// Handle window resize.
    fn on_resize(&mut self, width: i32, height: i32);

    /// Switch the active page of a tab control.
    fn switch_tab(&mut self, tab_id: i32, page_index: usize);
}

// ─── Widget traits ─────────────────────────────────────────

/// Code editor widget (RichEdit on Win32, multi-line text on Linux).
pub trait Editor: Clone {
    fn set_text(&self, text: &str);
    fn text(&self) -> String;
    fn is_dirty(&self) -> bool;
    fn mark_clean(&self);
    fn clear_error_line(&self);
    fn set_error_line(&self, line: usize);
    fn refresh_view(&self);
    fn refresh_gutter(&self);
    fn refresh_marks(&self);
}

/// Log output view.
pub trait LogView: Clone {
    fn append(&self, text: &str);
    fn replace(&self, lines: &[String], total_lines: usize);
    fn clear(&self);
    fn set_max_chars(&self, max: i32);
}

/// Tab control (multiple pages).
pub trait TabControl: Clone {
    fn insert_item(&self, index: i32, label: &str);
    fn delete_item(&self, index: i32);
    fn selected(&self) -> i32;
    fn set_selected(&self, index: i32);
    fn set_item_text(&self, index: i32, label: &str);
    fn count(&self) -> i32;
}

/// Status bar.
pub trait StatusBar: Clone {
    fn set_text(&self, text: &str);
}

/// Push button.
pub trait Button: Clone {
    fn set_enabled(&self, enabled: bool);
}

/// Checkbox.
pub trait CheckBox: Clone {
    fn is_checked(&self) -> bool;
    fn set_checked(&self, checked: bool);
}

/// Progress bar.
pub trait ProgressBar: Clone {
    fn set_value(&self, value: i32);
}

/// Search/edit single-line input.
pub trait SearchEdit: Clone {
    fn text(&self) -> String;
    fn set_text(&self, text: &str);
}
