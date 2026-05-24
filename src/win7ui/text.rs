use std::path::Path;

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    Graphics::Gdi::RedrawWindow,
    UI::{
        Controls::{EM_REPLACESEL, EM_SCROLLCARET, EM_SETSEL},
        Input::KeyboardAndMouse::SetFocus,
        WindowsAndMessaging::{
            GetWindowTextLengthW, GetWindowTextW, SendMessageW, SetWindowTextW, WM_SETREDRAW,
        },
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
    // 将光标移到文本末尾再追加（不能用 EM_SETSEL(-1,-1) 因为那只取消选择不移动光标）
    let text_len = GetWindowTextLengthW(hwnd).max(0) as usize;
    SendMessageW(hwnd, EM_SETSEL, text_len as WPARAM, text_len as LPARAM);
    SendMessageW(hwnd, EM_REPLACESEL, 0, text.as_ptr() as LPARAM);
}

pub unsafe fn replace_edit_text(hwnd: HWND, text: &str, scroll_to_end: bool) {
    if hwnd.is_null() {
        return;
    }

    // 方案：用 EM_SETSEL 全选 + EM_REPLACESEL 替换
    // 这样 RichEdit 会正确更新内部排版和滚动条范围，不依赖 WM_SIZE hack

    // 1. 禁止重绘
    SendMessageW(hwnd, WM_SETREDRAW, 0, 0);

    // 2. 全选并替换
    SendMessageW(hwnd, EM_SETSEL, 0, 0); // 选区起点设为 0
    let text_len = GetWindowTextLengthW(hwnd);
    SendMessageW(hwnd, EM_SETSEL, 0, text_len as LPARAM); // 全选 [0, text_len)
    SendMessageW(hwnd, EM_REPLACESEL, 1, wide(text).as_ptr() as LPARAM); // 替换选中内容

    // 3. 滚动到底部
    if scroll_to_end {
        let new_len = GetWindowTextLengthW(hwnd);
        SendMessageW(hwnd, EM_SETSEL, new_len as WPARAM, new_len as LPARAM);
        SendMessageW(hwnd, EM_SCROLLCARET, 0, 0);
    }

    // 4. 恢复重绘并完整刷新（RDW_FRAME 刷新非客户区/滚动条）
    SendMessageW(hwnd, WM_SETREDRAW, 1, 0);
    RedrawWindow(hwnd, std::ptr::null(), std::ptr::null_mut(), 0x0485);
    // 0x0485 = RDW_INVALIDATE(0x0001) | RDW_ERASE(0x0004) | RDW_FRAME(0x0400) | RDW_ALLCHILDREN(0x0080)
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
