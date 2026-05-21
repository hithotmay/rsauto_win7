use std::ptr::null_mut;

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, RECT},
    Graphics::Gdi::{
        BeginPaint, CreatePen, DeleteObject, EndPaint, FillRect, GetStockObject, InvalidateRect,
        Rectangle, SelectObject, BLACK_BRUSH, HOLLOW_BRUSH, PAINTSTRUCT, PS_SOLID,
    },
    UI::WindowsAndMessaging::GetClientRect,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl SelectionRect {
    pub fn width(self) -> i32 {
        self.right - self.left
    }

    pub fn height(self) -> i32 {
        self.bottom - self.top
    }
}

pub fn lparam_pos(lparam: LPARAM) -> (i32, i32) {
    let value = lparam as u32;
    let x = (value & 0xffff) as i16 as i32;
    let y = ((value >> 16) & 0xffff) as i16 as i32;
    (x, y)
}

pub fn client_selection_rect(
    start: Option<(i32, i32)>,
    end: Option<(i32, i32)>,
    width: i32,
    height: i32,
    min_size: i32,
) -> Option<SelectionRect> {
    let (x1, y1) = start?;
    let (x2, y2) = end?;
    let left = x1.min(x2).clamp(0, width);
    let top = y1.min(y2).clamp(0, height);
    let right = x1.max(x2).clamp(0, width);
    let bottom = y1.max(y2).clamp(0, height);
    let rect = SelectionRect {
        left,
        top,
        right,
        bottom,
    };
    if rect.width() >= min_size && rect.height() >= min_size {
        Some(rect)
    } else {
        None
    }
}

pub unsafe fn invalidate(hwnd: HWND) {
    InvalidateRect(hwnd, null_mut(), 1);
}

pub unsafe fn paint_selection_overlay(hwnd: HWND, selection: Option<SelectionRect>) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    GetClientRect(hwnd, &mut rect);
    FillRect(hdc, &rect, GetStockObject(BLACK_BRUSH) as _);

    if let Some(selection) = selection {
        let pen = CreatePen(PS_SOLID, 3, rgb(255, 64, 128));
        let old_pen = SelectObject(hdc, pen as _);
        let old_brush = SelectObject(hdc, GetStockObject(HOLLOW_BRUSH));
        Rectangle(
            hdc,
            selection.left,
            selection.top,
            selection.right,
            selection.bottom,
        );
        SelectObject(hdc, old_brush);
        SelectObject(hdc, old_pen);
        DeleteObject(pen as _);
    }

    EndPaint(hwnd, &ps);
}

pub fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | ((g as u32) << 8) | ((b as u32) << 16)
}
