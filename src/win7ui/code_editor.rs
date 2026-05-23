use std::ptr::null_mut;

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::{
        BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateSolidBrush,
        DeleteDC, DeleteObject, EndPaint, FillRect, GetSysColorBrush, GetTextMetricsW,
        RedrawWindow, SelectClipRgn, SelectObject, SetBkMode, SetTextAlign, SetTextColor,
        TextOutW, UpdateWindow, COLOR_WINDOW, HBRUSH, HDC, PAINTSTRUCT, RDW_ERASE,
        RDW_INVALIDATE, RDW_NOCHILDREN, RDW_UPDATENOW, SRCCOPY, TA_RIGHT, TA_TOP, TEXTMETRICW,
        TRANSPARENT,
    },
    UI::{
        Controls::EM_REPLACESEL,
        Input::KeyboardAndMouse::{
            VK_DELETE, VK_DOWN, VK_END, VK_HOME, VK_LEFT, VK_NEXT, VK_PRIOR, VK_RIGHT, VK_TAB,
            VK_UP,
        },
        WindowsAndMessaging::*,
    },
};

use super::{
    apply_font_handle, create_line_number_gutter, get_window_text, insert_line_at_end, move_window,
    rgb, to_hwnd, wide, HighlightSpan, RawHwnd, RichEdit,
};

pub const CODE_EDITOR_REFRESH_GUTTER: u32 = WM_APP + 20;
pub const CODE_EDITOR_REFRESH_ALL: u32 = WM_APP + 21;
pub const CODE_EDITOR_REFRESH_MARKS: u32 = WM_APP + 22;

const GUTTER_TIMER_BASE: usize = 20_000;
const HIGHLIGHT_TIMER_BASE: usize = 30_000;
const MARK_TIMER_BASE: usize = 40_000;
const GUTTER_WHEEL_SYNC_MS: u32 = 45;
const HIGHLIGHT_SYNC_MS: u32 = 90;
const MARK_SYNC_MS: u32 = 35;

#[derive(Debug, Clone, Copy, Default)]
pub struct CodeEditor {
    parent: RawHwnd,
    script: RawHwnd,
    gutter: RawHwnd,
    font: isize,
    gutter_timer: usize,
    highlight_timer: usize,
    mark_timer: usize,
}

impl CodeEditor {
    pub unsafe fn create(
        parent: HWND,
        text: &str,
        id: i32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        gutter_width: i32,
        font: isize,
    ) -> Self {
        let gutter = create_line_number_gutter(parent, "1", 0, x, y, gutter_width, h);
        let script_x = x + gutter_width + 4;
        let script_w = (w - gutter_width - 4).max(1);
        let script = RichEdit::create(parent, text, id, script_x, y, script_w, h).hwnd();

        // 扁平化：禁用视觉主题
        unsafe {
            super::controls::set_flat_theme(gutter);
            super::controls::set_flat_theme(script);
        }

        apply_font_handle(gutter, font);
        apply_font_handle(script, font);

        let editor = Self {
            parent: parent as RawHwnd,
            script: script as RawHwnd,
            gutter: gutter as RawHwnd,
            font,
            gutter_timer: GUTTER_TIMER_BASE + id.max(0) as usize,
            highlight_timer: HIGHLIGHT_TIMER_BASE + id.max(0) as usize,
            mark_timer: MARK_TIMER_BASE + id.max(0) as usize,
        };
        subclass_script_editor(script, editor);
        subclass_line_number_gutter(gutter, editor);
        editor
    }

    pub fn script_hwnd(self) -> HWND {
        to_hwnd(self.script)
    }

    pub fn gutter_hwnd(self) -> HWND {
        to_hwnd(self.gutter)
    }

    pub unsafe fn layout(self, x: i32, y: i32, w: i32, h: i32, gutter_width: i32) {
        move_window(self.gutter_hwnd(), x, y, gutter_width, h);
        move_window(
            self.script_hwnd(),
            x + gutter_width + 4,
            y,
            (w - gutter_width - 4).max(1),
            h,
        );
        self.refresh_gutter();
    }

