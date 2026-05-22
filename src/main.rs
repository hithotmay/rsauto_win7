use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
        Arc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use eframe::egui;
use egui::{text::LayoutJob, Color32, FontId, TextFormat};
use enigo::{Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use image::{imageops, DynamicImage, RgbaImage};
use screenshots::Screen;
use thiserror::Error;

const MAX_RUN_LOG_LINES: usize = 1000;
const LOG_SNAPSHOT_INTERVAL_MS: u64 = 80;

const SAMPLE_SCRIPT: &str = r#"# PyAuto Rust MVP
# 命令示例：
# click 500 300
# move 500 300
# screenshot output.png
# find image.png 0.92
# find_click image.png 0.92 3000
# sleep 500
#
# 中文别名：
# 点击坐标 500 300
# 移动鼠标 500 300
# 截图 output.png
# 查找图片 image.png 0.92
# 查找图片并点击 image.png 0.92 3000

screenshot output.png
"#;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([920.0, 560.0]),
        ..Default::default()
    };

    eframe::run_native(
        "PyAuto Rust MVP",
        options,
        Box::new(|cc| {
            configure_chinese_fonts(&cc.egui_ctx);
            Ok(Box::new(PyAutoApp::default()))
        }),
    )
}

fn configure_chinese_fonts(ctx: &egui::Context) {
    let candidates = [
        r"C:\Windows\Fonts\NotoSansSC-VF.ttf",
        r"C:\Windows\Fonts\msyh.ttc",
        r"C:\Windows\Fonts\simhei.ttf",
        r"C:\Windows\Fonts\simsun.ttc",
    ];

    let Some(bytes) = candidates.iter().find_map(|path| fs::read(path).ok()) else {
        return;
    };

    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "chinese".to_owned(),
        egui::FontData::from_owned(bytes).into(),
    );

    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, "chinese".to_owned());
    }

    ctx.set_fonts(fonts);

    let mut style = (*ctx.style()).clone();
    style
        .text_styles
        .insert(egui::TextStyle::Monospace, FontId::monospace(15.0));
    style
        .text_styles
        .insert(egui::TextStyle::Body, FontId::proportional(14.0));
    ctx.set_style(style);
}

struct PyAutoApp {
    script: String,
    logs: Vec<String>,
    running: bool,
    stop_requested: Option<Arc<AtomicBool>>,
    rx: Option<Receiver<AppEvent>>,
    hotkey_rx: Option<Receiver<AppEvent>>,
    hotkeys_started: bool,
    capture: Option<CaptureSession>,
}

impl Default for PyAutoApp {
    fn default() -> Self {
        Self {
            script: SAMPLE_SCRIPT.to_string(),
            logs: vec!["就绪。输入命令后点击“运行”。".to_string()],
            running: false,
            stop_requested: None,
            rx: None,
            hotkey_rx: None,
            hotkeys_started: false,
            capture: None,
        }
    }
}

impl eframe::App for PyAutoApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ensure_hotkeys(ctx);
        self.drain_hotkey_events();
        self.drain_events(ctx);

        if self.capture.is_some() {
            self.render_fullscreen_capture(ctx);
            return;
        }

        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Python 办公助手 Rust MVP");
                ui.separator();

                if ui
                    .add_enabled(!self.running, egui::Button::new("运行"))
                    .clicked()
                {
                    self.start_run();
                }

                if ui
                    .add_enabled(self.running, egui::Button::new("停止"))
                    .clicked()
                {
                    self.stop_run();
                }

                if ui.button("框选截图").clicked() {
                    self.begin_screen_capture(CaptureMode::SaveRegion, ui.ctx());
                }

                if ui.button("点击截图").clicked() {
                    self.begin_screen_capture(CaptureMode::ClickImage, ui.ctx());
                }

                if ui.button("清空日志").clicked() {
                    self.logs.clear();
                }

                if ui.button("示例脚本").clicked() {
                    self.script = SAMPLE_SCRIPT.to_string();
                }

                ui.separator();
                ui.label(if self.running { "运行中" } else { "空闲" });
            });
        });

        egui::SidePanel::right("help")
            .resizable(true)
            .default_width(320.0)
            .show(ctx, |ui| {
                ui.heading("命令");
                ui.separator();
                ui.monospace("click x y");
                ui.monospace("move x y");
                ui.monospace("screenshot path.png");
                ui.monospace("find path.png [threshold]");
                ui.monospace("find_click path.png [threshold] [timeout_ms]");
                ui.monospace("sleep ms");
                ui.add_space(8.0);
                ui.label("Python-style syntax:");
                ui.monospace("x = 100");
                ui.monospace("print(\"x\", x)");
                ui.monospace("def task(a, b):");
                ui.monospace("for i in range(3):");
                ui.monospace("while x < 5:");
                ui.monospace("label start / start: / goto start");
                ui.add_space(10.0);
                ui.label("支持中文别名：");
                ui.monospace("点击坐标 x y");
                ui.monospace("移动鼠标 x y");
                ui.monospace("截图 path.png");
                ui.monospace("查找图片 path.png 0.92");
                ui.monospace("查找图片并点击 path.png 0.92 3000");
                ui.add_space(10.0);
                ui.label("工具按钮：");
                ui.label("框选截图：隐藏界面后进入全屏框选，确认后保存截图。");
                ui.label("点击截图：隐藏界面后进入全屏框选，确认后保存模板图并插入点击图片代码。");
            });

        egui::TopBottomPanel::bottom("log")
            .resizable(true)
            .default_height(220.0)
            .show(ctx, |ui| {
                ui.heading("运行日志");
                ui.separator();
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in &self.logs {
                            ui.monospace(line);
                        }
                    });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("脚本编辑器");
            ui.separator();
            let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap_width: f32| {
                let mut job = highlight_script(text.as_str(), ui.visuals().dark_mode);
                job.wrap.max_width = wrap_width;
                ui.fonts_mut(|fonts| fonts.layout_job(job))
            };
            ui.add(
                egui::TextEdit::multiline(&mut self.script)
                    .code_editor()
                    .layouter(&mut layouter)
                    .desired_rows(28)
                    .lock_focus(true)
                    .desired_width(f32::INFINITY),
            );
        });

        if self.running {
            ctx.request_repaint_after(Duration::from_millis(80));
        }
    }
}

impl PyAutoApp {
    fn ensure_hotkeys(&mut self, ctx: &egui::Context) {
        if self.hotkeys_started {
            return;
        }
        self.hotkeys_started = true;

        let (tx, rx) = mpsc::channel();
        self.hotkey_rx = Some(rx);
        start_global_hotkey_listener(tx, ctx.clone());
        self.logs
            .push("全局快捷键已启用：F5 运行，F11 停止。".to_string());
    }

    fn drain_hotkey_events(&mut self) {
        let Some(rx) = self.hotkey_rx.take() else {
            return;
        };

        while let Ok(event) = rx.try_recv() {
            match event {
                AppEvent::StartRequested => {
                    if self.running {
                        self.logs
                            .push("F5：脚本正在运行，已忽略重复运行请求。".to_string());
                    } else {
                        self.start_run();
                    }
                }
                AppEvent::StopRequested => self.stop_run(),
                AppEvent::Log(line) => self.append_log(line),
                AppEvent::ReplaceRunLog { .. } | AppEvent::RunDone | AppEvent::CaptureReady { .. } => {}
            }
        }

        self.hotkey_rx = Some(rx);
    }

    fn start_run(&mut self) {
        self.running = true;
        self.logs.clear();
        self.append_log("开始运行。".to_string());

        let script = self.script.clone();
        let stop_requested = Arc::new(AtomicBool::new(false));
        self.stop_requested = Some(stop_requested.clone());
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);

