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
    Graphics::Gdi::{UpdateWindow, COLOR_WINDOW},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, VK_ESCAPE},
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
const VK_F5: u32 = 0x74;
const VK_F11: u32 = 0x7A;
const MAX_LOG_CHARS: i32 = 80_000;
const MAX_RUN_LOG_LINES: usize = 1000;
const LOG_SNAPSHOT_INTERVAL_MS: u64 = 160;
const RUN_HOTKEY: win7ui::HotKey = win7ui::HotKey::new(HOTKEY_RUN, VK_F5);
const STOP_HOTKEY: win7ui::HotKey = win7ui::HotKey::new(HOTKEY_STOP, VK_F11);

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
    Done { status: String },
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
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
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

    let status = win7ui::create_label(hwnd, "就绪", 10, 44, 400, 22);
    let script = win7ui::create_multiline_edit(
        hwnd,
        SAMPLE_SCRIPT,
        IDC_SCRIPT,
        10,
        70,
        650,
        620,
        false,
        true,
    );
    let log = win7ui::create_multiline_edit(hwnd, "", IDC_LOG, 670, 70, 420, 620, true, false);

    if let Some(app) = APP.get() {
        let mut app = app.lock().unwrap();
        app.hwnd = hwnd_value(hwnd);
        app.script = hwnd_value(script);
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
    }

    update_running_ui(false);
    layout_controls(hwnd);
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
        win7ui::move_window(to_hwnd(app.script), split.left_x, split.y, split.left_w, split.h);
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
        let (final_line, status) = match result {
            Ok(()) => ("运行完成。".to_string(), "运行完成。"),
            Err(RunError::Stopped) => ("运行已停止。".to_string(), "运行已停止。"),
            Err(err) => (format!("错误：{err}"), "运行出错。"),
        };
        push_tail_log(&mut tail_logs, &mut total_lines, final_line);
        send_log_snapshot(&tx, &tail_logs, total_lines);
        let _ = tx.send(AppEvent::Done {
            status: status.to_string(),
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

unsafe fn open_script() {
    let Some(path) = choose_script_file(false) else { return; };
    match fs::read_to_string(&path) {
        Ok(text) => {
            if let Some(app) = APP.get() {
                let mut app = app.lock().unwrap();
                win7ui::set_window_text(to_hwnd(app.script), &text);
                app.current_file = Some(path.clone());
            }
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

    win7ui::create_label(hwnd, "目录", 18, 20, 70, 22);
    let dir_edit =
        win7ui::create_single_line_edit(hwnd, "captures", IDC_CONFIRM_DIR, 90, 18, 430, 24);

    win7ui::create_label(hwnd, "文件名", 18, 55, 70, 22);
    let file_edit =
        win7ui::create_single_line_edit(hwnd, &file_name, IDC_CONFIRM_FILE, 90, 53, 430, 24);

    win7ui::create_label(hwnd, &selected_text, 90, 86, 430, 22);

    let mut threshold_edit = null_mut();
    let mut timeout_edit = null_mut();
    let mut y = 116;
    if mode == CaptureMode::ClickImage {
        win7ui::create_label(hwnd, "匹配阈值", 18, 92, 70, 22);
        threshold_edit =
            win7ui::create_single_line_edit(hwnd, "0.92", IDC_CONFIRM_THRESHOLD, 90, 90, 90, 24);
        win7ui::create_label(hwnd, "超时 ms", 205, 92, 70, 22);
        timeout_edit =
            win7ui::create_single_line_edit(hwnd, "3000", IDC_CONFIRM_TIMEOUT, 275, 90, 90, 24);
        y = 150;
    }

    win7ui::create_button_at(hwnd, "确认", IDC_CONFIRM_OK, 210, y, 82, 28);
    win7ui::create_button_at(hwnd, "重选", IDC_CONFIRM_RESELECT, 310, y, 82, 28);
    win7ui::create_button_at(hwnd, "取消", IDC_CONFIRM_CANCEL, 410, y, 82, 28);

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
                    "find_click(\"{}\", {:.2}, {})",
                    win7ui::script_path_literal(&path),
                    threshold,
                    timeout_ms
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
}

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
            AppEvent::Done { status } => {
                keep = false;
                if let Some(app) = APP.get() {
                    let mut app = app.lock().unwrap();
                    app.running = false;
                    app.stop_requested = None;
                }
                update_running_ui(false);
                set_status(&status);
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
