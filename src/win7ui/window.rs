use std::ptr::null_mut;

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    Graphics::Gdi::HBRUSH,
    UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, LoadCursorW,
        RegisterClassW, TranslateMessage, CW_USEDEFAULT, IDC_ARROW, MSG, WNDCLASSW, WNDPROC,
        WS_OVERLAPPEDWINDOW, WS_VISIBLE,
    },
};

use super::{module_handle, wide};

pub unsafe fn register_class(name: &str, proc: WNDPROC, background: HBRUSH) {
    let class_name = wide(name);
    let wc = WNDCLASSW {
        lpfnWndProc: proc,
        hInstance: module_handle(),
        lpszClassName: class_name.as_ptr(),
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        hbrBackground: background,
        ..std::mem::zeroed()
    };
    RegisterClassW(&wc);
}

pub unsafe fn create_main_window(
    class_name: &str,
    title: &str,
    width: i32,
    height: i32,
) -> HWND {
    CreateWindowExW(
        0,
        wide(class_name).as_ptr(),
        wide(title).as_ptr(),
        WS_OVERLAPPEDWINDOW | WS_VISIBLE,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        width,
        height,
        null_mut(),
        null_mut(),
        module_handle(),
        null_mut(),
    )
}

pub unsafe fn message_loop() {
    let mut msg: MSG = std::mem::zeroed();
    while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}

pub unsafe extern "system" fn default_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    DefWindowProcW(hwnd, msg, wparam, lparam)
}
