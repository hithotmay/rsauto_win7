//! Cross-platform application logic.
//!
//! Script execution, event formatting, and other pure logic
//! shared by all platform backends. No HWND or platform imports.

use std::{
    collections::VecDeque,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use crate::core::{RunError, Runner};
use crate::ui::{app_common, EventSender};

/// Send a log snapshot event through the event channel.
pub fn send_log_snapshot<Tx: EventSender<app_common::AppEvent>>(
    tx: &Tx,
    tail_logs: &VecDeque<String>,
    total_lines: usize,
) {
    if tail_logs.is_empty() {
        return;
    }
    let _ = tx.send(app_common::AppEvent::ReplaceLog {
        lines: tail_logs.iter().cloned().collect(),
        total_lines,
    });
    tx.wake();
}

/// Run a script in a background thread, sending events through `tx`.
///
/// This is the core execution logic shared by all backends.
/// The caller provides an `EventSender` implementation that delivers
/// `AppEvent` messages to the UI thread.
pub fn run_script_thread<Tx: EventSender<app_common::AppEvent> + Send + 'static>(
    script: String,
    stop_flag: std::sync::Arc<AtomicBool>,
    tx: Tx,
) {
    std::thread::spawn(move || {
        let log_stop = stop_flag.clone();
        let mut tail_logs: VecDeque<String> = VecDeque::with_capacity(app_common::MAX_RUN_LOG_LINES);
        let mut total_lines = 0usize;
        let result = Runner::new(stop_flag).and_then(|mut runner| {
            let mut last_flush = Instant::now();
            let mut last_vars_flush = Instant::now();
            let vars_tx = tx.clone();

            runner.run_script(&script, |msg| {
                if log_stop.load(Ordering::Relaxed) {
                    return;
                }
                app_common::push_tail_log(&mut tail_logs, &mut total_lines, msg);

                if last_flush.elapsed() >= Duration::from_millis(app_common::LOG_SNAPSHOT_INTERVAL_MS) {
                    send_log_snapshot(&tx, &tail_logs, total_lines);
                    last_flush = Instant::now();
                }
            }, |vars| {
                if last_vars_flush.elapsed() >= Duration::from_millis(500) {
                    let _ = vars_tx.send(app_common::AppEvent::VarsUpdate { vars });
                    vars_tx.wake();
                    last_vars_flush = Instant::now();
                }
            })?;
            Ok(())
        });

        let error_line = result.as_ref().err().and_then(|err| match err {
            RunError::Line { line, .. } => Some(*line),
            _ => None,
        });
        let (final_line, status) = match result {
            Ok(()) => ("运行完成。".to_string(), "运行完成。"),
            Err(RunError::Stopped) => ("运行已停止。".to_string(), "运行已停止。"),
            Err(err) => (format!("错误：{err}"), "运行出错。"),
        };
        app_common::push_tail_log(&mut tail_logs, &mut total_lines, final_line);
        send_log_snapshot(&tx, &tail_logs, total_lines);
        let _ = tx.send(app_common::AppEvent::Done {
            status: status.to_string(),
            error_line,
        });
        tx.wake();
    });
}

/// Format variables for display.
pub fn format_vars(vars: &[(String, String)]) -> String {
    vars.iter()
        .map(|(name, value)| format!("{name} = {value}"))
        .collect::<Vec<_>>()
        .join("\r\n")
}
