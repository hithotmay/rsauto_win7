#![windows_subsystem = "windows"]

#[path = "../core.rs"]
mod core;

use std::{
    collections::VecDeque,
    ffi::c_void,
    fs,
    path::{Path, PathBuf},
    ptr::null_mut,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::Receiver,
        Arc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use core::{RunError, Runner};
use image::{imageops, RgbaImage};
use pyauto_rs::win7ui;
use screenshots::Screen;
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    Graphics::Gdi::{
        BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
        EndPaint, FillRect, GetSysColorBrush, GetTextMetricsW, RedrawWindow, SelectClipRgn,
        SelectObject, SetBkMode, SetTextAlign, SetTextColor, TextOutW, UpdateWindow, COLOR_WINDOW,
        HDC, PAINTSTRUCT, RDW_ERASE, RDW_INVALIDATE, RDW_NOCHILDREN, RDW_UPDATENOW, SRCCOPY,
        TA_RIGHT, TA_TOP, TEXTMETRICW, TRANSPARENT,
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::EM_REPLACESEL,
        Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, VK_DELETE, VK_ESCAPE, VK_TAB},
        WindowsAndMessaging::*,
    },
};

const IDC_SCRIPT: i32 = 101;
const IDC_LOG: i32 = 102;
const IDC_RUN: i32 = 103;
const IDC_STOP: i32 = 104;
const IDC_OPEN: i32 = 105;
const IDC_SAVE: i32 = 106;
const IDC_SAVE_AS: i32 = 107;
const IDC_CAPTURE: i32 = 108;
const IDC_CLICK_IMAGE: i32 = 109;
const IDC_CAPTURE_POINT: i32 = 110;

const IDC_CONFIRM_DIR: i32 = 301;
const IDC_CONFIRM_FILE: i32 = 302;
const IDC_CONFIRM_THRESHOLD: i32 = 303;
const IDC_CONFIRM_TIMEOUT: i32 = 304;
const IDC_CONFIRM_OK: i32 = 305;
const IDC_CONFIRM_RESELECT: i32 = 306;
const IDC_CONFIRM_CANCEL: i32 = 307;

const HOTKEY_RUN: i32 = 201;
const HOTKEY_STOP: i32 = 202;
const TIMER_LINE_GUTTER_SYNC: usize = 301;
const TIMER_SCRIPT_HIGHLIGHT: usize = 302;
const VK_F5: u32 = 0x74;
const VK_F11: u32 = 0x7A;
const MAX_LOG_CHARS: i32 = 80_000;
const MAX_RUN_LOG_LINES: usize = 1000;
const LOG_SNAPSHOT_INTERVAL_MS: u64 = 160;
const RUN_HOTKEY: win7ui::HotKey = win7ui::HotKey::new(HOTKEY_RUN, VK_F5);
const STOP_HOTKEY: win7ui::HotKey = win7ui::HotKey::new(HOTKEY_STOP, VK_F11);
static mut SCRIPT_EDIT_PROC: WNDPROC = None;
static mut LINE_NUMBER_GUTTER_PROC: WNDPROC = None;

const SAMPLE_SCRIPT: &str = r#"# Win7 原生模式：无 OpenGL，支持中文
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

