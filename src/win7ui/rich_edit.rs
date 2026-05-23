use std::ptr::null_mut;

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    Graphics::Gdi::InvalidateRect,
    System::LibraryLoader::LoadLibraryW,
    UI::{
        Controls::{
            EM_GETFIRSTVISIBLELINE, EM_GETLINECOUNT, EM_LINEFROMCHAR, EM_LINEINDEX,
            EM_REPLACESEL, EM_SCROLLCARET, EM_SETSEL,
        },
        WindowsAndMessaging::{
            CreateWindowExW, SendMessageW, ES_AUTOHSCROLL, ES_AUTOVSCROLL, ES_MULTILINE,
            ES_NOHIDESEL, ES_WANTRETURN, GWL_STYLE, SW_HIDE, SW_SHOW, WM_SETREDRAW, WS_BORDER,
            WS_CHILD, WS_EX_CLIENTEDGE, WS_HSCROLL, WS_VISIBLE, WS_VSCROLL, WM_GETTEXTLENGTH,
        },
    },
};

use super::{module_handle, replace_edit_text, set_window_text, wide};

const EM_EXSETSEL: u32 = 0x0400 + 55;
const EM_EXGETSEL: u32 = 0x0400 + 52;
const EM_SETCHARFORMAT: u32 = 0x0400 + 68;
const EM_POSFROMCHAR_RICH: u32 = 0x0400 + 38;
const EM_LINESCROLL: u32 = 0x00B6;
const SCF_SELECTION: WPARAM = 0x0001;
const CFM_COLOR: u32 = 0x40000000;
const CFM_BACKCOLOR: u32 = 0x04000000;
const CFE_AUTOBACKCOLOR: u32 = 0x04000000;

#[repr(C)]
struct CharRange {
    cp_min: i32,
    cp_max: i32,
}

#[repr(C)]
struct PointL {
    x: i32,
    y: i32,
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

#[repr(C)]
struct CharFormat2W {
    cb_size: u32,
    dw_mask: u32,
    dw_effects: u32,
    y_height: i32,
    y_offset: i32,
    cr_text_color: u32,
    b_char_set: u8,
    b_pitch_and_family: u8,
    sz_face_name: [u16; 32],
    w_weight: u16,
    s_spacing: i16,
    cr_back_color: u32,
    lcid: u32,
    dw_reserved: u32,
    s_style: i16,
    w_kerning: u16,
    b_underline_type: u8,
    b_animation: u8,
    b_rev_author: u8,
    b_underline_color: u8,
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
            0, // 扁平：去掉 WS_EX_CLIENTEDGE
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
                0, // 扁平
                fallback.as_ptr(),
                wide(text).as_ptr(),
                WS_CHILD
                    | WS_VISIBLE
                    | WS_VSCROLL
                    | WS_HSCROLL
                    | WS_BORDER
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

    /// 在当前光标行之后插入一行文本
    pub unsafe fn insert_after_current_line(self, line: &str) {
        let mut range = CharRange { cp_min: 0, cp_max: 0 };
        SendMessageW(self.hwnd, EM_EXGETSEL, 0, &mut range as *mut CharRange as LPARAM);
        let cur_line_idx = SendMessageW(self.hwnd, EM_LINEFROMCHAR, range.cp_min as usize, 0);
        // 获取下一行的起始位置
        let next_line_start = SendMessageW(self.hwnd, EM_LINEINDEX, (cur_line_idx as usize).wrapping_add(1), 0);
        let insert_pos = if next_line_start >= 0 {
            next_line_start as usize
        } else {
            // 没有下一行，插入到文本末尾
            let text_len = SendMessageW(self.hwnd, WM_GETTEXTLENGTH, 0, 0);
            text_len.max(0) as usize
        };
        // 将插入点移到目标位置
        SendMessageW(self.hwnd, EM_SETSEL, insert_pos as WPARAM, insert_pos as LPARAM);
        let insert_line = line.replace(['\r', '\n'], " ");
        let insert_line = insert_line.trim();
        if insert_line.is_empty() {
            return;
        }
        // 判断是否需要前缀换行
        let current = super::get_window_text(self.hwnd);
        let prefix = if current.is_empty() { "" } else { "\r\n" };
        let text = wide(&format!("{prefix}{insert_line}"));
        SendMessageW(self.hwnd, EM_REPLACESEL, 1, text.as_ptr() as LPARAM);
        SendMessageW(self.hwnd, EM_SCROLLCARET, 0, 0);
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
        let mut range = CharRange { cp_min: 0, cp_max: 0 };
        SendMessageW(self.hwnd, EM_EXGETSEL, 0, &mut range as *mut CharRange as LPARAM);
        let line = SendMessageW(self.hwnd, EM_LINEFROMCHAR, range.cp_min as usize, 0);
        line.max(0) as usize + 1
    }