    pub unsafe fn text(self) -> String {
        get_window_text(self.script_hwnd())
    }

    pub unsafe fn set_text(self, text: &str) {
        RichEdit::new(self.script_hwnd()).set_text(text);
        self.clear_error_line();
    }

    pub unsafe fn insert_line_at_end(self, line: &str) {
        insert_line_at_end(self.script_hwnd(), line);
    }

    pub unsafe fn insert_after_current_line(self, line: &str) {
        RichEdit::new(self.script_hwnd()).insert_after_current_line(line);
    }

    pub unsafe fn focus_line(self, line: usize) {
        RichEdit::new(self.script_hwnd()).focus_line(line);
        self.refresh_marks();
    }

    pub unsafe fn mark_error_line(self, line: usize) {
        if let Some(data) = script_data_mut(self.script_hwnd()) {
            data.error_line = Some(line.max(1));
        }
        self.refresh_marks();
    }

    pub unsafe fn clear_error_line(self) {
        if let Some(data) = script_data_mut(self.script_hwnd()) {
            data.error_line = None;
        }
        self.refresh_marks();
    }

    pub unsafe fn refresh_all(self) {
        let script = self.script_hwnd();
        let text = get_window_text(script);
        let spans = highlight_script_spans(&text);
        RichEdit::new(script).apply_highlights(rich_edit_text_units(&text), &spans, color_default());
        self.refresh_marks();
    }

    pub unsafe fn refresh_marks(self) {
        let script = self.script_hwnd();
        let rich_edit = RichEdit::new(script);
        let text = get_window_text(script);
        let text_len = rich_edit_text_units(&text);
        let current_line = Some(rich_edit.current_line());
        let error_line = self.error_line();
        rich_edit.apply_line_markers(
            text_len,
            current_line,
            error_line,
            color_current_line_background(),
            color_error_line_background(),
        );
        self.refresh_gutter();
    }

    pub unsafe fn refresh_gutter(self) {
        UpdateWindow(self.script_hwnd());
        RedrawWindow(
            self.gutter_hwnd(),
            null_mut(),
            null_mut(),
            RDW_INVALIDATE | RDW_ERASE | RDW_UPDATENOW | RDW_NOCHILDREN,
        );
    }

    pub unsafe fn handle_timer(self, timer_id: usize) -> bool {
        if timer_id == self.gutter_timer {
            KillTimer(to_hwnd(self.parent), self.gutter_timer);
            self.refresh_gutter();
            true
        } else if timer_id == self.highlight_timer {
            KillTimer(to_hwnd(self.parent), self.highlight_timer);
            self.refresh_all();
            true
        } else if timer_id == self.mark_timer {
            KillTimer(to_hwnd(self.parent), self.mark_timer);
            self.refresh_marks();
            true
        } else {
            false
        }
    }

    unsafe fn error_line(self) -> Option<usize> {
        script_data(self.script_hwnd()).and_then(|data| data.error_line)
    }
}

#[derive(Clone, Copy)]
struct ScriptSubclassData {
    previous: WNDPROC,
    editor: CodeEditor,
    error_line: Option<usize>,
}

#[derive(Clone, Copy)]
struct GutterSubclassData {
    previous: WNDPROC,
    editor: CodeEditor,
}

unsafe fn subclass_script_editor(hwnd: HWND, editor: CodeEditor) {
    if hwnd.is_null() {
        return;
    }
    let previous = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, script_edit_proc as *const () as isize);
    let data = Box::new(ScriptSubclassData {
        previous: std::mem::transmute(previous),
        editor,
        error_line: None,
    });
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(data) as isize);
}

unsafe fn subclass_line_number_gutter(hwnd: HWND, editor: CodeEditor) {
    if hwnd.is_null() {
        return;
    }
    let previous = SetWindowLongPtrW(
        hwnd,
        GWLP_WNDPROC,
        line_number_gutter_proc as *const () as isize,
    );
    let data = Box::new(GutterSubclassData {
        previous: std::mem::transmute(previous),
        editor,
    });
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(data) as isize);
}