#[derive(Default)]
struct AppState {
    hwnd: isize,
    script: isize,
    line_numbers: isize,
    log: isize,
    status: isize,
    run_button: isize,
    stop_button: isize,
    open_button: isize,
    save_button: isize,
    save_as_button: isize,
    capture_button: isize,
    click_image_button: isize,
    capture_point_button: isize,
    fonts: win7ui::UiFonts,
    running: bool,
    stop_requested: Option<Arc<AtomicBool>>,
    rx: Option<Receiver<AppEvent>>,
    current_file: Option<PathBuf>,
    capture: Option<CaptureState>,
    confirm: Option<ConfirmState>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CaptureMode {
    SaveRegion,
    ClickImage,
    PointClick,
}

struct CaptureState {
    mode: CaptureMode,
    screen_x: i32,
    screen_y: i32,
    width: i32,
    height: i32,
    image: RgbaImage,
    overlay_hwnd: isize,
    dragging: bool,
    start: Option<(i32, i32)>,
    end: Option<(i32, i32)>,
    selection: Option<ImageRect>,
}

struct ConfirmState {
    hwnd: isize,
    dir_edit: isize,
    file_edit: isize,
    threshold_edit: isize,
    timeout_edit: isize,
}

#[derive(Clone, Copy)]
struct ImageRect {
    left: u32,
    top: u32,
    width: u32,
    height: u32,
}

struct CapturedScreen {
    screen_x: i32,
    screen_y: i32,
    width: i32,
    height: i32,
    image: RgbaImage,
}

enum AppEvent {
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
}

static APP: win7ui::AppStore<AppState> = win7ui::AppStore::new();

fn to_hwnd(value: isize) -> HWND {
    value as *mut c_void
}

fn hwnd_value(value: HWND) -> isize {
    value as isize
}

fn main() {
    unsafe {
        let Some(start) = win7ui::AppShell::new()
            .class("PyAutoRsWin7Native", Some(wnd_proc), (COLOR_WINDOW + 1) as _)
            .class("PyAutoRsCaptureOverlay", Some(overlay_proc), null_mut())
            .class("PyAutoRsCaptureConfirm", Some(confirm_proc), (COLOR_WINDOW + 1) as _)
            .main_window("PyAutoRsWin7Native", "PyAuto Rust Win7 Native", 1120, 780)
            .hotkey(RUN_HOTKEY)
            .hotkey(STOP_HOTKEY)
            .start_with_store(&APP)
        else {
            return;
        };

        for hotkey in start.failed_hotkeys {
            match hotkey.id {
                HOTKEY_RUN => append_log("F5 全局运行热键注册失败，可能被其他程序占用。"),
                HOTKEY_STOP => append_log("F11 全局停止热键注册失败，可能被其他程序占用。"),
                _ => {}
            }
        }

        win7ui::message_loop();
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            create_controls(hwnd);
            append_log("Win7 原生模式已启动。F5 运行，F11 停止。");
            0
        }
        WM_SIZE => {
            layout_controls(hwnd);
            0
        }
        WM_COMMAND => {
            match (wparam & 0xffff) as i32 {
                IDC_RUN => start_script(),
                IDC_STOP => stop_script(),
                IDC_OPEN => open_script(),
                IDC_SAVE => save_script(false),
                IDC_SAVE_AS => save_script(true),
                IDC_CAPTURE => begin_capture(CaptureMode::SaveRegion),
                IDC_CLICK_IMAGE => begin_capture(CaptureMode::ClickImage),
                IDC_CAPTURE_POINT => begin_capture(CaptureMode::PointClick),
                _ => {}
            }
            0
        }
        WM_HOTKEY => {
            match wparam as i32 {
                HOTKEY_RUN => start_script(),
                HOTKEY_STOP => stop_script(),
                _ => {}
            }
            0
        }
        WM_APP => {
            drain_events();
            0
        }
        WM_TIMER if wparam == TIMER_LINE_GUTTER_SYNC => {
            KillTimer(hwnd, TIMER_LINE_GUTTER_SYNC);
            refresh_line_numbers();
            0
        }
        WM_TIMER if wparam == TIMER_SCRIPT_HIGHLIGHT => {
            KillTimer(hwnd, TIMER_SCRIPT_HIGHLIGHT);
            refresh_editor_view();
            0
        }
        msg if msg == WM_APP + 2 => {
            refresh_line_numbers();
            0
        }
        msg if msg == WM_APP + 3 => {
            refresh_editor_view();
            0
        }
        WM_DESTROY => {
            destroy_fonts();
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe extern "system" fn script_edit_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_KEYDOWN && wparam as u32 == VK_TAB as u32 {
        let spaces = win7ui::wide("    ");
        SendMessageW(hwnd, EM_REPLACESEL, 1, spaces.as_ptr() as LPARAM);
        schedule_editor_highlight();
        return 0;
    }
    if msg == WM_CHAR && wparam as u32 == VK_TAB as u32 {
        return 0;
    }
    let result = CallWindowProcW(SCRIPT_EDIT_PROC, hwnd, msg, wparam, lparam);
    if matches!(msg, WM_CHAR | WM_IME_CHAR | WM_PASTE | WM_CUT | WM_CLEAR | WM_UNDO) {
        schedule_editor_highlight();
    } else if msg == WM_KEYDOWN && wparam as u32 == VK_DELETE as u32 {
        schedule_editor_highlight();
    } else if msg == WM_MOUSEWHEEL {
        schedule_line_number_refresh_after_wheel();
    } else if matches!(msg, WM_KEYUP | WM_VSCROLL) {
        PostMessageW(main_hwnd(), WM_APP + 2, 0, 0);
    }
    result
}

unsafe fn destroy_fonts() {
    if let Some(app) = APP.get() {
        let mut app = app.lock().unwrap();
        app.fonts.destroy();
    }
}

unsafe fn create_controls(hwnd: HWND) {
    let open_button = win7ui::create_button(hwnd, "打开", IDC_OPEN);
    let save_button = win7ui::create_button(hwnd, "保存", IDC_SAVE);
    let save_as_button = win7ui::create_button(hwnd, "另存为", IDC_SAVE_AS);
    let run_button = win7ui::create_button(hwnd, "运行 F5", IDC_RUN);
    let stop_button = win7ui::create_button(hwnd, "停止 F11", IDC_STOP);
    let capture_button = win7ui::create_button(hwnd, "框选截图", IDC_CAPTURE);
    let click_image_button = win7ui::create_button(hwnd, "点击截图", IDC_CLICK_IMAGE);
    let capture_point_button = win7ui::create_button(hwnd, "捕获坐标", IDC_CAPTURE_POINT);

    let status = win7ui::create_label(hwnd, "就绪。编辑器 Tab 会插入 4 个空格。", 10, 44, 400, 22);
    let line_numbers = win7ui::create_line_number_gutter(hwnd, "1", 0, 10, 70, 48, 620);
    let script = win7ui::RichEdit::create(
        hwnd,
        SAMPLE_SCRIPT,
        IDC_SCRIPT,
        62,
        70,
        598,
        620,
    )
    .hwnd();
    let log = win7ui::create_multiline_edit(hwnd, "", IDC_LOG, 670, 70, 420, 620, true, false);
    let fonts = win7ui::UiFonts::win7_defaults();
    win7ui::apply_font_handle(status, fonts.ui);
    win7ui::apply_font_handle_to_many(&[
        open_button,
        save_button,
        save_as_button,
        run_button,
        stop_button,
        capture_button,
        click_image_button,
        capture_point_button,
    ], fonts.ui);
    win7ui::apply_font_handle(line_numbers, fonts.editor);
    win7ui::apply_font_handle(script, fonts.editor);
    win7ui::apply_font_handle(log, fonts.log);
    subclass_line_number_gutter(line_numbers);
    subclass_script_editor(script);

    if let Some(app) = APP.get() {
        let mut app = app.lock().unwrap();
        app.hwnd = hwnd_value(hwnd);
        app.script = hwnd_value(script);
        app.line_numbers = hwnd_value(line_numbers);
        app.log = hwnd_value(log);
        app.status = hwnd_value(status);
        app.run_button = hwnd_value(run_button);
        app.stop_button = hwnd_value(stop_button);
        app.open_button = hwnd_value(open_button);
        app.save_button = hwnd_value(save_button);
        app.save_as_button = hwnd_value(save_as_button);
        app.capture_button = hwnd_value(capture_button);
        app.click_image_button = hwnd_value(click_image_button);
        app.capture_point_button = hwnd_value(capture_point_button);
        app.fonts = fonts;
    }

    update_running_ui(false);
    layout_controls(hwnd);
    refresh_editor_view();
}

unsafe fn current_ui_font() -> HWND {
    APP.get()
        .map(|app| app.lock().unwrap().fonts.ui)
        .map(|font| font as HWND)
        .unwrap_or(null_mut())
}

unsafe fn subclass_script_editor(hwnd: HWND) {
    if hwnd.is_null() {
        return;
    }
    let previous = SetWindowLongPtrW(
        hwnd,
        GWLP_WNDPROC,
        script_edit_proc as *const () as isize,
    );
    SCRIPT_EDIT_PROC = std::mem::transmute(previous);
}

unsafe extern "system" fn line_number_gutter_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_ERASEBKGND => {
            erase_line_number_gutter(hwnd, wparam as HDC);
            1
        }
        WM_PAINT => {
            paint_line_number_gutter(hwnd);
            0
        }
        WM_MOUSEWHEEL => {
            if let Some(app) = APP.get() {
                let script = to_hwnd(app.lock().unwrap().script);
                if !script.is_null() {
                    SendMessageW(script, msg, wparam, lparam);
                }
            }
            schedule_line_number_refresh_after_wheel();
            0
        }
        _ => CallWindowProcW(LINE_NUMBER_GUTTER_PROC, hwnd, msg, wparam, lparam),
    }
}