    pub unsafe fn line_range(self, line: usize, text_len: usize) -> Option<(usize, usize)> {
        let line = line.checked_sub(1)?;
        let start = SendMessageW(self.hwnd, EM_LINEINDEX, line, 0);
        if start < 0 {
            return None;
        }
        let next = SendMessageW(self.hwnd, EM_LINEINDEX, line + 1, 0);
        let end = if next >= 0 {
            next as usize
        } else {
            text_len
        };
        let start = start as usize;
        (end > start).then_some((start, end.min(text_len)))
    }

    pub unsafe fn first_visible_line(self) -> usize {
        SendMessageW(self.hwnd, EM_GETFIRSTVISIBLELINE, 0, 0).max(0) as usize
    }

    pub unsafe fn line_top(self, line: usize) -> Option<i32> {
        let char_index = SendMessageW(self.hwnd, EM_LINEINDEX, line, 0);
        if char_index < 0 {
            return None;
        }
        let mut point = PointL { x: 0, y: 0 };
        SendMessageW(
            self.hwnd,
            EM_POSFROMCHAR_RICH,
            &mut point as *mut PointL as WPARAM,
            char_index as LPARAM,
        );
        Some(point.y)
    }

    unsafe fn scroll_to_first_visible_line(self, target: usize) {
        let current = self.first_visible_line();
        let delta = target as isize - current as isize;
        if delta != 0 {
            SendMessageW(self.hwnd, EM_LINESCROLL, 0, delta as LPARAM);
        }
    }

    pub unsafe fn apply_highlights(self, text_len: usize, spans: &[HighlightSpan], default_color: u32) {
        if self.hwnd.is_null() {
            return;
        }
        let first_visible = self.first_visible_line();
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
        self.scroll_to_first_visible_line(first_visible);
        SendMessageW(self.hwnd, WM_SETREDRAW, 1, 0);
        InvalidateRect(self.hwnd, null_mut(), 1);
    }

    pub unsafe fn apply_line_markers(
        self,
        text_len: usize,
        current_line: Option<usize>,
        error_line: Option<usize>,
        current_color: u32,
        error_color: u32,
    ) {
        if self.hwnd.is_null() {
            return;
        }
        let first_visible = self.first_visible_line();
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
        self.apply_auto_back_color(0, text_len);
        if let Some(line) = current_line {
            if Some(line) != error_line {
                if let Some((start, end)) = self.line_range(line, text_len) {
                    self.apply_back_color(start, end, current_color);
                }
            }
        }
        if let Some(line) = error_line {
            if let Some((start, end)) = self.line_range(line, text_len) {
                self.apply_back_color(start, end, error_color);
            }
        }
        SendMessageW(
            self.hwnd,
            EM_EXSETSEL,
            0,
            &mut original as *mut CharRange as LPARAM,
        );
        self.scroll_to_first_visible_line(first_visible);
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

    unsafe fn apply_back_color(self, start: usize, end: usize, color: u32) {
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
        let mut format = CharFormat2W {
            cb_size: std::mem::size_of::<CharFormat2W>() as u32,
            dw_mask: CFM_BACKCOLOR,
            dw_effects: 0,
            y_height: 0,
            y_offset: 0,
            cr_text_color: 0,
            b_char_set: 0,
            b_pitch_and_family: 0,
            sz_face_name: [0; 32],
            w_weight: 0,
            s_spacing: 0,
            cr_back_color: color,
            lcid: 0,
            dw_reserved: 0,
            s_style: 0,
            w_kerning: 0,
            b_underline_type: 0,
            b_animation: 0,
            b_rev_author: 0,
            b_underline_color: 0,
        };
        SendMessageW(
            self.hwnd,
            EM_SETCHARFORMAT,
            SCF_SELECTION,
            &mut format as *mut CharFormat2W as LPARAM,
        );
    }

    unsafe fn apply_auto_back_color(self, start: usize, end: usize) {
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
        let mut format = CharFormat2W {
            cb_size: std::mem::size_of::<CharFormat2W>() as u32,
            dw_mask: CFM_BACKCOLOR,
            dw_effects: CFE_AUTOBACKCOLOR,
            y_height: 0,
            y_offset: 0,
            cr_text_color: 0,
            b_char_set: 0,
            b_pitch_and_family: 0,
            sz_face_name: [0; 32],
            w_weight: 0,
            s_spacing: 0,
            cr_back_color: 0,
            lcid: 0,
            dw_reserved: 0,
            s_style: 0,
            w_kerning: 0,
            b_underline_type: 0,
            b_animation: 0,
            b_rev_author: 0,
            b_underline_color: 0,
        };
        SendMessageW(
            self.hwnd,
            EM_SETCHARFORMAT,
            SCF_SELECTION,
            &mut format as *mut CharFormat2W as LPARAM,
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
