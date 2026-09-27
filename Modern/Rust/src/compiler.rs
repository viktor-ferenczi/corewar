//! Redcode compiler, a port of `COMPILE`, `PASS1`, `PASS2` and their helpers in `MARS.ASM`.
//!
//! MARS reads the source through a 16 byte window (`QQ`): `QQ[0]` is the current character and
//! every read shifts the window by one. Labels, mnemonics and symbols are compared on this window,
//! so the port keeps it, together with the 4096 byte read-ahead buffer and a few register values
//! that leak from one routine into the next. These quirks change what compiles and how, see the
//! comments at each of them.

use crate::{modulo, MAXLEN};

const SYMLEN: usize = 16;
const MAXSYMS: usize = 100;
const PRERDLEN: usize = 4096;
/// Number of instructions, also what `DECODEUK` returns for "not a mnemonic".
const COMMS: u8 = 10;

/// Mnemonics in opcode order.
pub const MNEMONICS: [&str; COMMS as usize] = ["DAT", "MOV", "ADD", "SUB", "JMP", "JMZ", "JMN", "DJN", "CMP", "SPL"];

/// Parameter descriptors of the A and B parameter of each opcode, from the `COMMANDS` table.
/// Bits 0..3 enable the `#`, `$`, `@`, `<` modes, bit 4: may be present, bit 5: must be present,
/// bits 6..7: default mode.
const PARAMS: [(u8, u8); COMMS as usize] = [
    (0b0001_1111, 0b0001_1111),
    (0b0111_1111, 0b0111_1110),
    (0b0111_1111, 0b0111_1110),
    (0b0111_1111, 0b0111_1110),
    (0b0111_1110, 0b0001_1111),
    (0b0111_1110, 0b0111_1111),
    (0b0111_1110, 0b0111_1111),
    (0b0111_1110, 0b0111_1111),
    (0b0111_1111, 0b0111_1111),
    (0b0111_1110, 0b0001_1111),
];
const PARAM_CAN: u8 = 16;
const PARAM_MST: u8 = 32;
/// `PARAMX` returns this mode for a parameter that is not there.
const MODE_NONE: u8 = 4;

/// One compiled instruction. `modes` holds the A mode in bits 0..1 and the B mode in bits 2..3,
/// 0 = `#`, 1 = `$`, 2 = `@`, 3 = `<`. Both fields are always in 0..8000.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Instruction {
    pub op: u8,
    pub modes: u8,
    pub a: u16,
    pub b: u16,
}

impl std::fmt::Display for Instruction {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        const MODE: [char; 4] = ['#', '$', '@', '<'];
        let name = MNEMONICS.get(self.op as usize).copied().unwrap_or("???");
        let (ma, mb) = (MODE[(self.modes & 3) as usize], MODE[(self.modes >> 2 & 3) as usize]);
        write!(f, "{name} {ma}{} {mb}{}", self.a, self.b)
    }
}

/// A compiled program, ready to be loaded into the arena.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Program {
    pub code: Vec<Instruction>,
    /// Offset of the `START` label, 0 when there is none.
    pub start: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageKind {
    Undefined,
    Duplicated,
    TooLong,
    MissingParameter,
    ParameterError,
    DivideByZero,
    SymbolTableFull,
    IllegalMode,
    ZeroLength,
}

/// An error message as MARS prints it. Every message counts as an error: MARS refuses to start
/// the war if a program has any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Message {
    pub kind: MessageKind,
    /// The source line counter at the time of the message. MARS counts carriage returns and misses
    /// some of them, so this can be lower than the real line number.
    pub line: u16,
}

impl Message {
    pub fn text(&self, program: &str) -> String {
        let l = self.line;
        let m = program;
        match self.kind {
            MessageKind::Undefined => format!("Undefined symbol at line {l} in program {m} !"),
            MessageKind::Duplicated => format!("Duplicated symbol at line {l} in program {m} !"),
            MessageKind::TooLong => format!("{m} program is too long at line {l} !"),
            MessageKind::MissingParameter => format!("Missing parameter at line {l} in program {m} !"),
            MessageKind::ParameterError => format!("Parameter syntax error at line {l} in program {m} !"),
            MessageKind::DivideByZero => format!("Divide by zero at line {l} in program {m} !"),
            MessageKind::SymbolTableFull => format!("Symbol table full at line {l} in program {m} !"),
            MessageKind::IllegalMode => format!("Illegal addressing mode at line {l} in program {m} !"),
            MessageKind::ZeroLength => format!("Zero length in program {m} !"),
        }
    }
}

