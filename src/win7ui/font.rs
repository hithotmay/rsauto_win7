use windows_sys::Win32::{
    Foundation::HWND,
    Graphics::Gdi::{
        CreateFontW, DeleteObject, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_QUALITY,
        FF_MODERN, FIXED_PITCH, FW_NORMAL, OUT_DEFAULT_PRECIS,
    },
    UI::WindowsAndMessaging::{SendMessageW, WM_SETFONT},
};

use super::wide;

#[derive(Debug, Clone, Copy, Default)]
pub struct UiFonts {
    pub ui: isize,
    pub editor: isize,
    pub log: isize,
}

impl UiFonts {
    pub unsafe fn win7_defaults() -> Self {
        Self {
            ui: create_ui_font(16) as isize,
            editor: create_fixed_font(16) as isize,
            log: create_fixed_font(15) as isize,
        }
    }

    pub unsafe fn destroy(&mut self) {
        for font in [self.ui, self.editor, self.log] {
            destroy_font(font as HWND);
        }
        *self = Self::default();
    }
}

pub unsafe fn create_ui_font(height: i32) -> HWND {
    create_font("Microsoft YaHei", height, false)
}

pub unsafe fn create_fixed_font(height: i32) -> HWND {
    create_font("NSimSun", height, true)
}

pub unsafe fn create_font(face: &str, height: i32, fixed: bool) -> HWND {
    CreateFontW(
        -height,
        0,
        0,
        0,
        FW_NORMAL as i32,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        OUT_DEFAULT_PRECIS as u32,
        CLIP_DEFAULT_PRECIS as u32,
        DEFAULT_QUALITY as u32,
        if fixed {
            (FIXED_PITCH | FF_MODERN) as u32
        } else {
            0
        },
        wide(face).as_ptr(),
    )
}

pub unsafe fn apply_font(hwnd: HWND, font: HWND) {
    if !hwnd.is_null() && !font.is_null() {
        SendMessageW(hwnd, WM_SETFONT, font as usize, 1);
    }
}

pub unsafe fn apply_font_handle(hwnd: HWND, font: isize) {
    apply_font(hwnd, font as HWND);
}

pub unsafe fn apply_font_to_many(controls: &[HWND], font: HWND) {
    for control in controls {
        apply_font(*control, font);
    }
}

pub unsafe fn apply_font_handle_to_many(controls: &[HWND], font: isize) {
    apply_font_to_many(controls, font as HWND);
}

pub unsafe fn destroy_font(font: HWND) {
    if !font.is_null() {
        DeleteObject(font as _);
    }
}