unsafe fn subclass_line_number_gutter(hwnd: HWND) {
    if hwnd.is_null() {
        return;
    }
    let previous = SetWindowLongPtrW(
        hwnd,
        GWLP_WNDPROC,
        line_number_gutter_proc as *const () as isize,
    );
    LINE_NUMBER_GUTTER_PROC = std::mem::transmute(previous);
}

unsafe fn paint_line_number_gutter(hwnd: HWND) {
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
        draw_line_number_gutter(hwnd, mem_dc);
        BitBlt(hdc, 0, 0, width, height, mem_dc, 0, 0, SRCCOPY);
        if !old_bitmap.is_null() {
            SelectObject(mem_dc, old_bitmap);
        }
        DeleteObject(bitmap as _);
        DeleteDC(mem_dc);
    } else {
        draw_line_number_gutter(hwnd, hdc);
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

unsafe fn draw_line_number_gutter(hwnd: HWND, hdc: HDC) {
    if hdc.is_null() {
        return;
    }

    erase_line_number_gutter(hwnd, hdc);
    let mut rect = std::mem::zeroed();
    GetClientRect(hwnd, &mut rect);

    let (script, font) = if let Some(app) = APP.get() {
        let app = app.lock().unwrap();
        (to_hwnd(app.script), app.fonts.editor)
    } else {
        (null_mut(), 0)
    };

    if !script.is_null() {
        let old_font = if font != 0 {
            SelectObject(hdc, font as _)
        } else {
            null_mut()
        };
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextAlign(hdc, TA_RIGHT | TA_TOP);
        SetTextColor(hdc, win7ui::rgb(86, 96, 112));

        let mut metrics: TEXTMETRICW = std::mem::zeroed();
        GetTextMetricsW(hdc, &mut metrics);
        let line_height = (metrics.tmHeight + metrics.tmExternalLeading).max(16);
        let editor = win7ui::RichEdit::new(script);
        let line_count = editor.line_count();
        let mut line = editor.first_visible_line();

        while line < line_count {
            let Some(y) = editor.line_top(line) else { break; };
            if y > rect.bottom {
                break;
            }
            if y + line_height >= rect.top {
                let number = format!("{}", line + 1);
                let number = win7ui::wide(&number);
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

        if !old_font.is_null() {
            SelectObject(hdc, old_font);
        }
    }
}

unsafe fn layout_controls(hwnd: HWND) {
    let mut rect = std::mem::zeroed();
    GetClientRect(hwnd, &mut rect);
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;

    if let Some(app) = APP.get() {
        let app = app.lock().unwrap();
        win7ui::row_layout(
            &[
                (to_hwnd(app.open_button), 76),
                (to_hwnd(app.save_button), 76),
                (to_hwnd(app.save_as_button), 86),
                (to_hwnd(app.run_button), 92),
                (to_hwnd(app.stop_button), 96),
                (to_hwnd(app.capture_button), 98),
                (to_hwnd(app.click_image_button), 98),
                (to_hwnd(app.capture_point_button), 98),
            ],
            10,
            10,
            28,
            8,
        );

        win7ui::move_window(to_hwnd(app.status), 10, 44, width - 20, 22);

        let split = win7ui::split_left_right(width, height, 10, 70, 10, 410, 250);
        win7ui::move_window(to_hwnd(app.line_numbers), split.left_x, split.y, 48, split.h);
        win7ui::move_window(
            to_hwnd(app.script),
            split.left_x + 52,
            split.y,
            split.left_w - 52,
            split.h,
        );
        win7ui::move_window(to_hwnd(app.log), split.right_x, split.y, split.right_w, split.h);
    }
}

unsafe fn start_script() {
    let (script, tx, stop_requested) = {
        let Some(app_lock) = APP.get() else { return; };
        let mut app = app_lock.lock().unwrap();
        if app.running {
            append_log("脚本已经在运行，忽略重复运行请求。");
            return;
        }
        let script = win7ui::get_window_text(to_hwnd(app.script));
        let (tx, rx) = win7ui::event_channel(to_hwnd(app.hwnd), WM_APP);
        let stop_requested = Arc::new(AtomicBool::new(false));
        app.running = true;
        app.stop_requested = Some(stop_requested.clone());
        app.rx = Some(rx);
        (script, tx, stop_requested)
    };

    clear_log();
    update_running_ui(true);
    append_log("开始运行。");
    thread::spawn(move || {
        let log_stop = stop_requested.clone();
        let mut tail_logs: VecDeque<String> = VecDeque::with_capacity(MAX_RUN_LOG_LINES);
        let mut total_lines = 0usize;
        let result = Runner::new(stop_requested).and_then(|mut runner| {
            let mut last_flush = Instant::now();

            runner.run_script(&script, |msg| {
                if log_stop.load(Ordering::Relaxed) {
                    return;
                }
                push_tail_log(&mut tail_logs, &mut total_lines, msg);

                if last_flush.elapsed() >= Duration::from_millis(LOG_SNAPSHOT_INTERVAL_MS) {
                    send_log_snapshot(&tx, &tail_logs, total_lines);
                    last_flush = Instant::now();
                }
            })?;
            Ok(())
        });
        let error_line = result.as_ref().err().and_then(run_error_line);
        let (final_line, status) = match result {
            Ok(()) => ("运行完成。".to_string(), "运行完成。"),
            Err(RunError::Stopped) => ("运行已停止。".to_string(), "运行已停止。"),
            Err(err) => (format!("错误：{err}"), "运行出错。"),
        };
        push_tail_log(&mut tail_logs, &mut total_lines, final_line);
        send_log_snapshot(&tx, &tail_logs, total_lines);
        let _ = tx.send(AppEvent::Done {
            status: status.to_string(),
            error_line,
        });
        unsafe { tx.wake(); }
    });
}

unsafe fn stop_script() {
    let stop_requested = {
        let Some(app) = APP.get() else { return; };
        app.lock().unwrap().stop_requested.clone()
    };
    if let Some(flag) = stop_requested {
        flag.store(true, Ordering::Relaxed);
        append_log("正在请求停止脚本...");
        set_status("正在停止...");
    }
}

fn send_log_snapshot(
    tx: &win7ui::UiEventSender<AppEvent>,
    tail_logs: &VecDeque<String>,
    total_lines: usize,
) {
    if tail_logs.is_empty() {
        return;
    }
    let _ = tx.send(AppEvent::ReplaceLog {
        lines: tail_logs.iter().cloned().collect(),
        total_lines,
    });
    unsafe { tx.wake(); }
}

fn push_tail_log(tail_logs: &mut VecDeque<String>, total_lines: &mut usize, line: String) {
    *total_lines += 1;
    if tail_logs.len() >= MAX_RUN_LOG_LINES {
        tail_logs.pop_front();
    }
    tail_logs.push_back(line);
}

fn run_error_line(err: &RunError) -> Option<usize> {
    match err {
        RunError::Line { line, .. } => Some(*line),
        _ => None,
    }
}

unsafe fn open_script() {
    let Some(path) = choose_script_file(false) else { return; };
    match fs::read_to_string(&path) {
        Ok(text) => {
            if let Some(app) = APP.get() {
                let mut app = app.lock().unwrap();
                win7ui::RichEdit::new(to_hwnd(app.script)).set_text(&text);
                app.current_file = Some(path.clone());
            }
            refresh_editor_view();
            append_log(&format!("已打开脚本：{}", path.display()));
            set_status(&format!("当前脚本：{}", path.display()));
        }
        Err(err) => append_log(&format!("打开失败：{err}")),
    }
}

unsafe fn save_script(force_dialog: bool) {
    let (text, existing) = {
        let Some(app) = APP.get() else { return; };
        let app = app.lock().unwrap();
        (win7ui::get_window_text(to_hwnd(app.script)), app.current_file.clone())
    };

    let path = if force_dialog {
        choose_script_file(true)
    } else {
        existing.or_else(|| choose_script_file(true))
    };

    let Some(path) = path else { return; };
    match fs::write(&path, text) {
        Ok(()) => {
            if let Some(app) = APP.get() {
                app.lock().unwrap().current_file = Some(path.clone());
            }
            append_log(&format!("已保存脚本：{}", path.display()));
            set_status(&format!("当前脚本：{}", path.display()));
        }
        Err(err) => append_log(&format!("保存失败：{err}")),
    }
}

unsafe fn choose_script_file(save: bool) -> Option<PathBuf> {
    let owner = APP
        .get()
        .map(|app| to_hwnd(app.lock().unwrap().hwnd))
        .unwrap_or(null_mut());
    win7ui::choose_file(
        owner,
        save,
        "脚本文件 (*.txt;*.py;*.pyauto)\0*.txt;*.py;*.pyauto\0所有文件 (*.*)\0*.*\0",
        if save { "保存脚本" } else { "打开脚本" },
        "txt",
    )
}

unsafe fn begin_capture(mode: CaptureMode) {
    let (tx, target_hwnd) = {
        let Some(app_lock) = APP.get() else { return; };
        let mut app = app_lock.lock().unwrap();
        if app.running {
            append_log("脚本运行中，暂不开始截图。");
            return;
        }
        let (tx, rx) = win7ui::event_channel(to_hwnd(app.hwnd), WM_APP);
        app.rx = Some(rx);
        (tx, app.hwnd)
    };

    append_log(match mode {
        CaptureMode::SaveRegion => "正在隐藏窗口并准备框选截图...",
        CaptureMode::ClickImage => "正在隐藏窗口并准备点击截图...",
        CaptureMode::PointClick => "正在隐藏窗口并准备捕获点击坐标...",
    });
    ShowWindow(to_hwnd(target_hwnd), SW_HIDE);

    thread::spawn(move || {
        thread::sleep(Duration::from_millis(350));
        let result = capture_primary_screen().map_err(|err| err.to_string());
        let _ = unsafe { tx.send_and_wake(AppEvent::CaptureReady { mode, result }) };
    });
}

fn capture_primary_screen() -> Result<CapturedScreen, String> {
    let screen = Screen::from_point(0, 0).map_err(|err| err.to_string())?;
    let info = screen.display_info;
    let image = screen.capture().map_err(|err| err.to_string())?;
    Ok(CapturedScreen {
        screen_x: info.x,
        screen_y: info.y,
        width: info.width as i32,
        height: info.height as i32,
        image,
    })
}

unsafe fn show_capture_overlay(mode: CaptureMode, captured: CapturedScreen) {
    let hinstance = GetModuleHandleW(null_mut());
    let overlay = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_LAYERED,
        wide("PyAutoRsCaptureOverlay").as_ptr(),
        wide("PyAuto 截图框选").as_ptr(),
        WS_POPUP | WS_VISIBLE,
        captured.screen_x,
        captured.screen_y,
        captured.width,
        captured.height,
        null_mut(),
        null_mut(),
        hinstance,
        null_mut(),
    );
    if overlay.is_null() {
        ShowWindow(main_hwnd(), SW_SHOW);
        append_log("创建截图框选层失败。");
        return;
    }

    // Win7 treats color-keyed transparent layered windows as hit-test transparent in
    // practice on some machines, so use a real alpha overlay that still receives input.
    SetLayeredWindowAttributes(overlay, 0, 90, LWA_ALPHA);
    ShowWindow(overlay, SW_SHOW);
    UpdateWindow(overlay);
    SetForegroundWindow(overlay);
    SetCapture(overlay);
    set_status("拖动鼠标框选区域，Esc 取消。");
    if mode == CaptureMode::PointClick {
        set_status("点击屏幕位置捕获坐标，Esc 取消。");
    }

    if let Some(app) = APP.get() {
        app.lock().unwrap().capture = Some(CaptureState {
            mode,
            screen_x: captured.screen_x,
            screen_y: captured.screen_y,
            width: captured.width,
            height: captured.height,
            image: captured.image,
            overlay_hwnd: hwnd_value(overlay),
            dragging: false,
            start: None,
            end: None,
            selection: None,
        });
    }
}

unsafe extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_KEYDOWN => {
            if wparam as u16 == VK_ESCAPE {
                cancel_capture();
            }
            0
        }
        WM_LBUTTONDOWN => {
            let pos = win7ui::lparam_pos(lparam);
            if capture_point_click(hwnd, pos) {
                return 0;
            }
            if let Some(app) = APP.get() {
                if let Some(capture) = &mut app.lock().unwrap().capture {
                    capture.dragging = true;
                    capture.start = Some(pos);
                    capture.end = Some(pos);
                    capture.selection = None;
                }
            }
            SetCapture(hwnd);
            win7ui::overlay::invalidate(hwnd);
            0
        }
        WM_MOUSEMOVE => {
            if let Some(app) = APP.get() {
                if let Some(capture) = &mut app.lock().unwrap().capture {
                    if capture.dragging {
                        capture.end = Some(win7ui::lparam_pos(lparam));
                        win7ui::overlay::invalidate(hwnd);
                    }
                }
            }
            0
        }
        WM_LBUTTONUP => {
            ReleaseCapture();
            let mut confirmed = false;
            if let Some(app) = APP.get() {
                if let Some(capture) = &mut app.lock().unwrap().capture {
                    capture.dragging = false;
                    capture.end = Some(win7ui::lparam_pos(lparam));
                    capture.selection = capture_rect(capture);
                    confirmed = capture.selection.is_some();
                    if confirmed {
                        capture.overlay_hwnd = 0;
                    }
                }
            }
            if confirmed {
                DestroyWindow(hwnd);
                show_confirm_window();
            } else {
                append_log("框选区域太小，请重新拖动选择。");
                win7ui::overlay::invalidate(hwnd);
            }
            0
        }
        WM_PAINT => {
            paint_overlay(hwnd);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn paint_overlay(hwnd: HWND) {
    let selection = APP
        .get()
        .and_then(|app| app.lock().unwrap().capture.as_ref().and_then(current_rect));
    win7ui::paint_selection_overlay(hwnd, selection);
}

unsafe fn capture_point_click(hwnd: HWND, pos: (i32, i32)) -> bool {
    let Some(app_lock) = APP.get() else {
        return false;
    };

    let Some((screen_x, screen_y)) = ({
        let mut app = app_lock.lock().unwrap();
        let Some(capture) = &mut app.capture else {
            return false;
        };
        if capture.mode != CaptureMode::PointClick {
            return false;
        }
        capture.overlay_hwnd = 0;
        Some((capture.screen_x + pos.0, capture.screen_y + pos.1))
    }) else {
        return false;
    };

    ReleaseCapture();
    DestroyWindow(hwnd);
    let code = format!("click({screen_x}, {screen_y})");
    insert_script_line(&code);
    append_log(&format!("已捕获坐标并插入代码：{code}"));
    finish_capture();
    true
}

fn current_rect(capture: &CaptureState) -> Option<win7ui::SelectionRect> {
    win7ui::client_selection_rect(capture.start, capture.end, capture.width, capture.height, 3)
}

fn capture_rect(capture: &CaptureState) -> Option<ImageRect> {
    let rect = current_rect(capture)?;
    Some(ImageRect {
        left: rect.left as u32,
        top: rect.top as u32,
        width: rect.width() as u32,
        height: rect.height() as u32,
    })
}

unsafe fn show_confirm_window() {
    let Some(app_lock) = APP.get() else { return; };
    let (mode, selected) = {
        let app = app_lock.lock().unwrap();
        let Some(capture) = &app.capture else { return; };
        let Some(selected) = capture.selection else { return; };
        (capture.mode, selected)
    };

    let hinstance = GetModuleHandleW(null_mut());
    let hwnd = CreateWindowExW(
        WS_EX_TOPMOST,
        wide("PyAutoRsCaptureConfirm").as_ptr(),
        wide(match mode {
            CaptureMode::SaveRegion => "保存框选截图",
            CaptureMode::ClickImage => "保存图片并插入点击代码",
            CaptureMode::PointClick => "捕获坐标",
        })
        .as_ptr(),
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        560,
        if mode == CaptureMode::ClickImage { 250 } else { 205 },
        null_mut(),
        null_mut(),
        hinstance,
        null_mut(),
    );
    if hwnd.is_null() {
        cancel_capture();
        return;
    }

    append_log(&format!("已框选区域：{} x {}", selected.width, selected.height));
}

unsafe extern "system" fn confirm_proc(hwnd: HWND, msg: u32, wparam: WPARAM, _lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            create_confirm_controls(hwnd);
            0
        }
        WM_COMMAND => {
            match (wparam & 0xffff) as i32 {
                IDC_CONFIRM_OK => confirm_capture(),
                IDC_CONFIRM_RESELECT => reselect_capture(),
                IDC_CONFIRM_CANCEL => cancel_capture(),
                _ => {}
            }
            0
        }
        WM_CLOSE => {
            cancel_capture();
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, _lparam),
    }
}

unsafe fn create_confirm_controls(hwnd: HWND) {
    let (mode, file_name, selected_text) = {
        let Some(app) = APP.get() else { return; };
        let app = app.lock().unwrap();
        let Some(capture) = &app.capture else { return; };
        let selected = capture.selection.unwrap_or(ImageRect {
            left: 0,
            top: 0,
            width: 0,
            height: 0,
        });
        let prefix = match capture.mode {
            CaptureMode::SaveRegion => "screenshot",
            CaptureMode::ClickImage => "click_image",
            CaptureMode::PointClick => "point",
        };
        (
            capture.mode,
            format!("{prefix}_{}.png", timestamp_for_file()),
            format!("选区：{} x {}", selected.width, selected.height),
        )
    };

    let dir_label = win7ui::create_label(hwnd, "目录", 18, 20, 70, 22);
    let dir_edit =
        win7ui::create_single_line_edit(hwnd, "captures", IDC_CONFIRM_DIR, 90, 18, 430, 24);

    let file_label = win7ui::create_label(hwnd, "文件名", 18, 55, 70, 22);
    let file_edit =
        win7ui::create_single_line_edit(hwnd, &file_name, IDC_CONFIRM_FILE, 90, 53, 430, 24);

    let selected_label = win7ui::create_label(hwnd, &selected_text, 90, 86, 430, 22);

    let mut threshold_edit = null_mut();
    let mut timeout_edit = null_mut();
    let mut threshold_label = null_mut();
    let mut timeout_label = null_mut();
    let mut y = 116;
    if mode == CaptureMode::ClickImage {
        threshold_label = win7ui::create_label(hwnd, "匹配阈值", 18, 92, 70, 22);
        threshold_edit =
            win7ui::create_single_line_edit(hwnd, "0.92", IDC_CONFIRM_THRESHOLD, 90, 90, 90, 24);
        timeout_label = win7ui::create_label(hwnd, "超时 ms", 205, 92, 70, 22);
        timeout_edit =
            win7ui::create_single_line_edit(hwnd, "3000", IDC_CONFIRM_TIMEOUT, 275, 90, 90, 24);
        y = 150;
    }

    let ok_button = win7ui::create_button_at(hwnd, "确认", IDC_CONFIRM_OK, 210, y, 82, 28);
    let reselect_button = win7ui::create_button_at(hwnd, "重选", IDC_CONFIRM_RESELECT, 310, y, 82, 28);
    let cancel_button = win7ui::create_button_at(hwnd, "取消", IDC_CONFIRM_CANCEL, 410, y, 82, 28);

    let ui_font = current_ui_font();
    for control in [
        dir_label,
        dir_edit,
        file_label,
        file_edit,
        selected_label,
        threshold_label,
        threshold_edit,
        timeout_label,
        timeout_edit,
        ok_button,
        reselect_button,
        cancel_button,
    ] {
        win7ui::apply_font(control, ui_font);
    }

    if let Some(app) = APP.get() {
        app.lock().unwrap().confirm = Some(ConfirmState {
            hwnd: hwnd_value(hwnd),
            dir_edit: hwnd_value(dir_edit),
            file_edit: hwnd_value(file_edit),
            threshold_edit: hwnd_value(threshold_edit),
            timeout_edit: hwnd_value(timeout_edit),
        });
    }
}

unsafe fn confirm_capture() {
    let (path, mode, threshold, timeout_ms, selected, image, confirm_hwnd) = {
        let Some(app_lock) = APP.get() else { return; };
        let app = app_lock.lock().unwrap();
        let Some(capture) = &app.capture else { return; };
        let Some(confirm) = &app.confirm else { return; };
        let Some(selected) = capture.selection else { return; };

        let dir = win7ui::get_window_text(to_hwnd(confirm.dir_edit));
        let mut file_name = win7ui::get_window_text(to_hwnd(confirm.file_edit));
        if Path::new(file_name.trim()).extension().is_none() {
            file_name.push_str(".png");
        }
        let threshold = win7ui::get_window_text(to_hwnd(confirm.threshold_edit))
            .trim()
            .parse::<f32>()
            .unwrap_or(0.92)
            .clamp(0.1, 1.0);
        let timeout_ms = win7ui::get_window_text(to_hwnd(confirm.timeout_edit))
            .trim()
            .parse::<u64>()
            .unwrap_or(3000);
        (
            PathBuf::from(dir.trim()).join(file_name.trim()),
            capture.mode,
            threshold,
            timeout_ms,
            selected,
            capture.image.clone(),
            confirm.hwnd,
        )
    };

    match save_crop(&image, selected, &path) {
        Ok(()) => {
            append_log(&format!("已保存图片：{}", path.display()));
            if mode == CaptureMode::ClickImage {
                let code = format!(
                    "find_click(\"{}\", {:.2}, {}, {}, {}, {}, {})",
                    win7ui::script_path_literal(&path),
                    threshold,
                    timeout_ms,
                    selected.left,
                    selected.top,
                    selected.width,
                    selected.height
                );
                insert_script_line(&code);
                append_log(&format!("已插入代码：{code}"));
            }
            DestroyWindow(to_hwnd(confirm_hwnd));
            finish_capture();
        }
        Err(err) => append_log(&format!("保存失败：{err}")),
    }
}

fn save_crop(image: &RgbaImage, selected: ImageRect, path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
    }
    let cropped = imageops::crop_imm(image, selected.left, selected.top, selected.width, selected.height).to_image();
    cropped.save(path).map_err(|err| err.to_string())
}

