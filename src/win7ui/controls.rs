use std::{ffi::c_void, mem::zeroed, ptr::null_mut};

use windows_sys::Win32::{
    Foundation::{HINSTANCE, HWND, LPARAM, WPARAM},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::{InitCommonControlsEx, INITCOMMONCONTROLSEX, ICC_TAB_CLASSES},
        Input::KeyboardAndMouse::EnableWindow,
        WindowsAndMessaging::{
            CreateWindowExW, SendMessageW, BS_GROUPBOX, BS_PUSHBUTTON, ES_AUTOHSCROLL, ES_AUTOVSCROLL,
            ES_MULTILINE, ES_NOHIDESEL, ES_READONLY, ES_WANTRETURN, WS_BORDER, WS_CHILD, WS_HSCROLL,
            WS_VISIBLE, WS_VSCROLL, WS_EX_CLIENTEDGE,
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
    create_button_at(parent, text, id, 0, 0, 90, 28)
}

pub unsafe fn create_button_at(
    parent: HWND,
    text: &str,
    id: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> HWND {
    let class = wide("BUTTON");
    CreateWindowExW(
        0,
        class.as_ptr(),
        wide(text).as_ptr(),
        WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON as u32 | 0x8000, // BS_FLAT = 0x8000
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

pub unsafe fn create_multiline_edit(
    parent: HWND,
    text: &str,
    id: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    readonly: bool,
    hscroll: bool,
) -> HWND {
    let class = wide("EDIT");
    let mut style = WS_CHILD
        | WS_VISIBLE
        | WS_VSCROLL
        | ES_MULTILINE as u32
        | ES_AUTOVSCROLL as u32
        | ES_WANTRETURN as u32
        | ES_NOHIDESEL as u32;
    if readonly {
        style |= ES_READONLY as u32;
    }
    if hscroll {
        style |= WS_HSCROLL | ES_AUTOHSCROLL as u32;
    }

    CreateWindowExW(
        0, // 扁平：去掉 WS_EX_CLIENTEDGE
        class.as_ptr(),
        wide(text).as_ptr(),
        style,
        x,
        y,
        w,
        h,
        parent,
        id as _,
        module_handle(),
        std::ptr::null_mut(),
    )
}

pub unsafe fn create_line_number_gutter(
    parent: HWND,
    text: &str,
    id: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> HWND {
    const SS_RIGHT: u32 = 0x0000_0002;
    const SS_NOPREFIX: u32 = 0x0000_0080;
    let class = wide("STATIC");
    CreateWindowExW(
        0, // 扁平：去掉 WS_EX_CLIENTEDGE
        class.as_ptr(),
        wide(text).as_ptr(),
        WS_CHILD | WS_VISIBLE | SS_RIGHT | SS_NOPREFIX,
        x,
        y,
        w,
        h,
        parent,
        id as _,
        module_handle(),
        std::ptr::null_mut(),
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

pub unsafe fn create_group_box(
    parent: HWND,
    text: &str,
    id: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> HWND {
    let class = wide("BUTTON");
    CreateWindowExW(
        0,
        class.as_ptr(),
        wide(text).as_ptr(),
        (WS_CHILD | WS_VISIBLE) as u32 | BS_GROUPBOX as u32 | 0x8000, // BS_FLAT
        x,
        y,
        w,
        h,
        parent,
        if id > 0 { id as *mut c_void } else { null_mut() },
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
        0, // 扁平：去掉 WS_EX_CLIENTEDGE
        class.as_ptr(),
        wide(text).as_ptr(),
        WS_CHILD | WS_VISIBLE | ES_AUTOHSCROLL as u32,
        x, y, w, h,
        parent, id as _, module_handle(), null_mut(),
    )
}

// ---------------------------------------------------------------------------
// Checkbox
// ---------------------------------------------------------------------------

const BS_AUTOCHECKBOX: u32 = 0x0003;
const BM_GETCHECK: u32 = 0x00F0;
const BM_SETCHECK: u32 = 0x00F1;
const BST_CHECKED: WPARAM = 1;
const BST_UNCHECKED: WPARAM = 0;

pub unsafe fn create_checkbox(
    parent: HWND,
    text: &str,
    id: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> HWND {
    let class = wide("BUTTON");
    CreateWindowExW(
        0,
        class.as_ptr(),
        wide(text).as_ptr(),
        WS_CHILD | WS_VISIBLE | BS_AUTOCHECKBOX,
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

pub unsafe fn checkbox_is_checked(hwnd: HWND) -> bool {
    SendMessageW(hwnd, BM_GETCHECK, 0, 0) == BST_CHECKED as isize
}

pub unsafe fn checkbox_set_checked(hwnd: HWND, checked: bool) {
    SendMessageW(
        hwnd,
        BM_SETCHECK,
        if checked { BST_CHECKED } else { BST_UNCHECKED },
        0,
    );
}

// ---------------------------------------------------------------------------
// ComboBox
// ---------------------------------------------------------------------------

const CBS_DROPDOWNLIST: u32 = 0x0003;
const CB_ADDSTRING: u32 = 0x0143;
const CB_RESETCONTENT: u32 = 0x0149;
const CB_GETCURSEL: u32 = 0x0147;
const CB_SETCURSEL: u32 = 0x014E;
const CB_GETLBTEXT: u32 = 0x0148;
const CB_GETLBTEXTLEN: u32 = 0x0149;
const CB_ERR: isize = -1;

pub unsafe fn create_combo_box(
    parent: HWND,
    id: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> HWND {
    let class = wide("COMBOBOX");
    CreateWindowExW(
        0,
        class.as_ptr(),
        null_mut(),
        WS_CHILD | WS_VISIBLE | CBS_DROPDOWNLIST | WS_VSCROLL,
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

pub unsafe fn combo_add_string(hwnd: HWND, text: &str) {
    let wide_text = wide(text);
    SendMessageW(hwnd, CB_ADDSTRING, 0, wide_text.as_ptr() as LPARAM);
}

pub unsafe fn combo_clear(hwnd: HWND) {
    SendMessageW(hwnd, CB_RESETCONTENT, 0, 0);
}

pub unsafe fn combo_get_selected_index(hwnd: HWND) -> i32 {
    SendMessageW(hwnd, CB_GETCURSEL, 0, 0) as i32
}

pub unsafe fn combo_get_selected_text(hwnd: HWND) -> String {
    let idx = SendMessageW(hwnd, CB_GETCURSEL, 0, 0);
    if idx == CB_ERR {
        return String::new();
    }
    let len = SendMessageW(hwnd, CB_GETLBTEXTLEN, idx as usize, 0);
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; (len as usize) + 1];
    SendMessageW(
        hwnd,
        CB_GETLBTEXT,
        idx as usize,
        buf.as_mut_ptr() as LPARAM,
    );
    String::from_utf16_lossy(&buf[..len as usize])
}

pub unsafe fn combo_set_selected(hwnd: HWND, index: i32) {
    SendMessageW(hwnd, CB_SETCURSEL, index as usize, 0);
}

// ---------------------------------------------------------------------------
// ListBox
// ---------------------------------------------------------------------------

const LBS_NOTIFY: u32 = 0x0001;
const LB_ADDSTRING: u32 = 0x0180;
const LB_RESETCONTENT: u32 = 0x0184;
const LB_GETCURSEL: u32 = 0x0188;
const LB_SETCURSEL: u32 = 0x0186;
const LB_GETTEXT: u32 = 0x0189;
const LB_GETTEXTLEN: u32 = 0x018A;
const LB_ERR: isize = -1;

pub unsafe fn create_list_box(
    parent: HWND,
    id: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> HWND {
    let class = wide("LISTBOX");
    CreateWindowExW(
        0, // 扁平：去掉 WS_EX_CLIENTEDGE
        class.as_ptr(),
        null_mut(),
        WS_CHILD | WS_VISIBLE | LBS_NOTIFY | WS_VSCROLL,
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

pub unsafe fn listbox_add_string(hwnd: HWND, text: &str) {
    let wide_text = wide(text);
    SendMessageW(hwnd, LB_ADDSTRING, 0, wide_text.as_ptr() as LPARAM);
}

pub unsafe fn listbox_clear(hwnd: HWND) {
    SendMessageW(hwnd, LB_RESETCONTENT, 0, 0);
}

pub unsafe fn listbox_get_selected_index(hwnd: HWND) -> i32 {
    SendMessageW(hwnd, LB_GETCURSEL, 0, 0) as i32
}

pub unsafe fn listbox_get_selected_text(hwnd: HWND) -> String {
    let idx = SendMessageW(hwnd, LB_GETCURSEL, 0, 0);
    if idx == LB_ERR {
        return String::new();
    }
    let len = SendMessageW(hwnd, LB_GETTEXTLEN, idx as usize, 0);
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; (len as usize) + 1];
    SendMessageW(
        hwnd,
        LB_GETTEXT,
        idx as usize,
        buf.as_mut_ptr() as LPARAM,
    );
    String::from_utf16_lossy(&buf[..len as usize])
}

pub unsafe fn listbox_set_selected(hwnd: HWND, index: i32) {
    SendMessageW(hwnd, LB_SETCURSEL, index as usize, 0);
}

// ---------------------------------------------------------------------------
// ProgressBar
// ---------------------------------------------------------------------------

const PBM_SETRANGE32: u32 = 0x0430;
const PBM_SETPOS: u32 = 0x0402;
const PBM_GETPOS: u32 = 0x0408;
const PBM_SETBKCOLOR: u32 = 0x2001;
const PBM_SETBARCOLOR: u32 = 0x2009;
const PBS_SMOOTH: u32 = 0x01;

pub unsafe fn create_progress_bar(
    parent: HWND,
    id: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> HWND {
    let class = wide("msctls_progress32");
    CreateWindowExW(
        0,
        class.as_ptr(),
        null_mut(),
        WS_CHILD | WS_VISIBLE | PBS_SMOOTH,
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

/// 设置进度条扁平配色
pub unsafe fn progress_set_flat_colors(hwnd: HWND, bar_color: u32, bg_color: u32) {
    SendMessageW(hwnd, PBM_SETBARCOLOR, 0, bar_color as isize);
    SendMessageW(hwnd, PBM_SETBKCOLOR, 0, bg_color as isize);
}

pub unsafe fn progress_set_range(hwnd: HWND, min: i32, max: i32) {
    SendMessageW(hwnd, PBM_SETRANGE32, min as usize, max as isize);
}

pub unsafe fn progress_set_value(hwnd: HWND, value: i32) {
    SendMessageW(hwnd, PBM_SETPOS, value as usize, 0);
}

pub unsafe fn progress_get_value(hwnd: HWND) -> i32 {
    SendMessageW(hwnd, PBM_GETPOS, 0, 0) as i32
}

// ---------------------------------------------------------------------------
// TabControl
// ---------------------------------------------------------------------------

const TCM_INSERTITEMW: u32 = 0x133E; // TCM_FIRST + 62
const TCM_GETCURSEL: u32 = 0x130B;
const TCM_SETCURSEL: u32 = 0x130C;
const TCM_GETITEMCOUNT: u32 = 0x1304;
const TCIF_TEXT: u32 = 0x0001;

#[repr(C)]
#[allow(non_snake_case)]
struct TCITEMW {
    mask: u32,
    dw_state: u32,
    dw_state_mask: u32,
    psz_text: *mut u16,
    cch_text_max: i32,
    i_image: i32,
    lParam: LPARAM,
}

pub unsafe fn create_tab_control(parent: HWND, id: i32, x: i32, y: i32, w: i32, h: i32) -> HWND {
    // 确保通用控件（含 Tab）已注册
    let icc: INITCOMMONCONTROLSEX = INITCOMMONCONTROLSEX {
        dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
        dwICC: ICC_TAB_CLASSES,
    };
    InitCommonControlsEx(&icc);

    let class = wide("SysTabControl32");
    CreateWindowExW(
        0,
        class.as_ptr(),
        null_mut(),
        WS_CHILD | WS_VISIBLE,
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

pub unsafe fn tab_insert_item(hwnd: HWND, index: i32, text: &str) -> isize {
    let mut wide_text = wide(text);
    let mut item: TCITEMW = zeroed();
    item.mask = TCIF_TEXT;
    item.psz_text = wide_text.as_mut_ptr();
    item.cch_text_max = wide_text.len() as i32;
    SendMessageW(
        hwnd,
        TCM_INSERTITEMW,
        index as usize,
        &item as *const TCITEMW as LPARAM,
    )
}

pub unsafe fn tab_get_selected(hwnd: HWND) -> i32 {
    SendMessageW(hwnd, TCM_GETCURSEL, 0, 0) as i32
}

pub unsafe fn tab_set_selected(hwnd: HWND, index: i32) {
    SendMessageW(hwnd, TCM_SETCURSEL, index as usize, 0);
}

pub unsafe fn tab_get_count(hwnd: HWND) -> i32 {
    SendMessageW(hwnd, TCM_GETITEMCOUNT, 0, 0) as i32
}

const TCM_DELETEITEM: u32 = 0x1308;
const TCM_SETITEMW: u32 = 0x133D;

#[repr(C)]
struct TcItemWForSetText {
    mask: u32,
    _dw_state: u32,
    _dw_state_mask: u32,
    psz_text: *mut u16,
    cch_text_max: i32,
    _i_image: i32,
    _l_param: isize,
}

pub unsafe fn tab_delete_item(hwnd: HWND, index: i32) {
    SendMessageW(hwnd, TCM_DELETEITEM, index as usize, 0);
}

pub unsafe fn tab_set_item_text(hwnd: HWND, index: i32, text: &str) {
    let mut wide_text: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let mut item = TcItemWForSetText {
        mask: TCIF_TEXT,
        _dw_state: 0,
        _dw_state_mask: 0,
        psz_text: wide_text.as_mut_ptr(),
        cch_text_max: (wide_text.len() + 1) as i32,
        _i_image: 0,
        _l_param: 0,
    };
    SendMessageW(hwnd, TCM_SETITEMW, index as usize, &mut item as *mut _ as isize);
}

// ─── 扁平化：禁用控件主题 ──────────────────────────────────

/// 对指定控件调用 SetWindowTheme("", "") 禁用视觉主题，回到经典扁平风格。
/// 需要在 uxtheme.dll 中动态查找，因为 windows-sys 不直接暴露此函数。
pub unsafe fn set_flat_theme(hwnd: HWND) {
    use std::ffi::c_void;
    type FnSetWindowTheme = unsafe extern "system" fn(HWND, *const u16, *const u16) -> i32;

    static mut FN: Option<FnSetWindowTheme> = None;
    static INIT: std::sync::Once = std::sync::Once::new();

    INIT.call_once(|| {
        let lib = wide("uxtheme");
        let mod_: HINSTANCE =
            windows_sys::Win32::System::LibraryLoader::LoadLibraryW(lib.as_ptr());
        if !mod_.is_null() {
            let proc_name = b"SetWindowTheme\0";
            let fn_ = windows_sys::Win32::System::LibraryLoader::GetProcAddress(
                mod_,
                proc_name.as_ptr() as *const _,
            );
            if let Some(raw) = fn_ {
                unsafe {
                    FN = Some(std::mem::transmute::<*const c_void, FnSetWindowTheme>(
                        raw as *const c_void,
                    ))
                };
            }
        }
    });

    if let Some(set_theme) = FN {
        let empty = wide("");
        set_theme(hwnd, empty.as_ptr(), empty.as_ptr());
    }
}