        thread::spawn(move || {
            let log_stop = stop_requested.clone();
            let mut tail_logs: VecDeque<String> = VecDeque::with_capacity(MAX_RUN_LOG_LINES);
            let mut total_lines = 0usize;
            let result = Runner::new(stop_requested.clone()).and_then(|mut runner| {
                let mut last_flush = Instant::now();
                runner.run_script(&script, |msg| {
                    if log_stop.load(Ordering::Relaxed) {
                        return;
                    }
                    push_tail_log(&mut tail_logs, &mut total_lines, msg);

                    if last_flush.elapsed() >= Duration::from_millis(LOG_SNAPSHOT_INTERVAL_MS) {
                        send_run_log_snapshot(&tx, &tail_logs, total_lines);
                        last_flush = Instant::now();
                    }
                })
            });

            let final_line = match result {
                Ok(()) => "运行完成。".to_string(),
                Err(RunError::Stopped) => "运行已停止。".to_string(),
                Err(err) => format!("错误：{err}"),
            };
            push_tail_log(&mut tail_logs, &mut total_lines, final_line);
            send_run_log_snapshot(&tx, &tail_logs, total_lines);
            let _ = tx.send(AppEvent::RunDone);
        });
    }

    fn stop_run(&mut self) {
        if let Some(stop_requested) = &self.stop_requested {
            stop_requested.store(true, Ordering::Relaxed);
            self.append_log("正在请求停止脚本...".to_string());
        }
    }

    fn begin_screen_capture(&mut self, mode: CaptureMode, ctx: &egui::Context) {
        self.logs.push("正在最小化窗口并准备截图...".to_string());
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        ctx.request_repaint_after(Duration::from_millis(320));

        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let repaint_ctx = ctx.clone();

        thread::spawn(move || {
            thread::sleep(Duration::from_millis(520));
            let result = capture_primary_screen_with_info().map_err(|err| err.to_string());
            repaint_ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            repaint_ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
            repaint_ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
            let _ = tx.send(AppEvent::CaptureReady { mode, result });
            repaint_ctx.request_repaint();
        });
    }

    fn render_fullscreen_capture(&mut self, ctx: &egui::Context) {
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.exit_capture(ctx);
            return;
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(egui::Color32::BLACK))
            .show(ctx, |ui| {
                self.render_capture_canvas(ui);
            });

        self.render_confirm_panel(ctx);
    }

    fn render_capture_canvas(&mut self, ui: &mut egui::Ui) {
        let Some(session) = &mut self.capture else {
            return;
        };

        let texture = session.texture.get_or_insert_with(|| {
            ui.ctx().load_texture(
                "fullscreen_screen_capture",
                rgba_to_color_image(&session.image),
                egui::TextureOptions::LINEAR,
            )
        });

        let display_size = fit_size(
            egui::vec2(session.image.width() as f32, session.image.height() as f32),
            ui.available_size(),
        );
        let image = egui::Image::new((texture.id(), texture.size_vec2()))
            .fit_to_exact_size(display_size)
            .sense(egui::Sense::click_and_drag());

        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() - display_size.y).max(0.0) / 2.0);
            let response = ui.add(image);
            session.last_image_rect = Some(response.rect);

            if session.stage == CaptureStage::Selecting {
                if response.drag_started() {
                    session.drag_start = response.interact_pointer_pos();
                    session.drag_end = session.drag_start;
                    session.selected_image_rect = None;
                }
                if response.dragged() {
                    session.drag_end = response.interact_pointer_pos();
                }
                if response.drag_stopped() {
                    session.freeze_selection();
                }
            }

            if let Some(rect) = session.selection_rect() {
                ui.painter().rect_stroke(
                    rect,
                    0.0,
                    egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(255, 64, 128)),
                    egui::StrokeKind::Outside,
                );
                ui.painter().rect_filled(
                    rect,
                    0.0,
                    egui::Color32::from_rgba_premultiplied(255, 64, 128, 28),
                );
            }
        });

        egui::Area::new("capture_hint".into())
            .fixed_pos(egui::pos2(24.0, 20.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_rgba_premultiplied(0, 0, 0, 168))
                    .corner_radius(6.0)
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.colored_label(egui::Color32::WHITE, "拖拽框选图片区域。Esc 取消。");
                    });
            });
    }

    fn render_confirm_panel(&mut self, ctx: &egui::Context) {
        let Some(session) = &mut self.capture else {
            return;
        };
        if session.stage != CaptureStage::Confirming {
            return;
        }

        let mut action: Option<ConfirmAction> = None;

        egui::Window::new(match session.mode {
            CaptureMode::SaveRegion => "保存框选截图",
            CaptureMode::ClickImage => "保存图片并插入点击代码",
        })
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.label("确认保存位置：");
            ui.horizontal(|ui| {
                ui.label("目录");
                ui.text_edit_singleline(&mut session.save_dir);
            });
            ui.horizontal(|ui| {
                ui.label("文件名");
                ui.text_edit_singleline(&mut session.file_name);
            });
            if let Some(rect) = session.selected_image_rect {
                ui.label(format!("选区尺寸：{} x {}", rect.width, rect.height));
            }
            if session.mode == CaptureMode::ClickImage {
                ui.horizontal(|ui| {
                    ui.label("匹配阈值");
                    ui.add(egui::DragValue::new(&mut session.threshold).range(0.1..=1.0));
                    ui.label("超时 ms");
                    ui.add(egui::DragValue::new(&mut session.timeout_ms).range(0..=60000));
                });
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("确认").clicked() {
                    action = Some(ConfirmAction::Confirm);
                }
                if ui.button("重新框选").clicked() {
                    action = Some(ConfirmAction::Reselect);
                }
                if ui.button("取消").clicked() {
                    action = Some(ConfirmAction::Cancel);
                }
            });
        });

        match action {
            Some(ConfirmAction::Confirm) => self.confirm_capture(ctx),
            Some(ConfirmAction::Reselect) => {
                if let Some(session) = &mut self.capture {
                    session.stage = CaptureStage::Selecting;
                    session.drag_start = None;
                    session.drag_end = None;
                    session.selected_image_rect = None;
                }
            }
            Some(ConfirmAction::Cancel) => self.exit_capture(ctx),
            None => {}
        }
    }

    fn confirm_capture(&mut self, ctx: &egui::Context) {
        let Some(session) = &self.capture else {
            return;
        };

        let path = session.output_path();
        let result = session.save_selection_to(&path);

        match result {
            Ok(()) => {
                self.logs.push(format!("已保存图片：{}", path.display()));
                if session.mode == CaptureMode::ClickImage {
                    let code = format!(
                        "查找图片并点击 \"{}\" {:.2} {}",
                        path.display(),
                        session.threshold,
                        session.timeout_ms
                    );
                    if !self.script.ends_with('\n') {
                        self.script.push('\n');
                    }
                    self.script.push_str(&code);
                    self.script.push('\n');
                    self.logs.push(format!("已插入代码：{code}"));
                }
                self.exit_capture(ctx);
            }
            Err(err) => self.logs.push(format!("保存失败：{err}")),
        }
    }

    fn exit_capture(&mut self, ctx: &egui::Context) {
        self.capture = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    }

    fn drain_events(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.rx.take() else {
            return;
        };

        let mut keep_rx = true;
        while let Ok(event) = rx.try_recv() {
            match event {
                AppEvent::Log(line) => self.append_log(line),
                AppEvent::ReplaceRunLog { lines, total_lines } => {
                    self.replace_run_log_snapshot(lines, total_lines);
                }
                AppEvent::RunDone => {
                    self.running = false;
                    self.stop_requested = None;
                }
                AppEvent::CaptureReady { mode, result } => {
                    keep_rx = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    match result {
                        Ok(captured) => {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
                            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                            self.logs.push("已进入全屏框选截图。".to_string());
                            self.capture = Some(CaptureSession::new(mode, captured));
                        }
                        Err(err) => {
                            self.logs.push(format!("截图失败：{err}"));
                        }
                    }
                }
                AppEvent::StartRequested | AppEvent::StopRequested => {}
            }
        }

        if keep_rx {
            self.rx = Some(rx);
        }
    }

    fn append_log(&mut self, line: String) {
        self.logs.push(line);
        if self.logs.len() > MAX_RUN_LOG_LINES + 1 {
            let overflow = self.logs.len() - MAX_RUN_LOG_LINES;
            self.logs.drain(0..overflow);
            if self.logs.first().map(|line| !line.starts_with('[')).unwrap_or(true) {
                self.logs.insert(
                    0,
                    format!("[日志超过 {MAX_RUN_LOG_LINES} 行，历史输出已省略]"),
                );
            }
        }
    }

    fn replace_run_log_snapshot(&mut self, lines: Vec<String>, total_lines: usize) {
        self.logs.clear();
        if total_lines > lines.len() {
            self.logs.push(format!(
                "[本次运行日志超过 {MAX_RUN_LOG_LINES} 行，历史输出已省略]"
            ));
        }
        self.logs.extend(lines);
    }
}

fn highlight_script(code: &str, dark_mode: bool) -> LayoutJob {
    let mut job = LayoutJob::default();
    let plain = script_text_format(SyntaxKind::Plain, dark_mode);
    for line in code.split_inclusive('\n') {
        highlight_line(&mut job, line, dark_mode);
    }
    if code.is_empty() {
        job.append("", 0.0, plain);
    }
    job
}

fn highlight_line(job: &mut LayoutJob, line: &str, dark_mode: bool) {
    let plain = script_text_format(SyntaxKind::Plain, dark_mode);
    let mut index = 0usize;
    let chars = line.char_indices().collect::<Vec<_>>();
    while index < chars.len() {
        let (start, ch) = chars[index];
        if ch == '#' {
            job.append(&line[start..], 0.0, script_text_format(SyntaxKind::Comment, dark_mode));
            return;
        }
        if ch == '"' || ch == '\'' {
            let end = string_end(line, start, ch);
            job.append(&line[start..end], 0.0, script_text_format(SyntaxKind::String, dark_mode));
            index = chars.partition_point(|(idx, _)| *idx < end);
            continue;
        }
        if ch.is_ascii_digit() {
            let end = token_end(line, start, |c| c.is_ascii_digit() || c == '.');
            job.append(&line[start..end], 0.0, script_text_format(SyntaxKind::Number, dark_mode));
            index = chars.partition_point(|(idx, _)| *idx < end);
            continue;
        }
        if is_ident_start(ch) {
            let end = token_end(line, start, is_ident_continue);
            let token = &line[start..end];
            let kind = if SCRIPT_KEYWORDS.contains(&token) {
                SyntaxKind::Keyword
            } else if SCRIPT_BUILTINS.contains(&token) {
                SyntaxKind::Builtin
            } else if SCRIPT_COMMANDS.contains(&token) {
                SyntaxKind::Command
            } else {
                SyntaxKind::Plain
            };
            job.append(token, 0.0, script_text_format(kind, dark_mode));
            index = chars.partition_point(|(idx, _)| *idx < end);
            continue;
        }
        let end = start + ch.len_utf8();
        job.append(&line[start..end], 0.0, plain.clone());
        index += 1;
    }
}

#[derive(Clone, Copy)]
enum SyntaxKind {
    Plain,
    Keyword,
    Builtin,
    Command,
    String,
    Number,
    Comment,
}

fn script_text_format(kind: SyntaxKind, dark_mode: bool) -> TextFormat {
    let color = match (kind, dark_mode) {
        (SyntaxKind::Plain, true) => Color32::from_rgb(220, 224, 232),
        (SyntaxKind::Plain, false) => Color32::from_rgb(34, 39, 46),
        (SyntaxKind::Keyword, true) => Color32::from_rgb(255, 139, 148),
        (SyntaxKind::Keyword, false) => Color32::from_rgb(190, 45, 60),
        (SyntaxKind::Builtin, true) => Color32::from_rgb(130, 170, 255),
        (SyntaxKind::Builtin, false) => Color32::from_rgb(30, 92, 190),
        (SyntaxKind::Command, true) => Color32::from_rgb(105, 210, 170),
        (SyntaxKind::Command, false) => Color32::from_rgb(20, 135, 95),
        (SyntaxKind::String, true) => Color32::from_rgb(230, 190, 120),
        (SyntaxKind::String, false) => Color32::from_rgb(145, 95, 20),
        (SyntaxKind::Number, true) => Color32::from_rgb(190, 150, 255),
        (SyntaxKind::Number, false) => Color32::from_rgb(115, 70, 175),
        (SyntaxKind::Comment, true) => Color32::from_rgb(130, 140, 150),
        (SyntaxKind::Comment, false) => Color32::from_rgb(105, 115, 125),
    };
    TextFormat {
        font_id: FontId::monospace(15.0),
        color,
        ..Default::default()
    }
}