unsafe fn reselect_capture() {
    let (confirm_hwnd, overlay_data) = {
        let Some(app_lock) = APP.get() else { return; };
        let mut app = app_lock.lock().unwrap();
        let confirm_hwnd = app.confirm.as_ref().map(|confirm| confirm.hwnd).unwrap_or_default();
        app.confirm = None;
        let Some(capture) = &mut app.capture else { return; };
        capture.selection = None;
        capture.start = None;
        capture.end = None;
        capture.dragging = false;
        (
            confirm_hwnd,
            (
                capture.screen_x,
                capture.screen_y,
                capture.width,
                capture.height,
                capture.overlay_hwnd,
            ),
        )
    };
    if confirm_hwnd != 0 {
        DestroyWindow(to_hwnd(confirm_hwnd));
    }
    let (screen_x, screen_y, width, height, existing_overlay) = overlay_data;
    let overlay = if existing_overlay != 0 {
        to_hwnd(existing_overlay)
    } else {
        let hinstance = GetModuleHandleW(null_mut());
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_LAYERED,
            wide("PyAutoRsCaptureOverlay").as_ptr(),
            wide("PyAuto 截图框选").as_ptr(),
            WS_POPUP | WS_VISIBLE,
            screen_x,
            screen_y,
            width,
            height,
            null_mut(),
            null_mut(),
            hinstance,
            null_mut(),
        )
    };
    if !overlay.is_null() {
        if let Some(app) = APP.get() {
            if let Some(capture) = &mut app.lock().unwrap().capture {
                capture.overlay_hwnd = hwnd_value(overlay);
            }
        }
        SetLayeredWindowAttributes(overlay, 0, 90, LWA_ALPHA);
        SetForegroundWindow(overlay);
        SetCapture(overlay);
        ShowWindow(overlay, SW_SHOW);
        UpdateWindow(overlay);
        win7ui::overlay::invalidate(overlay);
        set_status("拖动鼠标框选区域，Esc 取消。");
    }
}