unsafe extern "system" fn script_edit_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCDESTROY {
        if let Some(data) = take_script_data(hwnd) {
            return CallWindowProcW(data.previous, hwnd, msg, wparam, lparam);
        }
    }
    let Some(data) = script_data(hwnd) else {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    };

    if msg == WM_KEYDOWN && wparam as u32 == VK_TAB as u32 {
        let spaces = wide("    ");
        SendMessageW(hwnd, EM_REPLACESEL, 1, spaces.as_ptr() as LPARAM);
        schedule_editor_highlight(data.editor);
        return 0;
    }
    if msg == WM_CHAR && wparam as u32 == VK_TAB as u32 {
        return 0;
    }

    let result = CallWindowProcW(data.previous, hwnd, msg, wparam, lparam);
    if matches!(msg, WM_CHAR | WM_IME_CHAR | WM_PASTE | WM_CUT | WM_CLEAR | WM_UNDO) {
        schedule_editor_highlight(data.editor);
    } else if msg == WM_KEYDOWN && wparam as u32 == VK_DELETE as u32 {
        schedule_editor_highlight(data.editor);
    } else if msg == WM_KEYDOWN {
        // 方向键 / Home / End / PageUp / PageDown 等导航键移动光标
        let vk = wparam as u32;
        // VK_UP=0x26 VK_DOWN=0x28 VK_LEFT=0x25 VK_RIGHT=0x27
        // VK_HOME=0x24 VK_END=0x23 VK_PRIOR=0x21 VK_NEXT=0x22
        if matches!(vk, 0x26 | 0x28 | 0x25 | 0x27 | 0x24 | 0x23 | 0x21 | 0x22) {
            schedule_editor_mark_refresh(data.editor);
        }
    } else if matches!(msg, WM_KEYUP | WM_LBUTTONUP | WM_LBUTTONDOWN | WM_SETFOCUS) {
        schedule_editor_mark_refresh(data.editor);
    } else if msg == WM_MOUSEWHEEL {
        schedule_line_number_refresh_after_wheel(data.editor);
    } else if msg == WM_VSCROLL {
        PostMessageW(to_hwnd(data.editor.parent), CODE_EDITOR_REFRESH_GUTTER, 0, 0);
        schedule_editor_mark_refresh(data.editor);
    }
    result
}

unsafe extern "system" fn line_number_gutter_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCDESTROY {
        if let Some(data) = take_gutter_data(hwnd) {
            return CallWindowProcW(data.previous, hwnd, msg, wparam, lparam);
        }
    }
    let Some(data) = gutter_data(hwnd) else {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    };

    match msg {
        WM_ERASEBKGND => {
            erase_line_number_gutter(hwnd, wparam as HDC);
            1
        }
        WM_PAINT => {
            paint_line_number_gutter(hwnd, data.editor);
            0
        }
        WM_MOUSEWHEEL => {
            let script = data.editor.script_hwnd();
            if !script.is_null() {
                SendMessageW(script, msg, wparam, lparam);
            }
            schedule_line_number_refresh_after_wheel(data.editor);
            0
        }
        _ => CallWindowProcW(data.previous, hwnd, msg, wparam, lparam),
    }
}

unsafe fn script_data(hwnd: HWND) -> Option<&'static ScriptSubclassData> {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const ScriptSubclassData;
    (!ptr.is_null()).then(|| &*ptr)
}

unsafe fn script_data_mut(hwnd: HWND) -> Option<&'static mut ScriptSubclassData> {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut ScriptSubclassData;
    (!ptr.is_null()).then(|| &mut *ptr)
}

unsafe fn gutter_data(hwnd: HWND) -> Option<&'static GutterSubclassData> {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const GutterSubclassData;
    (!ptr.is_null()).then(|| &*ptr)
}