fn string_end(line: &str, start: usize, quote: char) -> usize {
    let mut escaped = false;
    for (idx, ch) in line[start + quote.len_utf8()..].char_indices() {
        let absolute = start + quote.len_utf8() + idx;
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == quote {
            return absolute + ch.len_utf8();
        }
    }
    line.len()
}

fn token_end(line: &str, start: usize, keep: impl Fn(char) -> bool) -> usize {
    for (idx, ch) in line[start..].char_indices() {
        if !keep(ch) {
            return start + idx;
        }
    }
    line.len()
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_alphanumeric()
}

const SCRIPT_KEYWORDS: &[&str] = &[
    "def", "for", "in", "range", "while", "if", "elif", "else", "break", "continue", "goto",
    "label", "true", "false", "True", "False",
];

const SCRIPT_BUILTINS: &[&str] = &["print", "str", "int", "float", "bool", "type"];

const SCRIPT_COMMANDS: &[&str] = &[
    "click",
    "move",
    "key",
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

fn send_run_log_snapshot(
    tx: &mpsc::Sender<AppEvent>,
    tail_logs: &VecDeque<String>,
    total_lines: usize,
) {
    if tail_logs.is_empty() {
        return;
    }
    let _ = tx.send(AppEvent::ReplaceRunLog {
        lines: tail_logs.iter().cloned().collect(),
        total_lines,
    });
}

fn push_tail_log(tail_logs: &mut VecDeque<String>, total_lines: &mut usize, line: String) {
    *total_lines += 1;
    if tail_logs.len() >= MAX_RUN_LOG_LINES {
        tail_logs.pop_front();
    }
    tail_logs.push_back(line);
}

#[derive(Debug)]
enum AppEvent {
    Log(String),
    ReplaceRunLog {
        lines: Vec<String>,
        total_lines: usize,
    },
    RunDone,
    StartRequested,
    StopRequested,
    CaptureReady {
        mode: CaptureMode,
        result: Result<CapturedScreen, String>,
    },
}

#[cfg(target_os = "windows")]
fn start_global_hotkey_listener(tx: mpsc::Sender<AppEvent>, ctx: egui::Context) {
    thread::spawn(move || unsafe {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, MOD_NOREPEAT};
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY};

        const HOTKEY_RUN: i32 = 1;
        const HOTKEY_STOP: i32 = 2;
        const VK_F5: u32 = 0x74;
        const VK_F11: u32 = 0x7A;

        let run_ok = RegisterHotKey(std::ptr::null_mut(), HOTKEY_RUN, MOD_NOREPEAT, VK_F5) != 0;
        let stop_ok = RegisterHotKey(std::ptr::null_mut(), HOTKEY_STOP, MOD_NOREPEAT, VK_F11) != 0;

        if !run_ok {
            let _ = tx.send(AppEvent::Log(
                "F5 全局运行快捷键注册失败，可能被其他程序占用。".to_string(),
            ));
        }
        if !stop_ok {
            let _ = tx.send(AppEvent::Log(
                "F11 全局停止快捷键注册失败，可能被其他程序占用。".to_string(),
            ));
        }
        if !run_ok || !stop_ok {
            ctx.request_repaint();
        }
        if !run_ok && !stop_ok {
            return;
        }

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            if msg.message == WM_HOTKEY {
                match msg.wParam as i32 {
                    HOTKEY_RUN => {
                        let _ = tx.send(AppEvent::StartRequested);
                        ctx.request_repaint();
                    }
                    HOTKEY_STOP => {
                        let _ = tx.send(AppEvent::StopRequested);
                        ctx.request_repaint();
                    }
                    _ => {}
                }
            }
        }
    });
}

#[cfg(not(target_os = "windows"))]
fn start_global_hotkey_listener(tx: mpsc::Sender<AppEvent>, ctx: egui::Context) {
    let _ = tx.send(AppEvent::Log(
        "当前平台暂不支持全局 F5/F11 快捷键。".to_string(),
    ));
    ctx.request_repaint();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureMode {
    SaveRegion,
    ClickImage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureStage {
    Selecting,
    Confirming,
}

enum ConfirmAction {
    Confirm,
    Reselect,
    Cancel,
}

struct CaptureSession {
    mode: CaptureMode,
    stage: CaptureStage,
    image: RgbaImage,
    texture: Option<egui::TextureHandle>,
    save_dir: String,
    file_name: String,
    threshold: f32,
    timeout_ms: u64,
    drag_start: Option<egui::Pos2>,
    drag_end: Option<egui::Pos2>,
    last_image_rect: Option<egui::Rect>,
    selected_image_rect: Option<ImageRect>,
}

impl CaptureSession {
    fn new(mode: CaptureMode, captured: CapturedScreen) -> Self {
        let prefix = match mode {
            CaptureMode::SaveRegion => "screenshot",
            CaptureMode::ClickImage => "click_image",
        };

        Self {
            mode,
            stage: CaptureStage::Selecting,
            image: captured.image,
            texture: None,
            save_dir: "captures".to_string(),
            file_name: format!("{prefix}_{}.png", timestamp_for_file()),
            threshold: 0.92,
            timeout_ms: 3000,
            drag_start: None,
            drag_end: None,
            last_image_rect: None,
            selected_image_rect: None,
        }
    }

    fn output_path(&self) -> PathBuf {
        let mut file_name = self.file_name.trim().to_string();
        if Path::new(&file_name).extension().is_none() {
            file_name.push_str(".png");
        }
        PathBuf::from(self.save_dir.trim()).join(file_name)
    }

    fn selection_rect(&self) -> Option<egui::Rect> {
        let start = self.drag_start?;
        let end = self.drag_end?;
        let image_rect = self.last_image_rect?;
        let rect = egui::Rect::from_two_pos(start, end).intersect(image_rect);
        if rect.width() >= 2.0 && rect.height() >= 2.0 {
            Some(rect)
        } else {
            None
        }
    }

    fn freeze_selection(&mut self) {
        let Some(rect) = self.selection_rect() else {
            return;
        };
        let Some(image_rect) = self.last_image_rect else {
            return;
        };

        let x1 = preview_x_to_image(rect.min.x, image_rect, self.image.width());
        let y1 = preview_y_to_image(rect.min.y, image_rect, self.image.height());
        let x2 = preview_x_to_image(rect.max.x, image_rect, self.image.width());
        let y2 = preview_y_to_image(rect.max.y, image_rect, self.image.height());

        let left = x1.min(x2).min(self.image.width().saturating_sub(1));
        let top = y1.min(y2).min(self.image.height().saturating_sub(1));
        let right = x1.max(x2).min(self.image.width());
        let bottom = y1.max(y2).min(self.image.height());
        let width = right.saturating_sub(left).max(1);
        let height = bottom.saturating_sub(top).max(1);

        self.selected_image_rect = Some(ImageRect {
            left,
            top,
            width,
            height,
        });
        self.stage = CaptureStage::Confirming;
    }

    fn save_selection_to(&self, path: &Path) -> Result<(), RunError> {
        let selected = self
            .selected_image_rect
            .ok_or_else(|| RunError::Capture("请先拖拽选择截图区域。".to_string()))?;

        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|err| RunError::Capture(err.to_string()))?;
            }
        }

        let cropped = imageops::crop_imm(
            &self.image,
            selected.left,
            selected.top,
            selected.width,
            selected.height,
        )
        .to_image();
        cropped.save(path)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
struct ImageRect {
    left: u32,
    top: u32,
    width: u32,
    height: u32,
}

fn timestamp_for_file() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn preview_x_to_image(x: f32, rect: egui::Rect, image_width: u32) -> u32 {
    let t = ((x - rect.min.x) / rect.width()).clamp(0.0, 1.0);
    (t * image_width as f32).round() as u32
}

fn preview_y_to_image(y: f32, rect: egui::Rect, image_height: u32) -> u32 {
    let t = ((y - rect.min.y) / rect.height()).clamp(0.0, 1.0);
    (t * image_height as f32).round() as u32
}

fn fit_size(image_size: egui::Vec2, max_size: egui::Vec2) -> egui::Vec2 {
    let scale = (max_size.x / image_size.x)
        .min(max_size.y / image_size.y)
        .min(1.0)
        .max(0.05);
    image_size * scale
}

fn rgba_to_color_image(image: &RgbaImage) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        image.as_raw(),
    )
}

struct Runner {
    enigo: Enigo,
    stop_requested: Arc<AtomicBool>,
}

impl Runner {
    fn new(stop_requested: Arc<AtomicBool>) -> Result<Self, RunError> {
        Ok(Self {
            enigo: Enigo::new(&Settings::default())?,
            stop_requested,
        })
    }

    fn check_stop(&self) -> Result<(), RunError> {
        if self.stop_requested.load(Ordering::Relaxed) {
            Err(RunError::Stopped)
        } else {
            Ok(())
        }
    }

    #[allow(unreachable_code)]
    fn run_script<F>(&mut self, script: &str, mut log: F) -> Result<(), RunError>
    where
        F: FnMut(String),
    {
        let mut interpreter = ScriptInterpreter::new(script)?;
        return interpreter.run(self, &mut log);

        for (idx, raw_line) in script.lines().enumerate() {
            let line_no = idx + 1;
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            log(format!("[第 {line_no} 行] {line}"));
            let command = parse_command(line).map_err(|err| RunError::Line {
                line: line_no,
                source: Box::new(err),
            })?;

            self.run_command(command, &mut log)
                .map_err(|err| RunError::Line {
                    line: line_no,
                    source: Box::new(err),
                })?;
        }
        Ok(())
    }

