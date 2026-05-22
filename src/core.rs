use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use enigo::{Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use image::{DynamicImage, RgbaImage};
use screenshots::Screen;
use thiserror::Error;

pub struct Runner {
    enigo: Enigo,
    stop_requested: Arc<AtomicBool>,
}

impl Runner {
    pub fn new(stop_requested: Arc<AtomicBool>) -> Result<Self, RunError> {
        Ok(Self {
            enigo: Enigo::new(&Settings::default())?,
            stop_requested,
        })
    }

    pub fn check_stop(&self) -> Result<(), RunError> {
        if self.stop_requested.load(Ordering::Relaxed) {
            Err(RunError::Stopped)
        } else {
            Ok(())
        }
    }

    #[allow(unreachable_code)]
    pub fn run_script<F>(&mut self, script: &str, mut log: F) -> Result<(), RunError>
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

            log(format!("[line {line_no}] {line}"));
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
                log(format!("clicked ({x}, {y})"));
            }
            Command::Move { x, y } => {
                self.enigo.move_mouse(x, y, Coordinate::Abs)?;
                log(format!("moved mouse to ({x}, {y})"));
            }
            Command::Key { text } => {
                for ch in text.chars() {
                    self.enigo.key(Key::Unicode(ch), Direction::Click)?;
                }
                log(format!("typed {text:?}"));
            }
            Command::Sleep { ms } => {
                let deadline = Instant::now() + Duration::from_millis(ms);
                while Instant::now() < deadline {
                    self.check_stop()?;
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    thread::sleep(remaining.min(Duration::from_millis(50)));
                }
                log(format!("slept {ms} ms"));
            }
            Command::Screenshot { path } => {
                let captured = capture_primary_screen_with_info()?;
                captured.image.save(&path)?;
                log(format!("screenshot saved: {}", path.display()));
            }
            Command::Find {
                image,
                threshold,
                options,
            } => {
                let captured = capture_primary_screen_with_info()?;
                let needle = load_template(&image)?;
                let found = find_template_with_options(
                    &captured.image,
                    &needle,
                    threshold,
                    options,
                    Some(self.stop_requested.as_ref()),
                )?;
                log(format!(
                    "found image {}, position ({}, {}), score {:.4}, scale {:.2}",
                    image.display(),
                    found.x + captured.screen_x,
                    found.y + captured.screen_y,
                    found.score,
                    found.scale
                ));
            }
            Command::FindClick {
                image,
                threshold,
                timeout_ms,
                options,
            } => {
                let deadline = Instant::now() + Duration::from_millis(timeout_ms);
                let needle = load_template(&image)?;
                loop {
                    self.check_stop()?;
                    let captured = capture_primary_screen_with_info()?;
                    match find_template_with_options(
                        &captured.image,
                        &needle,
                        threshold,
                        options,
                        Some(self.stop_requested.as_ref()),
                    ) {
                        Ok(found) => {
                            let image_cx = found.x + (found.width as i32 / 2);
                            let image_cy = found.y + (found.height as i32 / 2);
                            let (cx, cy) = captured.image_point_to_screen(image_cx, image_cy);
                            self.enigo.move_mouse(cx, cy, Coordinate::Abs)?;
                            self.enigo.button(Button::Left, Direction::Click)?;
                            log(format!(
                                "found and clicked image {}, position ({cx}, {cy}), score {:.4}, scale {:.2}",
                                image.display(),
                                found.score,
                                found.scale
                            ));
                            break;
                        }
                        Err(err) if Instant::now() < deadline => {
                            log(format!("not found yet: {err}"));
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
                .map_err(|_| line_error(line_no, &format!("cannot convert to int: {value}"))),
        }
    }

    fn to_float(&self, line_no: usize) -> Result<f64, RunError> {
        match self {
            Value::Number(value) => Ok(*value),
            Value::Bool(value) => Ok(if *value { 1.0 } else { 0.0 }),
            Value::Text(value) => value
                .trim()
                .parse::<f64>()
                .map_err(|_| line_error(line_no, &format!("cannot convert to float: {value}"))),
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
                (Flow::Break, _) => return Err(line_error(self.lines[pc].line_no, "break outside loop")),
                (Flow::ContinueLoop, _) => {
                    return Err(line_error(self.lines[pc].line_no, "continue outside loop"));
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
                source: Box::new(RunError::Parse("missing indented block".to_string())),
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
                source: Box::new(RunError::Parse("too many script steps; possible infinite loop".to_string())),
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
                "elif/else must follow an if block at the same indent",
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
                .ok_or_else(|| line_error(line.line_no, "while statement must end with ':'"))?;
            let (body_start, body_end) = self.block_bounds(pc)?;
            let mut loop_count = 0usize;
            while self.eval_expr(condition, line.line_no)?.as_bool() {
                runner.check_stop()?;
                loop_count += 1;
                if loop_count > 100_000 {
                    return Err(line_error(line.line_no, "while loop ran too many times"));
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
                    line_error(line.line_no, "if/elif statement must end with ':'")
                })?)
            } else if line.text == "else:" {
                None
            } else {
                return Err(line_error(line.line_no, "invalid if/elif/else branch"));
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
            return Err(line_error(line_no, "unknown function"));
        };
        let params = parse_def_params(&self.lines[function.body_start - 1].text)?;
        if params.len() != args.len() {
            return Err(line_error(line_no, "function argument count mismatch"));
        }

        self.scopes.push(HashMap::new());
        for (param, value) in params.iter().zip(args) {
            self.set_var(param, value);
        }

        let result = self.execute_block(function.body_start, function.body_end, runner, log);
        self.scopes.pop();

        match result? {
            Flow::Continue => Ok(()),
            Flow::Break => Err(line_error(line_no, "break from inside function is not supported")),
            Flow::ContinueLoop => Err(line_error(
                line_no,
                "continue from inside function is not supported",
            )),
            Flow::Goto(label) => Err(line_error(
                line_no,
                &format!("goto from inside function is not supported: {label}"),
            )),
        }
    }

    fn parse_for(&mut self, text: &str, line_no: usize) -> Result<(String, ForRange), RunError> {
        let body = text
            .strip_prefix("for ")
            .and_then(|value| value.strip_suffix(':'))
            .ok_or_else(|| line_error(line_no, "for statement must end with ':'"))?;
        let Some((var, range_expr)) = body.split_once(" in ") else {
            return Err(line_error(
                line_no,
                "for statement format: for i in range(...):",
            ));
        };
        let args = call_args(range_expr.trim(), "range")
            .ok_or_else(|| line_error(line_no, "for currently supports range(...)"))?;
        let nums = split_args(args)
            .into_iter()
            .map(|arg| self.eval_number(&arg, line_no).map(|value| value as i64))
            .collect::<Result<Vec<_>, _>>()?;
        let (start, stop, step) = match nums.as_slice() {
            [stop] => (0, *stop, 1),
            [start, stop] => (*start, *stop, 1),
            [start, stop, step] => (*start, *stop, *step),
            _ => return Err(line_error(line_no, "range supports 1 to 3 arguments")),
        };
        if step == 0 {
            return Err(line_error(line_no, "range step cannot be 0"));
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
        log(format!("[line {line_no}] {command_line}"));
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
            .ok_or_else(|| line_error(line_no, &format!("unknown label: {label}")))
    }

    fn eval_number(&mut self, expr: &str, line_no: usize) -> Result<f64, RunError> {
        match self.eval_expr(expr, line_no)? {
            Value::Number(value) => Ok(value),
            Value::Bool(value) => Ok(if value { 1.0 } else { 0.0 }),
            Value::Text(_) => Err(line_error(line_no, "number expression expected")),
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
            return Err(line_error(line_no, "empty expression"));
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
                        return Err(line_error(line_no, "f-string missing }"));
                    }
                    out.push_str(&self.eval_f_string_expr(&expr, line_no)?);
                }
                '}' => return Err(line_error(line_no, "f-string has extra }")),
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
            "function definition format: def name(...):".to_string(),
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
            &format!("{name}() expects exactly 1 argument"),
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
            &format!("unsupported f-string format specifier: {spec}"),
        ));
    };
    let precision = precision
        .strip_suffix('f')
        .unwrap_or(precision)
        .parse::<usize>()
        .map_err(|_| line_error(line_no, &format!("invalid f-string precision: {spec}")))?;
    match value {
        Value::Number(value) => Ok(format!("{value:.precision$}")),
        Value::Bool(value) => Ok(format!("{:.precision$}", if *value { 1.0 } else { 0.0 })),
        Value::Text(value) => value
            .parse::<f64>()
            .map(|value| format!("{value:.precision$}"))
            .map_err(|_| line_error(line_no, "f-string numeric format requires a number")),
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
        options: ImageSearchOptions,
    },
    FindClick {
        image: PathBuf,
        threshold: f32,
        timeout_ms: u64,
        options: ImageSearchOptions,
    },
}

