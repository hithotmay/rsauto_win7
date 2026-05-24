use windows_sys::Win32::{Foundation::HWND, UI::WindowsAndMessaging::GetWindowTextLengthW};

use super::rich_edit::RichEdit;
use super::text::{append_edit_text, replace_edit_text};

#[derive(Debug, Clone, Copy)]
pub struct LogView {
    hwnd: HWND,
    max_chars: i32,
    hfont: isize,
}

impl LogView {
    pub fn new(hwnd: HWND, max_chars: i32) -> Self {
        Self { hwnd, max_chars, hfont: 0 }
    }

    pub fn set_font(&mut self, hfont: isize) {
        self.hfont = hfont;
    }

    pub unsafe fn clear(self) {
        replace_edit_text(self.hwnd, "", false);
        if self.hfont != 0 {
            RichEdit::new(self.hwnd).sync_font(self.hfont);
        }
    }

    pub unsafe fn append_line(self, line: &str) {
        self.append_text(&format!("{line}\r\n"));
    }

    pub unsafe fn replace_snapshot(self, lines: &[String], total_lines: usize, max_lines: usize) {
        let mut text = String::new();
        if total_lines > lines.len() {
            text.push_str(&format!(
                "[本次运行日志超过 {max_lines} 行，历史输出已省略]\r\n"
            ));
        }
        for line in lines {
            text.push_str(line);
            text.push_str("\r\n");
        }
        replace_edit_text(self.hwnd, &text, true);
        if self.hfont != 0 {
            RichEdit::new(self.hwnd).sync_font(self.hfont);
        }
    }

    pub unsafe fn append_text(self, text: &str) {
        if self.hwnd.is_null() {
            return;
        }
        let len = GetWindowTextLengthW(self.hwnd);
        if len > self.max_chars {
            // 文本过多时全文替换（带 WM_SETREDRAW 保护，replace_edit_text 已处理）
            let replacement = format!("[日志过多，仅保留最新输出]\r\n{text}");
            replace_edit_text(self.hwnd, &replacement, true);
            return;
        }
        append_edit_text(self.hwnd, text);
    }
}
