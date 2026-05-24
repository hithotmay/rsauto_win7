use std::ptr::null_mut;

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::{
        BeginPaint, BitBlt, ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC,
        CreateSolidBrush, DeleteDC, DeleteObject, EndPaint, FillRect, GetSysColorBrush,
        GetTextMetricsW, InvalidateRect, RedrawWindow, SelectClipRgn, SelectObject, SetBkMode,
        SetTextAlign, SetTextColor, TextOutW, UpdateWindow, COLOR_WINDOW, HBRUSH, HDC,
        PAINTSTRUCT, RDW_ERASE, RDW_INVALIDATE, RDW_NOCHILDREN, RDW_UPDATENOW, SRCCOPY,
        TA_RIGHT, TA_TOP, TEXTMETRICW, TRANSPARENT,
    },
    UI::{
        Controls::{EM_REPLACESEL, EM_SETSEL, EM_UNDO},
        Input::KeyboardAndMouse::{
            GetKeyState, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_HOME, VK_LEFT, VK_NEXT,
            VK_PRIOR, VK_RIGHT, VK_SHIFT, VK_TAB, VK_UP,
        },
        WindowsAndMessaging::*,
    },
};

/// EM_GETRECT: 获取 RichEdit 格式化矩形（实际文本绘制区域）
const EM_GETRECT: u32 = 0x00B2;

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
const EM_LINESCROLL: u32 = 0x00B6;
const EM_EXGETSEL: u32 = 0x0400 + 52;
const EM_LINEFROMCHAR: u32 = 0x00C9;
const EM_LINEINDEX: u32 = 0x00BB;
const EM_GETLINECOUNT: u32 = 0x00BA;
const EM_SCROLLCARET: u32 = 0x00B7;
const EM_GETSEL: u32 = 0x00B0;

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
        // RichEdit needs EM_SETCHARFORMAT to sync font for all text + paste/insert
        unsafe { RichEdit::new(script).sync_font(font as isize); }

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
        if gutter_width <= 0 {
            // 隐藏行号栏，脚本区占满全宽
            ShowWindow(self.gutter_hwnd(), SW_HIDE);
            move_window(self.script_hwnd(), x, y, w, h);
        } else {
            ShowWindow(self.gutter_hwnd(), SW_SHOW);
            move_window(self.gutter_hwnd(), x, y, gutter_width, h);
            move_window(
                self.script_hwnd(),
                x + gutter_width + 4,
                y,
                (w - gutter_width - 4).max(1),
                h,
            );
        }
        self.refresh_gutter();
    }

    pub unsafe fn text(self) -> String {
        get_window_text(self.script_hwnd())
    }

    pub unsafe fn set_text(self, text: &str) {
        RichEdit::new(self.script_hwnd()).set_text(text);
        RichEdit::new(self.script_hwnd()).sync_font(self.font);
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
        // 不要对 RichEdit 调用 UpdateWindow — 它会触发 scroll-to-caret 导致视图跳到光标位置
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

    /// Search for query text in the editor, starting from `start_pos` (UTF-16 char index).
    /// Returns (start, end) of the match, or None.
    pub unsafe fn find_text(self, query: &str, start_pos: usize) -> Option<(usize, usize)> {
        RichEdit::new(self.script_hwnd()).find_text(query, start_pos)
    }

    /// Select a range and scroll it into view.
    pub unsafe fn select_and_scroll(self, start: usize, end: usize) {
        RichEdit::new(self.script_hwnd()).select_and_scroll(start, end);
    }

    /// Get current selection (cp_min, cp_max) in UTF-16 char units.
    pub unsafe fn get_selection(self) -> (usize, usize) {
        RichEdit::new(self.script_hwnd()).get_selection()
    }
}