#[derive(Debug, Clone, Copy, Default)]
struct ImageSearchOptions {
    region: Option<SearchRegion>,
    scale: ScaleSearch,
}

#[derive(Debug, Clone, Copy)]
struct SearchRegion {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[derive(Debug, Clone, Copy)]
struct ScaleSearch {
    min: f32,
    max: f32,
    step: f32,
}

impl Default for ScaleSearch {
    fn default() -> Self {
        Self {
            min: 1.0,
            max: 1.0,
            step: 0.0,
        }
    }
}

fn parse_command(line: &str) -> Result<Command, RunError> {
    let tokens = split_command(line);
    if tokens.is_empty() {
        return Err(RunError::Parse("empty command".to_string()));
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
            options: parse_image_search_options(&tokens, 3)?,
        }),
        "find_click" | "查找图片并点击" => Ok(Command::FindClick {
            image: parse_path(&tokens, 1, "image")?,
            threshold: parse_optional_f32(&tokens, 2, 0.92)?,
            timeout_ms: parse_optional_u64(&tokens, 3, 0)?,
            options: parse_image_search_options(&tokens, 4)?,
        }),
        _ => Err(RunError::Parse(format!("unknown command: {command}"))),
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
        .ok_or_else(|| RunError::Parse(format!("missing argument {name}")))?
        .parse()
        .map_err(|_| RunError::Parse(format!("argument {name} is not a valid integer")))
}