unsafe fn take_script_data(hwnd: HWND) -> Option<Box<ScriptSubclassData>> {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut ScriptSubclassData;
    if ptr.is_null() {
        None
    } else {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        Some(Box::from_raw(ptr))
    }
}

unsafe fn take_gutter_data(hwnd: HWND) -> Option<Box<GutterSubclassData>> {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut GutterSubclassData;
    if ptr.is_null() {
        None
    } else {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        Some(Box::from_raw(ptr))
    }
}

unsafe fn schedule_line_number_refresh_after_wheel(editor: CodeEditor) {
    let parent = to_hwnd(editor.parent);
    PostMessageW(parent, CODE_EDITOR_REFRESH_GUTTER, 0, 0);
    SetTimer(parent, editor.gutter_timer, GUTTER_WHEEL_SYNC_MS, None);
}

unsafe fn schedule_editor_highlight(editor: CodeEditor) {
    let parent = to_hwnd(editor.parent);
    PostMessageW(parent, CODE_EDITOR_REFRESH_GUTTER, 0, 0);
    SetTimer(parent, editor.highlight_timer, HIGHLIGHT_SYNC_MS, None);
}

unsafe fn schedule_editor_mark_refresh(editor: CodeEditor) {
    let parent = to_hwnd(editor.parent);
    PostMessageW(parent, CODE_EDITOR_REFRESH_MARKS, 0, 0);
    SetTimer(parent, editor.mark_timer, MARK_SYNC_MS, None);
}

unsafe fn paint_line_number_gutter(hwnd: HWND, editor: CodeEditor) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    SelectClipRgn(hdc, null_mut());

    let mut rect = std::mem::zeroed();
    GetClientRect(hwnd, &mut rect);
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;

    let mem_dc = CreateCompatibleDC(hdc);
    let bitmap = if !mem_dc.is_null() && width > 0 && height > 0 {
        CreateCompatibleBitmap(hdc, width, height)
    } else {
        null_mut()
    };

    if !mem_dc.is_null() && !bitmap.is_null() {
        let old_bitmap = SelectObject(mem_dc, bitmap as _);
        draw_line_number_gutter(hwnd, mem_dc, editor);
        BitBlt(hdc, 0, 0, width, height, mem_dc, 0, 0, SRCCOPY);
        if !old_bitmap.is_null() {
            SelectObject(mem_dc, old_bitmap);
        }
        DeleteObject(bitmap as _);
        DeleteDC(mem_dc);
    } else {
        draw_line_number_gutter(hwnd, hdc, editor);
        if !bitmap.is_null() {
            DeleteObject(bitmap as _);
        }
        if !mem_dc.is_null() {
            DeleteDC(mem_dc);
        }
    }

    EndPaint(hwnd, &ps);
}

unsafe fn erase_line_number_gutter(hwnd: HWND, hdc: HDC) {
    if hdc.is_null() {
        return;
    }
    let mut rect = std::mem::zeroed();
    GetClientRect(hwnd, &mut rect);
    FillRect(hdc, &rect, GetSysColorBrush(COLOR_WINDOW));
}