    fn run_command<F>(&mut self, command: Command, log: &mut F) -> Result<(), RunError>
    where
        F: FnMut(String),
    {
        self.check_stop()?;
        match command {
            Command::Click { x, y } => {
                self.enigo.move_mouse(x, y, Coordinate::Abs)?;
                self.enigo.button(Button::Left, Direction::Click)?;
                log(format!("已点击坐标 ({x}, {y})"));
            }
            Command::Move { x, y } => {
                self.enigo.move_mouse(x, y, Coordinate::Abs)?;
                log(format!("已移动鼠标到 ({x}, {y})"));
            }
            Command::Key { text } => {
                for ch in text.chars() {
                    self.enigo.key(Key::Unicode(ch), Direction::Click)?;
                }
                log(format!("已输入文本 {text:?}"));
            }
            Command::Sleep { ms } => {
                let deadline = Instant::now() + Duration::from_millis(ms);
                while Instant::now() < deadline {
                    self.check_stop()?;
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    thread::sleep(remaining.min(Duration::from_millis(50)));
                }
                log(format!("已等待 {ms} ms"));
            }
            Command::Screenshot { path } => {
                let captured = capture_primary_screen_with_info()?;
                captured.image.save(&path)?;
                log(format!("截图已保存：{}", path.display()));
            }
            Command::Find { image, threshold } => {
                let captured = capture_primary_screen_with_info()?;
                let needle = load_template(&image)?;
                let prepared = PreparedTemplate::new(&needle)?;
                let found = find_prepared_template(
                    &captured.image,
                    &prepared,
                    threshold,
                    Some(self.stop_requested.as_ref()),
                )?;
                log(format!(
                    "找到图片 {}，坐标 ({}, {})，匹配度 {:.4}",
                    image.display(),
                    found.x + captured.screen_x,
                    found.y + captured.screen_y,
                    found.score
                ));
            }
            Command::FindClick {
                image,
                threshold,
                timeout_ms,
            } => {
                let deadline = Instant::now() + Duration::from_millis(timeout_ms);
                let needle = load_template(&image)?;
                let prepared = PreparedTemplate::new(&needle)?;
                loop {
                    self.check_stop()?;
                    let captured = capture_primary_screen_with_info()?;
                    match find_prepared_template(
                        &captured.image,
                        &prepared,
                        threshold,
                        Some(self.stop_requested.as_ref()),
                    ) {
                        Ok(found) => {
                            let image_cx = found.x + (needle.width() as i32 / 2);
                            let image_cy = found.y + (needle.height() as i32 / 2);
                            let (cx, cy) = captured.image_point_to_screen(image_cx, image_cy);
                            self.enigo.move_mouse(cx, cy, Coordinate::Abs)?;
                            self.enigo.button(Button::Left, Direction::Click)?;
                            log(format!(
                                "找到并点击图片 {}，坐标 ({cx}, {cy})，匹配度 {:.4}",
                                image.display(),
                                found.score
                            ));
                            break;
                        }
                        Err(err) if Instant::now() < deadline => {
                            log(format!("暂未找到：{err}"));
                            for _ in 0..5 {
                                self.check_stop()?;
                                thread::sleep(Duration::from_millis(50));
                            }
                        }
                        Err(err) => return Err(err),
                    }
                }
            }
        }

        Ok(())
    }
}

#[derive(Clone)]
struct ScriptLine {
    line_no: usize,
    indent: usize,
    text: String,
}

#[derive(Clone, Copy)]
struct FunctionDef {
    body_start: usize,
    body_end: usize,
}

struct ForRange {
    current: i64,
    stop: i64,
    step: i64,
}

impl Iterator for ForRange {
    type Item = i64;

    fn next(&mut self) -> Option<Self::Item> {
        let keep_going = if self.step > 0 {
            self.current < self.stop
        } else {
            self.current > self.stop
        };
        if !keep_going {
            return None;
        }

        let value = self.current;
        self.current = self.current.saturating_add(self.step);
        Some(value)
    }
}

#[derive(Clone, Debug)]
enum Value {
    Number(f64),
    Text(String),
    Bool(bool),
}

impl Value {
    fn as_bool(&self) -> bool {
        match self {
            Value::Number(value) => *value != 0.0,
            Value::Text(value) => !value.is_empty(),
            Value::Bool(value) => *value,
        }
    }

    fn to_script_string(&self) -> String {
        match self {
            Value::Number(value) if value.fract() == 0.0 => format!("{}", *value as i64),
            Value::Number(value) => format!("{value}"),
            Value::Text(value) => value.clone(),
            Value::Bool(value) => value.to_string(),
        }
    }

    fn type_name(&self) -> &'static str {
        match self {
            Value::Number(value) if value.fract() == 0.0 => "int",
            Value::Number(_) => "float",
            Value::Text(_) => "str",
            Value::Bool(_) => "bool",
        }
    }

    fn to_int(&self, line_no: usize) -> Result<i64, RunError> {
        match self {
            Value::Number(value) => Ok(*value as i64),
            Value::Bool(value) => Ok(if *value { 1 } else { 0 }),
            Value::Text(value) => value
                .trim()
                .parse::<i64>()
                .map_err(|_| line_error(line_no, &format!("无法转换为 int：{value}"))),
        }
    }

    fn to_float(&self, line_no: usize) -> Result<f64, RunError> {
        match self {
            Value::Number(value) => Ok(*value),
            Value::Bool(value) => Ok(if *value { 1.0 } else { 0.0 }),
            Value::Text(value) => value
                .trim()
                .parse::<f64>()
                .map_err(|_| line_error(line_no, &format!("无法转换为 float：{value}"))),
        }
    }
}

enum Flow {
    Continue,
    Break,
    ContinueLoop,
    Goto(String),
}

struct ScriptInterpreter {
    lines: Vec<ScriptLine>,
    scopes: Vec<HashMap<String, Value>>,
    labels: HashMap<String, usize>,
    functions: HashMap<String, FunctionDef>,
    steps: usize,
}

impl ScriptInterpreter {
    fn new(script: &str) -> Result<Self, RunError> {
        let mut lines = Vec::new();
        for (idx, raw_line) in script.lines().enumerate() {
            let Some(text) = strip_comment(raw_line).map(str::trim_end) else {
                continue;
            };
            if text.trim().is_empty() {
                continue;
            }
            lines.push(ScriptLine {
                line_no: idx + 1,
                indent: count_indent(raw_line),
                text: text.trim().to_string(),
            });
        }

        let mut interpreter = Self {
            lines,
            scopes: vec![HashMap::new()],
            labels: HashMap::new(),
            functions: HashMap::new(),
            steps: 0,
        };
        interpreter.index_symbols()?;
        Ok(interpreter)
    }

    fn run<F>(&mut self, runner: &mut Runner, log: &mut F) -> Result<(), RunError>
    where
        F: FnMut(String),
    {
        let mut pc = 0;
        while pc < self.lines.len() {
            runner.check_stop()?;
            match self.execute_at(pc, runner, log)? {
                (Flow::Continue, next) => pc = next,
                (Flow::Goto(label), _) => pc = self.goto_target(&label, self.lines[pc].line_no)?,
                (Flow::Break, _) => return Err(line_error(self.lines[pc].line_no, "break 只能在循环内使用")),
                (Flow::ContinueLoop, _) => {
                    return Err(line_error(self.lines[pc].line_no, "continue 只能在循环内使用"));
                }
            }
        }
        Ok(())
    }

    fn index_symbols(&mut self) -> Result<(), RunError> {
        for i in 0..self.lines.len() {
            let line = &self.lines[i];
            if let Some(name) = parse_label_name(&line.text) {
                self.labels.insert(name.to_string(), i);
                continue;
            }
            if let Some(name) = parse_def_name(&line.text) {
                let (body_start, body_end) = self.block_bounds(i)?;
                self.functions.insert(
                    name.to_string(),
                    FunctionDef {
                        body_start,
                        body_end,
                    },
                );
            }
        }
        Ok(())
    }

    fn block_bounds(&self, header: usize) -> Result<(usize, usize), RunError> {
        let base_indent = self.lines[header].indent;
        let body_start = header + 1;
        if body_start >= self.lines.len() || self.lines[body_start].indent <= base_indent {
            return Err(RunError::Line {
                line: self.lines[header].line_no,
                source: Box::new(RunError::Parse("缺少缩进代码块".to_string())),
            });
        }
        let mut body_end = body_start;
        while body_end < self.lines.len() && self.lines[body_end].indent > base_indent {
            body_end += 1;
        }
        Ok((body_start, body_end))
    }

    fn execute_block<F>(
        &mut self,
        start: usize,
        end: usize,
        runner: &mut Runner,
        log: &mut F,
    ) -> Result<Flow, RunError>
    where
        F: FnMut(String),
    {
        let mut pc = start;
        while pc < end {
            runner.check_stop()?;
            match self.execute_at(pc, runner, log)? {
                (Flow::Continue, next) => pc = next,
                (flow @ (Flow::Break | Flow::ContinueLoop | Flow::Goto(_)), _) => return Ok(flow),
            }
        }
        Ok(Flow::Continue)
    }