fn parse_u64(tokens: &[String], index: usize, name: &str) -> Result<u64, RunError> {
    tokens
        .get(index)
        .ok_or_else(|| RunError::Parse(format!("missing argument {name}")))?
        .parse()
        .map_err(|_| RunError::Parse(format!("argument {name} is not a valid integer")))
}

fn parse_optional_f32(tokens: &[String], index: usize, default: f32) -> Result<f32, RunError> {
    match tokens.get(index) {
        Some(value) => value
            .parse()
            .map_err(|_| RunError::Parse(format!("not a valid number: {value}"))),
        None => Ok(default),
    }
}

fn parse_optional_u64(tokens: &[String], index: usize, default: u64) -> Result<u64, RunError> {
    match tokens.get(index) {
        Some(value) => value
            .parse()
            .map_err(|_| RunError::Parse(format!("not a valid integer: {value}"))),
        None => Ok(default),
    }
}

fn parse_path(tokens: &[String], index: usize, name: &str) -> Result<PathBuf, RunError> {
    tokens
        .get(index)
        .map(PathBuf::from)
        .ok_or_else(|| RunError::Parse(format!("missing path argument {name}")))
}

fn parse_image_search_options(
    tokens: &[String],
    start: usize,
) -> Result<ImageSearchOptions, RunError> {
    let region = if tokens.len() >= start + 4 {
        Some(SearchRegion {
            x: parse_optional_u32(tokens, start, 0)?,
            y: parse_optional_u32(tokens, start + 1, 0)?,
            width: parse_optional_u32(tokens, start + 2, 0)?,
            height: parse_optional_u32(tokens, start + 3, 0)?,
        })
        .filter(|region| region.width > 0 && region.height > 0)
    } else {
        None
    };
    let scale = if tokens.len() >= start + 7 {
        ScaleSearch {
            min: parse_optional_f32(tokens, start + 4, 1.0)?.max(0.2),
            max: parse_optional_f32(tokens, start + 5, 1.0)?.max(0.2),
            step: parse_optional_f32(tokens, start + 6, 0.0)?.max(0.0),
        }
    } else {
        ScaleSearch::default()
    };
    Ok(ImageSearchOptions { region, scale })
}