unsafe fn cancel_capture() {
    let (overlay, confirm) = {
        let Some(app_lock) = APP.get() else { return; };
        let mut app = app_lock.lock().unwrap();
        let overlay = app.capture.as_ref().map(|capture| capture.overlay_hwnd).unwrap_or_default();
        let confirm = app.confirm.as_ref().map(|confirm| confirm.hwnd).unwrap_or_default();
        app.capture = None;
        app.confirm = None;
        (overlay, confirm)
    };
    ReleaseCapture();
    if overlay != 0 {
        DestroyWindow(to_hwnd(overlay));
    }
    if confirm != 0 {
        DestroyWindow(to_hwnd(confirm));
    }
    ShowWindow(main_hwnd(), SW_SHOW);
    SetForegroundWindow(main_hwnd());
    set_status("已取消截图。");
}

unsafe fn finish_capture() {
    let overlay = {
        let Some(app_lock) = APP.get() else { return; };
        let mut app = app_lock.lock().unwrap();
        let overlay = app.capture.as_ref().map(|capture| capture.overlay_hwnd).unwrap_or_default();
        app.capture = None;
        app.confirm = None;
        overlay
    };
    ReleaseCapture();
    if overlay != 0 {
        DestroyWindow(to_hwnd(overlay));
    }
    ShowWindow(main_hwnd(), SW_SHOW);
    SetForegroundWindow(main_hwnd());
    set_status("就绪");
}