/// Sources on which MARS.COM itself would not finish compiling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fatal {
    /// `RCNOSPACE` after a label runs past the end of the file and loops forever.
    Hang { line: u16 },
    /// A `/` or `%` with a quotient outside 16 bits raises the CPU's divide error and DOS aborts MARS.
    DivideOverflow { line: u16 },
}

impl std::fmt::Display for Fatal {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Fatal::Hang { line } => {
                write!(f, "MARS.COM would hang at line {line}: whitespace after a label up to the end of the file")
            }
            Fatal::DivideOverflow { line } => write!(f, "MARS.COM would abort with a divide overflow at line {line}"),
        }
    }
}

impl std::error::Error for Fatal {}

/// Result of compiling one source. The program is only usable when `messages` is empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Compiled {
    pub program: Program,
    pub messages: Vec<Message>,
}

impl Compiled {
    pub fn is_ok(&self) -> bool {
        self.messages.is_empty()
    }
}

/// Compile a Redcode source exactly like MARS.COM does.
pub fn compile(source: &[u8]) -> Result<Compiled, Fatal> {
    let mut c = Compiler::new(source);
    c.pass1()?;
    c.reader.seek0();
    c.pass2()?;
    let start = modulo(c.lookup(&key(b"START\0", |_| false)).unwrap_or(0));
    c.code.truncate(MAXLEN);
    if c.code.is_empty() {
        c.message(MessageKind::ZeroLength);
    }
    Ok(Compiled { program: Program { code: c.code, start }, messages: c.messages })
}

/// DOS file reading through the `PREREAD` buffer (`READBYTE`, `SEEK0`).
struct Reader<'a> {
    file: &'a [u8],
    pos: usize,
    buf: Box<[u8; PRERDLEN]>,
    ptr: usize,
    predata: usize,
    feof: bool,
}

impl<'a> Reader<'a> {
    fn new(file: &'a [u8]) -> Self {
        Reader { file, pos: 0, buf: Box::new([0; PRERDLEN]), ptr: 0, predata: 0, feof: false }
    }

    fn read_byte(&mut self) -> u8 {
        loop {
            if self.predata != 0 {
                let byte = self.buf[self.ptr];
                self.ptr += 1;
                self.predata -= 1;
                return translate(byte);
            }
            if self.feof {
                return 0;
            }
            let n = PRERDLEN.min(self.file.len() - self.pos);
            self.buf[..n].copy_from_slice(&self.file[self.pos..self.pos + n]);
            self.pos += n;
            self.ptr = 0;
            self.predata = n;
            self.feof = n < PRERDLEN;
        }
    }

    /// Only the file pointer and the EOF flag are reset. Bytes left in the buffer (when pass 1
    /// stopped early) are read first, then the file from its start.
    fn seek0(&mut self) {
        self.pos = 0;
        self.feof = false;
    }

    fn exhausted(&self) -> bool {
        self.predata == 0 && (self.feof || self.pos == self.file.len())
    }
}

/// `READBYTE`: bytes from 80H and control characters except CR and NUL become spaces, then
/// letters are upper cased. A NUL in the file reads like the end of the file.
fn translate(byte: u8) -> u8 {
    let byte = if byte >= 0x80 || (byte < 32 && byte != 0x0D && byte != 0) { b' ' } else { byte };
    byte.to_ascii_uppercase()
}

/// A symbol as stored in `SYMTAB`: up to 16 characters, zero padded after the first terminator.
fn key(window: &[u8], terminator: impl Fn(u8) -> bool) -> [u8; SYMLEN] {
    let mut k = [0; SYMLEN];
    for (dst, &c) in k.iter_mut().zip(window) {
        if c < 33 || terminator(c) {
            break;
        }
        *dst = c;
    }
    k
}

fn is_operator(c: u8) -> bool {
    matches!(c, b'+' | b'-' | b'*' | b'/' | b'%')
}

struct Compiler<'a> {
    reader: Reader<'a>,
    qq: [u8; SYMLEN],
    sline: u16,
    cpc: u16,
    symbols: Vec<([u8; SYMLEN], u16)>,
    code: Vec<Instruction>,
    messages: Vec<Message>,
}

impl<'a> Compiler<'a> {
    fn new(source: &'a [u8]) -> Self {
        Compiler {
            reader: Reader::new(source),
            qq: [0; SYMLEN],
            sline: 1,
            cpc: 0,
            symbols: Vec::new(),
            code: Vec::new(),
            messages: Vec::new(),
        }
    }

    fn message(&mut self, kind: MessageKind) {
        self.messages.push(Message { kind, line: self.sline });
    }