    fn execute_at<F>(
        &mut self,
        pc: usize,
        runner: &mut Runner,
        log: &mut F,
    ) -> Result<(Flow, usize), RunError>
    where
        F: FnMut(String),
    {
        runner.check_stop()?;
        self.steps += 1;
        if self.steps > 200_000 {
            return Err(RunError::Line {
                line: self.lines[pc].line_no,
                source: Box::new(RunError::Parse("脚本步数过多，可能存在死循环".to_string())),
            });
        }

        let line = self.lines[pc].clone();
        let text = line.text.as_str();
        if parse_label_name(text).is_some() {
            return Ok((Flow::Continue, pc + 1));
        }
        if text.starts_with("def ") {
            let (_, end) = self.block_bounds(pc)?;
            return Ok((Flow::Continue, end));
        }
        if text.starts_with("if ") {
            return self.execute_if_chain(pc, runner, log);
        }
        if text.starts_with("elif ") || text == "else:" {
            return Err(line_error(
                line.line_no,
                "elif/else 必须跟在同缩进的 if 后面",
            ));
        }
        if let Some(label) = text.strip_prefix("goto ") {
            return Ok((Flow::Goto(label.trim().to_string()), pc + 1));
        }
        if text == "break" {
            return Ok((Flow::Break, pc + 1));
        }
        if text == "continue" {
            return Ok((Flow::ContinueLoop, pc + 1));
        }
        if text.starts_with("for ") {
            let (var, values) = self.parse_for(text, line.line_no)?;
            let (body_start, body_end) = self.block_bounds(pc)?;
            for value in values {
                runner.check_stop()?;
                self.set_var(&var, Value::Number(value as f64));
                match self.execute_block(body_start, body_end, runner, log)? {
                    Flow::Continue => {}
                    Flow::ContinueLoop => continue,
                    Flow::Break => break,
                    flow @ Flow::Goto(_) => return Ok((flow, pc + 1)),
                }
            }
            return Ok((Flow::Continue, body_end));
        }
        if text.starts_with("while ") {
            let condition = text
                .strip_prefix("while ")
                .and_then(|value| value.strip_suffix(':'))
                .ok_or_else(|| line_error(line.line_no, "while 语句需要以冒号结尾"))?;
            let (body_start, body_end) = self.block_bounds(pc)?;
            let mut loop_count = 0usize;
            while self.eval_expr(condition, line.line_no)?.as_bool() {
                runner.check_stop()?;
                loop_count += 1;
                if loop_count > 100_000 {
                    return Err(line_error(line.line_no, "while 循环次数过多"));
                }
                match self.execute_block(body_start, body_end, runner, log)? {
                    Flow::Continue => {}
                    Flow::ContinueLoop => continue,
                    Flow::Break => break,
                    flow @ Flow::Goto(_) => return Ok((flow, pc + 1)),
                }
            }
            return Ok((Flow::Continue, body_end));
        }

        self.execute_statement(text, line.line_no, runner, log)?;
        Ok((Flow::Continue, pc + 1))
    }

    fn execute_if_chain<F>(
        &mut self,
        start: usize,
        runner: &mut Runner,
        log: &mut F,
    ) -> Result<(Flow, usize), RunError>
    where
        F: FnMut(String),
    {
        let base_indent = self.lines[start].indent;
        let mut branch = start;
        loop {
            let line = self.lines[branch].clone();
            let condition = if let Some(condition) = line
                .text
                .strip_prefix("if ")
                .or_else(|| line.text.strip_prefix("elif "))
            {
                Some(condition.strip_suffix(':').ok_or_else(|| {
                    line_error(line.line_no, "if/elif 语句需要以冒号结尾")
                })?)
            } else if line.text == "else:" {
                None
            } else {
                return Err(line_error(line.line_no, "无效的 if/elif/else 分支"));
            };

            let (body_start, body_end) = self.block_bounds(branch)?;
            let chain_end = self.if_chain_end(body_end, base_indent)?;
            let should_run = match condition {
                Some(condition) => self.eval_expr(condition, line.line_no)?.as_bool(),
                None => true,
            };
            if should_run {
                let flow = self.execute_block(body_start, body_end, runner, log)?;
                return Ok((flow, chain_end));
            }

            if let Some(next_branch) = self.next_if_branch(body_end, base_indent) {
                branch = next_branch;
            } else {
                return Ok((Flow::Continue, chain_end));
            }
        }
    }

    fn next_if_branch(&self, index: usize, base_indent: usize) -> Option<usize> {
        let line = self.lines.get(index)?;
        (line.indent == base_indent
            && (line.text.starts_with("elif ") || line.text == "else:"))
            .then_some(index)
    }

    fn if_chain_end(&self, index: usize, base_indent: usize) -> Result<usize, RunError> {
        let mut end = index;
        while let Some(branch) = self.next_if_branch(end, base_indent) {
            let (_, body_end) = self.block_bounds(branch)?;
            end = body_end;
        }
        Ok(end)
    }

    fn execute_statement<F>(
        &mut self,
        text: &str,
        line_no: usize,
        runner: &mut Runner,
        log: &mut F,
    ) -> Result<(), RunError>
    where
        F: FnMut(String),
    {
        if let Some(args) = call_args(text, "print") {
            let values = split_args(args)
                .into_iter()
                .map(|arg| {
                    self.eval_expr(&arg, line_no)
                        .map(|value| value.to_script_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            log(values.join(" "));
            return Ok(());
        }

        if let Some((name, expr)) = split_assignment(text) {
            let value = self.eval_expr(expr, line_no)?;
            self.set_var(name.trim(), value);
            return Ok(());
        }

        if let Some((name, args)) = parse_call(text) {
            if self.functions.contains_key(name) {
                let evaluated = split_args(args)
                    .into_iter()
                    .map(|arg| self.eval_expr(&arg, line_no))
                    .collect::<Result<Vec<_>, _>>()?;
                self.call_function(name, evaluated, runner, log, line_no)?;
                return Ok(());
            }

            let command_line = self.command_from_call(name, args, line_no)?;
            return self.run_command_line(&command_line, line_no, runner, log);
        }

        let command_line = self.resolve_command_line(text, line_no)?;
        self.run_command_line(&command_line, line_no, runner, log)
    }

    fn call_function<F>(
        &mut self,
        name: &str,
        args: Vec<Value>,
        runner: &mut Runner,
        log: &mut F,
        line_no: usize,
    ) -> Result<(), RunError>
    where
        F: FnMut(String),
    {
        let Some(function) = self.functions.get(name).copied() else {
            return Err(line_error(line_no, "未知函数"));
        };
        let params = parse_def_params(&self.lines[function.body_start - 1].text)?;
        if params.len() != args.len() {
            return Err(line_error(line_no, "函数参数数量不匹配"));
        }

        self.scopes.push(HashMap::new());
        for (param, value) in params.iter().zip(args) {
            self.set_var(param, value);
        }

        let result = self.execute_block(function.body_start, function.body_end, runner, log);
        self.scopes.pop();

        match result? {
            Flow::Continue => Ok(()),
            Flow::Break => Err(line_error(line_no, "函数内暂不支持 break 跳出外层循环")),
            Flow::ContinueLoop => Err(line_error(line_no, "函数内暂不支持 continue 跳过外层循环")),
            Flow::Goto(label) => Err(line_error(
                line_no,
                &format!("函数内 goto 暂不支持跳出到标签：{label}"),
            )),
        }
    }

    fn parse_for(&mut self, text: &str, line_no: usize) -> Result<(String, ForRange), RunError> {
        let body = text
            .strip_prefix("for ")
            .and_then(|value| value.strip_suffix(':'))
            .ok_or_else(|| line_error(line_no, "for 语句需要以冒号结尾"))?;
        let Some((var, range_expr)) = body.split_once(" in ") else {
            return Err(line_error(
                line_no,
                "for 语句格式应为：for i in range(...):",
            ));
        };
        let args = call_args(range_expr.trim(), "range")
            .ok_or_else(|| line_error(line_no, "for 目前支持 range(...)"))?;
        let nums = split_args(args)
            .into_iter()
            .map(|arg| self.eval_number(&arg, line_no).map(|value| value as i64))
            .collect::<Result<Vec<_>, _>>()?;
        let (start, stop, step) = match nums.as_slice() {
            [stop] => (0, *stop, 1),
            [start, stop] => (*start, *stop, 1),
            [start, stop, step] => (*start, *stop, *step),
            _ => return Err(line_error(line_no, "range 支持 1 到 3 个参数")),
        };
        if step == 0 {
            return Err(line_error(line_no, "range 的 step 不能为 0"));
        }

        Ok((
            var.trim().to_string(),
            ForRange {
                current: start,
                stop,
                step,
            },
        ))
    }

    fn command_from_call(
        &mut self,
        name: &str,
        args: &str,
        line_no: usize,
    ) -> Result<String, RunError> {
        let mut parts = vec![name.to_string()];
        for arg in split_args(args) {
            parts.push(self.eval_expr(&arg, line_no)?.to_script_string());
        }
        Ok(parts.join(" "))
    }

    fn resolve_command_line(&mut self, text: &str, line_no: usize) -> Result<String, RunError> {
        let tokens = split_command(text);
        if tokens.is_empty() {
            return Ok(String::new());
        }
        let mut resolved = vec![tokens[0].clone()];
        for token in tokens.iter().skip(1) {
            if self.get_var(token).is_some() || looks_like_expr(token) {
                resolved.push(self.eval_expr(token, line_no)?.to_script_string());
            } else {
                resolved.push(token.clone());
            }
        }
        Ok(resolved.join(" "))
    }

    fn run_command_line<F>(
        &mut self,
        command_line: &str,
        line_no: usize,
        runner: &mut Runner,
        log: &mut F,
    ) -> Result<(), RunError>
    where
        F: FnMut(String),
    {
        log(format!("[第 {line_no} 行] {command_line}"));
        let command = parse_command(command_line).map_err(|err| RunError::Line {
            line: line_no,
            source: Box::new(err),
        })?;
        runner
            .run_command(command, log)
            .map_err(|err| RunError::Line {
                line: line_no,
                source: Box::new(err),
            })
    }

    fn goto_target(&self, label: &str, line_no: usize) -> Result<usize, RunError> {
        self.labels
            .get(label)
            .copied()
            .map(|idx| idx + 1)
            .ok_or_else(|| line_error(line_no, &format!("未知标签：{label}")))
    }

    fn eval_number(&mut self, expr: &str, line_no: usize) -> Result<f64, RunError> {
        match self.eval_expr(expr, line_no)? {
            Value::Number(value) => Ok(value),
            Value::Bool(value) => Ok(if value { 1.0 } else { 0.0 }),
            Value::Text(_) => Err(line_error(line_no, "需要数字表达式")),
        }
    }

    fn get_var(&self, name: &str) -> Option<&Value> {
        self.scopes.iter().rev().find_map(|scope| scope.get(name))
    }

    fn set_var(&mut self, name: &str, value: Value) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), value);
        }
    }

    fn eval_expr(&mut self, expr: &str, line_no: usize) -> Result<Value, RunError> {
        let expr = expr.trim();
        if expr.is_empty() {
            return Err(line_error(line_no, "空表达式"));
        }
        if let Some(template) = parse_f_string(expr) {
            return Ok(Value::Text(self.eval_f_string(template, line_no)?));
        }
        if let Some(value) = parse_quoted(expr) {
            return Ok(Value::Text(value));
        }
        if let Some((name, args)) = parse_call(expr) {
            if let Some(value) = self.eval_builtin(name, args, line_no)? {
                return Ok(value);
            }
        }
        for op in ["==", "!=", ">=", "<=", ">", "<"] {
            if let Some((left, right)) = split_outside(expr, op) {
                let left = self.eval_expr(left, line_no)?;
                let right = self.eval_expr(right, line_no)?;
                return Ok(Value::Bool(compare_values(&left, &right, op)));
            }
        }
        if let Some(value) = self.eval_arithmetic(expr, line_no)? {
            return Ok(Value::Number(value));
        }
        if let Some(value) = self.get_var(expr) {
            return Ok(value.clone());
        }
        if expr.eq_ignore_ascii_case("true") {
            return Ok(Value::Bool(true));
        }
        if expr.eq_ignore_ascii_case("false") {
            return Ok(Value::Bool(false));
        }
        Ok(Value::Text(expr.to_string()))
    }

    fn eval_builtin(
        &mut self,
        name: &str,
        args: &str,
        line_no: usize,
    ) -> Result<Option<Value>, RunError> {
        let args = split_args(args);
        let name = name.trim();
        match name {
            "str" => {
                let arg = expect_one_arg(name, args.as_slice(), line_no)?;
                Ok(Some(Value::Text(
                    self.eval_expr(arg, line_no)?.to_script_string(),
                )))
            }
            "int" => {
                let arg = expect_one_arg(name, args.as_slice(), line_no)?;
                Ok(Some(Value::Number(
                    self.eval_expr(arg, line_no)?.to_int(line_no)? as f64,
                )))
            }
            "float" => {
                let arg = expect_one_arg(name, args.as_slice(), line_no)?;
                Ok(Some(Value::Number(
                    self.eval_expr(arg, line_no)?.to_float(line_no)?,
                )))
            }
            "bool" => {
                let arg = expect_one_arg(name, args.as_slice(), line_no)?;
                Ok(Some(Value::Bool(self.eval_expr(arg, line_no)?.as_bool())))
            }
            "type" => {
                let arg = expect_one_arg(name, args.as_slice(), line_no)?;
                Ok(Some(Value::Text(
                    self.eval_expr(arg, line_no)?.type_name().to_string(),
                )))
            }
            _ => Ok(None),
        }
    }

    fn eval_f_string(&mut self, template: &str, line_no: usize) -> Result<String, RunError> {
        let mut out = String::new();
        let mut chars = template.chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                '{' if chars.peek() == Some(&'{') => {
                    chars.next();
                    out.push('{');
                }
                '}' if chars.peek() == Some(&'}') => {
                    chars.next();
                    out.push('}');
                }
                '{' => {
                    let mut expr = String::new();
                    let mut found_end = false;
                    for inner in chars.by_ref() {
                        if inner == '}' {
                            found_end = true;
                            break;
                        }
                        expr.push(inner);
                    }
                    if !found_end {
                        return Err(line_error(line_no, "f-string 缺少 }"));
                    }
                    out.push_str(&self.eval_f_string_expr(&expr, line_no)?);
                }
                '}' => return Err(line_error(line_no, "f-string 多余的 }")),
                _ => out.push(ch),
            }
        }
        Ok(out)
    }

    fn eval_f_string_expr(&mut self, expr: &str, line_no: usize) -> Result<String, RunError> {
        let (expr, spec) = split_format_spec(expr);
        let value = self.eval_expr(expr, line_no)?;
        if let Some(spec) = spec {
            return format_value(&value, spec, line_no);
        }
        Ok(value.to_script_string())
    }

    fn eval_arithmetic(&mut self, expr: &str, line_no: usize) -> Result<Option<f64>, RunError> {
        if let Ok(value) = expr.parse::<f64>() {
            return Ok(Some(value));
        }
        for ops in [["+", "-"], ["*", "/"]] {
            if let Some((left, op, right)) = split_last_operator(expr, &ops) {
                let left = self.eval_number(left, line_no)?;
                let right = self.eval_number(right, line_no)?;
                return Ok(Some(match op {
                    "+" => left + right,
                    "-" => left - right,
                    "*" => left * right,
                    "/" => left / right,
                    _ => unreachable!(),
                }));
            }
        }
        if let Some(value) = self.get_var(expr) {
            return match value {
                Value::Number(value) => Ok(Some(*value)),
                Value::Bool(value) => Ok(Some(if *value { 1.0 } else { 0.0 })),
                Value::Text(_) => Ok(None),
            };
        }
        Ok(None)
    }
}

