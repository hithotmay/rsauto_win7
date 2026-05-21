use windows_sys::Win32::{Foundation::HWND, UI::WindowsAndMessaging::MoveWindow};

pub unsafe fn move_window(hwnd: HWND, x: i32, y: i32, w: i32, h: i32) {
    MoveWindow(hwnd, x, y, w, h, 1);
}

pub unsafe fn row_layout(items: &[(HWND, i32)], x: i32, y: i32, h: i32, gap: i32) {
    let mut current_x = x;
    for (hwnd, w) in items {
        move_window(*hwnd, current_x, y, *w, h);
        current_x += *w + gap;
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SplitLayout {
    pub left_x: i32,
    pub right_x: i32,
    pub y: i32,
    pub left_w: i32,
    pub right_w: i32,
    pub h: i32,
}

pub fn split_left_right(
    client_w: i32,
    client_h: i32,
    margin: i32,
    top: i32,
    gap: i32,
    right_w: i32,
    min_left_w: i32,
) -> SplitLayout {
    let left_w = (client_w - right_w - gap * 3).max(min_left_w);
    let h = (client_h - top - margin).max(160);
    SplitLayout {
        left_x: margin,
        right_x: margin * 2 + left_w,
        y: top,
        left_w,
        right_w,
        h,
    }
}
