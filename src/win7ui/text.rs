use std::path::Path;

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    UI::{
        Controls::{EM_REPLACESEL, EM_SCROLLCARET, EM_SETSEL},
        Input::KeyboardAndMouse::SetFocus,
        WindowsAndMessaging::{GetWindowTextLengthW, GetWindowTextW, SendMessageW, SetWindowTextW},
    },
};

pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub unsafe fn get_window_text(hwnd: HWND) -> String {
    if hwnd.is_null() {
        return String::new();
    }
    let len = GetWindowTextLengthW(hwnd);
    let mut buf = vec![0u16; len as usize + 1];
    GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
    String::from_utf16_lossy(&buf[..len as usize])
}

pub unsafe fn set_window_text(hwnd: HWND, text: &str) {
    if !hwnd.is_null() {
        SetWindowTextW(hwnd, wide(text).as_ptr());
    }
}

pub unsafe fn append_edit_text(hwnd: HWND, text: &str) {
    if hwnd.is_null() {
        return;
    }
    let text = wide(text);
    SendMessageW(hwnd, EM_SETSEL, usize::MAX, isize::MAX);
    SendMessageW(hwnd, EM_REPLACESEL, 0, text.as_ptr() as LPARAM);
}

pub unsafe fn replace_edit_text(hwnd: HWND, text: &str, scroll_to_end: bool) {
    if hwnd.is_null() {
        return;
    }
    SetWindowTextW(hwnd, wide(text).as_ptr());
    if scroll_to_end {
        SendMessageW(hwnd, EM_SETSEL, usize::MAX, isize::MAX);
        SendMessageW(hwnd, EM_SCROLLCARET, 0, 0);
    }
}

pub unsafe fn insert_line_at_end(hwnd: HWND, line: &str) {
    if hwnd.is_null() {
        return;
    }

    let current = get_window_text(hwnd);
    let insert_line = line.replace(['\r', '\n'], " ");
    let insert_line = insert_line.trim();
    if insert_line.is_empty() {
        return;
    }

    let prefix = if current.is_empty() || current.ends_with('\n') {
        String::new()
    } else if current.ends_with('\r') {
        "\n".to_string()
    } else {
        "\r\n".to_string()
    };

    let text_len = GetWindowTextLengthW(hwnd);
    let text = wide(&format!("{prefix}{insert_line}\r\n"));
    SendMessageW(hwnd, EM_SETSEL, text_len as WPARAM, text_len as LPARAM);
    SendMessageW(hwnd, EM_REPLACESEL, 0, text.as_ptr() as LPARAM);
    SendMessageW(hwnd, EM_SCROLLCARET, 0, 0);
    SetFocus(hwnd);
}

pub fn script_path_literal(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .replace('"', "\\\"")
}