fn count_indent(line: &str) -> usize {
    line.chars()
        .take_while(|ch| *ch == ' ' || *ch == '\t')
        .map(|ch| if ch == '\t' { 4 } else { 1 })
        .sum()
}

fn strip_comment(line: &str) -> Option<&str> {
    let mut quote: Option<char> = None;
    for (idx, ch) in line.char_indices() {
        if ch == '"' || ch == '\'' {
            quote = if quote == Some(ch) {
                None
            } else if quote.is_none() {
                Some(ch)
            } else {
                quote
            };
        } else if ch == '#' && quote.is_none() {
            return Some(&line[..idx]);
        }
    }
    Some(line)
}

fn parse_label_name(text: &str) -> Option<&str> {
    if let Some(name) = text.strip_prefix("label ") {
        return Some(name.trim());
    }
    if text.starts_with("def ")
        || text.starts_with("for ")
        || text.starts_with("while ")
        || text.starts_with("if ")
        || text.starts_with("elif ")
        || text == "else:"
    {
        return None;
    }
    text.strip_suffix(':')
        .filter(|name| !name.contains(' ') && !name.is_empty())
}

fn parse_def_name(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("def ")?;
    let open = rest.find('(')?;
    Some(rest[..open].trim())
}

fn parse_def_params(text: &str) -> Result<Vec<String>, RunError> {
    let Some(args) = text
        .strip_prefix("def ")
        .and_then(|rest| rest.split_once('(').map(|(_, tail)| tail))
        .and_then(|tail| tail.strip_suffix(':'))
        .and_then(|tail| tail.strip_suffix(')'))
    else {
        return Err(RunError::Parse(
            "函数定义格式应为：def name(...):".to_string(),
        ));
    };
    Ok(split_args(args)
        .into_iter()
        .filter(|arg| !arg.trim().is_empty())
        .map(|arg| arg.trim().to_string())
        .collect())
}

fn call_args<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let rest = text.trim().strip_prefix(name)?.trim_start();
    rest.strip_prefix('(')?.strip_suffix(')')
}

fn parse_call(text: &str) -> Option<(&str, &str)> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    if close + 1 != text.len() {
        return None;
    }
    Some((text[..open].trim(), &text[open + 1..close]))
}

fn split_args(args: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut depth = 0_i32;
    for ch in args.chars() {
        match ch {
            '"' | '\'' => {
                quote = if quote == Some(ch) {
                    None
                } else if quote.is_none() {
                    Some(ch)
                } else {
                    quote
                };
                current.push(ch);
            }
            '(' if quote.is_none() => {
                depth += 1;
                current.push(ch);
            }
            ')' if quote.is_none() => {
                depth -= 1;
                current.push(ch);
            }
            ',' if quote.is_none() && depth == 0 => {
                out.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_string());
    }
    out
}

fn expect_one_arg<'a>(name: &str, args: &'a [String], line_no: usize) -> Result<&'a str, RunError> {
    if args.len() != 1 {
        return Err(line_error(
            line_no,
            &format!("{name}() 需要 1 个参数"),
        ));
    }
    Ok(args[0].as_str())
}

fn split_assignment(text: &str) -> Option<(&str, &str)> {
    if text.contains("==") || text.contains("!=") || text.contains(">=") || text.contains("<=") {
        return None;
    }
    let (left, right) = split_outside(text, "=")?;
    if left
        .trim()
        .chars()
        .all(|ch| ch == '_' || ch.is_alphanumeric())
    {
        Some((left, right))
    } else {
        None
    }
}

fn parse_quoted(text: &str) -> Option<String> {
    let text = text.trim();
    let quote = text.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    if !text.ends_with(quote) || text.len() < 2 {
        return None;
    }
    Some(text[quote.len_utf8()..text.len() - quote.len_utf8()].to_string())
}

fn parse_f_string(text: &str) -> Option<&str> {
    let text = text.trim();
    let rest = text.strip_prefix('f').or_else(|| text.strip_prefix('F'))?;
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    if !rest.ends_with(quote) || rest.len() < 2 {
        return None;
    }
    Some(&rest[quote.len_utf8()..rest.len() - quote.len_utf8()])
}

fn split_outside<'a>(text: &'a str, needle: &str) -> Option<(&'a str, &'a str)> {
    let mut quote: Option<char> = None;
    for (idx, ch) in text.char_indices() {
        if ch == '"' || ch == '\'' {
            quote = if quote == Some(ch) {
                None
            } else if quote.is_none() {
                Some(ch)
            } else {
                quote
            };
        }
        if quote.is_none() && text[idx..].starts_with(needle) {
            return Some((&text[..idx], &text[idx + needle.len()..]));
        }
    }
    None
}

