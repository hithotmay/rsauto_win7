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
    pub fn run_script<F, V>(&mut self, script: &str, mut log: F, mut vars_cb: V) -> Result<(), RunError>
    where
        F: FnMut(String),
        V: FnMut(Vec<(String, String)>),
    {
        let mut interpreter = ScriptInterpreter::new(script)?;
        return interpreter.run(self, &mut log, &mut vars_cb);

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

enum ForIter {
    Range(ForRange),
    Values(Vec<Value>),
}

#[derive(Clone, Debug)]
enum Value {
    Number(f64),
    Text(String),
    Bool(bool),
    List(Vec<Value>),
    Dict(HashMap<String, Value>),
    Tuple(Vec<Value>),
    None,
}

impl Value {
    fn as_bool(&self) -> bool {
        match self {
            Value::Number(value) => *value != 0.0,
            Value::Text(value) => !value.is_empty(),
            Value::Bool(value) => *value,
            Value::List(items) => !items.is_empty(),
            Value::Dict(map) => !map.is_empty(),
            Value::Tuple(items) => !items.is_empty(),
            Value::None => false,
        }
    }

    fn to_script_string(&self) -> String {
        match self {
            Value::Number(value) if value.fract() == 0.0 => format!("{}", *value as i64),
            Value::Number(value) => format!("{value}"),
            Value::Text(value) => value.clone(),
            Value::Bool(value) => value.to_string(),
            Value::List(items) => {
                let inner: Vec<String> = items.iter().map(|v| match v {
                    Value::Text(s) => format!("'{s}'"),
                    other => other.to_script_string(),
                }).collect();
                format!("[{}]", inner.join(", "))
            }
            Value::Dict(map) => {
                let inner: Vec<String> = map.iter().map(|(k, v)| {
                    let val = match v {
                        Value::Text(s) => format!("'{s}'"),
                        other => other.to_script_string(),
                    };
                    format!("'{k}': {val}")
                }).collect();
                format!("{{{}}}", inner.join(", "))
            }
            Value::Tuple(items) => {
                let inner: Vec<String> = items.iter().map(|v| match v {
                    Value::Text(s) => format!("'{s}'"),
                    other => other.to_script_string(),
                }).collect();
                if items.len() == 1 {
                    format!("({},)", inner.join(", "))
                } else {
                    format!("({})", inner.join(", "))
                }
            }
            Value::None => "None".to_string(),
        }
    }

    fn type_name(&self) -> &'static str {
        match self {
            Value::Number(value) if value.fract() == 0.0 => "int",
            Value::Number(_) => "float",
            Value::Text(_) => "str",
            Value::Bool(_) => "bool",
            Value::List(_) => "list",
            Value::Dict(_) => "dict",
            Value::Tuple(_) => "tuple",
            Value::None => "NoneType",
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
            Value::None => Err(line_error(line_no, "cannot convert NoneType to int")),
            _ => Err(line_error(line_no, &format!("cannot convert {} to int", self.type_name()))),
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
            Value::None => Err(line_error(line_no, "cannot convert NoneType to float")),
            _ => Err(line_error(line_no, &format!("cannot convert {} to float", self.type_name()))),
        }
    }
}

enum Flow {
    Continue,
    Break,
    ContinueLoop,
    Goto(String),
    Return(Value),
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

    pub fn snapshot_vars(&self) -> Vec<(String, String)> {
        let mut vars = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for scope in self.scopes.iter().rev() {
            for (name, value) in scope {
                if seen.insert(name.clone()) {
                    vars.push((name.clone(), value.to_script_string()));
                }
            }
        }
        vars
    }