unsafe fn draw_line_number_gutter(hwnd: HWND, hdc: HDC, editor: CodeEditor) {
    if hdc.is_null() {
        return;
    }

    erase_line_number_gutter(hwnd, hdc);
    let mut rect = std::mem::zeroed();
    GetClientRect(hwnd, &mut rect);

    let script = editor.script_hwnd();
    if !script.is_null() {
        let old_font = if editor.font != 0 {
            SelectObject(hdc, editor.font as _)
        } else {
            null_mut()
        };
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextAlign(hdc, TA_RIGHT | TA_TOP);
        SetTextColor(hdc, rgb(86, 96, 112));

        let mut metrics: TEXTMETRICW = std::mem::zeroed();
        GetTextMetricsW(hdc, &mut metrics);
        let line_height = (metrics.tmHeight + metrics.tmExternalLeading).max(16);
        let rich_edit = RichEdit::new(script);
        let line_count = rich_edit.line_count();
        let current_line = rich_edit.current_line();
        let error_line = editor.error_line();
        let mut line = rich_edit.first_visible_line();
        let current_brush = CreateSolidBrush(color_current_line_gutter());
        let error_brush = CreateSolidBrush(color_error_line_gutter());
        let error_marker_brush = CreateSolidBrush(color_error_marker());

        while line < line_count {
            let Some(y) = rich_edit.line_top(line) else {
                break;
            };
            if y > rect.bottom {
                break;
            }
            if y + line_height >= rect.top {
                let number_line = line + 1;
                let line_rect = RECT {
                    left: rect.left,
                    top: y.max(rect.top),
                    right: rect.right,
                    bottom: (y + line_height).min(rect.bottom),
                };
                if error_line == Some(number_line) {
                    FillRect(hdc, &line_rect, error_brush as HBRUSH);
                    let marker_rect = RECT {
                        left: rect.left + 2,
                        top: (y + 2).max(rect.top),
                        right: rect.left + 5,
                        bottom: (y + line_height - 2).min(rect.bottom),
                    };
                    FillRect(hdc, &marker_rect, error_marker_brush as HBRUSH);
                    SetTextColor(hdc, color_error_line_text());
                } else if current_line == number_line {
                    FillRect(hdc, &line_rect, current_brush as HBRUSH);
                    SetTextColor(hdc, color_current_line_text());
                } else {
                    SetTextColor(hdc, rgb(86, 96, 112));
                }

                let number = format!("{number_line}");
                let number = wide(&number);
                TextOutW(
                    hdc,
                    rect.right - 6,
                    y,
                    number.as_ptr(),
                    number.len().saturating_sub(1) as i32,
                );
            }
            line += 1;
        }

        if !current_brush.is_null() {
            DeleteObject(current_brush as _);
        }
        if !error_brush.is_null() {
            DeleteObject(error_brush as _);
        }
        if !error_marker_brush.is_null() {
            DeleteObject(error_marker_brush as _);
        }
        if !old_font.is_null() {
            SelectObject(hdc, old_font);
        }
    }
}

fn highlight_script_spans(text: &str) -> Vec<HighlightSpan> {
    let mut spans = Vec::new();
    let mut pos = 0usize;
    for line in text.split_inclusive('\n') {
        highlight_line_spans(line, pos, &mut spans);
        pos += rich_edit_text_units(line);
    }
    spans
}

fn rich_edit_text_units(text: &str) -> usize {
    let mut units = 0usize;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\r' && chars.peek() == Some(&'\n') {
            chars.next();
            units += 1;
        } else {
            units += ch.len_utf16();
        }
    }
    units
}

fn highlight_line_spans(line: &str, base: usize, spans: &mut Vec<HighlightSpan>) {
    highlight_fragment_tokens(line, 0, 0, line.len(), base, spans, true);
}

