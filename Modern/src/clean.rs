//! Line-oriented compiler for the 1993 rules without MARS.COM's parser defects.

use std::collections::HashMap;

use crate::compiler::{Compiled, Instruction, Message, MessageKind, Program, MNEMONICS, PARAMS};
use crate::MAXLEN;

struct Line {
    number: u16,
    pc: usize,
    op: u8,
    fields: Vec<String>,
}

pub fn compile_hu93_clean(source: &[u8], core_size: u16) -> Compiled {
    let source = source.split(|byte| *byte == 0x1A).next().unwrap_or_default();
    let source = String::from_utf8_lossy(source).replace("\r\n", "\n").replace('\r', "\n").replace('\0', " ");
    let mut labels = HashMap::new();
    let mut lines = Vec::new();
    let mut messages = Vec::new();
    for (index, raw) in source.lines().enumerate() {
        let number = (index + 1).min(u16::MAX as usize) as u16;
        let words: Vec<_> = raw.split(';').next().unwrap_or("").split_whitespace().collect();
        if words.is_empty() {
            continue;
        }
        let opcode = words.iter().position(|word| MNEMONICS.iter().any(|name| word.eq_ignore_ascii_case(name)));
        let before = opcode.unwrap_or(words.len());
        for label in &words[..before] {
            if !valid_label(label) {
                messages.push(Message { kind: MessageKind::ParameterError, line: number });
                continue;
            }
            let label = label.to_ascii_uppercase();
            if let std::collections::hash_map::Entry::Vacant(entry) = labels.entry(label) {
                entry.insert(lines.len());
            } else {
                messages.push(Message { kind: MessageKind::Duplicated, line: number });
            }
        }
        if let Some(at) = opcode {
            if lines.len() == MAXLEN {
                messages.push(Message { kind: MessageKind::TooLong, line: number });
                continue;
            }
            lines.push(Line {
                number,
                pc: lines.len(),
                op: MNEMONICS.iter().position(|name| words[at].eq_ignore_ascii_case(name)).unwrap() as u8,
                fields: words[at + 1..].iter().map(|word| word.to_string()).collect(),
            });
        }
    }
    let start = labels.get("START").copied().unwrap_or(0) as u16;
    let mut code = Vec::with_capacity(lines.len());
    for line in &lines {
        let (desc_a, desc_b) = PARAMS[line.op as usize];
        let fields = line.fields.as_slice();
        if fields.len() > 2 {
            messages.push(Message { kind: MessageKind::ParameterError, line: line.number });
            code.push(Instruction::default());
            continue;
        }
        let one_dat = line.op == 0 && fields.len() == 1;
        let a = if one_dat { None } else { fields.first().map(String::as_str) };
        let b = if one_dat { fields.first().map(String::as_str) } else { fields.get(1).map(String::as_str) };
        let parsed_a = parse_field(a, desc_a, line.pc, &labels, core_size);
        let parsed_b = parse_field(b, desc_b, line.pc, &labels, core_size);
        match (parsed_a, parsed_b) {
            (Ok((a, ma)), Ok((b, mb))) => {
                code.push(Instruction { op: line.op, modifier: 0, modes: ma | mb << 2, wide_modes: false, a, b })
            }
            (a, b) => {
                for error in [a.err(), b.err()].into_iter().flatten() {
                    messages.push(Message { kind: error, line: line.number });
                }
                code.push(Instruction::default());
            }
        }
    }
    if code.is_empty() {
        messages.push(Message {
            kind: MessageKind::ZeroLength,
            line: source.lines().count().saturating_add(1).min(u16::MAX as usize) as u16,
        });
    }
    Compiled { program: Program { code, start }, messages }
}

pub(crate) fn valid_label(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn parse_field(
    field: Option<&str>,
    desc: u8,
    pc: usize,
    labels: &HashMap<String, usize>,
    core_size: u16,
) -> Result<(u16, u8), MessageKind> {
    let Some(field) = field else {
        if desc & 32 != 0 {
            return Err(MessageKind::MissingParameter);
        }
        return Ok((0, desc >> 6));
    };
    let (mode, expr) = match field.as_bytes()[0] {
        b'#' => (0, &field[1..]),
        b'$' => (1, &field[1..]),
        b'@' => (2, &field[1..]),
        b'<' => (3, &field[1..]),
        _ => (desc >> 6, field),
    };
    if desc >> mode & 1 == 0 {
        return Err(MessageKind::IllegalMode);
    }
    let value = eval(expr, &|name| labels.get(&name.to_ascii_uppercase()).map(|&value| value as i64 - pc as i64))?;
    Ok((value.rem_euclid(core_size as i64) as u16, mode))
}

pub(crate) fn eval(expr: &str, resolve: &dyn Fn(&str) -> Option<i64>) -> Result<i64, MessageKind> {
    let mut parser = Expr { source: expr.as_bytes(), pos: 0, resolve };
    let value = parser.logical_or()?;
    parser.skip_space();
    if parser.pos != parser.source.len() {
        return Err(MessageKind::ParameterError);
    }
    Ok(value)
}

struct Expr<'a> {
    source: &'a [u8],
    pos: usize,
    resolve: &'a dyn Fn(&str) -> Option<i64>,
}