    fn run<F, V>(&mut self, runner: &mut Runner, log: &mut F, vars_cb: &mut V) -> Result<(), RunError>
    where
        F: FnMut(String),
        V: FnMut(Vec<(String, String)>),
    {
        let mut pc = 0;
        while pc < self.lines.len() {
            runner.check_stop()?;
            match self.execute_at(pc, runner, log, vars_cb)? {
                (Flow::Continue, next) => pc = next,
                (Flow::Goto(label), _) => pc = self.goto_target(&label, self.lines[pc].line_no)?,
                (Flow::Break, _) => {
                    return Err(line_error(self.lines[pc].line_no, "break outside loop"))
                }
                (Flow::ContinueLoop, _) => {
                    return Err(line_error(self.lines[pc].line_no, "continue outside loop"));
                }
                (Flow::Return(_), _) => {
                    return Err(line_error(
                        self.lines[pc].line_no,
                        "return outside function",
                    ));
                }
            }
            if self.steps % 100 == 0 {
                vars_cb(self.snapshot_vars());
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

    fn execute_block<F, V>(
        &mut self,
        start: usize,
        end: usize,
        runner: &mut Runner,
        log: &mut F,
        vars_cb: &mut V,
    ) -> Result<Flow, RunError>
    where
        F: FnMut(String),
        V: FnMut(Vec<(String, String)>),
    {
        let mut pc = start;
        while pc < end {
            runner.check_stop()?;
            match self.execute_at(pc, runner, log, vars_cb)? {
                (Flow::Continue, next) => pc = next,
                (flow @ (Flow::Break | Flow::ContinueLoop | Flow::Goto(_) | Flow::Return(_)), _) => return Ok(flow),
            }
        }
        Ok(Flow::Continue)
    }

    fn execute_at<F, V>(
        &mut self,
        pc: usize,
        runner: &mut Runner,
        log: &mut F,
        vars_cb: &mut V,
    ) -> Result<(Flow, usize), RunError>
    where
        F: FnMut(String),
        V: FnMut(Vec<(String, String)>),
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
            return self.execute_if_chain(pc, runner, log, vars_cb);
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
        if let Some(rest) = text.strip_prefix("return") {
            let rest = rest.trim();
            let value = if rest.is_empty() {
                Value::Bool(false)
            } else {
                self.eval_expr(rest, line.line_no)?
            };
            return Ok((Flow::Return(value), pc + 1));
        }
        if text.starts_with("for ") {
            let (var, iter) = self.parse_for(text, line.line_no)?;
            let (body_start, body_end) = self.block_bounds(pc)?;
            match iter {
                ForIter::Range(values) => {
                    for value in values {
                        runner.check_stop()?;
                        self.set_var(&var, Value::Number(value as f64));
                        match self.execute_block(body_start, body_end, runner, log, vars_cb)? {
                            Flow::Continue => {}
                            Flow::ContinueLoop => continue,
                            Flow::Break => break,
                            flow @ Flow::Goto(_) => return Ok((flow, pc + 1)),
                            flow @ Flow::Return(_) => return Ok((flow, pc + 1)),
                        }
                    }
                }
                ForIter::Values(vals) => {
                    for value in vals {
                        runner.check_stop()?;
                        self.set_var(&var, value);
                        match self.execute_block(body_start, body_end, runner, log, vars_cb)? {
                            Flow::Continue => {}
                            Flow::ContinueLoop => continue,
                            Flow::Break => break,
                            flow @ Flow::Goto(_) => return Ok((flow, pc + 1)),
                            flow @ Flow::Return(_) => return Ok((flow, pc + 1)),
                        }
                    }
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
                match self.execute_block(body_start, body_end, runner, log, vars_cb)? {
                    Flow::Continue => {}
                    Flow::ContinueLoop => continue,
                    Flow::Break => break,
                    flow @ Flow::Goto(_) => return Ok((flow, pc + 1)),
                    flow @ Flow::Return(_) => return Ok((flow, pc + 1)),
                }
            }
            return Ok((Flow::Continue, body_end));
        }

        // try / except / else / finally
        if text.starts_with("try:") || text == "try:" {
            return self.execute_try_block(pc, runner, log, vars_cb);
        }

        self.execute_statement(text, line.line_no, runner, log, vars_cb)?;
        Ok((Flow::Continue, pc + 1))
    }

    fn execute_if_chain<F, V>(
        &mut self,
        start: usize,
        runner: &mut Runner,
        log: &mut F,
        vars_cb: &mut V,
    ) -> Result<(Flow, usize), RunError>
    where
        F: FnMut(String),
        V: FnMut(Vec<(String, String)>),
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
                let flow = self.execute_block(body_start, body_end, runner, log, vars_cb)?;
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

    /// Execute a try/except/else/finally block with runner.
    fn execute_try_block<F, V>(
        &mut self,
        start_pc: usize,
        runner: &mut Runner,
        log: &mut F,
        vars_cb: &mut V,
    ) -> Result<(Flow, usize), RunError>
    where
        F: FnMut(String),
        V: FnMut(Vec<(String, String)>),
    {
        let base_indent = self.lines[start_pc].indent;
        let (try_body_start, try_body_end) = self.block_bounds(start_pc)?;
        let total_lines = self.lines.len();

        // Scan for except/else/finally clauses
        let mut except_clauses: Vec<(usize, usize, Option<String>, Option<String>)> = Vec::new();
        let mut else_clause: Option<(usize, usize)> = None;
        let mut finally_clause: Option<(usize, usize)> = None;

        let mut scan = try_body_end;
        while scan < total_lines {
            let sl = &self.lines[scan];
            if sl.indent != base_indent { break; }
            if let Some(rest) = sl.text.strip_prefix("except ") {
                let rest = rest.strip_suffix(':').unwrap_or(rest);
                let (exc_type, alias) = parse_except_header(rest.trim());
                let (bs, be) = self.block_bounds(scan)?;
                except_clauses.push((bs, be, exc_type, alias));
                scan = be;
            } else if sl.text == "except:" {
                let (bs, be) = self.block_bounds(scan)?;
                except_clauses.push((bs, be, None, None));
                scan = be;
            } else if sl.text == "else:" {
                let (bs, be) = self.block_bounds(scan)?;
                else_clause = Some((bs, be));
                scan = be;
            } else if sl.text == "finally:" {
                let (bs, be) = self.block_bounds(scan)?;
                finally_clause = Some((bs, be));
                scan = be;
            } else {
                break;
            }
        }
        let chain_end = scan;

        // Execute try body
        let try_result = self.execute_block(try_body_start, try_body_end, runner, log, vars_cb);

        match try_result {
            Ok(flow) => {
                let mut final_flow = flow;
                if let Some((bs, be)) = else_clause {
                    match final_flow {
                        Flow::Continue => {
                            final_flow = self.execute_block(bs, be, runner, log, vars_cb)?;
                        }
                        _ => {}
                    }
                }
                if let Some((bs, be)) = finally_clause {
                    let fflow = self.execute_block(bs, be, runner, log, vars_cb)?;
                    match fflow {
                        Flow::Return(v) => final_flow = Flow::Return(v),
                        Flow::Break => final_flow = Flow::Break,
                        Flow::ContinueLoop => final_flow = Flow::ContinueLoop,
                        _ => {}
                    }
                }
                Ok((final_flow, chain_end))
            }
            Err(err) => {
                let err_msg = format!("{err}");
                let mut matched = false;
                let mut final_flow = Flow::Continue;
                for (bs, be, exc_type, alias) in &except_clauses {
                    let catches = match exc_type {
                        None => true,
                        Some(t) => err_msg.starts_with(&format!("{}:", t)) || t == "Exception",
                    };
                    if catches {
                        matched = true;
                        if let Some(alias_name) = alias {
                            let msg_part = if let Some(colon_pos) = err_msg.find(": ") {
                                err_msg[colon_pos + 2..].to_string()
                            } else {
                                err_msg.clone()
                            };
                            self.set_var(alias_name, Value::Text(msg_part));
                        }
                        let flow = self.execute_block(*bs, *be, runner, log, vars_cb)?;
                        final_flow = flow;
                        break;
                    }
                }
                if !matched {
                    if let Some((bs, be)) = finally_clause {
                        let _ = self.execute_block(bs, be, runner, log, vars_cb);
                    }
                    return Err(err);
                }
                if let Some((bs, be)) = finally_clause {
                    let fflow = self.execute_block(bs, be, runner, log, vars_cb)?;
                    match fflow {
                        Flow::Return(v) => final_flow = Flow::Return(v),
                        Flow::Break => final_flow = Flow::Break,
                        Flow::ContinueLoop => final_flow = Flow::ContinueLoop,
                        _ => {}
                    }
                }
                Ok((final_flow, chain_end))
            }
        }
    }

    fn try_augmented_assignment(
        &mut self,
        text: &str,
        line_no: usize,
    ) -> Result<Option<()>, RunError> {
        // Check for augmented assignment operators: +=, -=, *=, /=, //=, **=, %=
        let augmented_ops: &[(&str, &str)] = &[
            ("+=", "+"),
            ("-=", "-"),
            ("*=", "*"),
            ("/=", "/"),
            ("//=", "//"),
            ("**=", "**"),
            ("%=", "%"),
        ];

        // Find the augmented operator outside brackets/quotes
        let mut quote: Option<char> = None;
        let mut depth = 0i32;
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
            if quote.is_some() {
                continue;
            }
            match ch {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                _ => {}
            }
            if depth > 0 {
                continue;
            }
            for &(op_str, _op_base) in augmented_ops {
                if text[idx..].starts_with(op_str) {
                    let name_part = text[..idx].trim();
                    let expr_part = text[idx + op_str.len()..].trim();
                    // Verify name_part is a valid identifier (or name[subscript])
                    if name_part.is_empty() || expr_part.is_empty() {
                        continue;
                    }
                    // Check if name_part has a subscript (e.g., x[0])
                    if let Some(bracket_pos) = name_part.find('[') {
                        let var_name = &name_part[..bracket_pos];
                        if !var_name
                            .chars()
                            .all(|c| c.is_alphanumeric() || c == '_')
                        {
                            continue;
                        }
                        // Evaluate the subscript expression
                        let subscript_expr =
                            name_part[bracket_pos + 1..].strip_suffix(']');
                        if subscript_expr.is_none() {
                            continue;
                        }
                        let sub_expr = subscript_expr.unwrap();

                        // Build the expression "name_part op_base expr_part"
                        // and evaluate it
                        let computed_expr = format!(
                            "{} {} ({})",
                            name_part, _op_base, expr_part
                        );
                        let new_val =
                            self.eval_expr(&computed_expr, line_no)?;

                        // Now do subscript assignment
                        // Get the container, set the element
                        let idx_val =
                            self.eval_expr(sub_expr, line_no)?;
                        let container =
                            self.get_var(var_name).cloned().ok_or_else(
                                || {
                                    line_error(
                                        line_no,
                                        &format!(
                                            "name '{}' is not defined",
                                            var_name
                                        ),
                                    )
                                },
                            )?;
                        match container {
                            Value::List(mut items) => {
                                let i = idx_val.to_int(line_no)?;
                                let i = if i < 0 {
                                    (items.len() as i64 + i) as usize
                                } else {
                                    i as usize
                                };
                                if i < items.len() {
                                    items[i] = new_val;
                                    self.set_var(var_name, Value::List(items));
                                    return Ok(Some(()));
                                }
                                return Err(line_error(
                                    line_no,
                                    "list index out of range",
                                ));
                            }
                            Value::Dict(mut map) => {
                                let key = match &idx_val {
                                    Value::Text(s) => s.clone(),
                                    Value::Number(n) => {
                                        format!("{}", *n as i64)
                                    }
                                    other => other.to_script_string(),
                                };
                                map.insert(key, new_val);
                                self.set_var(
                                    var_name,
                                    Value::Dict(map),
                                );
                                return Ok(Some(()));
                            }
                            _ => {
                                return Err(line_error(
                                    line_no,
                                    "subscript assignment requires list or dict",
                                ))
                            }
                        }
                    } else {
                        // Simple variable name
                        if !name_part
                            .chars()
                            .all(|c| c.is_alphanumeric() || c == '_')
                        {
                            continue;
                        }
                        // Build the expression: "current_value op_base expr_part"
                        let current =
                            self.get_var(name_part).cloned().ok_or_else(
                                || {
                                    line_error(
                                        line_no,
                                        &format!(
                                            "name '{}' is not defined",
                                            name_part
                                        ),
                                    )
                                },
                            )?;
                        let current_str = current.to_script_string();
                        let computed_expr = format!(
                            "{} {} ({})",
                            current_str, _op_base, expr_part
                        );
                        let new_val =
                            self.eval_expr(&computed_expr, line_no)?;
                        self.set_var(name_part, new_val);
                        return Ok(Some(()));
                    }
                }
            }
        }
        Ok(None)
    }

    fn execute_statement<F, V>(
        &mut self,
        text: &str,
        line_no: usize,
        runner: &mut Runner,
        log: &mut F,
        vars_cb: &mut V,
    ) -> Result<(), RunError>
    where
        F: FnMut(String),
        V: FnMut(Vec<(String, String)>),
    {
        // pass statement (no-op)
        if text == "pass" || text.trim() == "pass" {
            return Ok(());
        }

        // assert statement
        if let Some(expr) = text.strip_prefix("assert ") {
            let expr = expr.trim();
            // Check for optional message after comma
            let (cond_expr, msg) = if let Some(pos) = find_comma_outside(expr) {
                (&expr[..pos], expr[pos + 1..].trim())
            } else {
                (expr, "")
            };
            let val = self.eval_expr(cond_expr, line_no)?;
            if !val.as_bool() {
                let err_msg = if msg.is_empty() {
                    "assertion error".to_string()
                } else {
                    self.eval_expr(msg, line_no)?.to_script_string()
                };
                return Err(line_error(line_no, &format!("AssertionError: {err_msg}")));
            }
            return Ok(());
        }

        // del statement
        if let Some(target) = text.strip_prefix("del ") {
            let target = target.trim();
            if target.contains('[') {
                self.del_subscript(target, line_no)?;
            } else {
                self.del_var(target, line_no)?;
            }
            return Ok(());
        }

        // raise statement
        if let Some(rest) = text.strip_prefix("raise ") {
            let rest = rest.trim();
            let (exc_type, err_msg) = if rest.is_empty() {
                ("RuntimeError".to_string(), "raised error".to_string())
            } else {
                let exc_types = ["ValueError", "TypeError", "RuntimeError", "Exception", "KeyError", "IndexError"];
                let mut found = None;
                for exc in &exc_types {
                    if let Some(args) = call_args(rest, exc) {
                        let vals = split_args(args)
                            .into_iter()
                            .map(|arg| self.eval_expr(&arg, line_no))
                            .collect::<Result<Vec<_>, _>>()?;
                        found = Some((exc.to_string(), vals.iter().map(|v| v.to_script_string()).collect::<Vec<_>>().join(" ")));
                        break;
                    }
                }
                match found {
                    Some((t, m)) => (t, m),
                    None => ("RuntimeError".to_string(), self.eval_expr(rest, line_no)?.to_script_string()),
                }
            };
            return Err(line_error(line_no, &format!("{exc_type}: {err_msg}")));
        }

        // Augmented assignment: +=, -=, *=, /=, //=, **=, %=
        if self.try_augmented_assignment(text, line_no)?.is_some() {
            return Ok(());
        }

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

        // Tuple unpacking: a, b = expr1, expr2
        if let Some(eq_pos) = find_eq_outside(text) {
            let left = text[..eq_pos].trim();
            let right = text[eq_pos + 1..].trim();
            if left.contains(',') && !left.contains('[') && !left.contains('(') {
                let var_names: Vec<&str> = left.split(',').map(|s| s.trim()).collect();
                if var_names.iter().all(|v| !v.is_empty() && v.chars().all(|c| c.is_alphanumeric() || c == '_')) {
                    let values: Vec<Value> = if right.contains(',') && !right.starts_with('[') && !right.starts_with('(') && !right.starts_with('{') {
                        // Right side is comma-separated expressions: a, b = 1, 2
                        split_args(right).into_iter().map(|arg| self.eval_expr(arg.trim(), line_no)).collect::<Result<Vec<_>, _>>()?
                    } else {
                        // Right side is a single expression evaluating to iterable
                        let val = self.eval_expr(right, line_no)?;
                        match val {
                            Value::Tuple(items) => items,
                            Value::List(items) => items,
                            _ => return Err(line_error(line_no, "cannot unpack non-iterable")),
                        }
                    };
                    if var_names.len() != values.len() {
                        return Err(line_error(line_no, &format!("too many values to unpack (expected {}, got {})", var_names.len(), values.len())));
                    }
                    for (name, value) in var_names.into_iter().zip(values) {
                        self.set_var(name, value);
                    }
                    return Ok(());
                }
            }
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
                self.call_function(name, evaluated, runner, log, vars_cb, line_no)?;
                return Ok(());
            }

            let command_line = self.command_from_call(name, args, line_no)?;
            return self.run_command_line(&command_line, line_no, runner, log);
        }

        let command_line = self.resolve_command_line(text, line_no)?;
        self.run_command_line(&command_line, line_no, runner, log)
    }

    fn call_function<F, V>(
        &mut self,
        name: &str,
        args: Vec<Value>,
        runner: &mut Runner,
        log: &mut F,
        vars_cb: &mut V,
        line_no: usize,
    ) -> Result<(), RunError>
    where
        F: FnMut(String),
        V: FnMut(Vec<(String, String)>),
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

        let result = self.execute_block(function.body_start, function.body_end, runner, log, vars_cb);
        self.scopes.pop();

        match result? {
            Flow::Continue => Ok(()),
            Flow::Break => Err(line_error(
                line_no,
                "break from inside function is not supported",
            )),
            Flow::ContinueLoop => Err(line_error(
                line_no,
                "continue from inside function is not supported",
            )),
            Flow::Goto(label) => Err(line_error(
                line_no,
                &format!("goto from inside function is not supported: {label}"),
            )),
            Flow::Return(_) => Ok(()),
        }
    }