    fn rc(&mut self) {
        self.qq.copy_within(1.., 0);
        self.qq[SYMLEN - 1] = self.reader.read_byte();
    }

    fn fill(&mut self) {
        for _ in 0..SYMLEN {
            self.rc();
        }
    }

    fn rcspace(&mut self) {
        while self.qq[0] >= 33 {
            self.rc();
        }
    }

    /// Skips control characters too, CR included (without counting the line). At the end of the
    /// file it never stops.
    fn rcnospace(&mut self) -> Result<(), Fatal> {
        while self.qq[0] < 33 {
            if self.reader.exhausted() && self.qq.iter().all(|&c| c < 33) {
                return Err(Fatal::Hang { line: self.sline });
            }
            self.rc();
        }
        Ok(())
    }

    fn rcenter(&mut self) {
        while self.qq[0] >= 32 {
            self.rc();
        }
    }

    /// `DECODEUK`: a mnemonic is exactly three characters followed by a character below 33.
    fn decode(&self) -> u8 {
        if self.qq[3] >= 33 {
            return COMMS;
        }
        MNEMONICS.iter().position(|m| m.as_bytes() == &self.qq[..3]).map_or(COMMS, |op| op as u8)
    }

    fn lookup(&self, k: &[u8; SYMLEN]) -> Option<u16> {
        self.symbols.iter().find(|(s, _)| s == k).map(|&(_, v)| v)
    }

    /// `SAVESYM`, returns what is left in AL: the low byte of the value, or 0 after the
    /// duplicate message.
    fn save_symbol(&mut self) -> u8 {
        let k = key(&self.qq, |_| false);
        if self.lookup(&k).is_some() {
            self.message(MessageKind::Duplicated);
            return 0;
        }
        self.symbols.push((k, self.cpc));
        self.cpc as u8
    }

    /// Skips a label and the whitespace after it. Returns false at the end of the line.
    fn skip_label(&mut self) -> Result<bool, Fatal> {
        self.rcspace();
        if self.qq[0] < 32 {
            return Ok(false);
        }
        self.rcnospace()?;
        Ok(true)
    }