impl Expr<'_> {
    fn logical_or(&mut self) -> Result<i64, MessageKind> {
        let mut value = self.logical_and()?;
        while self.take(b"||") {
            let right = self.logical_and()?;
            value = i64::from(value != 0 || right != 0);
        }
        Ok(value)
    }

    fn logical_and(&mut self) -> Result<i64, MessageKind> {
        let mut value = self.equality()?;
        while self.take(b"&&") {
            let right = self.equality()?;
            value = i64::from(value != 0 && right != 0);
        }
        Ok(value)
    }

    fn equality(&mut self) -> Result<i64, MessageKind> {
        let mut value = self.relational()?;
        loop {
            if self.take(b"==") {
                value = i64::from(value == self.relational()?);
            } else if self.take(b"!=") {
                value = i64::from(value != self.relational()?);
            } else {
                break;
            }
        }
        Ok(value)
    }

    fn relational(&mut self) -> Result<i64, MessageKind> {
        let mut value = self.sum()?;
        loop {
            if self.take(b"<=") {
                value = i64::from(value <= self.sum()?);
            } else if self.take(b">=") {
                value = i64::from(value >= self.sum()?);
            } else if self.take(b"<") {
                value = i64::from(value < self.sum()?);
            } else if self.take(b">") {
                value = i64::from(value > self.sum()?);
            } else {
                break;
            }
        }
        Ok(value)
    }

    fn sum(&mut self) -> Result<i64, MessageKind> {
        let mut value = self.product()?;
        self.skip_space();
        while let Some(op @ (b'+' | b'-')) = self.peek() {
            self.pos += 1;
            let right = self.product()?;
            value = if op == b'+' { value.checked_add(right) } else { value.checked_sub(right) }
                .ok_or(MessageKind::ParameterError)?;
            self.skip_space();
        }
        Ok(value)
    }

    fn product(&mut self) -> Result<i64, MessageKind> {
        let mut value = self.atom()?;
        self.skip_space();
        while let Some(op @ (b'*' | b'/' | b'%')) = self.peek() {
            self.pos += 1;
            let right = self.atom()?;
            value = match op {
                b'*' => value.checked_mul(right).ok_or(MessageKind::ParameterError)?,
                _ if right == 0 => return Err(MessageKind::DivideByZero),
                b'/' => value.checked_div(right).ok_or(MessageKind::ParameterError)?,
                _ => value.checked_rem(right).ok_or(MessageKind::ParameterError)?,
            };
            self.skip_space();
        }
        Ok(value)
    }

    fn atom(&mut self) -> Result<i64, MessageKind> {
        self.skip_space();
        match self.peek() {
            Some(b'+') => {
                self.pos += 1;
                self.atom()
            }
            Some(b'-') => {
                self.pos += 1;
                self.atom()?.checked_neg().ok_or(MessageKind::ParameterError)
            }
            Some(b'!') => {
                self.pos += 1;
                Ok(i64::from(self.atom()? == 0))
            }
            Some(b'(') => {
                self.pos += 1;
                let value = self.logical_or()?;
                self.skip_space();
                if self.peek() != Some(b')') {
                    return Err(MessageKind::ParameterError);
                }
                self.pos += 1;
                Ok(value)
            }
            Some(c) if c.is_ascii_digit() => {
                let start = self.pos;
                while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    self.pos += 1;
                }
                std::str::from_utf8(&self.source[start..self.pos])
                    .unwrap()
                    .parse()
                    .map_err(|_| MessageKind::ParameterError)
            }
            Some(c) if c.is_ascii_alphabetic() || c == b'_' => {
                let start = self.pos;
                while self.peek().is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_') {
                    self.pos += 1;
                }
                let name = std::str::from_utf8(&self.source[start..self.pos]).unwrap();
                (self.resolve)(name).ok_or(MessageKind::Undefined)
            }
            _ => Err(MessageKind::ParameterError),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.source.get(self.pos).copied()
    }

    fn take(&mut self, token: &[u8]) -> bool {
        self.skip_space();
        if self.source.get(self.pos..self.pos + token.len()) == Some(token) {
            self.pos += token.len();
            true
        } else {
            false
        }
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }
}
