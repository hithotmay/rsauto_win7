use windows_sys::Win32::{Foundation::HWND, UI::WindowsAndMessaging::GetWindowTextLengthW};

use super::text::{append_edit_text, replace_edit_text};

#[derive(Debug, Clone, Copy)]
pub struct LogView {
    hwnd: HWND,
    max_chars: i32,
}

impl LogView {
    pub fn new(hwnd: HWND, max_chars: i32) -> Self {
        Self { hwnd, max_chars }
    }

    pub unsafe fn clear(self) {
        replace_edit_text(self.hwnd, "", true);
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
    }

    pub unsafe fn append_text(self, text: &str) {
        if self.hwnd.is_null() {
            return;
        }
        if GetWindowTextLengthW(self.hwnd) > self.max_chars {
            replace_edit_text(self.hwnd, "[日志过多，仅保留最新输出]\r\n", true);
        }
        append_edit_text(self.hwnd, text);
        if GetWindowTextLengthW(self.hwnd) > self.max_chars {
            let replacement = format!("[日志过多，仅保留最新输出]\r\n{text}");
            replace_edit_text(self.hwnd, &replacement, true);
        }
    }
}