fn parse_optional_u32(tokens: &[String], index: usize, default: u32) -> Result<u32, RunError> {
    match tokens.get(index) {
        Some(value) => value
            .parse()
            .map_err(|_| RunError::Parse(format!("not a valid integer: {value}"))),
        None => Ok(default),
    }
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
        .ok_or_else(|| RunError::Screen("screen not found".to_string()))?;
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
    width: u32,
    height: u32,
    score: f32,
    scale: f32,
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
        return Err(RunError::ImageSearch("template image is empty".to_string()));
    }
    if nw > hw || nh > hh {
        return Err(RunError::ImageSearch(format!(
            "template image {}x{} is larger than screenshot {}x{}",
            nw, nh, hw, hh
        )));
    }

    let haystack_gray = rgba_to_gray(haystack);
    let needle_gray = rgba_to_gray(needle);
    let needle_stats = TemplateStats::new(&needle_gray)?;

    let mut best = MatchResult {
        x: 0,
        y: 0,
        width: nw,
        height: nh,
        score: -1.0,
        scale: 1.0,
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
                    width: nw,
                    height: nh,
                    score,
                    scale: 1.0,
                };
            }
        }
    }

    if best.score >= threshold {
        Ok(best)
    } else {
        Err(RunError::ImageSearch(format!(
            "image not found; best score {:.4}, threshold {:.4}",
            best.score, threshold
        )))
    }
}

impl PreparedTemplate {
    fn new(needle: &RgbaImage) -> Result<Self, RunError> {
        let (width, height) = needle.dimensions();
        if width == 0 || height == 0 {
            return Err(RunError::ImageSearch("template image is empty".to_string()));
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
            "template image {}x{} is larger than screenshot {}x{}",
            needle.width, needle.height, hw, hh
        )));
    }

    let haystack_gray = rgba_to_gray(haystack);
    let candidates =
        find_template_candidates(&haystack_gray, hw, hh, needle, threshold, stop_requested)?;
    let mut best = MatchResult {
        x: 0,
        y: 0,
        width: needle.width,
        height: needle.height,
        score: -1.0,
        scale: 1.0,
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
                width: needle.width,
                height: needle.height,
                score,
                scale: candidate.scale,
            };
        }
    }

    if best.score >= threshold {
        Ok(best)
    } else {
        Err(RunError::ImageSearch(format!(
            "image not found; best score {:.4}, threshold {:.4}",
            best.score, threshold
        )))
    }
}

fn find_template_with_options(
    haystack: &RgbaImage,
    needle: &RgbaImage,
    threshold: f32,
    options: ImageSearchOptions,
    stop_requested: Option<&AtomicBool>,
) -> Result<MatchResult, RunError> {
    let (search_image, offset_x, offset_y) = crop_search_region(haystack, options.region)?;
    let mut best_match: Option<MatchResult> = None;
    let mut best_error: Option<MatchResult> = None;

    for scale in search_scales(options.scale) {
        check_stop_flag(stop_requested)?;
        let scaled = scale_template(needle, scale)?;
        let prepared = PreparedTemplate::new(&scaled)?;
        match find_prepared_template(&search_image, &prepared, threshold, stop_requested) {
            Ok(mut found) => {
                found.x += offset_x as i32;
                found.y += offset_y as i32;
                found.scale = scale;
                if best_match
                    .as_ref()
                    .map(|best| found.score > best.score)
                    .unwrap_or(true)
                {
                    best_match = Some(found);
                }
            }
            Err(RunError::ImageSearch(message)) => {
                if let Some(score) = parse_best_score(&message) {
                    let candidate = MatchResult {
                        x: offset_x as i32,
                        y: offset_y as i32,
                        width: scaled.width(),
                        height: scaled.height(),
                        score,
                        scale,
                    };
                    if best_error
                        .as_ref()
                        .map(|best| candidate.score > best.score)
                        .unwrap_or(true)
                    {
                        best_error = Some(candidate);
                    }
                }
            }
            Err(err) => return Err(err),
        }
    }

    if let Some(best) = best_match {
        return Ok(best);
    }

    if let Some(best) = best_error {
        Err(RunError::ImageSearch(format!(
            "image not found; best score {:.4}, threshold {:.4}, scale {:.2}",
            best.score, threshold, best.scale
        )))
    } else {
        Err(RunError::ImageSearch(format!(
            "image not found; threshold {:.4}",
            threshold
        )))
    }
}

fn crop_search_region(
    image: &RgbaImage,
    region: Option<SearchRegion>,
) -> Result<(RgbaImage, u32, u32), RunError> {
    let Some(region) = region else {
        return Ok((image.clone(), 0, 0));
    };
    let x = region.x.min(image.width());
    let y = region.y.min(image.height());
    let width = region.width.min(image.width().saturating_sub(x));
    let height = region.height.min(image.height().saturating_sub(y));
    if width == 0 || height == 0 {
        return Err(RunError::ImageSearch("search region is empty".to_string()));
    }
    Ok((image::imageops::crop_imm(image, x, y, width, height).to_image(), x, y))
}