    fn parse_for(&mut self, text: &str, line_no: usize) -> Result<(String, ForIter), RunError> {
        let body = text
            .strip_prefix("for ")
            .and_then(|value| value.strip_suffix(':'))
            .ok_or_else(|| line_error(line_no, "for statement must end with ':'"))?;
        let Some((var, range_expr)) = body.split_once(" in ") else {
            return Err(line_error(
                line_no,
                "for statement format: for i in range(...): or for i in expr:",
            ));
        };
        let range_expr = range_expr.trim();
        // Try range() first
        if let Some(args) = call_args(range_expr, "range") {
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
            return Ok((
                var.trim().to_string(),
                ForIter::Range(ForRange {
                    current: start,
                    stop,
                    step,
                }),
            ));
        }
        // For-in iteration over list/str/dict/tuple
        let iterable = self.eval_expr(range_expr, line_no)?;
        let values = match iterable {
            Value::List(items) => items,
            Value::Tuple(items) => items,
            Value::Text(s) => s.chars().map(|c| Value::Text(c.to_string())).collect(),
            Value::Dict(map) => map.into_keys().map(Value::Text).collect(),
            _ => return Err(line_error(line_no, "for-in requires a list, tuple, string, or dict")),
        };
        Ok((var.trim().to_string(), ForIter::Values(values)))
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
            _ => Err(line_error(line_no, "number expression expected")),
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

    fn del_var(&mut self, name: &str, line_no: usize) -> Result<(), RunError> {
        for scope in self.scopes.iter_mut().rev() {
            if scope.remove(name).is_some() {
                return Ok(());
            }
        }
        Err(line_error(line_no, &format!("cannot delete '{name}': name not defined")))
    }

    fn del_subscript(&mut self, target: &str, line_no: usize) -> Result<(), RunError> {
        // Find the last [ to split container and index
        let bracket_pos = target.rfind('[').ok_or_else(|| {
            line_error(line_no, &format!("invalid del target: {target}"))
        })?;
        let container_expr = &target[..bracket_pos];
        let index_expr = target[bracket_pos + 1..].strip_suffix(']').ok_or_else(|| {
            line_error(line_no, &format!("invalid subscript in del: {target}"))
        })?;
        let index_val = self.eval_expr(index_expr, line_no)?;
        // Get the container variable name (simple case: varname)
        if container_expr.chars().all(|c| c.is_alphanumeric() || c == '_') {
            let container = self.get_var(container_expr).cloned().ok_or_else(|| {
                line_error(line_no, &format!("name '{}' is not defined", container_expr))
            })?;
            match container {
                Value::List(mut items) => {
                    let idx = index_val.to_int(line_no)?;
                    let idx = if idx < 0 {
                        (items.len() as i64 + idx) as usize
                    } else {
                        idx as usize
                    };
                    if idx < items.len() {
                        items.remove(idx);
                        self.set_var(container_expr, Value::List(items));
                        return Ok(());
                    }
                    Err(line_error(line_no, "list index out of range in del"))
                }
                Value::Dict(mut map) => {
                    let key = match &index_val {
                        Value::Text(s) => s.clone(),
                        Value::Number(n) => format!("{}", *n as i64),
                        other => other.to_script_string(),
                    };
                    map.remove(&key);
                    self.set_var(container_expr, Value::Dict(map));
                    Ok(())
                }
                _ => Err(line_error(line_no, "cannot delete subscript of this type")),
            }
        } else {
            Err(line_error(line_no, &format!("complex del target not supported: {target}")))
        }
    }

    fn eval_expr(&mut self, expr: &str, line_no: usize) -> Result<Value, RunError> {
        let expr = expr.trim();
        if expr.is_empty() {
            return Err(line_error(line_no, "empty expression"));
        }

        // None literal
        if expr == "None" {
            return Ok(Value::None);
        }

        // Tuple literal: (a, b, c) or single item (a,)
        if expr.starts_with('(') && expr.ends_with(')') {
            let inner = &expr[1..expr.len() - 1];
            if inner.trim().is_empty() {
                return Ok(Value::Tuple(Vec::new()));
            }
            let items = split_args(inner);
            if items.len() == 1 && !inner.trim().ends_with(',') {
                // Grouped expression like (1 + 2)
                return self.eval_expr(&items[0], line_no);
            }
            let mut values = Vec::with_capacity(items.len());
            for item in items {
                values.push(self.eval_expr(&item, line_no)?);
            }
            return Ok(Value::Tuple(values));
        }

        // List literal: [item1, item2, ...]  or  list comprehension: [expr for var in iterable]
        if expr.starts_with('[') && expr.ends_with(']') {
            let inner = &expr[1..expr.len() - 1];
            if inner.trim().is_empty() {
                return Ok(Value::List(Vec::new()));
            }
            // Check for list comprehension: contains " for " outside brackets/quotes
            if let Some(for_pos) = find_keyword_outside(inner, " for ") {
                let output_expr = inner[..for_pos].trim();
                let rest = inner[for_pos + 5..].trim(); // after " for "
                // Parse: var in iterable [if condition]
                let in_pos = find_keyword_outside(rest, " in ")
                    .ok_or_else(|| line_error(line_no, "list comprehension: expected 'in'"))?;
                let var_name = rest[..in_pos].trim();
                let iter_rest = rest[in_pos + 4..].trim();
                // Check for optional "if" condition
                let (iter_expr, filter_expr) = if let Some(if_pos) = find_keyword_outside(iter_rest, " if ") {
                    (&iter_rest[..if_pos], Some(iter_rest[if_pos + 4..].trim()))
                } else {
                    (iter_rest, None)
                };
                // Evaluate the iterable
                let iterable = self.eval_expr(iter_expr, line_no)?;
                let items: Vec<Value> = match &iterable {
                    Value::List(items) => items.clone(),
                    Value::Tuple(items) => items.clone(),
                    Value::Text(s) => s.chars().map(|c| Value::Text(c.to_string())).collect(),
                    _ => return Err(line_error(line_no, "list comprehension requires an iterable")),
                };
                // Push a scope for the loop variable
                self.scopes.push(HashMap::new());
                let mut result = Vec::new();
                for item in items {
                    self.set_var(var_name, item);
                    // Check filter condition
                    if let Some(cond) = filter_expr {
                        if !self.eval_expr(cond, line_no)?.as_bool() {
                            continue;
                        }
                    }
                    result.push(self.eval_expr(output_expr, line_no)?);
                }
                self.scopes.pop();
                return Ok(Value::List(result));
            }
            let items = split_args(inner);
            let mut values = Vec::with_capacity(items.len());
            for item in items {
                values.push(self.eval_expr(&item, line_no)?);
            }
            return Ok(Value::List(values));
        }

        // Dict literal: {key: value, ...}
        if expr.starts_with('{') && expr.ends_with('}') {
            let inner = &expr[1..expr.len() - 1];
            if inner.trim().is_empty() {
                return Ok(Value::Dict(HashMap::new()));
            }
            let pairs = split_args(inner);
            let mut map = HashMap::new();
            for pair in pairs {
                let (key_str, val_str) = pair.split_once(':')
                    .ok_or_else(|| line_error(line_no, "dict item must be key: value"))?;
                let key = self.eval_expr(key_str.trim(), line_no)?;
                let key_text = match key {
                    Value::Text(s) => s,
                    Value::Number(n) => format!("{}", n as i64),
                    Value::Bool(b) => b.to_string(),
                    _ => return Err(line_error(line_no, "dict key must be string or number")),
                };
                let val = self.eval_expr(val_str.trim(), line_no)?;
                map.insert(key_text, val);
            }
            return Ok(Value::Dict(map));
        }

        // Slice: name[start:stop] or name[start:stop:step] (check before simple subscript)
        if let Some(bracket_start) = expr.rfind('[') {
            if expr.ends_with(']') {
                let name = &expr[..bracket_start];
                let inner = &expr[bracket_start + 1..expr.len() - 1];
                if !name.is_empty() && !inner.is_empty() {
                    if inner.contains(':') {
                        let container = self.eval_expr(name, line_no)?;
                        return Ok(self.eval_slice(&container, inner, line_no)?);
                    }
                    // Simple subscript with negative index support
                    let container = self.eval_expr(name, line_no)?;
                    let index_val = self.eval_expr(inner, line_no)?;
                    return self.eval_subscript(&container, &index_val, line_no);
                }
            }
        }

        // F-string
        if let Some(template) = parse_f_string(expr) {
            return Ok(Value::Text(self.eval_f_string(template, line_no)?));
        }
        // Quoted string
        if let Some(value) = parse_quoted(expr) {
            return Ok(Value::Text(value));
        }

        // Ternary: value if condition else other
        // Must be checked carefully - find " if " then " else " outside quotes/parens
        if let Some(ternary) = self.parse_ternary(expr) {
            let (true_expr, cond_expr, false_expr) = ternary;
            let cond = self.eval_expr(cond_expr, line_no)?;
            if cond.as_bool() {
                return self.eval_expr(true_expr, line_no);
            } else {
                return self.eval_expr(false_expr, line_no);
            }
        }

        // Boolean operators: or (lowest precedence), then and
        if let Some((left, right)) = split_outside(expr, " or ") {
            let left_val = self.eval_expr(left, line_no)?;
            if left_val.as_bool() { return Ok(left_val); }
            return self.eval_expr(right, line_no);
        }
        if let Some((left, right)) = split_outside(expr, " and ") {
            let left_val = self.eval_expr(left, line_no)?;
            if !left_val.as_bool() { return Ok(left_val); }
            return self.eval_expr(right, line_no);
        }

        // not operator (unary prefix)
        if let Some(rest) = expr.strip_prefix("not ") {
            let val = self.eval_expr(rest.trim(), line_no)?;
            return Ok(Value::Bool(!val.as_bool()));
        }

        // in / not in operators
        if let Some((left, right)) = split_outside(expr, " not in ") {
            let left_val = self.eval_expr(left, line_no)?;
            let right_val = self.eval_expr(right, line_no)?;
            return Ok(Value::Bool(!value_contains(&right_val, &left_val)));
        }
        if let Some((left, right)) = split_outside(expr, " in ") {
            let left_val = self.eval_expr(left, line_no)?;
            let right_val = self.eval_expr(right, line_no)?;
            return Ok(Value::Bool(value_contains(&right_val, &left_val)));
        }

        // is / is not operators
        if let Some((left, right)) = split_outside(expr, " is not ") {
            let left_val = self.eval_expr(left, line_no)?;
            let right_val = self.eval_expr(right, line_no)?;
            return Ok(Value::Bool(!values_equal(&left_val, &right_val)));
        }
        if let Some((left, right)) = split_outside(expr, " is ") {
            let left_val = self.eval_expr(left, line_no)?;
            let right_val = self.eval_expr(right, line_no)?;
            return Ok(Value::Bool(values_equal(&left_val, &right_val)));
        }

        // Comparison operators
        for op in ["==", "!=", ">=", "<=", ">", "<"] {
            if let Some((left, right)) = split_outside(expr, op) {
                let left = self.eval_expr(left, line_no)?;
                let right = self.eval_expr(right, line_no)?;
                return Ok(Value::Bool(compare_values(&left, &right, op)));
            }
        }

        // Binary operators with string/list operations
        // + : string concat, list concat, numeric add
        if let Some((left, op, right)) = split_last_operator(expr, &["+", "-"]) {
            if op == "+" {
                let left_res = self.eval_expr(left, line_no);
                let right_res = self.eval_expr(right, line_no);
                // String concatenation
                if let (Ok(Value::Text(ref a)), Ok(Value::Text(ref b))) = (&left_res, &right_res) {
                    return Ok(Value::Text(format!("{a}{b}")));
                }
                // List concatenation
                if let (Ok(Value::List(ref a)), Ok(Value::List(ref b))) = (&left_res, &right_res) {
                    return Ok(Value::List([&a[..], &b[..]].concat()));
                }
                // Numeric add
                let left_num = left_res?.to_float(line_no)?;
                let right_num = right_res?.to_float(line_no)?;
                return Ok(Value::Number(left_num + right_num));
            }
            // Subtraction (numeric only)
            let left_num = self.eval_number(left, line_no)?;
            let right_num = self.eval_number(right, line_no)?;
            return Ok(Value::Number(left_num - right_num));
        }

        // * / % // : string repeat, list repeat, numeric ops
        if let Some((left, op, right)) = split_last_operator(expr, &["//", "%", "/", "*"]) {
            // Check for string/list * number
            if op == "*" {
                let left_res = self.eval_expr(left, line_no);
                let right_res = self.eval_expr(right, line_no);
                match (&left_res, &right_res) {
                    (Ok(Value::Text(ref s)), Ok(Value::Number(n))) => {
                        let count = (*n as i64).max(0) as usize;
                        return Ok(Value::Text(s.repeat(count)));
                    }
                    (Ok(Value::Number(n)), Ok(Value::Text(ref s))) => {
                        let count = (*n as i64).max(0) as usize;
                        return Ok(Value::Text(s.repeat(count)));
                    }
                    (Ok(Value::List(ref items)), Ok(Value::Number(n))) => {
                        let count = (*n as i64).max(0) as usize;
                        let cloned: Vec<Value> = items.iter().cloned().collect();
                        let mut result = Vec::new();
                        for _ in 0..count {
                            result.extend(items.iter().cloned());
                        }
                        return Ok(Value::List(result));
                    }
                    _ => {}
                }
            }
            // Numeric operations
            let left_num = self.eval_number(left, line_no)?;
            let right_num = self.eval_number(right, line_no)?;
            let result = match op {
                "//" => (left_num / right_num).floor(),
                "%" => left_num % right_num,
                "/" => left_num / right_num,
                _ => left_num * right_num,
            };
            return Ok(Value::Number(result));
        }

        // ** power (right-to-left)
        if let Some((left, _, right)) = split_first_operator(expr, &["**"]) {
            let left_num = self.eval_number(left, line_no)?;
            let right_num = self.eval_number(right, line_no)?;
            return Ok(Value::Number(left_num.powf(right_num)));
        }

        // Unary minus
        if let Some(rest) = expr.strip_prefix('-') {
            if let Ok(value) = rest.parse::<f64>() {
                return Ok(Value::Number(-value));
            }
            if let Some(var_val) = self.get_var(rest.trim()) {
                if let Value::Number(n) = var_val {
                    return Ok(Value::Number(-n));
                }
            }
            let val = self.eval_expr(rest.trim(), line_no)?;
            return Ok(Value::Number(-val.to_float(line_no)?));
        }

        // Method call: obj.method(args)
        if let Some(dot_pos) = find_dot_method(expr) {
            let obj_expr = &expr[..dot_pos];
            let rest = &expr[dot_pos + 1..];
            if let Some((method, args_str)) = parse_call(rest) {
                let obj = self.eval_expr(obj_expr, line_no)?;
                let evaluated = split_args(args_str)
                    .into_iter()
                    .map(|arg| self.eval_expr(&arg, line_no))
                    .collect::<Result<Vec<_>, _>>()?;
                return self.call_method(&obj, method, evaluated, line_no);
            }
        }

        // Built-in function call
        if let Some((name, args)) = parse_call(expr) {
            if let Some(value) = self.eval_builtin(name, args, line_no)? {
                return Ok(value);
            }
            // User-defined function call as expression
            if self.functions.contains_key(name) {
                let evaluated = split_args(args)
                    .into_iter()
                    .map(|arg| self.eval_expr(&arg, line_no))
                    .collect::<Result<Vec<_>, _>>()?;
                return self.call_function_expr(name, evaluated, line_no);
            }
        }

        // Variable lookup
        if let Some(value) = self.get_var(expr) {
            return Ok(value.clone());
        }

        // Boolean literals
        if expr.eq_ignore_ascii_case("true") {
            return Ok(Value::Bool(true));
        }
        if expr.eq_ignore_ascii_case("false") {
            return Ok(Value::Bool(false));
        }

        // Number literal
        if let Ok(value) = expr.parse::<f64>() {
            return Ok(Value::Number(value));
        }

        // Fallback: treat as string
        Ok(Value::Text(expr.to_string()))
    }

    fn parse_ternary<'a>(&self, expr: &'a str) -> Option<(&'a str, &'a str, &'a str)> {
        // Find " if " outside quotes/parens, then " else " after it
        let if_pos = find_keyword_outside(expr, " if ")?;
        let true_expr = &expr[..if_pos];
        let rest = &expr[if_pos + 4..];
        let else_pos = find_keyword_outside(rest, " else ")?;
        let cond = &rest[..else_pos];
        let false_expr = &rest[else_pos + 6..];
        Some((true_expr, cond, false_expr))
    }

