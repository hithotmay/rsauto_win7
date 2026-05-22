use std::ptr::null_mut;

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    Graphics::Gdi::InvalidateRect,
    System::LibraryLoader::LoadLibraryW,
    UI::{
        Controls::{
            EM_GETLINECOUNT, EM_LINEFROMCHAR, EM_LINEINDEX, EM_REPLACESEL, EM_SCROLLCARET,
            EM_SETSEL,
        },
        WindowsAndMessaging::{
            CreateWindowExW, SendMessageW, ES_AUTOHSCROLL, ES_AUTOVSCROLL, ES_MULTILINE,
            ES_NOHIDESEL, ES_WANTRETURN, GWL_STYLE, SW_HIDE, SW_SHOW, WM_SETREDRAW, WS_CHILD,
            WS_HSCROLL, WS_VISIBLE, WS_VSCROLL, WS_EX_CLIENTEDGE,
        },
    },
};

use super::{module_handle, replace_edit_text, set_window_text, wide};

const EM_EXSETSEL: u32 = 0x0400 + 55;
const EM_EXGETSEL: u32 = 0x0400 + 52;
const EM_SETCHARFORMAT: u32 = 0x0400 + 68;
const SCF_SELECTION: WPARAM = 0x0001;
const CFM_COLOR: u32 = 0x40000000;

#[repr(C)]
struct CharRange {
    cp_min: i32,
    cp_max: i32,
}

#[repr(C)]
struct CharFormatW {
    cb_size: u32,
    dw_mask: u32,
    dw_effects: u32,
    y_height: i32,
    y_offset: i32,
    cr_text_color: u32,
    b_char_set: u8,
    b_pitch_and_family: u8,
    sz_face_name: [u16; 32],
}

#[derive(Debug, Clone, Copy)]
pub struct RichEdit {
    hwnd: HWND,
}

#[derive(Debug, Clone, Copy)]
pub struct HighlightSpan {
    pub start: usize,
    pub end: usize,
    pub color: u32,
}

impl RichEdit {
    pub fn new(hwnd: HWND) -> Self {
        Self { hwnd }
    }

    pub unsafe fn create(
        parent: HWND,
        text: &str,
        id: i32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    ) -> Self {
        load_richedit();
        let class = wide("RichEdit50W");
        let hwnd = CreateWindowExW(
            WS_EX_CLIENTEDGE,
            class.as_ptr(),
            wide(text).as_ptr(),
            WS_CHILD
                | WS_VISIBLE
                | WS_VSCROLL
                | WS_HSCROLL
                | ES_MULTILINE as u32
                | ES_AUTOVSCROLL as u32
                | ES_AUTOHSCROLL as u32
                | ES_WANTRETURN as u32
                | ES_NOHIDESEL as u32,
            x,
            y,
            w,
            h,
            parent,
            id as _,
            module_handle(),
            null_mut(),
        );
        if hwnd.is_null() {
            let fallback = wide("RichEdit20W");
            let hwnd = CreateWindowExW(
                WS_EX_CLIENTEDGE,
                fallback.as_ptr(),
                wide(text).as_ptr(),
                WS_CHILD
                    | WS_VISIBLE
                    | WS_VSCROLL
                    | WS_HSCROLL
                    | ES_MULTILINE as u32
                    | ES_AUTOVSCROLL as u32
                    | ES_AUTOHSCROLL as u32
                    | ES_WANTRETURN as u32
                    | ES_NOHIDESEL as u32,
                x,
                y,
                w,
                h,
                parent,
                id as _,
                module_handle(),
                null_mut(),
            );
            return Self { hwnd };
        }
        Self { hwnd }
    }

    pub fn hwnd(self) -> HWND {
        self.hwnd
    }

    pub unsafe fn set_text(self, text: &str) {
        set_window_text(self.hwnd, text);
    }

    pub unsafe fn replace_text(self, text: &str) {
        replace_edit_text(self.hwnd, text, true);
    }