fn split_format_spec(text: &str) -> (&str, Option<&str>) {
    let mut quote: Option<char> = None;
    let mut depth = 0_i32;
    for (idx, ch) in text.char_indices() {
        match ch {
            '"' | '\'' => {
                quote = if quote == Some(ch) {
                    None
                } else if quote.is_none() {
                    Some(ch)
                } else {
                    quote
                };
            }
            '(' if quote.is_none() => depth += 1,
            ')' if quote.is_none() => depth -= 1,
            ':' if quote.is_none() && depth == 0 => {
                return (&text[..idx], Some(text[idx + 1..].trim()));
            }
            _ => {}
        }
    }
    (text, None)
}

fn format_value(value: &Value, spec: &str, line_no: usize) -> Result<String, RunError> {
    if spec.is_empty() {
        return Ok(value.to_script_string());
    }
    let Some(precision) = spec.strip_prefix('.') else {
        return Err(line_error(
            line_no,
            &format!("暂不支持的 f-string 格式：{spec}"),
        ));
    };
    let precision = precision
        .strip_suffix('f')
        .unwrap_or(precision)
        .parse::<usize>()
        .map_err(|_| line_error(line_no, &format!("无效的 f-string 精度：{spec}")))?;
    match value {
        Value::Number(value) => Ok(format!("{value:.precision$}")),
        Value::Bool(value) => Ok(format!("{:.precision$}", if *value { 1.0 } else { 0.0 })),
        Value::Text(value) => value
            .parse::<f64>()
            .map(|value| format!("{value:.precision$}"))
            .map_err(|_| line_error(line_no, "f-string 数字格式需要数字值")),
    }
}

fn split_last_operator<'a>(
    text: &'a str,
    ops: &[&'static str],
) -> Option<(&'a str, &'static str, &'a str)> {
    let mut quote: Option<char> = None;
    for (idx, ch) in text.char_indices().rev() {
        if ch == '"' || ch == '\'' {
            quote = if quote == Some(ch) {
                None
            } else if quote.is_none() {
                Some(ch)
            } else {
                quote
            };
        }
        if quote.is_some() {
            continue;
        }
        for op in ops {
            if text[idx..].starts_with(op) && idx > 0 {
                return Some((&text[..idx], op, &text[idx + op.len()..]));
            }
        }
    }
    None
}

fn looks_like_expr(text: &str) -> bool {
    ["+", "-", "*", "/", "==", "!=", ">=", "<=", ">", "<"]
        .iter()
        .any(|op| text.contains(op))
}

fn compare_values(left: &Value, right: &Value, op: &str) -> bool {
    let pair = match (left, right) {
        (Value::Number(a), Value::Number(b)) => Some((*a, *b)),
        (Value::Bool(a), Value::Bool(b)) => Some((*a as i32 as f64, *b as i32 as f64)),
        _ => None,
    };
    if let Some((a, b)) = pair {
        return match op {
            "==" => (a - b).abs() <= f64::EPSILON,
            "!=" => (a - b).abs() > f64::EPSILON,
            ">" => a > b,
            "<" => a < b,
            ">=" => a >= b,
            "<=" => a <= b,
            _ => false,
        };
    }
    let a = left.to_script_string();
    let b = right.to_script_string();
    match op {
        "==" => a == b,
        "!=" => a != b,
        ">" => a > b,
        "<" => a < b,
        ">=" => a >= b,
        "<=" => a <= b,
        _ => false,
    }
}

fn line_error(line: usize, message: &str) -> RunError {
    RunError::Line {
        line,
        source: Box::new(RunError::Parse(message.to_string())),
    }
}

#[derive(Debug)]
enum Command {
    Click {
        x: i32,
        y: i32,
    },
    Move {
        x: i32,
        y: i32,
    },
    Key {
        text: String,
    },
    Sleep {
        ms: u64,
    },
    Screenshot {
        path: PathBuf,
    },
    Find {
        image: PathBuf,
        threshold: f32,
    },
    FindClick {
        image: PathBuf,
        threshold: f32,
        timeout_ms: u64,
    },
}

fn parse_command(line: &str) -> Result<Command, RunError> {
    let tokens = split_command(line);
    if tokens.is_empty() {
        return Err(RunError::Parse("空命令".to_string()));
    }

    let command = tokens[0].as_str();
    match command {
        "click" | "点击坐标" => Ok(Command::Click {
            x: parse_i32(&tokens, 1, "x")?,
            y: parse_i32(&tokens, 2, "y")?,
        }),
        "move" | "移动鼠标" => Ok(Command::Move {
            x: parse_i32(&tokens, 1, "x")?,
            y: parse_i32(&tokens, 2, "y")?,
        }),
        "type" | "输入文本" => Ok(Command::Key {
            text: tokens.get(1..).unwrap_or_default().join(" "),
        }),
        "sleep" | "等待" => Ok(Command::Sleep {
            ms: parse_u64(&tokens, 1, "ms")?,
        }),
        "screenshot" | "截图" => Ok(Command::Screenshot {
            path: parse_path(&tokens, 1, "path")?,
        }),
        "find" | "查找图片" => Ok(Command::Find {
            image: parse_path(&tokens, 1, "image")?,
            threshold: parse_optional_f32(&tokens, 2, 0.92)?,
        }),
        "find_click" | "查找图片并点击" => Ok(Command::FindClick {
            image: parse_path(&tokens, 1, "image")?,
            threshold: parse_optional_f32(&tokens, 2, 0.92)?,
            timeout_ms: parse_optional_u64(&tokens, 3, 0)?,
        }),
        _ => Err(RunError::Parse(format!("未知命令：{command}"))),
    }
}

