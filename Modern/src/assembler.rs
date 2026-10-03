//! ICWS'88, ICWS'94, and pMARS source assembly.

use std::collections::HashMap;

use crate::clean::{eval, valid_label};
use crate::compiler::{Compiled, Fatal, Instruction, Message, MessageKind, Program};
use crate::pmars_eval::eval_pmars;
use crate::{Settings, Standard};

pub const OPCODES: [&str; 17] = [
    "DAT", "MOV", "ADD", "SUB", "JMP", "JMZ", "JMN", "DJN", "CMP", "SPL", "SLT", "MUL", "DIV", "MOD", "SEQ", "SNE",
    "NOP",
];
pub const MODIFIERS: [&str; 7] = ["A", "B", "AB", "BA", "F", "X", "I"];
pub const MODES: [char; 8] = ['#', '$', '*', '@', '{', '<', '}', '>'];

#[derive(Clone, Copy)]
pub struct AssemblyContext {
    pub warriors: usize,
    pub rounds: u16,
    pub first: bool,
}

impl Default for AssemblyContext {
    fn default() -> Self {
        Self { warriors: 2, rounds: 1, first: true }
    }
}

#[derive(Clone)]
struct SourceLine {
    number: u16,
    text: String,
}

#[derive(Clone)]
enum Symbol {
    Address(usize),
    Value(i64),
    Text(Vec<String>),
    Counter(u16),
}

struct Head {
    labels: Vec<String>,
    op: Option<String>,
    modifier: Option<String>,
    rest: String,
    /// Text follows the opcode without whitespace up to the end of the line.
    glued_to_end: bool,
}

struct Record {
    number: u16,
    pc: usize,
    op: String,
    modifier: Option<String>,
    operands: String,
}

struct Assembler<'a> {
    settings: &'a Settings,
    context: AssemblyContext,
    symbols: HashMap<String, Symbol>,
    forward: HashMap<String, Symbol>,
    records: Vec<Record>,
    pending: Vec<String>,
    messages: Vec<Message>,
    registers: [i64; 26],
    pc: usize,
    work: usize,
    ended: bool,
}

pub fn compile_88(source: &[u8], settings: &Settings) -> Compiled {
    compile_with_context(source, settings, AssemblyContext::default())
}

pub fn compile_94(source: &[u8], settings: &Settings) -> Compiled {
    compile_with_context(source, settings, AssemblyContext::default())
}

pub fn assemble(source: &[u8], settings: &Settings, context: AssemblyContext) -> Result<Compiled, Fatal> {
    let mut compiled = if settings.hu93_syntax || settings.standard == Standard::Hu93 && settings.quirks {
        crate::compile(source)?
    } else if settings.standard == Standard::Hu93 {
        crate::compile_hu93_clean(source, settings.core_size)
    } else {
        compile_with_context(source, settings, context)
    };
    if compiled.program.code.len() > settings.program_len as usize {
        compiled.messages.push(Message { kind: MessageKind::TooLong, line: 0 });
    }
    Ok(compiled)
}

pub fn compile_with_context(source: &[u8], settings: &Settings, context: AssemblyContext) -> Compiled {
    let mut assembler = Assembler {
        settings,
        context,
        symbols: HashMap::new(),
        forward: HashMap::new(),
        records: Vec::new(),
        pending: Vec::new(),
        messages: Vec::new(),
        registers: [0; 26],
        pc: 0,
        work: 0,
        ended: false,
    };
    if settings.quirks && context.first {
        assembler.registers[22] = context.warriors as i64;
        assembler.registers[18] = context.warriors as i64;
    }
    match read_lines(source, settings) {
        Ok(lines) => {
            assembler.collect_forward(&lines);
            if let Err(message) = assembler.pass1(&lines, 0) {
                assembler.messages.push(message);
            }
        }
        Err(message) => assembler.messages.push(message),
    }
    assembler.finish()
}

fn comment_directive(line: &str, name: &str) -> bool {
    let Some(rest) = line.trim_start().strip_prefix(';') else { return false };
    let word = rest.split(|c: char| !c.is_ascii_alphanumeric() && c != '_').next().unwrap_or("");
    word.eq_ignore_ascii_case(name)
}