    pub unsafe fn insert_at_end(self, text: &str) {
        SendMessageW(self.hwnd, EM_SETSEL, usize::MAX, isize::MAX);
        SendMessageW(
            self.hwnd,
            EM_REPLACESEL,
            1,
            wide(text).as_ptr() as LPARAM,
        );
    }

    pub unsafe fn line_count(self) -> usize {
        SendMessageW(self.hwnd, EM_GETLINECOUNT, 0, 0).max(1) as usize
    }

    pub unsafe fn focus_line(self, line: usize) {
        let line = line.saturating_sub(1);
        let start = SendMessageW(self.hwnd, EM_LINEINDEX, line, 0).max(0) as usize;
        let next = SendMessageW(self.hwnd, EM_LINEINDEX, line + 1, 0);
        let end = if next >= 0 { next as usize } else { start };
        SendMessageW(self.hwnd, EM_SETSEL, start, end as isize);
        SendMessageW(self.hwnd, EM_SCROLLCARET, 0, 0);
    }

    pub unsafe fn current_line(self) -> usize {
        let pos = SendMessageW(self.hwnd, 0x00B0, 0, 0);
        let line = SendMessageW(self.hwnd, EM_LINEFROMCHAR, pos as usize, 0);
        line.max(0) as usize + 1
    }

    pub unsafe fn apply_highlights(self, text_len: usize, spans: &[HighlightSpan], default_color: u32) {
        if self.hwnd.is_null() {
            return;
        }
        let mut original = CharRange {
            cp_min: 0,
            cp_max: 0,
        };
        SendMessageW(
            self.hwnd,
            EM_EXGETSEL,
            0,
            &mut original as *mut CharRange as LPARAM,
        );
        SendMessageW(self.hwnd, WM_SETREDRAW, 0, 0);
        self.apply_color(0, text_len, default_color);
        for span in spans {
            self.apply_color(span.start, span.end, span.color);
        }
        SendMessageW(
            self.hwnd,
            EM_EXSETSEL,
            0,
            &mut original as *mut CharRange as LPARAM,
        );
        SendMessageW(self.hwnd, WM_SETREDRAW, 1, 0);
        InvalidateRect(self.hwnd, null_mut(), 1);
    }

    unsafe fn apply_color(self, start: usize, end: usize, color: u32) {
        if end <= start {
            return;
        }
        let mut range = CharRange {
            cp_min: start as i32,
            cp_max: end as i32,
        };
        SendMessageW(
            self.hwnd,
            EM_EXSETSEL,
            0,
            &mut range as *mut CharRange as LPARAM,
        );
        let mut format = CharFormatW {
            cb_size: std::mem::size_of::<CharFormatW>() as u32,
            dw_mask: CFM_COLOR,
            dw_effects: 0,
            y_height: 0,
            y_offset: 0,
            cr_text_color: color,
            b_char_set: 0,
            b_pitch_and_family: 0,
            sz_face_name: [0; 32],
        };
        SendMessageW(
            self.hwnd,
            EM_SETCHARFORMAT,
            SCF_SELECTION,
            &mut format as *mut CharFormatW as LPARAM,
        );
    }
}

pub unsafe fn show_line_number_gutter(hwnd: HWND, show: bool) {
    SendMessageW(hwnd, WM_SETREDRAW, 0, 0);
    let style = windows_sys::Win32::UI::WindowsAndMessaging::GetWindowLongW(hwnd, GWL_STYLE);
    let style = if show {
        style | WS_VISIBLE as i32
    } else {
        style & !(WS_VISIBLE as i32)
    };
    windows_sys::Win32::UI::WindowsAndMessaging::SetWindowLongW(hwnd, GWL_STYLE, style);
    windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(
        hwnd,
        if show { SW_SHOW } else { SW_HIDE },
    );
    SendMessageW(hwnd, WM_SETREDRAW, 1, 0);
}

unsafe fn load_richedit() {
    LoadLibraryW(wide("Msftedit.dll").as_ptr());
    LoadLibraryW(wide("Riched20.dll").as_ptr());
}