unsafe fn insert_script_line(line: &str) {
    let Some(app) = APP.get() else { return; };
    let script = to_hwnd(app.lock().unwrap().script);
    win7ui::insert_line_at_end(script, line);
    refresh_editor_view();
}

unsafe fn refresh_editor_view() {
    let Some(app) = APP.get() else { return; };
    let script = to_hwnd(app.lock().unwrap().script);
    let text = win7ui::get_window_text(script);
    let spans = highlight_script_spans(&text);
    win7ui::RichEdit::new(script).apply_highlights(text.encode_utf16().count(), &spans, win7ui::rgb(32, 32, 32));
    refresh_line_numbers();
}

unsafe fn refresh_line_numbers() {
    let Some(app) = APP.get() else { return; };
    let (script, line_numbers) = {
        let app = app.lock().unwrap();
        (to_hwnd(app.script), to_hwnd(app.line_numbers))
    };
    UpdateWindow(script);
    RedrawWindow(
        line_numbers,
        null_mut(),
        null_mut(),
        RDW_INVALIDATE | RDW_ERASE | RDW_UPDATENOW | RDW_NOCHILDREN,
    );
}

unsafe fn schedule_line_number_refresh_after_wheel() {
    let hwnd = main_hwnd();
    PostMessageW(hwnd, WM_APP + 2, 0, 0);
    SetTimer(hwnd, TIMER_LINE_GUTTER_SYNC, 45, None);
}