fn read_lines(source: &[u8], settings: &Settings) -> Result<Vec<SourceLine>, Message> {
    let mut lines = Vec::new();
    let mut continued = Vec::new();
    let mut continued_at = 0;
    let pmars = settings.standard != Standard::Icws94;
    // pMARS reads with `fgets` into a 256-byte buffer, so the rest of a longer line is read as
    // the next line.
    let buffer = if pmars && settings.quirks { 255 } else { usize::MAX };
    for (index, physical) in source.split(|&byte| byte == b'\n').enumerate() {
        let number = (index + 1).min(u16::MAX as usize) as u16;
        let mut unread = physical;
        loop {
            let (chunk, rest) = unread.split_at(unread.len().min(buffer - continued.len()));
            unread = rest;
            let raw = if pmars {
                chunk.split(|&byte| byte == b'\r').next().unwrap_or_default()
            } else {
                chunk.strip_suffix(b"\r").unwrap_or(chunk)
            };
            if continued.is_empty() {
                continued_at = number;
            }
            let continuation = pmars && raw.ends_with(b"\\");
            continued.extend_from_slice(if continuation { &raw[..raw.len() - 1] } else { raw });
            if continued.len() > 4096 {
                return Err(Message { kind: MessageKind::ParameterError, line: continued_at });
            }
            if !continuation {
                let text = String::from_utf8_lossy(&std::mem::take(&mut continued)).into_owned();
                lines.push(SourceLine { number: continued_at, text });
            }
            if unread.is_empty() {
                break;
            }
        }
    }
    if !continued.is_empty() {
        lines.push(SourceLine { number: continued_at, text: String::from_utf8_lossy(&continued).into_owned() });
    }
    if !pmars {
        let mut normalized = Vec::new();
        for line in lines {
            normalized
                .extend(line.text.split('\r').map(|text| SourceLine { number: line.number, text: text.to_string() }));
        }
        return Ok(normalized);
    }
    if let Some(first) = lines.iter().position(|line| comment_directive(&line.text, "redcode")) {
        let end = lines[first + 1..]
            .iter()
            .position(|line| comment_directive(&line.text, "redcode"))
            .map_or(lines.len(), |offset| first + 1 + offset);
        lines = lines[first + 1..end].to_vec();
    }
    Ok(lines)
}

fn reserved(name: &str) -> bool {
    OPCODES.iter().any(|op| name.eq_ignore_ascii_case(op))
        || ["ORG", "END", "EQU", "FOR", "ROF", "PIN", "LDP", "STP"].iter().any(|op| name.eq_ignore_ascii_case(op))
}

fn identifier(source: &str) -> Option<(&str, &str)> {
    let mut chars = source.char_indices();
    let (_, first) = chars.next()?;
    if !first.is_ascii_alphabetic() && first != '_' {
        return None;
    }
    let end = chars.find(|(_, c)| !c.is_ascii_alphanumeric() && *c != '_').map_or(source.len(), |(index, _)| index);
    Some((&source[..end], &source[end..]))
}

fn head(code: &str, settings: &Settings) -> Result<Head, MessageKind> {
    let mut rest = code.trim_start();
    let mut labels = Vec::new();
    while !rest.is_empty() {
        let (name, tail) = identifier(rest).ok_or(MessageKind::ParameterError)?;
        if reserved(name) {
            let op = name.to_ascii_uppercase();
            let mut operands = tail.trim_start();
            let mut modifier = None;
            if let Some(after_dot) = operands.strip_prefix('.') {
                let (name, tail) = identifier(after_dot.trim_start()).ok_or(MessageKind::ParameterError)?;
                modifier = Some(name.to_ascii_uppercase());
                operands = tail.trim_start();
            }
            let glued_to_end = !tail.is_empty() && !tail.contains(char::is_whitespace);
            return Ok(Head { labels, op: Some(op), modifier, rest: operands.to_string(), glued_to_end });
        }
        if !valid_label(name) || name.eq_ignore_ascii_case("CURLINE") {
            return Err(MessageKind::ParameterError);
        }
        labels.push(name.to_string());
        if settings.standard == Standard::Pmars && labels.len() > 7 {
            return Err(MessageKind::ParameterError);
        }
        rest = if let Some(tail) = tail.strip_prefix(':') {
            if settings.standard == Standard::Icws94 {
                return Err(MessageKind::ParameterError);
            }
            tail
        } else {
            tail
        };
        if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
            return Err(MessageKind::ParameterError);
        }
        rest = rest.trim_start();
    }
    Ok(Head { labels, op: None, modifier: None, rest: String::new(), glued_to_end: false })
}