    fn pass1(&mut self) -> Result<(), Fatal> {
        self.fill();
        'line: loop {
            let c = self.qq[0];
            if c == 0x0D {
                self.sline = self.sline.wrapping_add(1);
            }
            if c == 0 {
                return Ok(());
            }
            if c < 33 {
                self.rc();
                continue;
            }
            // AL decides whether the line is a comment. After a label it still holds what
            // SAVESYM left there, the low byte of the label's value: a label of instruction 59
            // (';') turns the rest of its line into a comment in pass 1 only.
            let mut al = c;
            loop {
                if al == b';' {
                    self.rcenter();
                    continue 'line;
                }
                if self.decode() < COMMS {
                    if self.cpc as usize >= MAXLEN {
                        self.message(MessageKind::TooLong);
                        return Ok(());
                    }
                    self.cpc += 1;
                    self.rcenter();
                    continue 'line;
                }
                if self.symbols.len() >= MAXSYMS {
                    self.message(MessageKind::SymbolTableFull);
                    return Ok(());
                }
                al = self.save_symbol();
                if !self.skip_label()? {
                    continue 'line;
                }
            }
        }
    }

    fn pass2(&mut self) -> Result<(), Fatal> {
        self.sline = 1;
        self.cpc = 0;
        self.fill();
        'line: loop {
            let c = self.qq[0];
            if c == 0x0D {
                self.sline = self.sline.wrapping_add(1);
            }
            if c == 0 {
                return Ok(());
            }
            if c < 33 {
                self.rc();
                continue;
            }
            // Here AL holds COMMS after a label, so only a line starting with ';' is a comment.
            // Everything else after a label, words of a comment included, is read as more labels
            // or a mnemonic.
            let mut al = c;
            let op = loop {
                if al == b';' {
                    self.rcenter();
                    continue 'line;
                }
                let op = self.decode();
                if op < COMMS {
                    break op;
                }
                al = COMMS;
                if !self.skip_label()? {
                    continue 'line;
                }
            };
            self.rcspace();
            let (desc_a, desc_b) = PARAMS[op as usize];
            let Some((mut a, mut mode_a)) = self.paramx(desc_a)? else {
                self.rcenter();
                continue;
            };
            let Some((mut b, mut mode_b)) = self.paramx(desc_b)? else {
                self.rcenter();
                continue;
            };
            // A DAT with one parameter stores it in B.
            if op == 0 && mode_b & MODE_NONE != 0 {
                std::mem::swap(&mut a, &mut b);
                std::mem::swap(&mut mode_a, &mut mode_b);
            }
            if self.cpc as usize >= MAXLEN {
                self.message(MessageKind::TooLong);
                return Ok(());
            }
            let modes = (mode_a & 3) | (mode_b & 3) << 2;
            self.code.push(Instruction { op, modes, a: modulo(a), b: modulo(b) });
            self.cpc += 1;
            self.rcenter();
        }
    }

    /// `PARAMX`: one parameter with its addressing mode. `None` after an error message.
    fn paramx(&mut self, desc: u8) -> Result<Option<(u16, u8)>, Fatal> {
        while self.qq[0] == 32 {
            self.rc();
        }
        let c = self.qq[0];
        if desc & PARAM_CAN == 0 {
            return Ok(Some((0, MODE_NONE)));
        }
        let absent = c == b';' || c < 33;
        if desc & PARAM_MST != 0 && absent {
            self.message(MessageKind::MissingParameter);
            return Ok(None);
        }
        if absent {
            return Ok(Some((0, MODE_NONE)));
        }
        let mode = match c {
            b'#' => Some(0),
            b'$' => Some(1),
            b'@' => Some(2),
            b'<' => Some(3),
            _ => None,
        };
        if mode.is_some() {
            self.rc();
        }
        let mode = mode.unwrap_or(desc >> 6);
        if desc >> mode & 1 == 0 {
            self.message(MessageKind::IllegalMode);
            return Ok(None);
        }
        match self.getparam()? {
            Some(value) => {
                self.rcspace();
                Ok(Some((value, mode)))
            }
            None => {
                self.message(MessageKind::ParameterError);
                Ok(None)
            }
        }
    }

    /// `GETPARAM`: an expression evaluated left to right in 16 bits, without precedence.
    /// `None` on a syntax error, which is any character above space that is not an operator.
    fn getparam(&mut self) -> Result<Option<u16>, Fatal> {
        let sign = self.qq[0];
        if sign == b'+' || sign == b'-' {
            self.rc();
        }
        let mut acc = self.getany();
        if sign == b'-' {
            acc = acc.wrapping_neg();
        }
        loop {
            let c = self.qq[0];
            if c < 33 {
                return Ok(Some(modulo(acc)));
            }
            if !is_operator(c) {
                self.rcspace();
                return Ok(None);
            }
            self.rc();
            let v = self.getany();
            acc = match c {
                b'+' => acc.wrapping_add(v),
                b'-' => acc.wrapping_sub(v),
                b'*' => (v as i16).wrapping_mul(acc as i16) as u16,
                _ => self.divide(acc, v, c == b'%')?,
            };
        }
    }

    /// `IDIV` of the accumulator, zero extended to 32 bits (`XOR DX,DX`, not `CWD`), by a signed
    /// 16 bit divisor.
    fn divide(&mut self, acc: u16, divisor: u16, remainder: bool) -> Result<u16, Fatal> {
        if divisor == 0 {
            self.message(MessageKind::DivideByZero);
            return Ok(if (acc as i16) < 0 { 0x8001 } else { 0x7FFF });
        }
        let (n, d) = (acc as i32, divisor as i16 as i32);
        let q = n / d;
        if q != q as i16 as i32 {
            return Err(Fatal::DivideOverflow { line: self.sline });
        }
        Ok(if remainder { n % d } else { q } as u16)
    }

    /// `GETANY`: a decimal number or a symbol. Symbols are relative to the current instruction.
    /// An empty or undefined operand counts as 1, after the error message.
    fn getany(&mut self) -> u16 {
        let c = self.qq[0];
        if c < 33 {
            self.message(MessageKind::Undefined);
            return 1;
        }
        if c.is_ascii_digit() {
            return self.getnum();
        }
        let value = self.lookup(&key(&self.qq, |c| c == b';' || is_operator(c)));
        while !(self.qq[0] < 33 || self.qq[0] == b';' || is_operator(self.qq[0])) {
            self.rc();
        }
        match value {
            Some(v) => v.wrapping_sub(self.cpc),
            None => {
                self.message(MessageKind::Undefined);
                1
            }
        }
    }

    /// `GETNUM`: stops before a digit that would push the value past 32769, leaving it for
    /// `GETPARAM` to reject.
    fn getnum(&mut self) -> u16 {
        let mut n: u16 = 0;
        loop {
            let d = self.qq[0].wrapping_sub(b'0');
            if d >= 10 || n >= 3277 {
                return n;
            }
            n = n * 10 + d as u16;
            self.rc();
        }
    }
}