fn highlight_fragment_tokens(
    line: &str,
    mut byte: usize,
    mut unit: usize,
    end_byte: usize,
    base: usize,
    spans: &mut Vec<HighlightSpan>,
    allow_comment: bool,
) {
    while byte < end_byte {
        let Some(ch) = next_char(line, byte) else {
            break;
        };
        if ch == '#' && allow_comment {
            let (end_byte, end_unit) =
                token_end(line, byte, unit, end_byte, |c| c != '\r' && c != '\n');
            push_span(spans, base + unit, base + end_unit, color_comment());
            byte = end_byte;
            unit = end_unit;
            continue;
        }
        if let Some(start) = string_start(line, byte, unit, end_byte) {
            let literal =
                string_literal_end(line, start.quote_byte, start.quote_unit, start.quote, end_byte);
            push_span(spans, base + unit, base + literal.end_unit, color_string());
            if start.is_f_string {
                highlight_fstring_expressions(line, &literal, base, spans);
            }
            byte = literal.end_byte;
            unit = literal.end_unit;
            continue;
        }
        if starts_number(line, byte, end_byte) {
            let (end_byte, end_unit) = number_token_end(line, byte, unit, end_byte);
            push_span(spans, base + unit, base + end_unit, color_number());
            byte = end_byte;
            unit = end_unit;
            continue;
        }
        if is_ident_start(ch) {
            let (end_byte, end_unit) = token_end(line, byte, unit, end_byte, is_ident_continue);
            let token = &line[byte..end_byte];
            let color = if is_attribute_token(line, byte) {
                None
            } else if SCRIPT_KEYWORDS.contains(&token) {
                Some(color_keyword())
            } else if SCRIPT_BUILTINS.contains(&token) {
                Some(color_builtin())
            } else if SCRIPT_COMMANDS.contains(&token) {
                Some(color_command())
            } else {
                None
            };
            if let Some(color) = color {
                push_span(spans, base + unit, base + end_unit, color);
            }
            byte = end_byte;
            unit = end_unit;
            continue;
        }
        byte += ch.len_utf8();
        unit += ch.len_utf16();
    }
}

fn highlight_fstring_expressions(
    line: &str,
    literal: &StringLiteral,
    base: usize,
    spans: &mut Vec<HighlightSpan>,
) {
    let mut byte = literal.content_start_byte;
    let mut unit = literal.content_start_unit;
    while byte < literal.content_end_byte {
        let Some(ch) = next_char(line, byte) else {
            break;
        };
        if ch == '{' {
            let (next_byte, next_unit) = advance_char(byte, unit, ch);
            if next_char(line, next_byte) == Some('{') {
                byte = next_byte + 1;
                unit = next_unit + 1;
                continue;
            }
            let expr_start_unit = unit;
            let expr_inner_byte = next_byte;
            let expr_inner_unit = next_unit;
            byte = next_byte;
            unit = next_unit;
            let mut depth = 1usize;
            while byte < literal.content_end_byte {
                let Some(inner_ch) = next_char(line, byte) else {
                    break;
                };
                if let Some(start) = string_start(line, byte, unit, literal.content_end_byte) {
                    let inner_literal = string_literal_end(
                        line,
                        start.quote_byte,
                        start.quote_unit,
                        start.quote,
                        literal.content_end_byte,
                    );
                    byte = inner_literal.end_byte;
                    unit = inner_literal.end_unit;
                    continue;
                }
                if inner_ch == '{' {
                    let (next_byte, next_unit) = advance_char(byte, unit, inner_ch);
                    if next_char(line, next_byte) == Some('{') {
                        byte = next_byte + 1;
                        unit = next_unit + 1;
                    } else {
                        depth += 1;
                        byte = next_byte;
                        unit = next_unit;
                    }
                    continue;
                }
                if inner_ch == '}' {
                    let close_byte = byte;
                    let (next_byte, next_unit) = advance_char(byte, unit, inner_ch);
                    depth = depth.saturating_sub(1);
                    byte = next_byte;
                    unit = next_unit;
                    if depth == 0 {
                        push_span(spans, base + expr_start_unit, base + unit, color_default());
                        highlight_fragment_tokens(
                            line,
                            expr_inner_byte,
                            expr_inner_unit,
                            close_byte,
                            base,
                            spans,
                            false,
                        );
                        break;
                    }
                    continue;
                }
                let (next_byte, next_unit) = advance_char(byte, unit, inner_ch);
                byte = next_byte;
                unit = next_unit;
            }
            continue;
        }
        if ch == '}' {
            let (next_byte, next_unit) = advance_char(byte, unit, ch);
            if next_char(line, next_byte) == Some('}') {
                byte = next_byte + 1;
                unit = next_unit + 1;
                continue;
            }
        }
        let (next_byte, next_unit) = advance_char(byte, unit, ch);
        byte = next_byte;
        unit = next_unit;
    }
}

#[derive(Clone, Copy)]
struct StringStart {
    quote_byte: usize,
    quote_unit: usize,
    quote: char,
    is_f_string: bool,
}

