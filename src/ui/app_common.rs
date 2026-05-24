//! Cross-platform application types and logic.
//!
//! Pure data types and utility functions shared by all platform backends.
//! No platform-specific imports (no windows-sys, no HWND, etc.).

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
// ─── 常量 ───────────────────────────────────────────────────

pub const MAX_LOG_CHARS: i32 = 80_000;
pub const MAX_RUN_LOG_LINES: usize = 1000;
pub const LOG_SNAPSHOT_INTERVAL_MS: u64 = 160;

// ─── 控件 ID（跨平台统一）──────────────────────────────────

pub const IDC_SCRIPT: i32 = 101;
pub const IDC_LOG: i32 = 102;
pub const IDC_RUN: i32 = 103;
pub const IDC_STOP: i32 = 104;
pub const IDC_OPEN: i32 = 105;
pub const IDC_SAVE: i32 = 106;
pub const IDC_SAVE_AS: i32 = 107;
pub const IDC_CAPTURE: i32 = 108;
pub const IDC_CLICK_IMAGE: i32 = 109;
pub const IDC_CAPTURE_POINT: i32 = 110;
pub const IDC_STATUS: i32 = 120;

pub const IDC_COMBO_LANG: i32 = 130;
pub const IDC_EDIT_SEARCH: i32 = 131;
pub const IDC_PROGRESS: i32 = 132;
pub const IDC_CHECK_WRAP: i32 = 133;
pub const IDC_CHECK_LINENO: i32 = 134;
pub const IDC_EDIT_INSERT: i32 = 135;
pub const IDC_BTN_INSERT: i32 = 136;
pub const IDC_MULTILINE: i32 = 137;
pub const IDC_LIST_SNIPPETS: i32 = 138;
pub const IDC_TAB_CTRL: i32 = 139;
pub const IDC_VAR_VIEW: i32 = 140;
pub const IDC_HELP_VIEW: i32 = 141;

pub const IDC_EDITOR_TABS: i32 = 150;
pub const IDC_CLOSE_TAB: i32 = 151;
pub const IDC_OPEN_WORKDIR: i32 = 152;

// ─── 数据类型 ───────────────────────────────────────────────

/// 编辑器标签页
#[derive(Clone)]
pub struct EditorTab {
    pub path: Option<PathBuf>,
    pub content: String,
    pub display_name: String,
}

impl EditorTab {
    pub fn new(display_name: &str) -> Self {
        Self {
            path: None,
            content: String::new(),
            display_name: display_name.to_string(),
        }
    }

    pub fn with_content(mut self, content: &str) -> Self {
        self.content = content.to_string();
        self
    }

    pub fn with_path(mut self, path: PathBuf) -> Self {
        self.display_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "未命名".to_string());
        self.path = Some(path);
        self
    }
}

/// 截图模式
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CaptureMode {
    SaveRegion,
    ClickImage,
    PointClick,
}

/// 截图选区
#[derive(Clone, Copy)]
pub struct ImageRect {
    pub left: u32,
    pub top: u32,
    pub width: u32,
    pub height: u32,
}

/// 屏幕截图结果
pub struct CapturedScreen {
    pub screen_x: i32,
    pub screen_y: i32,
    pub width: i32,
    pub height: i32,
    pub image: image::RgbaImage,
}

/// 应用事件（跨平台消息传递）
pub enum AppEvent {
    ReplaceLog {
        lines: Vec<String>,
        total_lines: usize,
    },
    Done {
        status: String,
        error_line: Option<usize>,
    },
    CaptureReady {
        mode: CaptureMode,
        result: Result<CapturedScreen, String>,
    },
    VarsUpdate {
        vars: Vec<(String, String)>,
    },
}

// ─── 纯逻辑函数 ─────────────────────────────────────────────

/// 追加日志到环形缓冲
pub fn push_tail_log(tail_logs: &mut VecDeque<String>, total_lines: &mut usize, line: String) {
    *total_lines += 1;
    if tail_logs.len() >= MAX_RUN_LOG_LINES {
        tail_logs.pop_front();
    }
    tail_logs.push_back(line);
}

/// 生成时间戳文件名后缀
pub fn timestamp_for_file() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 裁剪并保存截图
pub fn save_crop(image: &image::RgbaImage, selected: ImageRect, path: &Path) -> Result<(), String> {
    use std::fs;
    use image::imageops;

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
    }
    let cropped = imageops::crop_imm(image, selected.left, selected.top, selected.width, selected.height).to_image();
    cropped.save(path).map_err(|err| err.to_string())
}
/// 默认示例脚本
pub const SAMPLE_SCRIPT: &str = r#"# Win7 原生模式：无 OpenGL，支持中文
x = 1
print(f'你好，x = {x}')

# Python 风格语法示例：
# def hello(name):
#     local_x = int("2") + 3
#     print(f'{name}: {local_x:.2f}, type={type(local_x)}')
# hello("测试")
#
# for i in range(10):
#     if i == 3:
#         continue
#     if i == 8:
#         break
#     print(i)
#
# click(500, 300)
# sleep(500)
# find_click("captures/click_image.png", 0.92, 3000)
"#;
