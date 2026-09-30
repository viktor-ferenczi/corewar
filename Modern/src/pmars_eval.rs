//! The operator recursion of pMARS 0.9.2, including its precedence and `==` defects.

use crate::MessageKind;

const EQUAL: u8 = 0;
const NEQU: u8 = 1;
const GTE: u8 = 2;
const LTE: u8 = 3;
const AND: u8 = 4;
const OR: u8 = 5;
const IDENT: u8 = 6;

#[cfg(test)]
fn eval_092(expr: &str, resolve: &dyn Fn(&str) -> Option<i64>, registers: &mut [i64; 26]) -> Result<i64, MessageKind> {
    eval_pmars(expr, resolve, registers, true)
}

pub(crate) fn eval_pmars(
    expr: &str,
    resolve: &dyn Fn(&str) -> Option<i64>,
    registers: &mut [i64; 26],
    quirks: bool,
) -> Result<i64, MessageKind> {
    let mut parser = Parser { source: expr.as_bytes(), pos: 0, saved: 0, resolve, registers, quirks, depth: 0 };
    let value = parser.expression()?;
    parser.skip_space();
    if parser.pos != parser.source.len() {
        return Err(MessageKind::ParameterError);
    }
    Ok(value)
}

struct Parser<'a> {
    source: &'a [u8],
    pos: usize,
    saved: u8,
    resolve: &'a dyn Fn(&str) -> Option<i64>,
    registers: &'a mut [i64; 26],
    quirks: bool,
    depth: usize,
}

impl Parser<'_> {
    fn expression(&mut self) -> Result<i64, MessageKind> {
        self.depth += 1;
        if self.depth > 128 {
            return Err(MessageKind::ParameterError);
        }
        let result = if self.quirks { self.evaluate(-1, 0, IDENT) } else { self.corrected(1) };
        self.depth -= 1;
        result
    }

    fn corrected(&mut self, minimum: i32) -> Result<i64, MessageKind> {
        let mut left = self.value()?;
        loop {
            self.skip_space();
            if matches!(self.peek(), None | Some(b')')) {
                break;
            }
            let start = self.pos;
            let operator = self.operator()?;
            let priority = precedence(operator);
            if priority < minimum {
                self.pos = start;
                break;
            }
            let right = self.corrected(priority + 1)?;
            left = calculate(left, right, operator)?;
        }
        Ok(left)
    }

    fn evaluate(&mut self, previous: i32, left: i64, operator: u8) -> Result<i64, MessageKind> {
        let right = self.value()?;
        self.skip_space();
        if matches!(self.peek(), None | Some(b')')) {
            return calculate(left, right, operator);
        }
        let next = self.operator()?;
        self.saved = 0;
        let (priority, next_priority) = (precedence(operator), precedence(next));
        if priority >= next_priority {
            if next_priority >= previous || priority <= previous {
                self.evaluate(priority, calculate(left, right, operator)?, next)
            } else {
                self.saved = next;
                calculate(left, right, operator)
            }
        } else {
            let nested = self.evaluate(priority, right, next)?;
            let mut result = calculate(left, nested, operator)?;
            if self.saved != 0 && precedence(self.saved) >= previous {
                let saved = self.saved;
                self.saved = 0;
                result = self.evaluate(next_priority, result, saved)?;
            }
            Ok(result)
        }
    }

    fn value(&mut self) -> Result<i64, MessageKind> {
        self.skip_space();
        match self.peek() {
            Some(b'(') => {
                self.pos += 1;
                let value = self.expression()?;
                self.skip_space();
                if self.peek() != Some(b')') {
                    return Err(MessageKind::ParameterError);
                }
                self.pos += 1;
                Ok(value)
            }
            Some(b'-') => {
                self.pos += 1;
                Ok(self.value()?.wrapping_neg())
            }
            Some(b'+') => {
                self.pos += 1;
                self.value()
            }
            Some(b'!') => {
                self.pos += 1;
                Ok(i64::from(self.value()? == 0))
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
                if let Some(value) = (self.resolve)(name) {
                    return Ok(value);
                }
                if name.len() != 1 || !name.as_bytes()[0].is_ascii_alphabetic() {
                    return Err(MessageKind::Undefined);
                }
                let register = (name.as_bytes()[0].to_ascii_uppercase() - b'A') as usize;
                self.skip_space();
                if self.peek() == Some(b'=') && self.source.get(self.pos + 1) != Some(&b'=') {
                    self.pos += 1;
                    let value = self.expression()?;
                    self.registers[register] = value;
                }
                Ok(self.registers[register])
            }
            _ => Err(MessageKind::ParameterError),
        }
    }

    fn operator(&mut self) -> Result<u8, MessageKind> {
        self.skip_space();
        for (text, op) in [
            (b"==".as_slice(), EQUAL),
            (b"!=".as_slice(), NEQU),
            (b">=".as_slice(), GTE),
            (b"<=".as_slice(), LTE),
            (b"&&".as_slice(), AND),
            (b"||".as_slice(), OR),
        ] {
            if self.source.get(self.pos..self.pos + text.len()) == Some(text) {
                self.pos += text.len();
                return Ok(op);
            }
        }
        let op = self.peek().ok_or(MessageKind::ParameterError)?;
        if !matches!(op, b'+' | b'-' | b'*' | b'/' | b'%' | b'<' | b'>') {
            return Err(MessageKind::ParameterError);
        }
        self.pos += 1;
        Ok(op)
    }

    fn peek(&self) -> Option<u8> {
        self.source.get(self.pos).copied()
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }
}