fn split_command(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;

    for ch in line.chars() {
        match ch {
            '"' | '\'' => {
                quote = if quote == Some(ch) {
                    None
                } else if quote.is_none() {
                    Some(ch)
                } else {
                    quote
                };
            }
            ' ' | '\t' if quote.is_none() => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }

    if !current.is_empty() {
        out.push(current);
    }

    out
}

fn parse_i32(tokens: &[String], index: usize, name: &str) -> Result<i32, RunError> {
    tokens
        .get(index)
        .ok_or_else(|| RunError::Parse(format!("缺少参数 {name}")))?
        .parse()
        .map_err(|_| RunError::Parse(format!("参数 {name} 不是有效整数")))
}

fn parse_u64(tokens: &[String], index: usize, name: &str) -> Result<u64, RunError> {
    tokens
        .get(index)
        .ok_or_else(|| RunError::Parse(format!("缺少参数 {name}")))?
        .parse()
        .map_err(|_| RunError::Parse(format!("参数 {name} 不是有效整数")))
}

fn parse_optional_f32(tokens: &[String], index: usize, default: f32) -> Result<f32, RunError> {
    match tokens.get(index) {
        Some(value) => value
            .parse()
            .map_err(|_| RunError::Parse(format!("不是有效数字：{value}"))),
        None => Ok(default),
    }
}

fn parse_optional_u64(tokens: &[String], index: usize, default: u64) -> Result<u64, RunError> {
    match tokens.get(index) {
        Some(value) => value
            .parse()
            .map_err(|_| RunError::Parse(format!("不是有效整数：{value}"))),
        None => Ok(default),
    }
}

fn parse_path(tokens: &[String], index: usize, name: &str) -> Result<PathBuf, RunError> {
    tokens
        .get(index)
        .map(PathBuf::from)
        .ok_or_else(|| RunError::Parse(format!("缺少路径参数 {name}")))
}

#[derive(Debug)]
struct CapturedScreen {
    image: RgbaImage,
    screen_x: i32,
    screen_y: i32,
    screen_width: u32,
    screen_height: u32,
}

impl CapturedScreen {
    fn image_point_to_screen(&self, image_x: i32, image_y: i32) -> (i32, i32) {
        let image_w = self.image.width().max(1) as f32;
        let image_h = self.image.height().max(1) as f32;
        let x = self.screen_x as f32 + (image_x as f32 * self.screen_width as f32 / image_w);
        let y = self.screen_y as f32 + (image_y as f32 * self.screen_height as f32 / image_h);
        (x.round() as i32, y.round() as i32)
    }
}

fn capture_primary_screen_with_info() -> Result<CapturedScreen, RunError> {
    let screens = Screen::all().map_err(|err| RunError::Screen(err.to_string()))?;
    let screen = screens
        .first()
        .ok_or_else(|| RunError::Screen("未找到屏幕".to_string()))?;
    let image = screen
        .capture()
        .map_err(|err| RunError::Screen(err.to_string()))?;
    Ok(CapturedScreen {
        image,
        screen_x: screen.display_info.x,
        screen_y: screen.display_info.y,
        screen_width: screen.display_info.width,
        screen_height: screen.display_info.height,
    })
}

fn load_template(path: &Path) -> Result<RgbaImage, RunError> {
    let image = image::open(path)?;
    Ok(match image {
        DynamicImage::ImageRgba8(rgba) => rgba,
        other => other.to_rgba8(),
    })
}

#[derive(Debug, Clone, Copy)]
struct MatchResult {
    x: i32,
    y: i32,
    score: f32,
}

struct PreparedTemplate {
    width: u32,
    height: u32,
    gray: Vec<f32>,
    stats: TemplateStats,
    samples: Vec<TemplateSample>,
    sample_stats: TemplateStats,
}

struct TemplateSample {
    x: u32,
    y: u32,
    value: f32,
}

#[allow(dead_code)]
fn find_template(
    haystack: &RgbaImage,
    needle: &RgbaImage,
    threshold: f32,
) -> Result<MatchResult, RunError> {
    let (hw, hh) = haystack.dimensions();
    let (nw, nh) = needle.dimensions();

    if nw == 0 || nh == 0 {
        return Err(RunError::ImageSearch("模板图片为空".to_string()));
    }
    if nw > hw || nh > hh {
        return Err(RunError::ImageSearch(format!(
            "模板图片 {}x{} 大于截图 {}x{}",
            nw, nh, hw, hh
        )));
    }

    let haystack_gray = rgba_to_gray(haystack);
    let needle_gray = rgba_to_gray(needle);
    let needle_stats = TemplateStats::new(&needle_gray)?;

    let mut best = MatchResult {
        x: 0,
        y: 0,
        score: -1.0,
    };

    for y in 0..=(hh - nh) {
        for x in 0..=(hw - nw) {
            let score = template_score_normed(
                &haystack_gray,
                hw,
                &needle_gray,
                nw,
                nh,
                &needle_stats,
                x,
                y,
            );
            if score > best.score {
                best = MatchResult {
                    x: x as i32,
                    y: y as i32,
                    score,
                };
            }
        }
    }

    if best.score >= threshold {
        Ok(best)
    } else {
        Err(RunError::ImageSearch(format!(
            "未找到图片；最佳匹配度 {:.4}，阈值 {:.4}",
            best.score, threshold
        )))
    }
}

impl PreparedTemplate {
    fn new(needle: &RgbaImage) -> Result<Self, RunError> {
        let (width, height) = needle.dimensions();
        if width == 0 || height == 0 {
            return Err(RunError::ImageSearch("妯℃澘鍥剧墖涓虹┖".to_string()));
        }

        let gray = rgba_to_gray(needle);
        let stats = TemplateStats::new(&gray)?;
        let samples = build_template_samples(&gray, width, height);
        let sample_values = samples
            .iter()
            .map(|sample| sample.value)
            .collect::<Vec<_>>();
        let sample_stats = TemplateStats::new(&sample_values)?;

        Ok(Self {
            width,
            height,
            gray,
            stats,
            samples,
            sample_stats,
        })
    }
}

fn find_prepared_template(
    haystack: &RgbaImage,
    needle: &PreparedTemplate,
    threshold: f32,
    stop_requested: Option<&AtomicBool>,
) -> Result<MatchResult, RunError> {
    let (hw, hh) = haystack.dimensions();
    if needle.width > hw || needle.height > hh {
        return Err(RunError::ImageSearch(format!(
            "妯℃澘鍥剧墖 {}x{} 澶т簬鎴浘 {}x{}",
            needle.width, needle.height, hw, hh
        )));
    }

    let haystack_gray = rgba_to_gray(haystack);
    let candidates =
        find_template_candidates(&haystack_gray, hw, hh, needle, threshold, stop_requested)?;
    let mut best = MatchResult {
        x: 0,
        y: 0,
        score: -1.0,
    };

    for candidate in candidates {
        check_stop_flag(stop_requested)?;
        let score = template_score_normed(
            &haystack_gray,
            hw,
            &needle.gray,
            needle.width,
            needle.height,
            &needle.stats,
            candidate.x as u32,
            candidate.y as u32,
        );
        if score > best.score {
            best = MatchResult {
                x: candidate.x,
                y: candidate.y,
                score,
            };
        }
    }

    if best.score >= threshold {
        Ok(best)
    } else {
        Err(RunError::ImageSearch(format!(
            "鏈壘鍒板浘鐗囷紱鏈€浣冲尮閰嶅害 {:.4}锛岄槇鍊?{:.4}",
            best.score, threshold
        )))
    }
}

fn build_template_samples(gray: &[f32], width: u32, height: u32) -> Vec<TemplateSample> {
    let max_samples = 96_u32.min(width.saturating_mul(height)).max(1);
    let aspect = width as f32 / height.max(1) as f32;
    let cols = ((max_samples as f32 * aspect).sqrt().round() as u32).clamp(1, width.max(1));
    let rows = (max_samples / cols).max(1).min(height.max(1));

    let mut samples = Vec::new();
    for row in 0..rows {
        let y = ((row as f32 + 0.5) * height as f32 / rows as f32)
            .floor()
            .min((height - 1) as f32) as u32;
        for col in 0..cols {
            let x = ((col as f32 + 0.5) * width as f32 / cols as f32)
                .floor()
                .min((width - 1) as f32) as u32;
            let index = (y * width + x) as usize;
            samples.push(TemplateSample {
                x,
                y,
                value: gray[index],
            });
        }
    }
    samples
}

fn find_template_candidates(
    haystack: &[f32],
    haystack_width: u32,
    haystack_height: u32,
    needle: &PreparedTemplate,
    threshold: f32,
    stop_requested: Option<&AtomicBool>,
) -> Result<Vec<MatchResult>, RunError> {
    const MAX_FULL_CHECKS: usize = 64;

    let min_sample_score = (threshold - 0.08).max(0.50);
    let mut candidates = Vec::with_capacity(MAX_FULL_CHECKS);
    let mut best_sample = MatchResult {
        x: 0,
        y: 0,
        score: -1.0,
    };

    for y in 0..=(haystack_height - needle.height) {
        check_stop_flag(stop_requested)?;
        for x in 0..=(haystack_width - needle.width) {
            let score = sample_score_normed(haystack, haystack_width, needle, x, y);
            if score > best_sample.score {
                best_sample = MatchResult {
                    x: x as i32,
                    y: y as i32,
                    score,
                };
            }
            if score >= min_sample_score {
                push_top_candidate(
                    &mut candidates,
                    MatchResult {
                        x: x as i32,
                        y: y as i32,
                        score,
                    },
                    MAX_FULL_CHECKS,
                );
            }
        }
    }

    if candidates.is_empty() {
        candidates.push(best_sample);
    }
    Ok(candidates)
}

fn push_top_candidate(candidates: &mut Vec<MatchResult>, candidate: MatchResult, limit: usize) {
    if candidates.len() < limit {
        candidates.push(candidate);
        return;
    }

    let Some((lowest_index, lowest)) = candidates
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| a.score.total_cmp(&b.score))
    else {
        return;
    };

    if candidate.score > lowest.score {
        candidates[lowest_index] = candidate;
    }
}

fn check_stop_flag(stop_requested: Option<&AtomicBool>) -> Result<(), RunError> {
    if stop_requested
        .map(|flag| flag.load(Ordering::Relaxed))
        .unwrap_or(false)
    {
        Err(RunError::Stopped)
    } else {
        Ok(())
    }
}

struct TemplateStats {
    sum: f32,
    variance_term: f32,
}

impl TemplateStats {
    fn new(needle: &[f32]) -> Result<Self, RunError> {
        let n = needle.len() as f32;
        let sum: f32 = needle.iter().sum();
        let sum_sq: f32 = needle.iter().map(|value| value * value).sum();
        let variance_term = sum_sq - (sum * sum / n);
        if variance_term <= f32::EPSILON {
            return Err(RunError::ImageSearch(
                "模板图片颜色变化太少，无法可靠匹配".to_string(),
            ));
        }
        Ok(Self { sum, variance_term })
    }
}

fn rgba_to_gray(image: &RgbaImage) -> Vec<f32> {
    image
        .pixels()
        .map(|pixel| {
            let [r, g, b, _] = pixel.0;
            0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32
        })
        .collect()
}

fn sample_score_normed(
    haystack: &[f32],
    haystack_width: u32,
    needle: &PreparedTemplate,
    left: u32,
    top: u32,
) -> f32 {
    let mut hay_sum = 0.0_f32;
    let mut hay_sum_sq = 0.0_f32;
    let mut cross_sum = 0.0_f32;

    for sample in &needle.samples {
        let h = haystack[((top + sample.y) * haystack_width + left + sample.x) as usize];
        hay_sum += h;
        hay_sum_sq += h * h;
        cross_sum += h * sample.value;
    }

    let count = needle.samples.len() as f32;
    let hay_variance_term = hay_sum_sq - (hay_sum * hay_sum / count);
    if hay_variance_term <= f32::EPSILON {
        return -1.0;
    }

    let numerator = cross_sum - (hay_sum * needle.sample_stats.sum / count);
    let denominator = (hay_variance_term * needle.sample_stats.variance_term).sqrt();
    (numerator / denominator).clamp(-1.0, 1.0)
}

fn template_score_normed(
    haystack: &[f32],
    haystack_width: u32,
    needle: &[f32],
    needle_width: u32,
    needle_height: u32,
    needle_stats: &TemplateStats,
    left: u32,
    top: u32,
) -> f32 {
    let mut hay_sum = 0.0_f32;
    let mut hay_sum_sq = 0.0_f32;
    let mut cross_sum = 0.0_f32;

    for y in 0..needle_height {
        let hay_row = ((top + y) * haystack_width + left) as usize;
        let needle_row = (y * needle_width) as usize;
        for x in 0..needle_width {
            let h = haystack[hay_row + x as usize];
            let n = needle[needle_row + x as usize];

            hay_sum += h;
            hay_sum_sq += h * h;
            cross_sum += h * n;
        }
    }

    let count = (needle_width * needle_height) as f32;
    let hay_variance_term = hay_sum_sq - (hay_sum * hay_sum / count);
    if hay_variance_term <= f32::EPSILON {
        return -1.0;
    }

    let numerator = cross_sum - (hay_sum * needle_stats.sum / count);
    let denominator = (hay_variance_term * needle_stats.variance_term).sqrt();
    (numerator / denominator).clamp(-1.0, 1.0)
}

#[derive(Debug, Error)]
enum RunError {
    #[error("脚本已停止")]
    Stopped,
    #[error("第 {line} 行：{source}")]
    Line { line: usize, source: Box<RunError> },
    #[error("解析错误：{0}")]
    Parse(String),
    #[error("输入错误：{0}")]
    Input(#[from] enigo::InputError),
    #[error("设置错误：{0}")]
    Settings(#[from] enigo::NewConError),
    #[error("屏幕错误：{0}")]
    Screen(String),
    #[error("图片错误：{0}")]
    Image(#[from] image::ImageError),
    #[error("图片查找错误：{0}")]
    ImageSearch(String),
    #[error("截图工具错误：{0}")]
    Capture(String),
}