#[derive(Clone, Copy)]
struct ScriptSubclassData {
    previous: WNDPROC,
    editor: CodeEditor,
    error_line: Option<usize>,
    mouse_down: bool,
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
        mouse_down: false,
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

unsafe fn show_edit_context_menu(hwnd: HWND, lparam: LPARAM) {
    let menu = CreatePopupMenu();
    if menu.is_null() {
        return;
    }

    let label_undo = wide("撤销(&U)");
    let label_cut = wide("剪切(&T)");
    let label_copy = wide("复制(&C)");
    let label_paste = wide("粘贴(&P)");
    let label_delete = wide("删除(&D)");
    let label_selall = wide("全选(&A)");

    AppendMenuW(menu, MF_STRING, 1, label_undo.as_ptr());
    AppendMenuW(menu, MF_SEPARATOR, 0, null_mut());
    AppendMenuW(menu, MF_STRING, 2, label_cut.as_ptr());
    AppendMenuW(menu, MF_STRING, 3, label_copy.as_ptr());
    AppendMenuW(menu, MF_STRING, 4, label_paste.as_ptr());
    AppendMenuW(menu, MF_STRING, 5, label_delete.as_ptr());
    AppendMenuW(menu, MF_SEPARATOR, 0, null_mut());
    AppendMenuW(menu, MF_STRING, 6, label_selall.as_ptr());

    let (x, y) = if lparam == -1 {
        // 键盘触发：用光标位置
        let mut pt: POINT = std::mem::zeroed();
        GetCaretPos(&mut pt);
        ClientToScreen(hwnd, &mut pt);
        (pt.x, pt.y)
    } else {
        let lo = (lparam as u32) & 0xFFFF;
        let hi = ((lparam as u32) >> 16) & 0xFFFF;
        (lo as i32, hi as i32)
    };

    let cmd = TrackPopupMenu(
        menu,
        TPM_LEFTALIGN | TPM_TOPALIGN | TPM_RETURNCMD | TPM_NONOTIFY,
        x,
        y,
        0,
        hwnd,
        null_mut(),
    );

    match cmd {
        0 => {} // 取消或无选择
        1 => { SendMessageW(hwnd, EM_UNDO, 0, 0); }
        2 => { SendMessageW(hwnd, WM_CUT, 0, 0); }
        3 => { SendMessageW(hwnd, WM_COPY, 0, 0); }
        4 => { SendMessageW(hwnd, WM_PASTE, 0, 0); }
        5 => { SendMessageW(hwnd, WM_CLEAR, 0, 0); }
        6 => { SendMessageW(hwnd, EM_SETSEL, 0, -1 as LPARAM); }
        _ => {}
    }

    DestroyMenu(menu);
}

/// Get the selected line range (0-based) from a RichEdit control.
/// Returns (first_line, last_line, sel_start_char, sel_end_char).
unsafe fn get_selection_lines(hwnd: HWND) -> (usize, usize, isize, isize) {
    let mut sel_start: isize = 0;
    let mut sel_end: isize = 0;
    SendMessageW(hwnd, EM_GETSEL, &mut sel_start as *mut isize as usize, &mut sel_end as *mut isize as isize);
    let first_line = SendMessageW(hwnd, EM_LINEFROMCHAR, sel_start as usize, 0).max(0) as usize;
    let last_line = if sel_end > 0 {
        // If the selection ends exactly at a line start, the last affected line is the previous one
        let end_line = SendMessageW(hwnd, EM_LINEFROMCHAR, (sel_end - 1) as usize, 0).max(0) as usize;
        end_line
    } else {
        first_line
    };
    (first_line, last_line, sel_start, sel_end)
}

/// Tab: indent selected lines. Shift+Tab: dedent selected lines.
unsafe fn do_indent_dedent(hwnd: HWND, editor: CodeEditor, dedent: bool) {
    let (first_line, last_line, sel_start, sel_end) = get_selection_lines(hwnd);
    let line_count = SendMessageW(hwnd, EM_GETLINECOUNT, 0, 0).max(1) as usize;
    if first_line >= line_count {
        return;
    }

    let has_selection = sel_start != sel_end;
    let effective_last = if has_selection { last_line } else { first_line };

    // Check if multi-line selection: cursor must be at line start after selection end
    // For single line with no selection, just insert spaces
    if !has_selection || first_line == effective_last {
        if dedent {
            // Single-line dedent: remove up to 4 leading spaces from current line
            let line_idx = first_line;
            let line_char_start = SendMessageW(hwnd, EM_LINEINDEX, line_idx, 0).max(0) as isize;
            let next_line_start = SendMessageW(hwnd, EM_LINEINDEX, line_idx + 1, 0);
            let line_end = if next_line_start >= 0 { next_line_start } else { line_char_start + 1000 };
            let line_len = (line_end - line_char_start).max(0) as usize;
            if line_len == 0 { return; }
            let mut buf = vec![0u16; line_len + 1];
            buf[0] = line_len.min(65535) as u16;
            let copied = SendMessageW(hwnd, 0x00C4 /* EM_GETLINE */, line_idx as WPARAM, buf.as_mut_ptr() as LPARAM);
            let line_text = String::from_utf16_lossy(&buf[..copied as usize]);
            let spaces_to_remove = line_text.chars().take_while(|c| *c == ' ').count().min(4);
            if spaces_to_remove > 0 {
                SendMessageW(hwnd, EM_SETSEL, line_char_start as WPARAM, (line_char_start + spaces_to_remove as isize) as LPARAM);
                SendMessageW(hwnd, EM_REPLACESEL, 1, wide("").as_ptr() as LPARAM);
                // Restore cursor
                let new_pos = (sel_start - spaces_to_remove as isize).max(line_char_start);
                SendMessageW(hwnd, EM_SETSEL, new_pos as WPARAM, new_pos as LPARAM);
            }
        } else {
            // Single-line indent: insert 4 spaces at cursor
            let spaces = wide("    ");
            SendMessageW(hwnd, EM_REPLACESEL, 1, spaces.as_ptr() as LPARAM);
        }
        schedule_editor_highlight(editor);
        return;
    }

    // Multi-line: operate on all lines from first_line to effective_last
    // Freeze redraw
    SendMessageW(hwnd, WM_SETREDRAW, 0, 0);

    let mut delta: isize = 0;
    for line_idx in first_line..=effective_last {
        let line_char_start = SendMessageW(hwnd, EM_LINEINDEX, line_idx, 0).max(0) as isize;

        if dedent {
            // Remove up to 4 leading spaces
            let next_line_start = SendMessageW(hwnd, EM_LINEINDEX, line_idx + 1, 0);
            let line_end = if next_line_start >= 0 { next_line_start } else { line_char_start + 1000 };
            let line_len = (line_end - line_char_start).max(0) as usize;
            if line_len == 0 { continue; }
            let mut buf = vec![0u16; line_len + 1];
            buf[0] = line_len.min(65535) as u16;
            let copied = SendMessageW(hwnd, 0x00C4, line_idx as WPARAM, buf.as_mut_ptr() as LPARAM);
            let line_text = String::from_utf16_lossy(&buf[..copied as usize]);
            let spaces = line_text.chars().take_while(|c| *c == ' ').count().min(4);
            if spaces > 0 {
                SendMessageW(hwnd, EM_SETSEL, line_char_start as WPARAM, (line_char_start + spaces as isize) as LPARAM);
                SendMessageW(hwnd, EM_REPLACESEL, 1, wide("").as_ptr() as LPARAM);
                delta -= spaces as isize;
            }
        } else {
            // Insert 4 spaces at line start
            SendMessageW(hwnd, EM_SETSEL, line_char_start as WPARAM, line_char_start as LPARAM);
            SendMessageW(hwnd, EM_REPLACESEL, 1, wide("    ").as_ptr() as LPARAM);
            delta += 4;
        }
    }

    // Thaw redraw
    SendMessageW(hwnd, WM_SETREDRAW, 1, 0);

    // Restore selection adjusted by delta
    let new_sel_start = sel_start + if !dedent { 4 } else { 0 };
    let first_line_new_start = SendMessageW(hwnd, EM_LINEINDEX, first_line, 0).max(0) as isize;
    let last_line_new_end = {
        let next = SendMessageW(hwnd, EM_LINEINDEX, effective_last + 1, 0);
        if next >= 0 { next - 1 } else { sel_end + delta }
    };
    SendMessageW(hwnd, EM_SETSEL, first_line_new_start as WPARAM, last_line_new_end as LPARAM);

    // Refresh gutter + highlight
    let gutter = editor.gutter_hwnd();
    if !gutter.is_null() {
        RedrawWindow(gutter, null_mut(), null_mut(), RDW_INVALIDATE | RDW_ERASE | RDW_UPDATENOW | RDW_NOCHILDREN);
    }
    InvalidateRect(hwnd, null_mut(), 1);
    UpdateWindow(hwnd);
    schedule_editor_highlight(editor);
}

/// Ctrl+/: toggle comment on selected lines.
/// If all lines start with "# ", remove it. Otherwise, add "# ".
unsafe fn do_toggle_comment(hwnd: HWND, editor: CodeEditor) {
    let (first_line, last_line, sel_start, sel_end) = get_selection_lines(hwnd);
    let line_count = SendMessageW(hwnd, EM_GETLINECOUNT, 0, 0).max(1) as usize;
    if first_line >= line_count {
        return;
    }

    let has_selection = sel_start != sel_end;
    let effective_last = if has_selection { last_line } else { first_line };

    // Read all affected lines to determine action
    let mut line_texts: Vec<String> = Vec::new();
    for line_idx in first_line..=effective_last {
        let line_text = get_richedit_line(hwnd, line_idx);
        line_texts.push(line_text);
    }

    // Decide: uncomment if ALL non-empty lines start with "# "
    let all_commented = line_texts.iter().all(|t| {
        let trimmed = t.trim_start();
        trimmed.is_empty() || trimmed.starts_with('#')
    });

    // Freeze redraw
    SendMessageW(hwnd, WM_SETREDRAW, 0, 0);

    let mut delta: isize = 0;
    for (i, line_text) in line_texts.iter().enumerate() {
        let line_idx = first_line + i;
        let line_char_start = SendMessageW(hwnd, EM_LINEINDEX, line_idx, 0).max(0) as isize;
        let trimmed = line_text.trim_start();

        if trimmed.is_empty() {
            continue; // skip blank lines
        }

        if all_commented {
            // Remove "# " or "#" prefix
            let hash_pos = line_text.find('#').unwrap_or(0);
            let prefix_end = if line_text.get(hash_pos..hash_pos+2) == Some("# ") {
                hash_pos + 2
            } else {
                hash_pos + 1
            };
            // Also remove any space right after # if it's just "#"
            let remove_end = prefix_end;
            let char_count = remove_end - 0; // from start of line
            // Find how many chars before hash
            let leading_spaces = line_text.chars().take_while(|c| *c == ' ').count();
            // Remove from hash_pos to remove_end (in chars, not bytes)
            // We need to convert byte offsets to char offsets
            let chars_before_hash = line_text[..hash_pos].chars().count();
            let chars_to_remove = line_text[hash_pos..remove_end].chars().count();
            let start_offset = line_char_start + chars_before_hash as isize;
            SendMessageW(hwnd, EM_SETSEL, start_offset as WPARAM, (start_offset + chars_to_remove as isize) as LPARAM);
            SendMessageW(hwnd, EM_REPLACESEL, 1, wide("").as_ptr() as LPARAM);
            delta -= chars_to_remove as isize;
        } else {
            // Add "# " at the beginning of line content (after leading spaces)
            let leading_spaces = line_text.chars().take_while(|c| *c == ' ').count();
            let insert_pos = line_char_start + leading_spaces as isize;
            SendMessageW(hwnd, EM_SETSEL, insert_pos as WPARAM, insert_pos as LPARAM);
            SendMessageW(hwnd, EM_REPLACESEL, 1, wide("# ").as_ptr() as LPARAM);
            delta += 2;
        }
    }

    // Thaw redraw
    SendMessageW(hwnd, WM_SETREDRAW, 1, 0);

    // Restore selection
    let first_line_new_start = SendMessageW(hwnd, EM_LINEINDEX, first_line, 0).max(0) as isize;
    let last_line_new_end = {
        let next = SendMessageW(hwnd, EM_LINEINDEX, effective_last + 1, 0);
        if next >= 0 { next - 1 } else { sel_end + delta }
    };
    SendMessageW(hwnd, EM_SETSEL, first_line_new_start as WPARAM, last_line_new_end as LPARAM);

    let gutter = editor.gutter_hwnd();
    if !gutter.is_null() {
        RedrawWindow(gutter, null_mut(), null_mut(), RDW_INVALIDATE | RDW_ERASE | RDW_UPDATENOW | RDW_NOCHILDREN);
    }
    InvalidateRect(hwnd, null_mut(), 1);
    UpdateWindow(hwnd);
    schedule_editor_highlight(editor);
}

/// Read a single line (0-based index) from a RichEdit control.
unsafe fn get_richedit_line(hwnd: HWND, line_idx: usize) -> String {
    let line_char_start = SendMessageW(hwnd, EM_LINEINDEX, line_idx, 0);
    if line_char_start < 0 {
        return String::new();
    }
    let next_line_start = SendMessageW(hwnd, EM_LINEINDEX, line_idx + 1, 0);
    let line_len = if next_line_start >= 0 {
        (next_line_start - line_char_start) as usize
    } else {
        256 // fallback
    };
    if line_len == 0 {
        return String::new();
    }
    let buf_size = line_len + 2;
    let mut buf = vec![0u16; buf_size];
    buf[0] = (buf_size.min(65535) - 1) as u16;
    let copied = SendMessageW(hwnd, 0x00C4 /* EM_GETLINE */, line_idx as WPARAM, buf.as_mut_ptr() as LPARAM);
    // Strip trailing \r
    let mut end = copied as usize;
    while end > 0 && (buf[end - 1] == 0x0D || buf[end - 1] == 0x0A) {
        end -= 1;
    }
    String::from_utf16_lossy(&buf[..end])
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

    if msg == WM_CONTEXTMENU {
        show_edit_context_menu(hwnd, lparam);
        return 0;
    }

    if msg == WM_KEYDOWN && wparam as u32 == VK_TAB as u32 {
        let shift = GetKeyState(VK_SHIFT as i32) as u16;
        let shift_held = (shift & 0x8000) != 0;
        do_indent_dedent(hwnd, data.editor, shift_held);
        return 0;
    }
    if msg == WM_CHAR && wparam as u32 == VK_TAB as u32 {
        return 0; // suppress beep
    }

    // Ctrl+/ : toggle comment
    if msg == WM_KEYDOWN && wparam as u32 == 0xBF as u32 {
        // VK_OEM_2 = 0xBF = '/' key
        let ctrl = GetKeyState(VK_CONTROL as i32) as u16;
        if (ctrl & 0x8000) != 0 {
            do_toggle_comment(hwnd, data.editor);
            return 0;
        }
    }

    // 滚轮：手动 EM_LINESCROLL（同步），不走 RichEdit 默认的异步平滑滚动
    // 必须在 CallWindowProcW 之前拦截，否则 RichEdit 会再滚一次
    if msg == WM_MOUSEWHEEL {
        let delta = (wparam >> 16) as i16 as i32;
        // delta > 0 = 向上滚，EM_LINESCROLL lParam 负值 = 向上
        let lines = -delta * 3 / 120;
        if lines != 0 {
            SendMessageW(hwnd, EM_LINESCROLL, 0, lines as LPARAM);
        }
        let gutter = data.editor.gutter_hwnd();
        if !gutter.is_null() {
            RedrawWindow(
                gutter,
                null_mut(),
                null_mut(),
                RDW_INVALIDATE | RDW_ERASE | RDW_UPDATENOW | RDW_NOCHILDREN,
            );
        }
        return 0;
    }

    let result = CallWindowProcW(data.previous, hwnd, msg, wparam, lparam);
    if matches!(
        msg,
        WM_CHAR | WM_IME_CHAR | WM_PASTE | WM_CUT | WM_CLEAR | WM_UNDO
    ) {
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
    } else if msg == WM_LBUTTONDOWN {
        // 鼠标按下：标记状态，不触发 mark refresh
        if let Some(d) = script_data_mut(hwnd) {
            d.mouse_down = true;
        }
    } else if msg == WM_LBUTTONUP {
        // 鼠标释放：清除标记，触发 mark refresh
        if let Some(d) = script_data_mut(hwnd) {
            d.mouse_down = false;
        }
        schedule_editor_mark_refresh(data.editor);
    } else if msg == WM_KEYUP || msg == WM_SETFOCUS {
        schedule_editor_mark_refresh(data.editor);
    } else if msg == WM_VSCROLL {
        // 点击滚动条：同步刷新行号 gutter
        let gutter = data.editor.gutter_hwnd();
        if !gutter.is_null() {
            RedrawWindow(
                gutter,
                null_mut(),
                null_mut(),
                RDW_INVALIDATE | RDW_ERASE | RDW_UPDATENOW | RDW_NOCHILDREN,
            );
        }
        // 拖选过程中的 WM_VSCROLL 不触发 mark refresh，避免回弹
        let is_mouse_down = script_data(hwnd).map_or(false, |d| d.mouse_down);
        if !is_mouse_down {
            schedule_editor_mark_refresh(data.editor);
        }
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
            // 把滚轮消息转发给 RichEdit，让它处理滚动
            let script = data.editor.script_hwnd();
            if !script.is_null() {
                SendMessageW(script, msg, wparam, lparam);
            }
            // RichEdit 处理完后同步刷新 gutter
            let gutter = data.editor.gutter_hwnd();
            if !gutter.is_null() {
                RedrawWindow(
                    gutter,
                    null_mut(),
                    null_mut(),
                    RDW_INVALIDATE | RDW_ERASE | RDW_UPDATENOW | RDW_NOCHILDREN,
                );
            }
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

        // 获取 RichEdit 的格式化矩形，精确匹配文本绘制区域
        // 当 RichEdit 有水平滚动条时，格式化矩形底部比客户区底部小一个滚动条高度
        let mut fmt_rect: RECT = std::mem::zeroed();
        SendMessageW(script, EM_GETRECT, 0, &mut fmt_rect as *mut RECT as LPARAM);
        let clip_bottom = if fmt_rect.bottom > 0 {
            fmt_rect.bottom
        } else {
            rect.bottom
        };

        while line < line_count {
            let Some(y) = rich_edit.line_top(line) else {
                break;
            };
            if y > clip_bottom {
                break;
            }
            if y + line_height >= rect.top {
                let number_line = line + 1;
                let line_rect = RECT {
                    left: rect.left,
                    top: y.max(rect.top),
                    right: rect.right,
                    bottom: (y + line_height).min(clip_bottom),
                };
                if error_line == Some(number_line) {
                    FillRect(hdc, &line_rect, error_brush as HBRUSH);
                    let marker_rect = RECT {
                        left: rect.left + 2,
                        top: (y + 2).max(rect.top),
                        right: rect.left + 5,
                        bottom: (y + line_height - 2).min(clip_bottom),
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