    fn eval_subscript(&self, container: &Value, index: &Value, line_no: usize) -> Result<Value, RunError> {
        match container {
            Value::List(items) => {
                let idx_raw = index.to_int(line_no)?;
                let idx = if idx_raw < 0 {
                    (items.len() as i64 + idx_raw) as usize
                } else {
                    idx_raw as usize
                };
                if idx < items.len() {
                    Ok(items[idx].clone())
                } else {
                    Err(line_error(line_no, &format!("list index out of range (idx={}, len={})", idx, items.len())))
                }
            }
            Value::Tuple(items) => {
                let idx_raw = index.to_int(line_no)?;
                let idx = if idx_raw < 0 {
                    (items.len() as i64 + idx_raw) as usize
                } else {
                    idx_raw as usize
                };
                if idx < items.len() {
                    Ok(items[idx].clone())
                } else {
                    Err(line_error(line_no, &format!("tuple index out of range")))
                }
            }
            Value::Text(s) => {
                let chars: Vec<char> = s.chars().collect();
                let idx_raw = index.to_int(line_no)?;
                let idx = if idx_raw < 0 {
                    (chars.len() as i64 + idx_raw) as usize
                } else {
                    idx_raw as usize
                };
                if idx < chars.len() {
                    Ok(Value::Text(chars[idx].to_string()))
                } else {
                    Err(line_error(line_no, &format!("string index out of range")))
                }
            }
            Value::Dict(map) => {
                let key = match index {
                    Value::Text(s) => s.clone(),
                    Value::Number(n) => format!("{}", *n as i64),
                    other => other.to_script_string(),
                };
                map.get(&key).cloned().ok_or_else(|| line_error(line_no, &format!("key error: {key}")))
            }
            _ => Err(line_error(line_no, "subscript requires a list, tuple, string, or dict")),
        }
    }

    fn eval_slice(&mut self, container: &Value, inner: &str, line_no: usize) -> Result<Value, RunError> {
        let parts: Vec<&str> = inner.split(':').collect();
        let len = match container {
            Value::List(items) => items.len(),
            Value::Text(s) => s.chars().count(),
            Value::Tuple(items) => items.len(),
            _ => return Err(line_error(line_no, "slice requires a list, string, or tuple")),
        };

        let start = if let Some(s) = parts.get(0) {
            let s = s.trim();
            if s.is_empty() { 0i64 } else { self.eval_expr(s, line_no)?.to_int(line_no)? }
        } else { 0 };
        let stop = if let Some(s) = parts.get(1) {
            let s = s.trim();
            if s.is_empty() { len as i64 } else { self.eval_expr(s, line_no)?.to_int(line_no)? }
        } else { len as i64 };
        let step = if let Some(s) = parts.get(2) {
            let s = s.trim();
            if s.is_empty() { 1i64 } else { self.eval_expr(s, line_no)?.to_int(line_no)? }
        } else { 1 };

        if step == 0 { return Err(line_error(line_no, "slice step cannot be 0")); }

        let len_i = len as i64;
        let (start, stop) = if step > 0 {
            let s = if start < 0 { (len_i + start).max(0) } else { start.min(len_i) };
            let e = if stop < 0 { (len_i + stop).max(0) } else { stop.min(len_i) };
            (s as usize, e.max(s) as i64)
        } else {
            let s = if start < 0 { (len_i + start).max(-1) } else { start.min(len_i - 1) };
            let e = if stop < 0 { (len_i + stop).max(-1) } else { stop };
            (s.max(0) as usize, e.max(-1) as i64)
        };

        let stop_usize = stop as usize;
        match container {
            Value::List(items) => {
                let indices: Vec<usize> = if step > 0 {
                    (start..stop_usize).step_by(step as usize).filter(|&i| i < items.len()).collect()
                } else {
                    let mut v = Vec::new();
                    let mut i = start as i64;
                    while i > stop {
                        if i >= 0 && (i as usize) < items.len() { v.push(i as usize); }
                        i += step;
                    }
                    v
                };
                Ok(Value::List(indices.into_iter().map(|i| items[i].clone()).collect()))
            }
            Value::Text(s) => {
                let chars: Vec<char> = s.chars().collect();
                let indices: Vec<usize> = if step > 0 {
                    (start..stop_usize).step_by(step as usize).filter(|&i| i < chars.len()).collect()
                } else {
                    let mut v = Vec::new();
                    let mut i = start as i64;
                    while i > stop {
                        if i >= 0 && (i as usize) < chars.len() { v.push(i as usize); }
                        i += step;
                    }
                    v
                };
                Ok(Value::Text(indices.into_iter().map(|i| chars[i]).collect()))
            }
            Value::Tuple(items) => {
                let indices: Vec<usize> = if step > 0 {
                    (start..stop_usize).step_by(step as usize).filter(|&i| i < items.len()).collect()
                } else {
                    let mut v = Vec::new();
                    let mut i = start as i64;
                    while i > stop {
                        if i >= 0 && (i as usize) < items.len() { v.push(i as usize); }
                        i += step;
                    }
                    v
                };
                Ok(Value::Tuple(indices.into_iter().map(|i| items[i].clone()).collect()))
            }
            _ => unreachable!(),
        }
    }