impl Assembler<'_> {
    fn text_equ(&self) -> bool {
        self.settings.standard != Standard::Icws88 || self.settings.quirks
    }

    fn error(&self, number: u16, kind: MessageKind) -> Message {
        Message { kind, line: number }
    }

    fn collect_forward(&mut self, source: &[SourceLine]) {
        if !self.text_equ() {
            return;
        }
        for line in source {
            if let Ok(head) = head(line.text.split(';').next().unwrap_or(""), self.settings) {
                if head.op.as_deref() == Some("EQU") {
                    for label in head.labels {
                        self.forward.entry(label).or_insert_with(|| Symbol::Text(vec![head.rest.clone()]));
                    }
                }
            }
        }
    }

    fn define(&mut self, labels: &[String], value: Symbol, number: u16) -> bool {
        if labels.iter().any(|label| self.symbols.contains_key(label) || self.predefined(label).is_some()) {
            if !self.settings.quirks {
                self.messages.push(self.error(number, MessageKind::Duplicated));
            }
            return false;
        }
        for label in labels {
            self.symbols.insert(label.clone(), value.clone());
        }
        true
    }

    fn expand(&self, text: &str, forward: bool, multiline: bool, depth: usize) -> Result<Vec<String>, MessageKind> {
        if depth > 32 {
            return Err(MessageKind::ParameterError);
        }
        let mut result = String::new();
        let mut offset = 0;
        let mut concatenated = false;
        while offset < text.len() {
            let tail = &text[offset..];
            if let Some((name, remaining)) = identifier(tail) {
                let len = tail.len() - remaining.len();
                let symbol = self.symbols.get(name).or_else(|| if forward { self.forward.get(name) } else { None });
                match symbol {
                    Some(Symbol::Text(parts)) => {
                        let known = self.symbols.contains_key(name);
                        let parts = if known { parts.as_slice() } else { &parts[..1] };
                        if parts.len() > 1 {
                            if !multiline || !remaining.trim().is_empty() {
                                return Err(MessageKind::ParameterError);
                            }
                            let mut lines = Vec::new();
                            for (index, part) in parts.iter().enumerate() {
                                let expanded = self.expand(part, forward, true, depth + 1)?;
                                for part in expanded {
                                    lines.push(if index == 0 { format!("{result}{part}") } else { part });
                                }
                            }
                            return Ok(lines);
                        }
                        result.push_str(&self.expand(&parts[0], forward, false, depth + 1)?[0]);
                    }
                    Some(Symbol::Counter(value)) => result.push_str(&format!("{value:02}")),
                    _ => result.push_str(name),
                }
                offset += len;
                if reserved(name) {
                    // Like pMARS, copy text glued to an opcode verbatim, so a counter or EQU named
                    // `i` leaves `mov.i` alone.
                    let glued = remaining.find(char::is_whitespace).unwrap_or(remaining.len());
                    result.push_str(&remaining[..glued]);
                    offset += glued;
                }
            } else if tail.starts_with("&&") {
                result.push_str("&&");
                offset += 2;
            } else if let Some(tail) = tail.strip_prefix('&') {
                let (name, remaining) = identifier(tail).ok_or(MessageKind::ParameterError)?;
                let Some(Symbol::Counter(value)) = self.symbols.get(name) else {
                    return Err(MessageKind::ParameterError);
                };
                result.push_str(&format!("{value:02}"));
                concatenated = true;
                offset = text.len() - remaining.len();
            } else {
                let ch = tail.chars().next().unwrap();
                result.push(ch);
                offset += ch.len_utf8();
            }
            if result.len() > 4096 {
                return Err(MessageKind::ParameterError);
            }
        }
        if concatenated {
            self.expand(&result, forward, multiline, depth + 1)
        } else {
            Ok(vec![result])
        }
    }

    fn pass1(&mut self, source: &[SourceLine], depth: usize) -> Result<(), Message> {
        if depth > 32 {
            return Err(self.error(source.first().map_or(0, |line| line.number), MessageKind::ParameterError));
        }
        let mut index = 0;
        while index < source.len() && !self.ended {
            self.work += 1;
            let line = &source[index];
            if self.work > 200_000 {
                return Err(self.error(line.number, MessageKind::TooLong));
            }
            if comment_directive(&line.text, "assert") {
                let expression = line.text.trim_start()[1..].trim_start();
                let expression = expression[6..].trim();
                match self.evaluate(expression, self.pc, false, false) {
                    Ok(0) => self.messages.push(self.error(line.number, MessageKind::AssertionFailed)),
                    Ok(_) => {}
                    Err(kind) if self.settings.standard == Standard::Icws94 => {
                        self.messages.push(self.error(line.number, kind))
                    }
                    Err(_) => {}
                }
                index += 1;
                continue;
            }
            let code = line.text.split(';').next().unwrap_or("").trim();
            if code.is_empty() {
                index += 1;
                continue;
            }
            let original_head = head(code, self.settings);
            if original_head.as_ref().is_ok_and(|head| {
                head.modifier.as_ref().is_some_and(|modifier| !MODIFIERS.contains(&modifier.as_str()))
            }) {
                return Err(self.error(line.number, MessageKind::ParameterError));
            }
            if original_head.as_ref().is_ok_and(|head| head.op.as_deref() == Some("EQU")) {
                let parsed = original_head.unwrap();
                let mut labels = std::mem::take(&mut self.pending);
                labels.extend(parsed.labels);
                let mut text = vec![parsed.rest];
                index += 1;
                while index < source.len() {
                    let continuation = head(source[index].text.split(';').next().unwrap_or(""), self.settings);
                    if let Ok(next) = continuation {
                        if next.labels.is_empty() && next.op.as_deref() == Some("EQU") {
                            text.push(next.rest);
                            index += 1;
                            continue;
                        }
                    }
                    break;
                }
                if labels.is_empty() || text.iter().any(|text| text.is_empty()) {
                    self.messages.push(self.error(line.number, MessageKind::ParameterError));
                } else if self.text_equ() {
                    self.define(&labels, Symbol::Text(text), line.number);
                } else if text.len() != 1 || labels.len() != 1 {
                    self.messages.push(self.error(line.number, MessageKind::ParameterError));
                } else {
                    match self.evaluate(&text[0], 0, true, false) {
                        Ok(value) => {
                            self.define(&labels, Symbol::Value(value), line.number);
                        }
                        Err(kind) => self.messages.push(self.error(line.number, kind)),
                    }
                }
                continue;
            }
            let expanded = if original_head.as_ref().is_ok_and(|head| head.op.as_deref() == Some("FOR")) {
                vec![code.to_string()]
            } else {
                self.expand(code, true, true, 0).map_err(|kind| self.error(line.number, kind))?
            };
            if expanded.len() != 1 || expanded[0] != code {
                let generated: Vec<_> =
                    expanded.into_iter().map(|text| SourceLine { number: line.number, text }).collect();
                self.pass1(&generated, depth + 1)?;
                index += 1;
                continue;
            }
            let parsed = original_head.map_err(|kind| self.error(line.number, kind))?;
            // pMARS copies text glued to the opcode into a static buffer and only ends it at
            // whitespace, so at the end of the line it reads stale bytes after it.
            if self.settings.quirks
                && parsed.glued_to_end
                && !line.text.split(';').next().unwrap_or("").ends_with(char::is_whitespace)
            {
                return Err(self.error(line.number, MessageKind::ParameterError));
            }
            let mut labels = std::mem::take(&mut self.pending);
            labels.extend(parsed.labels);
            let Some(op) = parsed.op else {
                self.pending = labels;
                index += 1;
                continue;
            };
            if op == "FOR" {
                if self.settings.standard == Standard::Icws94 {
                    return Err(self.error(line.number, MessageKind::ParameterError));
                }
                let counter = labels.pop();
                if !self.define(&labels, Symbol::Address(self.pc), line.number) {
                    index += 1;
                    continue;
                }
                let count =
                    self.evaluate(&parsed.rest, self.pc, false, false).map_err(|kind| self.error(line.number, kind))?;
                let mut nesting = 1;
                let mut end = index + 1;
                while end < source.len() {
                    if let Ok(head) = head(source[end].text.split(';').next().unwrap_or(""), self.settings) {
                        match head.op.as_deref() {
                            Some("FOR") => nesting += 1,
                            Some("ROF") => {
                                nesting -= 1;
                                if nesting == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    end += 1;
                }
                let iterations = if end == source.len() {
                    1
                } else if count <= 0 {
                    0
                } else {
                    count.rem_euclid(65536) as u16
                };
                if iterations == 65535 {
                    return Err(self.error(line.number, MessageKind::ParameterError));
                }
                let iterations = if count > 0 && iterations == 0 && self.settings.quirks { 1 } else { iterations };
                for iteration in 1..=iterations {
                    if let Some(counter) = &counter {
                        self.symbols.insert(counter.clone(), Symbol::Counter(iteration));
                    }
                    self.pass1(&source[index + 1..end], depth + 1)?;
                    if self.ended {
                        break;
                    }
                }
                if let Some(counter) = counter {
                    self.symbols.remove(&counter);
                }
                index = end.saturating_add(1);
                continue;
            }
            if matches!(op.as_str(), "ROF" | "PIN" | "LDP" | "STP") {
                return Err(self.error(line.number, MessageKind::ParameterError));
            }
            if self.settings.standard == Standard::Icws88
                && (op == "ORG" || !OPCODES[..11].contains(&op.as_str()) && op != "END")
            {
                return Err(self.error(line.number, MessageKind::ParameterError));
            }
            if !self.define(&labels, Symbol::Address(self.pc), line.number) {
                index += 1;
                continue;
            }
            if op == "END" {
                self.ended = true;
            }
            if op != "ORG" && op != "END" {
                if self.pc >= self.settings.program_len.min(1000) as usize {
                    return Err(self.error(line.number, MessageKind::TooLong));
                }
                self.pc += 1;
            }
            self.records.push(Record {
                number: line.number,
                pc: self.pc - usize::from(op != "ORG" && op != "END"),
                op,
                modifier: parsed.modifier,
                operands: parsed.rest,
            });
            index += 1;
        }
        Ok(())
    }

    fn predefined(&self, name: &str) -> Option<i64> {
        match name {
            "CORESIZE" => Some(self.settings.core_size as i64),
            "MAXLENGTH" => Some(self.settings.program_len as i64),
            "MAXPROCESSES" => Some(self.settings.queue_len as i64),
            "MINDISTANCE" => Some(self.settings.min_distance as i64),
            "MAXCYCLES" => Some(if self.settings.max_steps == 0 { 1i64 << 32 } else { self.settings.max_steps as i64 }),
            "VERSION" if self.settings.standard != Standard::Icws94 => Some(92),
            "WARRIORS" if self.settings.standard != Standard::Icws94 => Some(self.context.warriors as i64),
            "ROUNDS" if self.settings.standard != Standard::Icws94 => Some(self.context.rounds as i64),
            "PSPACESIZE" if self.settings.standard != Standard::Icws94 => {
                let divisor =
                    (1..=16).rev().find(|divisor| (self.settings.core_size as usize).is_multiple_of(*divisor)).unwrap();
                Some((self.settings.core_size as usize / divisor) as i64)
            }
            "READLIMIT" | "WRITELIMIT" if self.settings.standard != Standard::Icws94 => {
                Some(self.settings.core_size as i64)
            }
            _ => None,
        }
    }

    fn evaluate(&mut self, expr: &str, pc: usize, absolute: bool, forward: bool) -> Result<i64, MessageKind> {
        let expanded = self.expand(expr, forward, false, 0)?;
        let expression = if self.settings.quirks {
            expanded[0].chars().filter(|c| !c.is_ascii_whitespace()).collect::<String>()
        } else {
            expanded[0].clone()
        };
        let mut registers = self.registers;
        let resolve = |name: &str| {
            if name == "CURLINE" && self.settings.standard != Standard::Icws94 {
                return Some(pc as i64);
            }
            self.predefined(name).or_else(|| match self.symbols.get(name)? {
                Symbol::Address(value) => Some(*value as i64 - if absolute { 0 } else { pc as i64 }),
                Symbol::Value(value) => Some(*value),
                Symbol::Counter(value) => Some(*value as i64),
                Symbol::Text(_) => None,
            })
        };
        let value = if self.settings.quirks || self.settings.standard == Standard::Pmars {
            eval_pmars(&expression, &resolve, &mut registers, self.settings.quirks)
        } else {
            eval(&expression, &resolve)
        };
        self.registers = registers;
        value
    }

    fn instruction(&mut self, record: &Record) -> Result<Instruction, MessageKind> {
        let mut op = OPCODES.iter().position(|name| *name == record.op).ok_or(MessageKind::ParameterError)? as u8;
        if self.settings.standard == Standard::Icws94 && op == 14 {
            op = 8;
        }
        let expanded = self.expand(&record.operands, true, false, 0)?;
        let fields: Vec<String> = if expanded[0].contains(',') {
            expanded[0].split(',').map(|field| field.trim().to_string()).collect()
        } else if self.settings.standard == Standard::Icws88 {
            let mut fields = Vec::new();
            let mut words = expanded[0].split_whitespace();
            while let Some(word) = words.next() {
                if word.len() == 1 && MODES.contains(&word.chars().next().unwrap()) {
                    fields.push(format!("{word}{}", words.next().ok_or(MessageKind::MissingParameter)?));
                } else {
                    fields.push(word.to_string());
                }
            }
            fields
        } else {
            vec![expanded[0].trim().to_string()]
        };
        if fields.len() > 2 || fields.iter().any(String::is_empty) {
            return Err(MessageKind::MissingParameter);
        }
        let (a, b) = match fields.as_slice() {
            [a, b] => (a.as_str(), b.as_str()),
            [b] if op == 0 => ("#0", b.as_str()),
            [a] if self.settings.standard == Standard::Icws94 => (a.as_str(), "#0"),
            [a] if matches!(op, 4 | 9 | 16) => (a.as_str(), "$0"),
            _ => return Err(MessageKind::MissingParameter),
        };
        let (a, ma) = self.operand(a, record.pc)?;
        let (b, mb) = self.operand(b, record.pc)?;
        if self.settings.standard == Standard::Icws88 {
            if record.modifier.is_some() {
                return Err(MessageKind::ParameterError);
            }
            if !matches!(ma, 0 | 1 | 3 | 5)
                || !matches!(mb, 0 | 1 | 3 | 5)
                || op == 0 && (!matches!(ma, 0 | 5) || !matches!(mb, 0 | 5))
                || matches!(op, 1 | 2 | 3 | 8 | 10) && mb == 0 && !(op == 10 && self.settings.quirks)
                || matches!(op, 4 | 5 | 6 | 7 | 9) && ma == 0
            {
                return Err(MessageKind::IllegalMode);
            }
        }
        let default = match op {
            0 => 4,
            1 | 8 | 14 | 15 => {
                if ma == 0 {
                    2
                } else if mb == 0 {
                    1
                } else {
                    6
                }
            }
            2 | 3 | 11 | 12 | 13 => {
                if ma == 0 {
                    2
                } else if mb == 0 {
                    1
                } else {
                    4
                }
            }
            10 => {
                if ma == 0 {
                    2
                } else {
                    1
                }
            }
            16 if self.settings.standard == Standard::Pmars => 4,
            _ => 1,
        };
        let modifier = match &record.modifier {
            Some(name) => {
                MODIFIERS.iter().position(|candidate| name == candidate).ok_or(MessageKind::ParameterError)? as u8
            }
            None => default,
        };
        Ok(Instruction { op, modifier, modes: ma | mb << 3, wide_modes: true, a, b })
    }

    fn operand(&mut self, field: &str, pc: usize) -> Result<(u16, u8), MessageKind> {
        let field = field.trim();
        let (mode, expression) = field
            .chars()
            .next()
            .and_then(|first| MODES.iter().position(|mode| *mode == first))
            .map_or((1, field), |mode| (mode as u8, &field[1..]));
        let value = self.evaluate(expression, pc, false, true)?;
        if value.unsigned_abs() > (1u64 << 40) {
            return Err(MessageKind::ParameterError);
        }
        Ok((value.rem_euclid(self.settings.core_size as i64) as u16, mode))
    }

    fn finish(mut self) -> Compiled {
        let mut code = Vec::new();
        let mut origin = 0u16;
        let mut origin_line = 0;
        for record in std::mem::take(&mut self.records) {
            if record.op == "ORG" || record.op == "END" {
                if record.operands.is_empty() && record.op == "END" {
                    continue;
                }
                let expression = if record.operands.is_empty() { "0" } else { &record.operands };
                match self.evaluate(expression, record.pc, true, true) {
                    Ok(value) => {
                        if record.op != "END" || self.settings.standard != Standard::Pmars || origin == 0 {
                            origin = value.rem_euclid(self.settings.core_size as i64) as u16;
                            origin_line = record.number;
                        }
                    }
                    Err(kind) => self.messages.push(self.error(record.number, kind)),
                }
            } else {
                match self.instruction(&record) {
                    Ok(instruction) => code.push(instruction),
                    Err(kind) => {
                        self.messages.push(self.error(record.number, kind));
                        code.push(Instruction::default());
                    }
                }
            }
        }
        if code.is_empty() {
            self.messages.push(self.error(0, MessageKind::ZeroLength));
        } else if origin as usize >= code.len() {
            // pMARS 0.9.2 warns and starts outside the core; later versions start in empty core.
            self.messages.push(self.error(origin_line, MessageKind::StartOutside));
        }
        Compiled { program: Program { code, start: origin }, messages: self.messages }
    }
}