#[derive(Clone, Copy)]
struct StringLiteral {
    end_byte: usize,
    end_unit: usize,
    content_start_byte: usize,
    content_start_unit: usize,
    content_end_byte: usize,
}

fn string_start(line: &str, byte: usize, unit: usize, end_byte: usize) -> Option<StringStart> {
    let ch = next_char(line, byte)?;
    if (ch == '"' || ch == '\'') && byte < end_byte {
        return Some(StringStart {
            quote_byte: byte,
            quote_unit: unit,
            quote: ch,
            is_f_string: false,
        });
    }
    if !ch.is_ascii_alphabetic() {
        return None;
    }
    let (prefix_end_byte, prefix_end_unit) =
        token_end(line, byte, unit, end_byte, |c| c.is_ascii_alphabetic());
    let prefix = &line[byte..prefix_end_byte];
    if !is_string_prefix(prefix) {
        return None;
    }
    let quote = next_char(line, prefix_end_byte)?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    Some(StringStart {
        quote_byte: prefix_end_byte,
        quote_unit: prefix_end_unit,
        quote,
        is_f_string: prefix.chars().any(|c| c == 'f' || c == 'F'),
    })
}

fn string_literal_end(line: &str, quote_byte: usize, quote_unit: usize, quote: char, end_byte: usize) -> StringLiteral {
    let triple = line[quote_byte..].starts_with(&quote.to_string().repeat(3));
    let quote_len = if triple { 3 } else { 1 };
    let mut byte = quote_byte + quote_len;
    let mut unit = quote_unit + quote_len;
    let content_start_byte = byte;
    let content_start_unit = unit;
    let mut content_end_byte = byte;
    let mut content_end_unit = unit;
    let mut escaped = false;
    while byte < end_byte {
        let Some(ch) = next_char(line, byte) else {
            break;
        };
        if ch == '\r' || ch == '\n' {
            break;
        }
        if triple {
            if line[byte..].starts_with(&quote.to_string().repeat(3)) {
                return StringLiteral {
                    end_byte: byte + 3,
                    end_unit: unit + 3,
                    content_start_byte,
                    content_start_unit,
                    content_end_byte: byte,
                };
            }
        } else if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == quote {
            return StringLiteral {
                end_byte: byte + ch.len_utf8(),
                end_unit: unit + ch.len_utf16(),
                content_start_byte,
                content_start_unit,
                content_end_byte,
            };
        }
        let (next_byte, next_unit) = advance_char(byte, unit, ch);
        byte = next_byte;
        unit = next_unit;
        content_end_byte = byte;
        content_end_unit = unit;
    }
    StringLiteral {
        end_byte: content_end_byte,
        end_unit: content_end_unit,
        content_start_byte,
        content_start_unit,
        content_end_byte,
    }
}

fn is_string_prefix(prefix: &str) -> bool {
    if prefix.is_empty() || prefix.len() > 3 {
        return false;
    }
    let mut has_f = false;
    let mut has_r = false;
    let mut has_b = false;
    let mut has_u = false;
    for ch in prefix.chars() {
        match ch.to_ascii_lowercase() {
            'f' if !has_f => has_f = true,
            'r' if !has_r => has_r = true,
            'b' if !has_b => has_b = true,
            'u' if !has_u => has_u = true,
            _ => return false,
        }
    }
    !(has_f && has_b)
}

fn starts_number(line: &str, byte: usize, end_byte: usize) -> bool {
    let Some(ch) = next_char(line, byte) else {
        return false;
    };
    if ch.is_ascii_digit() {
        return true;
    }
    ch == '.'
        && byte + 1 < end_byte
        && line[byte + 1..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit())
}