    fn call_function_expr(&mut self, name: &str, args: Vec<Value>, line_no: usize) -> Result<Value, RunError> {
        let function = self.functions.get(name).copied()
            .ok_or_else(|| line_error(line_no, "unknown function"))?;
        let params = parse_def_params(&self.lines[function.body_start - 1].text)?;
        if params.len() != args.len() {
            return Err(line_error(line_no, &format!("{}() expects {} args, got {}", name, params.len(), args.len())));
        }

        self.scopes.push(HashMap::new());
        for (param, value) in params.iter().zip(args) {
            self.set_var(param, value);
        }

        // Execute function body without runner
        let result = self.execute_block_no_runner(function.body_start, function.body_end);
        self.scopes.pop();

        match result {
            Ok(Flow::Return(value)) => Ok(value),
            Ok(Flow::Continue) | Ok(Flow::Break) | Ok(Flow::ContinueLoop) => Ok(Value::None),
            Ok(Flow::Goto(_)) => Ok(Value::None),
            Err(e) => Err(e),
        }
    }

    fn call_method(&self, obj: &Value, method: &str, args: Vec<Value>, line_no: usize) -> Result<Value, RunError> {
        match obj {
            Value::Text(s) => self.call_str_method(s, method, args, line_no),
            Value::List(items) => self.call_list_method(items, method, args, line_no),
            Value::Dict(map) => self.call_dict_method(map, method, args, line_no),
            _ => Err(line_error(line_no, &format!("{} has no method '{}'", obj.type_name(), method))),
        }
    }

    fn call_str_method(&self, s: &str, method: &str, args: Vec<Value>, line_no: usize) -> Result<Value, RunError> {
        match method {
            "upper" => {
                if !args.is_empty() { return Err(line_error(line_no, "upper() takes no arguments")); }
                Ok(Value::Text(s.to_uppercase()))
            }
            "lower" => {
                if !args.is_empty() { return Err(line_error(line_no, "lower() takes no arguments")); }
                Ok(Value::Text(s.to_lowercase()))
            }
            "strip" => {
                if !args.is_empty() { return Err(line_error(line_no, "strip() takes no arguments")); }
                Ok(Value::Text(s.trim().to_string()))
            }
            "lstrip" => {
                if !args.is_empty() { return Err(line_error(line_no, "lstrip() takes no arguments")); }
                Ok(Value::Text(s.trim_start().to_string()))
            }
            "rstrip" => {
                if !args.is_empty() { return Err(line_error(line_no, "rstrip() takes no arguments")); }
                Ok(Value::Text(s.trim_end().to_string()))
            }
            "split" => {
                let parts = if args.is_empty() {
                    s.split_whitespace().map(|p| Value::Text(p.to_string())).collect()
                } else if args.len() == 1 {
                    let sep = args[0].to_script_string();
                    s.split(&sep).map(|p| Value::Text(p.to_string())).collect()
                } else {
                    return Err(line_error(line_no, "split() takes at most 1 argument"));
                };
                Ok(Value::List(parts))
            }
            "replace" => {
                if args.len() != 2 { return Err(line_error(line_no, "replace() requires 2 arguments")); }
                let old = args[0].to_script_string();
                let new = args[1].to_script_string();
                Ok(Value::Text(s.replace(&old, &new)))
            }
            "find" => {
                if args.is_empty() { return Err(line_error(line_no, "find() requires at least 1 argument")); }
                let substr = args[0].to_script_string();
                let pos = s.find(&substr).map(|i| i as i64).unwrap_or(-1);
                Ok(Value::Number(pos as f64))
            }
            "rfind" => {
                if args.is_empty() { return Err(line_error(line_no, "rfind() requires at least 1 argument")); }
                let substr = args[0].to_script_string();
                let pos = s.rfind(&substr).map(|i| i as i64).unwrap_or(-1);
                Ok(Value::Number(pos as f64))
            }
            "startswith" => {
                if args.len() != 1 { return Err(line_error(line_no, "startswith() requires 1 argument")); }
                let prefix = args[0].to_script_string();
                Ok(Value::Bool(s.starts_with(&prefix)))
            }
            "endswith" => {
                if args.len() != 1 { return Err(line_error(line_no, "endswith() requires 1 argument")); }
                let suffix = args[0].to_script_string();
                Ok(Value::Bool(s.ends_with(&suffix)))
            }
            "count" => {
                if args.len() != 1 { return Err(line_error(line_no, "count() requires 1 argument")); }
                let substr = args[0].to_script_string();
                Ok(Value::Number(s.matches(&substr).count() as f64))
            }
            "join" => {
                if args.len() != 1 { return Err(line_error(line_no, "join() requires 1 argument")); }
                let parts: Vec<String> = match &args[0] {
                    Value::List(items) => items.iter().map(|v| v.to_script_string()).collect(),
                    Value::Tuple(items) => items.iter().map(|v| v.to_script_string()).collect(),
                    _ => return Err(line_error(line_no, "join() requires a list or tuple")),
                };
                Ok(Value::Text(parts.join(s)))
            }
            "isdigit" => { if !args.is_empty() { return Err(line_error(line_no, "isdigit() takes no arguments")); } Ok(Value::Bool(s.chars().all(|c| c.is_ascii_digit()) && !s.is_empty())) }
            "isalpha" => { if !args.is_empty() { return Err(line_error(line_no, "isalpha() takes no arguments")); } Ok(Value::Bool(s.chars().all(|c| c.is_ascii_alphabetic()) && !s.is_empty())) }
            "zfill" => {
                if args.len() != 1 { return Err(line_error(line_no, "zfill() requires 1 argument")); }
                let width = args[0].to_int(line_no)? as usize;
                let len = s.chars().count();
                if len >= width { Ok(Value::Text(s.to_string())) }
                else { Ok(Value::Text(format!("{}{}", "0".repeat(width - len), s))) }
            }
            "center" | "ljust" | "rjust" => {
                if args.is_empty() || args.len() > 2 { return Err(line_error(line_no, &format!("{method}() takes 1-2 arguments"))); }
                let width = args[0].to_int(line_no)? as usize;
                let fill = if args.len() > 1 { args[1].to_script_string() } else { " ".to_string() };
                let fill_char = fill.chars().next().unwrap_or(' ');
                let len = s.chars().count();
                if len >= width { return Ok(Value::Text(s.to_string())); }
                let total_pad = width - len;
                let result = match method {
                    "center" => {
                        let left = total_pad / 2;
                        let right = total_pad - left;
                        format!("{}{}{}", fill_char.to_string().repeat(left), s, fill_char.to_string().repeat(right))
                    }
                    "ljust" => format!("{}{}", s, fill_char.to_string().repeat(total_pad)),
                    "rjust" => format!("{}{}", fill_char.to_string().repeat(total_pad), s),
                    _ => unreachable!(),
                };
                Ok(Value::Text(result))
            }
            _ => Err(line_error(line_no, &format!("str has no method '{}'", method))),
        }
    }