fn scale_template(needle: &RgbaImage, scale: f32) -> Result<RgbaImage, RunError> {
    if (scale - 1.0).abs() <= f32::EPSILON {
        return Ok(needle.clone());
    }
    let width = ((needle.width() as f32 * scale).round() as u32).max(1);
    let height = ((needle.height() as f32 * scale).round() as u32).max(1);
    Ok(image::imageops::resize(
        needle,
        width,
        height,
        image::imageops::FilterType::Triangle,
    ))
}

fn search_scales(scale: ScaleSearch) -> Vec<f32> {
    let min = scale.min.min(scale.max).max(0.2);
    let max = scale.min.max(scale.max).max(0.2);
    let step = scale.step.max(0.0);
    if step <= f32::EPSILON || (max - min).abs() <= f32::EPSILON {
        return vec![min];
    }
    let mut values = Vec::new();
    let mut current = min;
    while current <= max + f32::EPSILON && values.len() < 32 {
        values.push((current * 100.0).round() / 100.0);
        current += step;
    }
    values
}

fn parse_best_score(message: &str) -> Option<f32> {
    let (_, tail) = message.split_once("best score ")?;
    let score = tail.split_once(',').map(|(score, _)| score).unwrap_or(tail);
    score.trim().parse().ok()
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
        width: needle.width,
        height: needle.height,
        score: -1.0,
        scale: 1.0,
    };

    for y in 0..=(haystack_height - needle.height) {
        check_stop_flag(stop_requested)?;
        for x in 0..=(haystack_width - needle.width) {
            let score = sample_score_normed(haystack, haystack_width, needle, x, y);
            if score > best_sample.score {
                best_sample = MatchResult {
                    x: x as i32,
                    y: y as i32,
                    width: needle.width,
                    height: needle.height,
                    score,
                    scale: 1.0,
                };
            }
            if score >= min_sample_score {
                push_top_candidate(
                    &mut candidates,
                    MatchResult {
                        x: x as i32,
                        y: y as i32,
                        width: needle.width,
                        height: needle.height,
                        score,
                        scale: 1.0,
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
                "template image has too little color variation".to_string(),
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
pub enum RunError {
    #[error("script stopped")]
    Stopped,
    #[error("line {line}: {source}")]
    Line { line: usize, source: Box<RunError> },
    #[error("parse error: {0}")]
    Parse(String),
    #[error("input error: {0}")]
    Input(#[from] enigo::InputError),
    #[error("settings error: {0}")]
    Settings(#[from] enigo::NewConError),
    #[error("screen error: {0}")]
    Screen(String),
    #[error("image error: {0}")]
    Image(#[from] image::ImageError),
    #[error("image search error: {0}")]
    ImageSearch(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evals_f_string_format_specs_and_conversions() {
        let mut interpreter = ScriptInterpreter::new("").unwrap();
        interpreter.set_var("x", Value::Number(3.14159));
        interpreter.set_var("name", Value::Text("7".to_string()));

        let formatted = interpreter.eval_expr("f'{x:.2f}'", 1).unwrap();
        assert_eq!(formatted.to_script_string(), "3.14");

        let int_value = interpreter.eval_expr("int(name) + 5", 1).unwrap();
        assert_eq!(int_value.to_script_string(), "12");

        let type_name = interpreter.eval_expr("type(x)", 1).unwrap();
        assert_eq!(type_name.to_script_string(), "float");
    }

    #[test]
    fn function_scope_reads_globals_but_keeps_assignments_local() {
        let mut interpreter = ScriptInterpreter::new("").unwrap();
        interpreter.set_var("x", Value::Number(10.0));
        interpreter.scopes.push(HashMap::new());
        let y = interpreter.eval_expr("x + 5", 1).unwrap();
        interpreter.set_var("y", y);
        interpreter.set_var("x", Value::Number(1.0));
        assert_eq!(interpreter.eval_expr("y", 1).unwrap().to_script_string(), "15");
        interpreter.scopes.pop();

        assert_eq!(interpreter.eval_expr("x", 1).unwrap().to_script_string(), "10");
        assert!(interpreter.get_var("y").is_none());
    }
}