fn number_token_end(line: &str, mut byte: usize, mut unit: usize, end_byte: usize) -> (usize, usize) {
    let mut prev = '\0';
    while byte < end_byte {
        let Some(ch) = next_char(line, byte) else {
            break;
        };
        let keep = ch.is_ascii_alphanumeric()
            || ch == '_'
            || ch == '.'
            || ((ch == '+' || ch == '-') && (prev == 'e' || prev == 'E'));
        if !keep {
            break;
        }
        let (next_byte, next_unit) = advance_char(byte, unit, ch);
        byte = next_byte;
        unit = next_unit;
        prev = ch;
    }
    (byte, unit)
}

fn push_span(spans: &mut Vec<HighlightSpan>, start: usize, end: usize, color: u32) {
    if end > start {
        spans.push(HighlightSpan { start, end, color });
    }
}

fn token_end(
    line: &str,
    mut byte: usize,
    mut unit: usize,
    end_byte: usize,
    keep: impl Fn(char) -> bool,
) -> (usize, usize) {
    while byte < end_byte {
        let Some(ch) = next_char(line, byte) else {
            break;
        };
        if !keep(ch) {
            break;
        }
        let (next_byte, next_unit) = advance_char(byte, unit, ch);
        byte = next_byte;
        unit = next_unit;
    }
    (byte, unit)
}

fn next_char(text: &str, byte: usize) -> Option<char> {
    text.get(byte..)?.chars().next()
}

fn advance_char(byte: usize, unit: usize, ch: char) -> (usize, usize) {
    (byte + ch.len_utf8(), unit + ch.len_utf16())
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_alphanumeric()
}

fn is_attribute_token(line: &str, byte: usize) -> bool {
    line[..byte]
        .chars()
        .rev()
        .find(|ch| !ch.is_whitespace())
        .is_some_and(|ch| ch == '.')
}

fn color_default() -> u32 {
    rgb(32, 32, 32)
}

fn color_comment() -> u32 {
    rgb(105, 120, 105)
}

fn color_string() -> u32 {
    rgb(145, 92, 25)
}

fn color_number() -> u32 {
    rgb(110, 70, 175)
}

fn color_keyword() -> u32 {
    rgb(175, 45, 60)
}

fn color_builtin() -> u32 {
    rgb(25, 90, 185)
}

fn color_command() -> u32 {
    rgb(20, 135, 95)
}

fn color_current_line_background() -> u32 {
    rgb(238, 244, 255)
}

fn color_error_line_background() -> u32 {
    rgb(255, 232, 232)
}

fn color_current_line_gutter() -> u32 {
    rgb(225, 235, 250)
}

fn color_current_line_text() -> u32 {
    rgb(45, 75, 120)
}

fn color_error_line_gutter() -> u32 {
    rgb(255, 218, 218)
}

fn color_error_marker() -> u32 {
    rgb(200, 45, 45)
}

fn color_error_line_text() -> u32 {
    rgb(165, 30, 30)
}

const SCRIPT_KEYWORDS: &[&str] = &[
    "and", "as", "assert", "break", "class", "continue", "def", "del", "elif", "else", "except",
    "finally", "for", "from", "global", "goto", "if", "import", "in", "is", "label", "lambda",
    "nonlocal", "not", "or", "pass", "raise", "return", "try", "while", "with", "yield", "None",
    "True", "False", "true", "false",
];
const SCRIPT_BUILTINS: &[&str] = &[
    "abs", "all", "any", "bool", "dict", "enumerate", "float", "int", "len", "list", "max", "min",
    "print", "range", "set", "str", "sum", "tuple", "type",
];
const SCRIPT_COMMANDS: &[&str] = &[
    "click",
    "move",
    "sleep",
    "screenshot",
    "find",
    "find_click",
    "点击坐标",
    "移动鼠标",
    "输入文本",
    "等待",
    "截图",
    "查找图片",
    "查找图片并点击",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_offsets_treat_crlf_as_one_rich_edit_unit() {
        let spans = highlight_script_spans("print(1)\r\nprint(2)");
        assert!(spans.iter().any(|span| span.start == 0 && span.end == 5));
        assert!(spans.iter().any(|span| span.start == 9 && span.end == 14));
    }
}
