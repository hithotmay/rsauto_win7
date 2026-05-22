pub mod app;
pub mod code_editor;
pub mod controls;
pub mod dialogs;
pub mod event;
pub mod font;
pub mod hotkey;
pub mod layout;
pub mod log_view;
pub mod overlay;
pub mod rich_edit;
pub mod text;
pub mod window;

pub use app::{AppShell, AppShellStart, AppStore};
pub use code_editor::{
    CodeEditor, CODE_EDITOR_REFRESH_ALL, CODE_EDITOR_REFRESH_GUTTER, CODE_EDITOR_REFRESH_MARKS,
};
pub use controls::{
    create_button, create_button_at, create_label, create_line_number_gutter,
    create_multiline_edit, create_single_line_edit, enable_window, hwnd_value, module_handle,
    to_hwnd, RawHwnd,
};
pub use dialogs::choose_file;
pub use event::{event_channel, wake_window, UiEventSender};
pub use font::{
    apply_font, apply_font_handle, apply_font_handle_to_many, apply_font_to_many,
    create_fixed_font, create_font, create_ui_font, destroy_font, UiFonts,
};
pub use hotkey::HotKey;
pub use layout::{move_window, row_layout, split_left_right};
pub use log_view::LogView;
pub use overlay::{client_selection_rect, lparam_pos, paint_selection_overlay, rgb, SelectionRect};
pub use rich_edit::{show_line_number_gutter, HighlightSpan, RichEdit};
pub use text::{
    append_edit_text, get_window_text, insert_line_at_end, replace_edit_text, script_path_literal,
    set_window_text, wide,
};
pub use window::{create_main_window, message_loop, register_class};
