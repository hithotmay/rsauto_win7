use std::{path::PathBuf, ptr::null_mut};

use windows_sys::Win32::{
    Foundation::{HINSTANCE, HWND, LPARAM, WPARAM},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::{
            Dialogs::{
                GetOpenFileNameW, GetSaveFileNameW, OPENFILENAMEW, OFN_FILEMUSTEXIST,
                OFN_HIDEREADONLY, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST,
            },
            EM_REPLACESEL, EM_SCROLLCARET, EM_SETSEL,
        },
        Input::KeyboardAndMouse::{EnableWindow, SetFocus},
        WindowsAndMessaging::{
            CreateWindowExW, GetWindowTextLengthW, GetWindowTextW, SendMessageW, SetWindowTextW,
            BS_PUSHBUTTON, ES_AUTOHSCROLL, WS_CHILD, WS_VISIBLE,
        },
    },
};

pub type RawHwnd = isize;

pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn to_hwnd(value: RawHwnd) -> HWND {
    value as HWND
}

pub fn hwnd_value(value: HWND) -> RawHwnd {
    value as RawHwnd
}

pub unsafe fn module_handle() -> HINSTANCE {
    GetModuleHandleW(null_mut())
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

pub unsafe fn enable_window(hwnd: HWND, enabled: bool) {
    EnableWindow(hwnd, i32::from(enabled));
}

pub unsafe fn create_button(parent: HWND, text: &str, id: i32) -> HWND {
    let class = wide("BUTTON");
    CreateWindowExW(
        0,
        class.as_ptr(),
        wide(text).as_ptr(),
        WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON as u32,
        0,
        0,
        90,
        28,
        parent,
        id as _,
        module_handle(),
        null_mut(),
    )
}

pub unsafe fn create_label(
    parent: HWND,
    text: &str,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> HWND {
    let class = wide("STATIC");
    CreateWindowExW(
        0,
        class.as_ptr(),
        wide(text).as_ptr(),
        WS_CHILD | WS_VISIBLE,
        x,
        y,
        w,
        h,
        parent,
        null_mut(),
        module_handle(),
        null_mut(),
    )
}

pub unsafe fn create_single_line_edit(parent: HWND, text: &str, id: i32, x: i32, y: i32, w: i32, h: i32) -> HWND {
    let class = wide("EDIT");
    CreateWindowExW(
        windows_sys::Win32::UI::WindowsAndMessaging::WS_EX_CLIENTEDGE,
        class.as_ptr(),
        wide(text).as_ptr(),
        WS_CHILD | WS_VISIBLE | ES_AUTOHSCROLL as u32,
        x,
        y,
        w,
        h,
        parent,
        id as _,
        module_handle(),
        null_mut(),
    )
}

pub unsafe fn choose_file(
    owner: HWND,
    save: bool,
    filter: &str,
    title: &str,
    def_ext: &str,
) -> Option<PathBuf> {
    let mut file_buf = vec![0u16; 1024];
    let filter = wide(filter);
    let title = wide(title);
    let def_ext = wide(def_ext);
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: filter.as_ptr(),
        lpstrFile: file_buf.as_mut_ptr(),
        nMaxFile: file_buf.len() as u32,
        lpstrTitle: title.as_ptr(),
        lpstrDefExt: def_ext.as_ptr(),
        Flags: OFN_HIDEREADONLY | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    if save {
        ofn.Flags |= OFN_OVERWRITEPROMPT;
    } else {
        ofn.Flags |= OFN_FILEMUSTEXIST;
    }

    let ok = if save {
        GetSaveFileNameW(&mut ofn)
    } else {
        GetOpenFileNameW(&mut ofn)
    };
    if ok == 0 {
        return None;
    }
    let len = file_buf.iter().position(|ch| *ch == 0).unwrap_or(0);
    Some(PathBuf::from(String::from_utf16_lossy(&file_buf[..len])))
}

pub fn script_path_literal(path: &std::path::Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .replace('"', "\\\"")
}