    fn call_list_method(&self, items: &Vec<Value>, method: &str, args: Vec<Value>, line_no: usize) -> Result<Value, RunError> {
        match method {
            "append" => {
                if args.len() != 1 { return Err(line_error(line_no, "append() takes 1 argument")); }
                let mut new_list = items.clone();
                new_list.push(args.into_iter().next().unwrap());
                Ok(Value::List(new_list))
            }
            "insert" => {
                if args.len() != 2 { return Err(line_error(line_no, "insert() takes 2 arguments")); }
                let idx = args[0].to_int(line_no)? as usize;
                let mut new_list = items.clone();
                let pos = idx.min(new_list.len());
                new_list.insert(pos, args.into_iter().nth(1).unwrap());
                Ok(Value::List(new_list))
            }
            "pop" => {
                if args.len() > 1 { return Err(line_error(line_no, "pop() takes at most 1 argument")); }
                let mut new_list = items.clone();
                if new_list.is_empty() { return Err(line_error(line_no, "pop from empty list")); }
                let idx = if args.is_empty() { new_list.len() - 1 } else { args[0].to_int(line_no)? as usize };
                if idx >= new_list.len() { return Err(line_error(line_no, "pop index out of range")); }
                let val = new_list.remove(idx);
                Ok(val)
            }
            "index" => {
                if args.len() != 1 { return Err(line_error(line_no, "index() takes 1 argument")); }
                let pos = items.iter().position(|v| values_equal(v, &args[0]));
                match pos {
                    Some(i) => Ok(Value::Number(i as f64)),
                    None => Err(line_error(line_no, "value not in list")),
                }
            }
            "count" => {
                if args.len() != 1 { return Err(line_error(line_no, "count() takes 1 argument")); }
                let c = items.iter().filter(|v| values_equal(v, &args[0])).count();
                Ok(Value::Number(c as f64))
            }
            "reverse" => {
                if !args.is_empty() { return Err(line_error(line_no, "reverse() takes no arguments")); }
                let mut new_list = items.clone();
                new_list.reverse();
                Ok(Value::List(new_list))
            }
            "extend" => {
                if args.len() != 1 { return Err(line_error(line_no, "extend() takes 1 argument")); }
                let mut new_list = items.clone();
                match &args[0] {
                    Value::List(other) => new_list.extend(other.iter().cloned()),
                    Value::Tuple(other) => new_list.extend(other.iter().cloned()),
                    _ => return Err(line_error(line_no, "extend() requires a list or tuple")),
                }
                Ok(Value::List(new_list))
            }
            "sort" => {
                if !args.is_empty() { return Err(line_error(line_no, "sort() takes no arguments (key not supported)")); }
                let mut new_list = items.clone();
                new_list.sort_by(|a, b| {
                    match (a, b) {
                        (Value::Number(x), Value::Number(y)) => x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal),
                        (Value::Text(x), Value::Text(y)) => x.cmp(y),
                        _ => std::cmp::Ordering::Equal,
                    }
                });
                Ok(Value::List(new_list))
            }
            "copy" => {
                if !args.is_empty() { return Err(line_error(line_no, "copy() takes no arguments")); }
                Ok(Value::List(items.clone()))
            }
            "clear" => {
                if !args.is_empty() { return Err(line_error(line_no, "clear() takes no arguments")); }
                Ok(Value::List(Vec::new()))
            }
            _ => Err(line_error(line_no, &format!("list has no method '{}'", method))),
        }
    }

    fn call_dict_method(&self, map: &HashMap<String, Value>, method: &str, args: Vec<Value>, line_no: usize) -> Result<Value, RunError> {
        match method {
            "keys" => {
                if !args.is_empty() { return Err(line_error(line_no, "keys() takes no arguments")); }
                Ok(Value::List(map.keys().map(|k| Value::Text(k.clone())).collect()))
            }
            "values" => {
                if !args.is_empty() { return Err(line_error(line_no, "values() takes no arguments")); }
                Ok(Value::List(map.values().cloned().collect()))
            }
            "items" => {
                if !args.is_empty() { return Err(line_error(line_no, "items() takes no arguments")); }
                Ok(Value::List(map.iter().map(|(k, v)| Value::Tuple(vec![Value::Text(k.clone()), v.clone()])).collect()))
            }
            "get" => {
                if args.len() < 1 || args.len() > 2 { return Err(line_error(line_no, "get() takes 1-2 arguments")); }
                let key = args[0].to_script_string();
                match map.get(&key) {
                    Some(v) => Ok(v.clone()),
                    None => Ok(if args.len() > 1 { args[1].clone() } else { Value::None }),
                }
            }
            "pop" => {
                if args.len() < 1 || args.len() > 2 { return Err(line_error(line_no, "pop() takes 1-2 arguments")); }
                let key = args[0].to_script_string();
                let mut new_map = map.clone();
                match new_map.remove(&key) {
                    Some(v) => Ok(v),
                    None => Ok(if args.len() > 1 { args[1].clone() } else { Value::None }),
                }
            }
            "update" => {
                if args.len() != 1 { return Err(line_error(line_no, "update() takes 1 argument")); }
                let mut new_map = map.clone();
                match &args[0] {
                    Value::Dict(other) => { for (k, v) in other { new_map.insert(k.clone(), v.clone()); } }
                    _ => return Err(line_error(line_no, "update() requires a dict")),
                }
                Ok(Value::Dict(new_map))
            }
            "clear" => {
                if !args.is_empty() { return Err(line_error(line_no, "clear() takes no arguments")); }
                Ok(Value::Dict(HashMap::new()))
            }
            "copy" => {
                if !args.is_empty() { return Err(line_error(line_no, "copy() takes no arguments")); }
                Ok(Value::Dict(map.clone()))
            }
            _ => Err(line_error(line_no, &format!("dict has no method '{}'", method))),
        }
    }

    fn execute_block_no_runner(&mut self, start: usize, end: usize) -> Result<Flow, RunError> {
        let mut pc = start;
        while pc < end {
            self.steps += 1;
            if self.steps > 500_000 {
                return Err(RunError::Line {
                    line: self.lines[pc].line_no,
                    source: Box::new(RunError::Parse("too many script steps; possible infinite loop".to_string())),
                });
            }

            let line = self.lines[pc].clone();
            let text = line.text.as_str();

            // break statement
            if text == "break" {
                return Ok(Flow::Break);
            }

            // continue statement
            if text == "continue" {
                return Ok(Flow::ContinueLoop);
            }

            // return statement
            if let Some(rest) = text.strip_prefix("return") {
                let rest = rest.trim();
                let value = if rest.is_empty() {
                    Value::None
                } else {
                    self.eval_expr(rest, line.line_no)?
                };
                return Ok(Flow::Return(value));
            }

            // Skip def blocks
            if text.starts_with("def ") {
                let (_, end2) = self.block_bounds(pc)?;
                pc = end2;
                continue;
            }

            // try / except / else / finally
            if text.starts_with("try:") || text == "try:" {
                let (flow, chain_end) = self.execute_try_block_no_runner(pc, end)?;
                match flow {
                    Flow::Return(v) => return Ok(Flow::Return(v)),
                    Flow::Break => return Ok(Flow::Break),
                    Flow::ContinueLoop => return Ok(Flow::ContinueLoop),
                    _ => {}
                }
                pc = chain_end;
                continue;
            }

            // if / elif / else
            if text.starts_with("if ") {
                let base_indent = line.indent;
                let condition = text
                    .strip_prefix("if ")
                    .and_then(|v| v.strip_suffix(':'))
                    .ok_or_else(|| line_error(line.line_no, "if statement must end with ':'"))?;
                let (body_start, body_end) = self.block_bounds(pc)?;
                if self.eval_expr(condition, line.line_no)?.as_bool() {
                    let flow = self.execute_block_no_runner(body_start, body_end)?;
                    match flow {
                        Flow::Return(_) => return Ok(flow),
                        _ => {}
                    }
                } else {
                    // Try elif / else chain
                    let mut next_pc = body_end;
                    let mut handled = false;
                    while next_pc < end {
                        let nl = self.lines[next_pc].clone();
                        if nl.indent != base_indent { break; }
                        if let Some(cond) = nl.text.strip_prefix("elif ") {
                            let cond = cond.strip_suffix(':')
                                .ok_or_else(|| line_error(nl.line_no, "elif must end with ':'"))?;
                            let (bs, be) = self.block_bounds(next_pc)?;
                            if !handled && self.eval_expr(cond, nl.line_no)?.as_bool() {
                                let flow = self.execute_block_no_runner(bs, be)?;
                                match flow {
                                    Flow::Return(_) => return Ok(flow),
                                    _ => {}
                                }
                                handled = true;
                            }
                            next_pc = be;
                        } else if nl.text == "else:" {
                            let (bs, be) = self.block_bounds(next_pc)?;
                            if !handled {
                                let flow = self.execute_block_no_runner(bs, be)?;
                                match flow {
                                    Flow::Return(_) => return Ok(flow),
                                    _ => {}
                                }
                            }
                            next_pc = be;
                            break;
                        } else {
                            break;
                        }
                    }
                    pc = next_pc;
                    continue;
                }
                // After executing the if-body, skip elif/else
                let mut next_pc = body_end;
                let base_indent = line.indent;
                while next_pc < end {
                    let nl2 = self.lines[next_pc].clone();
                    if nl2.indent != base_indent { break; }
                    if nl2.text.starts_with("elif ") || nl2.text == "else:" {
                        let (_, be) = self.block_bounds(next_pc)?;
                        next_pc = be;
                    } else {
                        break;
                    }
                }
                pc = next_pc;
                continue;
            }

            // for loop
            if text.starts_with("for ") {
                let (var, iter) = self.parse_for(text, line.line_no)?;
                let (body_start, body_end) = self.block_bounds(pc)?;
                match iter {
                    ForIter::Range(values) => {
                        for value in values {
                            self.set_var(&var, Value::Number(value as f64));
                            match self.execute_block_no_runner(body_start, body_end)? {
                                Flow::Continue => {}
                                Flow::ContinueLoop => continue,
                                Flow::Break => break,
                                Flow::Return(v) => return Ok(Flow::Return(v)),
                                Flow::Goto(_) => {}
                            }
                        }
                    }
                    ForIter::Values(vals) => {
                        for value in vals {
                            self.set_var(&var, value);
                            match self.execute_block_no_runner(body_start, body_end)? {
                                Flow::Continue => {}
                                Flow::ContinueLoop => continue,
                                Flow::Break => break,
                                Flow::Return(v) => return Ok(Flow::Return(v)),
                                Flow::Goto(_) => {}
                            }
                        }
                    }
                }
                pc = body_end;
                continue;
            }

            // while loop
            if text.starts_with("while ") {
                let condition = text
                    .strip_prefix("while ")
                    .and_then(|v| v.strip_suffix(':'))
                    .ok_or_else(|| line_error(line.line_no, "while must end with ':'"))?;
                let (body_start, body_end) = self.block_bounds(pc)?;
                let mut loop_count = 0usize;
                while self.eval_expr(condition, line.line_no)?.as_bool() {
                    loop_count += 1;
                    if loop_count > 100_000 {
                        return Err(line_error(line.line_no, "while loop ran too many times"));
                    }
                    match self.execute_block_no_runner(body_start, body_end)? {
                        Flow::Continue => {}
                        Flow::ContinueLoop => continue,
                        Flow::Break => break,
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                        Flow::Goto(_) => {}
                    }
                }
                pc = body_end;
                continue;
            }

            // pass
            if text == "pass" || text.trim() == "pass" {
                pc += 1;
                continue;
            }

            // assert
            if let Some(expr) = text.strip_prefix("assert ") {
                let expr = expr.trim();
                let (cond_expr, msg) = if let Some(pos) = find_comma_outside(expr) {
                    (&expr[..pos], expr[pos + 1..].trim())
                } else {
                    (expr, "")
                };
                let val = self.eval_expr(cond_expr, line.line_no)?;
                if !val.as_bool() {
                    let err_msg = if msg.is_empty() {
                        "assertion error".to_string()
                    } else {
                        self.eval_expr(msg, line.line_no)?.to_script_string()
                    };
                    return Err(line_error(line.line_no, &format!("AssertionError: {err_msg}")));
                }
                pc += 1;
                continue;
            }

            // del
            if let Some(target) = text.strip_prefix("del ") {
                let target = target.trim();
                if target.contains('[') {
                    self.del_subscript(target, line.line_no)?;
                } else {
                    self.del_var(target, line.line_no)?;
                }
                pc += 1;
                continue;
            }

            // raise
            if let Some(rest) = text.strip_prefix("raise ") {
                let rest = rest.trim();
                let (exc_type, err_msg) = if rest.is_empty() {
                    ("RuntimeError".to_string(), "raised error".to_string())
                } else {
                    let exc_types = ["ValueError", "TypeError", "RuntimeError", "Exception", "KeyError", "IndexError"];
                    let mut found = None;
                    for exc in &exc_types {
                        if let Some(args) = call_args(rest, exc) {
                            let vals = split_args(args)
                                .into_iter()
                                .map(|arg| self.eval_expr(&arg, line.line_no))
                                .collect::<Result<Vec<_>, _>>()?;
                            found = Some((exc.to_string(), vals.iter().map(|v| v.to_script_string()).collect::<Vec<_>>().join(" ")));
                            break;
                        }
                    }
                    match found {
                        Some((t, m)) => (t, m),
                        None => ("RuntimeError".to_string(), self.eval_expr(rest, line.line_no)?.to_script_string()),
                    }
                };
                return Err(line_error(line.line_no, &format!("{exc_type}: {err_msg}")));
            }

            // Augmented assignment
            if self.try_augmented_assignment(text, line.line_no)?.is_some() {
                pc += 1;
                continue;
            }

            // Tuple unpacking: a, b = expr1, expr2
            if let Some(eq_pos) = find_eq_outside(text) {
                let left = text[..eq_pos].trim();
                let right = text[eq_pos + 1..].trim();
                if left.contains(',') && !left.contains('[') && !left.contains('(') {
                    let var_names: Vec<&str> = left.split(',').map(|s| s.trim()).collect();
                    if var_names.iter().all(|v| !v.is_empty() && v.chars().all(|c| c.is_alphanumeric() || c == '_')) {
                        let values: Vec<Value> = if right.contains(',') && !right.starts_with('[') && !right.starts_with('(') && !right.starts_with('{') {
                            split_args(right).into_iter().map(|arg| self.eval_expr(arg.trim(), line.line_no)).collect::<Result<Vec<_>, _>>()?
                        } else {
                            let val = self.eval_expr(right, line.line_no)?;
                            match val {
                                Value::Tuple(items) => items,
                                Value::List(items) => items,
                                _ => return Err(line_error(line.line_no, "cannot unpack non-iterable")),
                            }
                        };
                        if var_names.len() != values.len() {
                            return Err(line_error(line.line_no, &format!("too many values to unpack (expected {}, got {})", var_names.len(), values.len())));
                        }
                        for (name, value) in var_names.into_iter().zip(values) {
                            self.set_var(name, value);
                        }
                        pc += 1;
                        continue;
                    }
                }
            }

            // Assignment
            if let Some((name, expr)) = split_assignment(text) {
                let value = self.eval_expr(expr, line.line_no)?;
                self.set_var(name.trim(), value);
                pc += 1;
                continue;
            }

            // Function call as statement (e.g., print)
            if let Some(args) = call_args(text, "print") {
                // print in no-runner context: evaluate but discard output
                let _values = split_args(args)
                    .into_iter()
                    .map(|arg| self.eval_expr(&arg, line.line_no))
                    .collect::<Result<Vec<_>, _>>()?;
                pc += 1;
                continue;
            }

            // Bare function call as statement
            if let Some((name, args_str)) = parse_call(text) {
                if self.functions.contains_key(name) {
                    let evaluated = split_args(args_str)
                        .into_iter()
                        .map(|arg| self.eval_expr(&arg, line.line_no))
                        .collect::<Result<Vec<_>, _>>()?;
                    self.call_function_expr(name, evaluated, line.line_no)?;
                } else {
                    // Try evaluating as expression (might be a builtin)
                    let _ = self.eval_expr(text, line.line_no)?;
                }
                pc += 1;
                continue;
            }

            // Expression evaluation (fallback)
            let _ = self.eval_expr(text, line.line_no)?;
            pc += 1;
        }
        Ok(Flow::Continue)
    }

    /// Execute a try/except/else/finally block without runner.
    /// Scans forward from `start_pc` for except/else/finally at the same indent level.
    fn execute_try_block_no_runner(&mut self, start_pc: usize, outer_end: usize) -> Result<(Flow, usize), RunError> {
        let base_indent = self.lines[start_pc].indent;
        let (try_body_start, try_body_end) = self.block_bounds(start_pc)?;

        // Scan for except/else/finally clauses at base_indent after try body
        let mut except_clauses: Vec<(usize, usize, Option<String>, Option<String>)> = Vec::new(); // (body_start, body_end, exc_type, alias)
        let mut else_clause: Option<(usize, usize)> = None;
        let mut finally_clause: Option<(usize, usize)> = None;

        let mut scan = try_body_end;
        while scan < outer_end {
            let sl = &self.lines[scan];
            if sl.indent != base_indent { break; }
            if let Some(rest) = sl.text.strip_prefix("except ") {
                let rest = rest.strip_suffix(':').unwrap_or(rest);
                let (exc_type, alias) = parse_except_header(rest.trim());
                let (bs, be) = self.block_bounds(scan)?;
                except_clauses.push((bs, be, exc_type, alias));
                scan = be;
            } else if sl.text == "except:" {
                let (bs, be) = self.block_bounds(scan)?;
                except_clauses.push((bs, be, None, None));
                scan = be;
            } else if sl.text == "else:" {
                let (bs, be) = self.block_bounds(scan)?;
                else_clause = Some((bs, be));
                scan = be;
            } else if sl.text == "finally:" {
                let (bs, be) = self.block_bounds(scan)?;
                finally_clause = Some((bs, be));
                scan = be;
            } else {
                break;
            }
        }

        let chain_end = scan;

        // Execute try body
        let try_result = self.execute_block_no_runner(try_body_start, try_body_end);

        match try_result {
            Ok(flow) => {
                // No exception — run else clause if present
                let mut final_flow = flow;
                if let Some((bs, be)) = else_clause {
                    match final_flow {
                        Flow::Continue => {
                            final_flow = self.execute_block_no_runner(bs, be)?;
                        }
                        _ => {} // don't run else if try body didn't complete normally
                    }
                }
                // Always run finally
                if let Some((bs, be)) = finally_clause {
                    let fflow = self.execute_block_no_runner(bs, be)?;
                    match fflow {
                        Flow::Return(v) => final_flow = Flow::Return(v),
                        Flow::Break => final_flow = Flow::Break,
                        Flow::ContinueLoop => final_flow = Flow::ContinueLoop,
                        _ => {}
                    }
                }
                Ok((final_flow, chain_end))
            }
            Err(err) => {
                // Try to match an except clause
                let err_msg = format!("{err}");
                let mut matched = false;
                let mut final_flow = Flow::Continue;
                for (bs, be, exc_type, alias) in &except_clauses {
                    let catches = match exc_type {
                        None => true, // bare except:
                        Some(t) => err_msg.starts_with(&format!("{}:", t)) || t == "Exception",
                    };
                    if catches {
                        matched = true;
                        // Set alias variable if specified
                        if let Some(alias_name) = alias {
                            // Extract message part after "Type: "
                            let msg_part = if let Some(colon_pos) = err_msg.find(": ") {
                                err_msg[colon_pos + 2..].to_string()
                            } else {
                                err_msg.clone()
                            };
                            self.set_var(alias_name, Value::Text(msg_part));
                        }
                        let flow = self.execute_block_no_runner(*bs, *be)?;
                        final_flow = flow;
                        break;
                    }
                }
                if !matched {
                    // No except matched — run finally then re-raise
                    if let Some((bs, be)) = finally_clause {
                        let _ = self.execute_block_no_runner(bs, be);
                    }
                    return Err(err);
                }
                // Run finally
                if let Some((bs, be)) = finally_clause {
                    let fflow = self.execute_block_no_runner(bs, be)?;
                    match fflow {
                        Flow::Return(v) => final_flow = Flow::Return(v),
                        Flow::Break => final_flow = Flow::Break,
                        Flow::ContinueLoop => final_flow = Flow::ContinueLoop,
                        _ => {}
                    }
                }
                Ok((final_flow, chain_end))
            }
        }
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
            "len" => {
                let arg = expect_one_arg(name, args.as_slice(), line_no)?;
                let value = self.eval_expr(arg, line_no)?;
                let len = match &value {
                    Value::List(items) => items.len(),
                    Value::Text(s) => s.len(),
                    Value::Dict(map) => map.len(),
                    Value::Tuple(items) => items.len(),
                    _ => return Err(line_error(line_no, "len() requires a list, string, dict, or tuple")),
                };
                Ok(Some(Value::Number(len as f64)))
            }
            "append" => {
                if args.len() != 2 {
                    return Err(line_error(line_no, "append(list, value) requires 2 arguments"));
                }
                let list_val = self.eval_expr(&args[0], line_no)?;
                let item = self.eval_expr(&args[1], line_no)?;
                match list_val {
                    Value::List(mut items) => {
                        items.push(item);
                        Ok(Some(Value::List(items)))
                    }
                    _ => Err(line_error(line_no, "append() first argument must be a list")),
                }
            }
            "pop" => {
                let list_val = self.eval_expr(&args[0], line_no)?;
                match list_val {
                    Value::List(mut items) => {
                        if items.is_empty() {
                            return Err(line_error(line_no, "pop() from empty list"));
                        }
                        let index = if args.len() > 1 {
                            self.eval_expr(&args[1], line_no)?.to_int(line_no)? as usize
                        } else {
                            items.len() - 1
                        };
                        if index >= items.len() {
                            return Err(line_error(line_no, "pop() index out of range"));
                        }
                        let removed = items.remove(index);
                        Ok(Some(removed))
                    }
                    _ => Err(line_error(line_no, "pop() requires a list")),
                }
            }
            "abs" => {
                let arg = expect_one_arg(name, args.as_slice(), line_no)?;
                let value = self.eval_expr(arg, line_no)?;
                Ok(Some(Value::Number(value.to_float(line_no)?.abs())))
            }
            "min" => {
                if args.len() < 2 {
                    return Err(line_error(line_no, "min() requires at least 2 arguments"));
                }
                let mut values = Vec::new();
                for arg in &args {
                    values.push(self.eval_expr(arg, line_no)?.to_float(line_no)?);
                }
                let min_val = values.into_iter().fold(f64::INFINITY, f64::min);
                Ok(Some(Value::Number(min_val)))
            }
            "max" => {
                if args.len() < 2 {
                    return Err(line_error(line_no, "max() requires at least 2 arguments"));
                }
                let mut values = Vec::new();
                for arg in &args {
                    values.push(self.eval_expr(arg, line_no)?.to_float(line_no)?);
                }
                let max_val = values.into_iter().fold(f64::NEG_INFINITY, f64::max);
                Ok(Some(Value::Number(max_val)))
            }
            "round" => {
                let arg = expect_one_arg(name, args.as_slice(), line_no)?;
                let value = self.eval_expr(arg, line_no)?.to_float(line_no)?;
                Ok(Some(Value::Number((value.round() as i64) as f64)))
            }
            "list" => {
                if args.is_empty() {
                    return Ok(Some(Value::List(Vec::new())));
                }
                let arg = expect_one_arg(name, args.as_slice(), line_no)?;
                let value = self.eval_expr(arg, line_no)?;
                let items = match &value {
                    Value::List(items) => items.clone(),
                    Value::Tuple(items) => items.clone(),
                    Value::Text(s) => s.chars().map(|c| Value::Text(c.to_string())).collect(),
                    _ => return Err(line_error(line_no, "list() argument must be a list, tuple, or string")),
                };
                Ok(Some(Value::List(items)))
            }
            "tuple" => {
                if args.is_empty() {
                    return Ok(Some(Value::Tuple(Vec::new())));
                }
                let arg = expect_one_arg(name, args.as_slice(), line_no)?;
                let value = self.eval_expr(arg, line_no)?;
                let items = match &value {
                    Value::List(items) => items.clone(),
                    Value::Tuple(items) => items.clone(),
                    Value::Text(s) => s.chars().map(|c| Value::Text(c.to_string())).collect(),
                    _ => return Err(line_error(line_no, "tuple() argument must be a list, tuple, or string")),
                };
                Ok(Some(Value::Tuple(items)))
            }
            "dict" => {
                if args.is_empty() {
                    return Ok(Some(Value::Dict(HashMap::new())));
                }
                let arg = expect_one_arg(name, args.as_slice(), line_no)?;
                let value = self.eval_expr(arg, line_no)?;
                match &value {
                    Value::List(pairs) | Value::Tuple(pairs) => {
                        let mut map = HashMap::new();
                        for pair in pairs {
                            match pair {
                                Value::Tuple(kv) if kv.len() == 2 => {
                                    let key = match &kv[0] {
                                        Value::Text(s) => s.clone(),
                                        Value::Number(n) => format!("{}", *n as i64),
                                        other => other.to_script_string(),
                                    };
                                    map.insert(key, kv[1].clone());
                                }
                                Value::List(kv) if kv.len() == 2 => {
                                    let key = match &kv[0] {
                                        Value::Text(s) => s.clone(),
                                        Value::Number(n) => format!("{}", *n as i64),
                                        other => other.to_script_string(),
                                    };
                                    map.insert(key, kv[1].clone());
                                }
                                _ => return Err(line_error(line_no, "dict() items must be (key, value) pairs")),
                            }
                        }
                        Ok(Some(Value::Dict(map)))
                    }
                    _ => Err(line_error(line_no, "dict() argument must be a list of (key, value) pairs")),
                }
            }
            "sum" => {
                if args.is_empty() {
                    return Err(line_error(line_no, "sum() requires at least 1 argument"));
                }
                let iterable = self.eval_expr(&args[0], line_no)?;
                let start = if args.len() > 1 {
                    self.eval_expr(&args[1], line_no)?.to_float(line_no)?
                } else {
                    0.0
                };
                let items: Vec<Value> = match &iterable {
                    Value::List(items) => items.clone(),
                    Value::Tuple(items) => items.clone(),
                    _ => return Err(line_error(line_no, "sum() first argument must be a list or tuple")),
                };
                let mut total = start;
                for item in items {
                    total += item.to_float(line_no)?;
                }
                Ok(Some(Value::Number(total)))
            }
            "sorted" => {
                let arg = expect_one_arg(name, args.as_slice(), line_no)?;
                let value = self.eval_expr(arg, line_no)?;
                let mut items: Vec<Value> = match &value {
                    Value::List(items) => items.clone(),
                    Value::Tuple(items) => items.clone(),
                    Value::Text(s) => {
                        let mut chars: Vec<char> = s.chars().collect();
                        chars.sort();
                        return Ok(Some(Value::List(chars.iter().map(|c| Value::Text(c.to_string())).collect())));
                    }
                    _ => return Err(line_error(line_no, "sorted() argument must be a list, tuple, or string")),
                };
                items.sort_by(|a, b| match (a, b) {
                    (Value::Number(x), Value::Number(y)) => {
                        x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal)
                    }
                    (Value::Text(x), Value::Text(y)) => x.cmp(y),
                    _ => std::cmp::Ordering::Equal,
                });
                Ok(Some(Value::List(items)))
            }
            "reversed" => {
                let arg = expect_one_arg(name, args.as_slice(), line_no)?;
                let value = self.eval_expr(arg, line_no)?;
                let items: Vec<Value> = match &value {
                    Value::List(items) => items.clone(),
                    Value::Tuple(items) => items.clone(),
                    Value::Text(s) => {
                        let chars: Vec<char> = s.chars().collect();
                        return Ok(Some(Value::List(
                            chars.into_iter().rev().map(|c| Value::Text(c.to_string())).collect(),
                        )));
                    }
                    _ => return Err(line_error(line_no, "reversed() argument must be a list, tuple, or string")),
                };
                Ok(Some(Value::List(items.into_iter().rev().collect())))
            }
            "enumerate" => {
                let arg = expect_one_arg(name, args.as_slice(), line_no)?;
                let value = self.eval_expr(arg, line_no)?;
                let items: Vec<Value> = match &value {
                    Value::List(items) => items.clone(),
                    Value::Tuple(items) => items.clone(),
                    Value::Text(s) => s.chars().map(|c| Value::Text(c.to_string())).collect(),
                    _ => return Err(line_error(line_no, "enumerate() argument must be a list, tuple, or string")),
                };
                let result: Vec<Value> = items
                    .into_iter()
                    .enumerate()
                    .map(|(i, v)| Value::Tuple(vec![Value::Number(i as f64), v]))
                    .collect();
                Ok(Some(Value::List(result)))
            }
            "zip" => {
                if args.len() < 2 {
                    return Err(line_error(line_no, "zip() requires at least 2 arguments"));
                }
                let mut iterables: Vec<Vec<Value>> = Vec::new();
                for arg in &args {
                    let value = self.eval_expr(arg, line_no)?;
                    match &value {
                        Value::List(items) => iterables.push(items.clone()),
                        Value::Tuple(items) => iterables.push(items.clone()),
                        _ => return Err(line_error(line_no, "zip() arguments must be lists or tuples")),
                    }
                }
                let min_len = iterables.iter().map(|v| v.len()).min().unwrap_or(0);
                let mut result = Vec::new();
                for i in 0..min_len {
                    let tuple: Vec<Value> = iterables.iter().map(|v| v[i].clone()).collect();
                    result.push(Value::Tuple(tuple));
                }
                Ok(Some(Value::List(result)))
            }
            "chr" => {
                let arg = expect_one_arg(name, args.as_slice(), line_no)?;
                let n = self.eval_expr(arg, line_no)?.to_int(line_no)?;
                if let Some(c) = char::from_u32(n as u32) {
                    Ok(Some(Value::Text(c.to_string())))
                } else {
                    Err(line_error(line_no, &format!("chr() argument {} out of range", n)))
                }
            }
            "ord" => {
                let arg = expect_one_arg(name, args.as_slice(), line_no)?;
                let value = self.eval_expr(arg, line_no)?;
                match &value {
                    Value::Text(s) => {
                        let ch = s.chars().next().ok_or_else(|| {
                            line_error(line_no, "ord() argument must be a single character")
                        })?;
                        Ok(Some(Value::Number(ch as u32 as f64)))
                    }
                    _ => Err(line_error(line_no, "ord() argument must be a single character string")),
                }
            }
            "range" => {
                let (start, stop, step) = if args.len() == 1 {
                    let n = self.eval_expr(&args[0], line_no)?.to_int(line_no)?;
                    (0i64, n, 1i64)
                } else if args.len() == 2 {
                    let a = self.eval_expr(&args[0], line_no)?.to_int(line_no)?;
                    let b = self.eval_expr(&args[1], line_no)?.to_int(line_no)?;
                    (a, b, 1i64)
                } else if args.len() == 3 {
                    let a = self.eval_expr(&args[0], line_no)?.to_int(line_no)?;
                    let b = self.eval_expr(&args[1], line_no)?.to_int(line_no)?;
                    let s = self.eval_expr(&args[2], line_no)?.to_int(line_no)?;
                    (a, b, s)
                } else {
                    return Err(line_error(line_no, "range() requires 1-3 arguments"));
                };
                if step == 0 {
                    return Err(line_error(line_no, "range() step cannot be 0"));
                }
                let mut result = Vec::new();
                if step > 0 {
                    let mut i = start;
                    while i < stop {
                        result.push(Value::Number(i as f64));
                        i += step;
                    }
                } else {
                    let mut i = start;
                    while i > stop {
                        result.push(Value::Number(i as f64));
                        i += step;
                    }
                }
                Ok(Some(Value::List(result)))
            }
            "input" => {
                Ok(Some(Value::Text(String::new())))
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
                _ => Ok(None),
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

/// Parse an except header like "ValueError", "ValueError as e", or "" (bare except)
/// Returns (exc_type, alias)
fn parse_except_header(text: &str) -> (Option<String>, Option<String>) {
    let text = text.trim();
    if text.is_empty() {
        return (None, None);
    }
    // Check for "Type as alias"
    if let Some(as_pos) = text.find(" as ") {
        let exc_type = text[..as_pos].trim().to_string();
        let alias = text[as_pos + 4..].trim().to_string();
        return (Some(exc_type), Some(alias));
    }
    (Some(text.to_string()), None)
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

fn find_comma_outside(text: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    let mut depth = 0i32;
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
        if quote.is_some() {
            continue;
        }
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
        if depth == 0 && ch == ',' {
            return Some(idx);
        }
    }
    None
}

/// Find the first `=` outside brackets/quotes that is NOT part of ==, !=, >=, <=
fn find_eq_outside(text: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    let mut depth = 0i32;
    let chars: Vec<char> = text.chars().collect();
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
        if quote.is_some() {
            continue;
        }
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
        if depth == 0 && ch == '=' {
            let next = chars.get(idx + 1);
            let prev = if idx > 0 { chars.get(idx - 1) } else { None };
            if next == Some(&'=') { continue; }
            if prev == Some(&'!') || prev == Some(&'>') || prev == Some(&'<') { continue; }
            return Some(idx);
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
        _ => Err(line_error(line_no, "f-string numeric format requires a number")),
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

fn split_first_operator<'a>(
    text: &'a str,
    ops: &[&'static str],
) -> Option<(&'a str, &'static str, &'a str)> {
    let mut quote: Option<char> = None;
    let mut depth = 0i32;
    for (idx, ch) in text.char_indices() {
        if ch == '"' || ch == '\'' {
            quote = if quote == Some(ch) { None } else if quote.is_none() { Some(ch) } else { quote };
        }
        if quote.is_some() { continue; }
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
        if depth > 0 { continue; }
        for op in ops {
            if text[idx..].starts_with(op) && idx > 0 {
                return Some((&text[..idx], op, &text[idx + op.len()..]));
            }
        }
    }
    None
}

fn find_keyword_outside(text: &str, keyword: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    let mut depth = 0i32;
    for (idx, ch) in text.char_indices() {
        if ch == '"' || ch == '\'' {
            quote = if quote == Some(ch) { None } else if quote.is_none() { Some(ch) } else { quote };
        }
        if quote.is_some() { continue; }
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
        if depth > 0 { continue; }
        if text[idx..].starts_with(keyword) {
            return Some(idx);
        }
    }
    None
}

/// Find the last `.` outside brackets/quotes that is followed by an identifier and `(`.
/// e.g. `a.b[0].c(x)` returns the dot before `c`.
fn find_dot_method(text: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    let mut depth = 0i32;
    let mut last_dot = None;
    for (idx, ch) in text.char_indices() {
        if ch == '"' || ch == '\'' {
            quote = if quote == Some(ch) { None } else if quote.is_none() { Some(ch) } else { quote };
        }
        if quote.is_some() { continue; }
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
        if depth > 0 { continue; }
        if ch == '.' {
            // Check that what follows is an identifier then '('
            let rest = text[idx + 1..].trim_start();
            if rest.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
                // find the '(' after the identifier
                let paren = rest.find('(');
                if let Some(pi) = paren {
                    // make sure there's no space or other operator between ident and '('
                    let maybe_ident = &rest[..pi];
                    if maybe_ident.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                        // also check the character before '.' is not an operator
                        if idx > 0 {
                            let before = text[..idx].chars().next_back().unwrap();
                            if before.is_ascii_alphanumeric() || before == ')' || before == ']' || before == '_' || before == '"' || before == '\'' {
                                last_dot = Some(idx);
                            }
                        }
                    }
                }
            }
        }
    }
    last_dot
}

fn value_contains(container: &Value, item: &Value) -> bool {
    match container {
        Value::List(items) => items.iter().any(|v| values_equal(v, item)),
        Value::Text(s) => match item {
            Value::Text(t) => s.contains(t.as_str()),
            _ => false,
        },
        Value::Tuple(items) => items.iter().any(|v| values_equal(v, item)),
        Value::Dict(map) => match item {
            Value::Text(key) => map.contains_key(key),
            _ => false,
        },
        _ => false,
    }
}

fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => (x - y).abs() <= f64::EPSILON,
        (Value::Text(x), Value::Text(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::None, Value::None) => true,
        (Value::List(x), Value::List(y)) => {
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(a, b)| values_equal(a, b))
        }
        (Value::Tuple(x), Value::Tuple(y)) => {
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(a, b)| values_equal(a, b))
        }
        _ => false,
    }
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

