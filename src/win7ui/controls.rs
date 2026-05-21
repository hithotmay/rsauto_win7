use std::{ffi::c_void, ptr::null_mut};

use windows_sys::Win32::{
    Foundation::{HINSTANCE, HWND},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Input::KeyboardAndMouse::EnableWindow,
        WindowsAndMessaging::{
            CreateWindowExW, BS_PUSHBUTTON, ES_AUTOHSCROLL, WS_CHILD, WS_VISIBLE,
            WS_EX_CLIENTEDGE,
        },
    },
};

use super::text::wide;

pub type RawHwnd = isize;

pub fn to_hwnd(value: RawHwnd) -> HWND {
    value as *mut c_void
}

pub fn hwnd_value(value: HWND) -> RawHwnd {
    value as RawHwnd
}

pub unsafe fn module_handle() -> HINSTANCE {
    GetModuleHandleW(null_mut())
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

pub unsafe fn create_single_line_edit(
    parent: HWND,
    text: &str,
    id: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> HWND {
    let class = wide("EDIT");
    CreateWindowExW(
        WS_EX_CLIENTEDGE,
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