fn precedence(op: u8) -> i32 {
    match op {
        b'*' | b'/' | b'%' => 5,
        b'+' | b'-' => 4,
        b'<' | b'>' | EQUAL | NEQU | GTE | LTE => 3,
        AND => 2,
        OR => 1,
        _ => 0,
    }
}

fn calculate(left: i64, right: i64, op: u8) -> Result<i64, MessageKind> {
    match op {
        b'+' => Ok(left.wrapping_add(right)),
        b'-' => Ok(left.wrapping_sub(right)),
        b'*' => Ok(left.wrapping_mul(right)),
        b'/' if right == 0 => Err(MessageKind::DivideByZero),
        b'/' => left.checked_div(right).ok_or(MessageKind::ParameterError),
        b'%' if right == 0 => Err(MessageKind::DivideByZero),
        b'%' => left.checked_rem(right).ok_or(MessageKind::ParameterError),
        b'<' => Ok(i64::from(left < right)),
        b'>' => Ok(i64::from(left > right)),
        EQUAL => Ok(i64::from(left == right)),
        NEQU => Ok(i64::from(left != right)),
        GTE => Ok(i64::from(left >= right)),
        LTE => Ok(i64::from(left <= right)),
        AND => Ok(i64::from(left != 0 && right != 0)),
        OR => Ok(i64::from(left != 0 || right != 0)),
        IDENT => Ok(right),
        _ => Err(MessageKind::ParameterError),
    }
}

#[cfg(test)]
mod tests {
    use super::eval_092;

    #[test]
    fn version_092_precedence_and_equality() {
        let resolve = |_: &str| None;
        let mut registers = [0; 26];
        assert_eq!(eval_092("1-2*3+4", &resolve, &mut registers).unwrap(), -9);
        assert_eq!(eval_092("10-2*3-1", &resolve, &mut registers).unwrap(), 5);
        assert!(eval_092("1+2*3==7", &resolve, &mut registers).is_err());
        assert_eq!(eval_092("(1+2*3)==7", &resolve, &mut registers).unwrap(), 1);
        assert_eq!(eval_092("q=3*4", &resolve, &mut registers).unwrap(), 12);
        assert_eq!(eval_092("q+1", &resolve, &mut registers).unwrap(), 13);
    }
}