unsafe fn schedule_editor_highlight() {
    let hwnd = main_hwnd();
    PostMessageW(hwnd, WM_APP + 2, 0, 0);
    SetTimer(hwnd, TIMER_SCRIPT_HIGHLIGHT, 90, None);
}

unsafe fn focus_script_line(line: usize) {
    let Some(app) = APP.get() else { return; };
    let script = to_hwnd(app.lock().unwrap().script);
    win7ui::RichEdit::new(script).focus_line(line);
    set_status(&format!("运行出错，已定位到第 {line} 行。"));
}

fn highlight_script_spans(text: &str) -> Vec<win7ui::HighlightSpan> {
    let mut spans = Vec::new();
    let mut pos = 0usize;
    for line in text.split_inclusive('\n') {
        highlight_line_spans(line, pos, &mut spans);
        pos += line.encode_utf16().count();
    }
    spans
}

fn highlight_line_spans(line: &str, base: usize, spans: &mut Vec<win7ui::HighlightSpan>) {
    highlight_fragment_tokens(line, 0, 0, line.len(), base, spans, true);
}

fn highlight_fragment_tokens(
    line: &str,
    mut byte: usize,
    mut unit: usize,
    end_byte: usize,
    base: usize,
    spans: &mut Vec<win7ui::HighlightSpan>,
    allow_comment: bool,
) {
    while byte < end_byte {
        let Some(ch) = next_char(line, byte) else { break; };
        if ch == '#' {
            if allow_comment {
                let (end_byte, end_unit) = token_end(line, byte, unit, end_byte, |c| c != '\r' && c != '\n');
                push_span(spans, base + unit, base + end_unit, color_comment());
                byte = end_byte;
                unit = end_unit;
                continue;
            }
        }
        if let Some(start) = string_start(line, byte, unit, end_byte) {
            let literal = string_literal_end(line, start.quote_byte, start.quote_unit, start.quote, end_byte);
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
    spans: &mut Vec<win7ui::HighlightSpan>,
) {
    let mut byte = literal.content_start_byte;
    let mut unit = literal.content_start_unit;
    while byte < literal.content_end_byte {
        let Some(ch) = next_char(line, byte) else { break; };
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
                let Some(inner_ch) = next_char(line, byte) else { break; };
                if let Some(start) = string_start(line, byte, unit, literal.content_end_byte) {
                    let inner_literal =
                        string_literal_end(line, start.quote_byte, start.quote_unit, start.quote, literal.content_end_byte);
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
        let Some(ch) = next_char(line, byte) else { break; };
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
    let Some(ch) = next_char(line, byte) else { return false; };
    if ch.is_ascii_digit() {
        return true;
    }
    ch == '.' && byte + 1 < end_byte && line[byte + 1..].chars().next().is_some_and(|c| c.is_ascii_digit())
}

fn number_token_end(line: &str, mut byte: usize, mut unit: usize, end_byte: usize) -> (usize, usize) {
    let mut prev = '\0';
    while byte < end_byte {
        let Some(ch) = next_char(line, byte) else { break; };
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

fn push_span(spans: &mut Vec<win7ui::HighlightSpan>, start: usize, end: usize, color: u32) {
    if end > start {
        spans.push(win7ui::HighlightSpan { start, end, color });
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
        let Some(ch) = next_char(line, byte) else { break; };
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
    win7ui::rgb(32, 32, 32)
}

fn color_comment() -> u32 {
    win7ui::rgb(105, 120, 105)
}

fn color_string() -> u32 {
    win7ui::rgb(145, 92, 25)
}

fn color_number() -> u32 {
    win7ui::rgb(110, 70, 175)
}

fn color_keyword() -> u32 {
    win7ui::rgb(175, 45, 60)
}

fn color_builtin() -> u32 {
    win7ui::rgb(25, 90, 185)
}

fn color_command() -> u32 {
    win7ui::rgb(20, 135, 95)
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

unsafe fn drain_events() {
    let rx = {
        let Some(app) = APP.get() else { return; };
        app.lock().unwrap().rx.take()
    };
    let Some(rx) = rx else { return; };

    let mut keep = true;
    let mut processed = 0usize;
    let max_events_per_tick = 32usize;
    while let Ok(event) = rx.try_recv() {
        processed += 1;
        match event {
            AppEvent::ReplaceLog { lines, total_lines } => replace_log_snapshot(&lines, total_lines),
            AppEvent::Done { status, error_line } => {
                keep = false;
                if let Some(app) = APP.get() {
                    let mut app = app.lock().unwrap();
                    app.running = false;
                    app.stop_requested = None;
                }
                update_running_ui(false);
                set_status(&status);
                if let Some(line) = error_line {
                    focus_script_line(line);
                }
            }
            AppEvent::CaptureReady { mode, result } => {
                keep = false;
                match result {
                    Ok(captured) => show_capture_overlay(mode, captured),
                    Err(err) => {
                        ShowWindow(main_hwnd(), SW_SHOW);
                        append_log(&format!("截图失败：{err}"));
                        set_status("截图失败。");
                    }
                }
            }
        }
        if processed >= max_events_per_tick {
            break;
        }
    }

    if keep {
        if let Some(app) = APP.get() {
            let hwnd = app.lock().unwrap().hwnd;
            app.lock().unwrap().rx = Some(rx);
            if processed >= max_events_per_tick {
                win7ui::wake_window(to_hwnd(hwnd), WM_APP);
            }
        }
    }
}

unsafe fn update_running_ui(running: bool) {
    if let Some(app) = APP.get() {
        let app = app.lock().unwrap();
        let enabled = !running;
        win7ui::enable_window(to_hwnd(app.run_button), enabled);
        win7ui::enable_window(to_hwnd(app.open_button), enabled);
        win7ui::enable_window(to_hwnd(app.save_button), enabled);
        win7ui::enable_window(to_hwnd(app.save_as_button), enabled);
        win7ui::enable_window(to_hwnd(app.capture_button), enabled);
        win7ui::enable_window(to_hwnd(app.click_image_button), enabled);
        win7ui::enable_window(to_hwnd(app.capture_point_button), enabled);
    }
    set_status(if running { "运行中... F11 可停止" } else { "就绪" });
}

unsafe fn append_log(line: &str) {
    log_view().append_line(line);
}

unsafe fn replace_log_snapshot(lines: &[String], total_lines: usize) {
    log_view().replace_snapshot(lines, total_lines, MAX_RUN_LOG_LINES);
}

unsafe fn clear_log() {
    log_view().clear();
}

unsafe fn log_view() -> win7ui::LogView {
    let log = APP
        .get()
        .map(|app| to_hwnd(app.lock().unwrap().log))
        .unwrap_or(null_mut());
    win7ui::LogView::new(log, MAX_LOG_CHARS)
}

unsafe fn set_status(text: &str) {
    let Some(app) = APP.get() else { return; };
    let status = to_hwnd(app.lock().unwrap().status);
    win7ui::set_window_text(status, text);
}

unsafe fn main_hwnd() -> HWND {
    APP.get()
        .map(|app| to_hwnd(app.lock().unwrap().hwnd))
        .unwrap_or(null_mut())
}

fn timestamp_for_file() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}
