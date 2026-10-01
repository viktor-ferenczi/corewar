//! The simulator, a port of `WAR`, `WAR1`, `LOADA`, `LOADB` and the `C_xxx` instruction routines.

use std::collections::VecDeque;

use crate::{Instruction, Program, Rng, ARENALEN, MAXLEN, QUEUELEN};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standard {
    Pmars,
    Hu93,
    Icws88,
    Icws94,
}

impl Standard {
    pub fn name(self) -> &'static str {
        match self {
            Self::Pmars => "pmars",
            Self::Hu93 => "hu93",
            Self::Icws88 => "88",
            Self::Icws94 => "94",
        }
    }
}

impl std::str::FromStr for Standard {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pmars" => Ok(Self::Pmars),
            "hu93" => Ok(Self::Hu93),
            "88" => Ok(Self::Icws88),
            "94" => Ok(Self::Icws94),
            _ => Err(format!("unknown standard {s}, use pmars, hu93, 88 or 94")),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheduler {
    Slots,
    Fifo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rules {
    pub scheduler: Scheduler,
    pub dat_test: bool,
    pub wrap_placement: bool,
    pub rotate: bool,
    pub dat_scan_len: u16,
}

/// Programs are copied into memory without wrapping around the end of the arena, so memory
/// continues for up to 99 cells after it. Nothing can address those cells.
pub const MEMLEN: usize = ARENALEN as usize + MAXLEN - 1;
const INACTIVE: u16 = 0xFFFF;
/// Steps between two DAT tests, in thousands.
const DAT_TEST_THOUSANDS: u16 = 16;

/// A memory cell (`ITEM`): the instruction and the number of the program that wrote it last,
/// 0 for none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cell {
    pub op: u8,
    pub modifier: u8,
    pub modes: u8,
    pub a: u16,
    pub b: u16,
    pub owner: u16,
}

#[derive(Clone, Copy)]
struct Operand {
    addr: u16,
    cell: Cell,
}

pub mod modifier {
    pub const A: u8 = 0;
    pub const B: u8 = 1;
    pub const AB: u8 = 2;
    pub const BA: u8 = 3;
    pub const F: u8 = 4;
    pub const X: u8 = 5;
    pub const I: u8 = 6;
}

pub fn load_modifier(settings: &Settings, ins: &Instruction) -> u8 {
    if settings.standard != Standard::Hu93 && !settings.hu93_syntax {
        return ins.modifier;
    }
    let immediate_a = ins.modes & 3 == 0;
    if settings.standard != Standard::Hu93 {
        let immediate_b = ins.modes >> 2 & 3 == 0;
        return match ins.op {
            0 => modifier::F,
            1 | 8 => {
                if immediate_a {
                    modifier::AB
                } else if immediate_b {
                    modifier::B
                } else {
                    modifier::I
                }
            }
            2 | 3 => {
                if immediate_a {
                    modifier::AB
                } else if immediate_b {
                    modifier::B
                } else {
                    modifier::F
                }
            }
            _ => modifier::B,
        };
    }
    match ins.op {
        0 => modifier::F,
        1 => {
            if immediate_a {
                modifier::AB
            } else {
                modifier::I
            }
        }
        2 | 3 => {
            if immediate_a {
                modifier::AB
            } else {
                modifier::B
            }
        }
        8 => {
            if settings.quirks {
                modifier::B
            } else {
                modifier::I
            }
        }
        _ => modifier::B,
    }
}

pub fn wide_modes(ins: &Instruction) -> u8 {
    if ins.wide_modes {
        return ins.modes;
    }
    const MODE: [u8; 4] = [0, 1, 3, 5];
    MODE[(ins.modes & 3) as usize] | MODE[(ins.modes >> 2 & 3) as usize] << 3
}

impl Cell {
    pub fn instruction(&self) -> Instruction {
        Instruction { op: self.op, modifier: self.modifier, modes: self.modes, wide_modes: true, a: self.a, b: self.b }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub standard: Standard,
    pub hu93_syntax: bool,
    pub quirks: bool,
    pub rotate: bool,
    pub core_size: u16,
    pub program_len: u16,
    pub min_distance: u16,
    /// Processes per program: up to 256 slots for hu93 or 8000 FIFO entries for other standards.
    pub queue_len: u16,
    /// Programs may execute cells written by others (not `/E`).
    pub exec_other: bool,
    /// Shared steps for hu93, cycles per warrior otherwise; 0 means 2^32.
    pub max_steps: u32,
    /// End a war as a draw when no DAT is left in the arena, checked every 16000 steps. MARS does
    /// this only in statistics mode (`/P`).
    pub dat_test: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self::pmars()
    }
}

impl Settings {
    pub fn hu93() -> Self {
        Settings {
            standard: Standard::Hu93,
            hu93_syntax: false,
            quirks: true,
            rotate: false,
            core_size: ARENALEN,
            program_len: MAXLEN as u16,
            min_distance: 0,
            queue_len: 64,
            exec_other: true,
            max_steps: 600_000,
            dat_test: true,
        }
    }

    pub fn icws88() -> Self {
        Settings {
            standard: Standard::Icws88,
            hu93_syntax: false,
            quirks: false,
            rotate: true,
            core_size: 8000,
            program_len: 100,
            min_distance: 100,
            queue_len: 8000,
            exec_other: true,
            max_steps: 80_000,
            dat_test: false,
        }
    }

    pub fn icws94() -> Self {
        Settings { standard: Standard::Icws94, ..Self::icws88() }
    }

    pub fn pmars() -> Self {
        Settings { standard: Standard::Pmars, ..Self::icws88() }
    }

    pub fn for_standard(standard: Standard) -> Self {
        match standard {
            Standard::Hu93 => Self::hu93(),
            Standard::Icws88 => Self::icws88(),
            Standard::Icws94 => Self::icws94(),
            Standard::Pmars => Self::pmars(),
        }
    }

    pub fn rules(&self) -> Rules {
        Rules {
            scheduler: if self.standard == Standard::Hu93 { Scheduler::Slots } else { Scheduler::Fifo },
            dat_test: self.standard == Standard::Hu93 && self.dat_test,
            wrap_placement: self.standard != Standard::Hu93 || !self.quirks,
            rotate: self.rotate,
            dat_scan_len: self.core_size - u16::from(self.standard == Standard::Hu93 && self.quirks),
        }
    }
}

/// Statistics of one program over the wars played by an engine.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Sum of the number of processes at the end of the wars.
    pub pcs: u32,
    /// Wars where this was the only program left.
    pub wins: u16,
    /// Wars where this program had no process left.
    pub losses: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// Index of the only surviving program, `None` for a draw.
    pub winner: Option<usize>,
    /// Steps executed, including the turns of both programs.
    pub steps: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceError {
    /// `Cannot place many programs into arena !`
    NoPlace,
}

#[derive(Clone, Debug)]
pub struct Warrior {
    pub num: u16,
    pub code: Vec<Instruction>,
    pub start: u16,
    /// Process slots (`PCS`): an arena address, or anything from ARENALEN up for a free slot.
    pub pcs: [u16; QUEUELEN],
    pub fifo: VecDeque<u16>,
    /// Bit i is set when slot i holds an address below ARENALEN.
    active: [u64; QUEUELEN / 64],
    pub currpc: usize,
    /// Process count (`PCNUM`). It counts the starting process even when it could not start.
    pub pcnum: u16,
    pub stats: Stats,
}

impl Warrior {
    fn new(num: u16, program: &Program, max_len: u16) -> Self {
        assert!(!program.code.is_empty() && program.code.len() <= max_len as usize, "Program length error !");
        Warrior {
            num,
            code: program.code.clone(),
            start: program.start,
            pcs: [INACTIVE; QUEUELEN],
            fifo: VecDeque::new(),
            active: [0; QUEUELEN / 64],
            currpc: 0,
            pcnum: 0,
            stats: Stats::default(),
        }
    }

    fn set_pc(&mut self, slot: usize, pc: u16, core_size: u16) {
        self.pcs[slot] = pc;
        let bit = 1u64 << (slot % 64);
        if pc < core_size {
            self.active[slot / 64] |= bit;
        } else {
            self.active[slot / 64] &= !bit;
        }
    }

    /// First slot at or after `from` and before `limit` whose bit is `set`.
    fn find(&self, from: usize, limit: usize, set: bool) -> Option<usize> {
        let mut slot = from;
        while slot < limit {
            let word = if set { self.active[slot / 64] } else { !self.active[slot / 64] };
            let bits = word >> (slot % 64);
            if bits != 0 {
                let found = slot + bits.trailing_zeros() as usize;
                return (found < limit).then_some(found);
            }
            slot = (slot / 64 + 1) * 64;
        }
        None
    }

    /// The slot `WAR1` executes: the first active one from `currpc` on, wrapping around.
    fn next_active(&self, queue_len: usize) -> Option<usize> {
        self.find(self.currpc, queue_len, true).or_else(|| self.find(0, self.currpc, true))
    }
}

/// One MARS run: the arena, the programs and the random generator carried from war to war.
pub struct Engine {
    pub settings: Settings,
    pub rules: Rules,
    pub warriors: Vec<Warrior>,
    pub mem: Vec<Cell>,
    pub rng: Rng,
    /// Wars played (`STATCTR0`).
    pub wars: u16,
    /// Programs with processes left (`NPROG`).
    alive: usize,
    /// DAT cells in the scan range. hu93 quirks omit cell 7999, as `WAR_DATTEST` did.
    dats: u32,
}

#[inline(always)]
fn add(x: u16, y: u16, core_size: u16) -> u16 {
    let s = x as u32 + y as u32;
    if s >= core_size as u32 {
        (s - core_size as u32) as u16
    } else {
        s as u16
    }
}

#[inline(always)]
fn dec(x: u16, core_size: u16) -> u16 {
    if x == 0 {
        core_size - 1
    } else {
        x - 1
    }
}

fn test_zero(modifier: u8, cell: Cell) -> bool {
    match modifier {
        modifier::A | modifier::BA => cell.a == 0,
        modifier::B | modifier::AB => cell.b == 0,
        _ => cell.a == 0 && cell.b == 0,
    }
}

fn test_nonzero(modifier: u8, cell: Cell) -> bool {
    match modifier {
        modifier::A | modifier::BA => cell.a != 0,
        modifier::B | modifier::AB => cell.b != 0,
        _ => cell.a != 0 || cell.b != 0,
    }
}

fn decremented(modifier: u8, cell: Cell, core_size: u16) -> (u16, u16) {
    match modifier {
        modifier::A | modifier::BA => (dec(cell.a, core_size), cell.b),
        modifier::B | modifier::AB => (cell.a, dec(cell.b, core_size)),
        _ => (dec(cell.a, core_size), dec(cell.b, core_size)),
    }
}

fn compare(modifier: u8, a: Cell, b: Cell) -> bool {
    match modifier {
        modifier::A => a.a == b.a,
        modifier::B => a.b == b.b,
        modifier::AB => a.a == b.b,
        modifier::BA => a.b == b.a,
        modifier::F => a.a == b.a && a.b == b.b,
        modifier::X => a.a == b.b && a.b == b.a,
        modifier::I => (a.op, a.modifier, a.modes, a.a, a.b) == (b.op, b.modifier, b.modes, b.a, b.b),
        _ => false,
    }
}

fn less_than(modifier: u8, a: Cell, b: Cell) -> bool {
    match modifier {
        modifier::A => a.a < b.a,
        modifier::B => a.b < b.b,
        modifier::AB => a.a < b.b,
        modifier::BA => a.b < b.a,
        modifier::X => a.a < b.b && a.b < b.a,
        _ => a.a < b.a && a.b < b.b,
    }
}

impl Engine {
    /// An engine with zeroed memory, like the first MARS run after DOS started.
    pub fn new(settings: Settings, programs: &[Program], rng: Rng) -> Self {
        let mem = vec![Cell::default(); settings.core_size as usize + MAXLEN - 1];
        Self::with_memory(settings, programs, rng, mem)
    }

    /// An engine whose memory, the arena and the cells after it, starts with what an earlier
    /// program left there. See `report::Session`.
    pub fn with_memory(settings: Settings, programs: &[Program], rng: Rng, mem: Vec<Cell>) -> Self {
        assert!((1..=if settings.standard == Standard::Hu93 { QUEUELEN as u16 } else { 8000 })
            .contains(&settings.queue_len));
        assert!(settings.core_size >= settings.program_len && settings.min_distance <= settings.core_size / 2);
        assert_eq!(mem.len(), settings.core_size as usize + MAXLEN - 1);
        let warriors =
            programs.iter().enumerate().map(|(i, p)| Warrior::new(i as u16 + 1, p, settings.program_len)).collect();
        let rules = settings.rules();
        Engine { settings, rules, warriors, mem, rng, wars: 0, alive: 0, dats: 0 }
    }

    /// Play one war with random placement and add it to the statistics.
    pub fn war(&mut self) -> Result<Outcome, PlaceError> {
        self.place(None)?;
        Ok(self.fight())
    }

    /// Clear the arena and load the programs, at random positions like MARS or at the given ones,
    /// and return the positions. The cells after the arena are not cleared: a program copied there
    /// in an earlier war still blocks those positions.
    pub fn place(&mut self, positions: Option<&[u16]>) -> Result<Vec<u16>, PlaceError> {
        let mut placed = Vec::with_capacity(self.warriors.len());
        let core_size = self.settings.core_size;
        let wrap_placement = self.rules.wrap_placement;
        let min_distance = self.settings.min_distance;
        let empty = if self.settings.standard == Standard::Hu93 {
            Cell { modifier: modifier::F, ..Cell::default() }
        } else {
            Cell { op: 0, modifier: modifier::F, modes: 1 | (1 << 3), a: 0, b: 0, owner: 0 }
        };
        self.mem[..core_size as usize].fill(empty);
        if wrap_placement {
            self.mem[core_size as usize..].fill(Cell::default());
        }
        self.dats = self.rules.dat_scan_len as u32;
        for w in 0..self.warriors.len() {
            let warrior = &mut self.warriors[w];
            warrior.pcs = [INACTIVE; QUEUELEN];
            warrior.active = [0; QUEUELEN / 64];
            warrior.fifo.clear();
            warrior.currpc = 0;
            warrior.pcnum = 1;
            let len = warrior.code.len();
            let free = |mem: &[Cell], pos: usize| {
                let empty = if wrap_placement {
                    (0..len).all(|i| mem[(pos + i) % core_size as usize].owner == 0)
                } else {
                    mem[pos..pos + len].iter().all(|c| c.owner == 0)
                };
                let separated = placed.iter().all(|&other: &u16| {
                    let distance = (pos as u16).abs_diff(other);
                    distance.min(core_size - distance) >= min_distance
                });
                empty && separated
            };
            let pos = match positions {
                Some(p) => {
                    let pos = p[w];
                    if self.settings.standard != Standard::Hu93 && (pos >= core_size || !free(&self.mem, pos as usize))
                    {
                        return Err(PlaceError::NoPlace);
                    }
                    pos
                }
                None => {
                    if self.settings.standard != Standard::Hu93 && w == 0 {
                        0
                    } else {
                        (0..core_size)
                            .map(|_| self.rng.next_in(core_size))
                            .find(|&pos| free(&self.mem, pos as usize))
                            .ok_or(PlaceError::NoPlace)?
                    }
                }
            };
            // The start address is not reduced modulo ARENALEN. Past the arena the process can
            // never run, but it still counts, so the program can not lose.
            placed.push(pos);
            let warrior = &mut self.warriors[w];
            let start =
                if wrap_placement { add(warrior.start, pos, core_size) } else { warrior.start.wrapping_add(pos) };
            if self.rules.scheduler == Scheduler::Slots {
                warrior.set_pc(0, start, core_size);
            } else {
                warrior.fifo.push_back(start);
            }
            let num = warrior.num;
            for i in 0..len {
                let ins = self.warriors[w].code[i];
                let modifier = load_modifier(&self.settings, &ins);
                let modes = wide_modes(&ins);
                let cell = Cell { op: ins.op, modifier, modes, a: ins.a % core_size, b: ins.b % core_size, owner: num };
                let addr = if wrap_placement { (pos as usize + i) % core_size as usize } else { pos as usize + i };
                self.set_cell(addr, cell);
            }
        }
        self.alive = self.warriors.len();
        Ok(placed)
    }

    /// Write a memory cell, keeping the DAT count of the DAT test up to date.
    pub fn set_cell(&mut self, addr: usize, cell: Cell) {
        if addr < self.rules.dat_scan_len as usize {
            self.dats -= (self.mem[addr].op == 0) as u32;
            self.dats += (cell.op == 0) as u32;
        }
        self.mem[addr] = cell;
    }

    /// Run the war placed by `place` to its end and add it to the statistics (`WAR_WAR`).
    pub fn fight(&mut self) -> Outcome {
        if self.rules.scheduler == Scheduler::Fifo {
            return if self.settings.quirks {
                if self.warriors.len() > 2 {
                    self.fight_pmars_multi()
                } else {
                    self.fight_standard::<true>()
                }
            } else {
                self.fight_standard::<false>()
            };
        }
        if self.settings.core_size == ARENALEN {
            self.fight_inner::<ARENALEN>()
        } else {
            self.fight_inner::<0>()
        }
    }

    fn fight_standard<const FETCHED_B: bool>(&mut self) -> Outcome {
        let n = self.warriors.len();
        let limit = if self.settings.max_steps == 0 { 1u64 << 32 } else { self.settings.max_steps as u64 };
        let mut cycles = vec![0u64; n];
        let mut w = if self.rules.rotate { self.wars as usize % n } else { 0 };
        let mut steps = 0u64;
        let mut idle = 0usize;
        while self.alive >= if n >= 2 { 2 } else { 1 } && idle < n {
            if self.warriors[w].pcnum != 0 && cycles[w] < limit {
                self.step_standard::<FETCHED_B>(w);
                cycles[w] += 1;
                steps += 1;
                idle = 0;
            } else {
                idle += 1;
            }
            w = (w + 1) % n;
        }
        self.finish_standard(steps)
    }

    fn fight_pmars_multi(&mut self) -> Outcome {
        let n = self.warriors.len();
        let limit = if self.settings.max_steps == 0 { 1u64 << 32 } else { self.settings.max_steps as u64 };
        let mut remaining = n as u64 * limit;
        let mut w = if self.rules.rotate { self.wars as usize % n } else { 0 };
        let mut steps = 0;
        while remaining != 0 && self.alive >= 2 {
            if self.warriors[w].pcnum != 0 {
                let alive = self.alive;
                self.step_standard::<true>(w);
                steps += 1;
                if self.alive < alive {
                    remaining = remaining - 1 - (remaining - 1) / alive as u64;
                }
                remaining = remaining.saturating_sub(1);
            }
            w = if w + 1 == n { 0 } else { w + 1 };
        }
        self.finish_standard(steps)
    }

    fn finish_standard(&mut self, steps: u64) -> Outcome {
        self.wars = self.wars.wrapping_add(1);
        let sole_survivor = self.alive == 1;
        let mut winner = None;
        for (i, warrior) in self.warriors.iter_mut().enumerate() {
            warrior.stats.pcs = warrior.stats.pcs.wrapping_add(warrior.pcnum as u32);
            if sole_survivor && warrior.pcnum != 0 {
                warrior.stats.wins = warrior.stats.wins.wrapping_add(1);
                winner = Some(i);
            }
            if warrior.pcnum == 0 {
                warrior.stats.losses = warrior.stats.losses.wrapping_add(1);
            }
        }
        Outcome { winner, steps }
    }

    fn fight_inner<const CORE: u16>(&mut self) -> Outcome {
        let n = self.warriors.len();
        let min_alive = if n >= 2 { 2 } else { 1 };
        let mut remaining: u64 = if self.settings.max_steps == 0 { 1 << 32 } else { self.settings.max_steps as u64 };
        let mut steps = 0;
        let (mut counter_lo, mut counter_hi) = (0u16, 0u16);
        let mut w = if self.rules.rotate { self.wars as usize % n } else { 0 };
        loop {
            if self.warriors[w].pcnum != 0 {
                counter_lo += 1;
                if counter_lo >= 1000 {
                    counter_lo = 0;
                    counter_hi = counter_hi.wrapping_add(1);
                    // With no DAT anywhere nobody can die any more: one more step, then a draw.
                    if counter_hi % DAT_TEST_THOUSANDS == 0 && self.settings.dat_test && self.dats == 0 {
                        remaining = 1;
                    }
                }
                self.step_inner::<CORE>(w);
                steps += 1;
                remaining -= 1;
                if remaining == 0 {
                    break;
                }
            }
            w = if w + 1 == n { 0 } else { w + 1 };
            if self.alive < min_alive {
                break;
            }
        }
        self.wars = self.wars.wrapping_add(1);
        let sole_survivor = self.alive == 1;
        let mut winner = None;
        for (i, warrior) in self.warriors.iter_mut().enumerate() {
            warrior.stats.pcs = warrior.stats.pcs.wrapping_add(warrior.pcnum as u32);
            if sole_survivor && warrior.pcnum != 0 {
                warrior.stats.wins = warrior.stats.wins.wrapping_add(1);
                winner = Some(i);
            }
            if warrior.pcnum == 0 {
                warrior.stats.losses = warrior.stats.losses.wrapping_add(1);
            }
        }
        Outcome { winner, steps }
    }

    /// One step of a program (`WAR1`): execute the instruction of its next process.
    pub fn step(&mut self, w: usize) {
        if self.rules.scheduler == Scheduler::Fifo {
            if self.settings.quirks {
                self.step_standard::<true>(w);
            } else {
                self.step_standard::<false>(w);
            }
            return;
        }
        if self.settings.core_size == ARENALEN {
            self.step_inner::<ARENALEN>(w);
        } else {
            self.step_inner::<0>(w);
        }
    }

    fn step_inner<const CORE: u16>(&mut self, w: usize) {
        let core_size = if CORE == 0 { self.settings.core_size } else { CORE };
        let queue_len = self.settings.queue_len as usize;
        let Some(slot) = self.warriors[w].next_active(queue_len) else {
            return;
        };
        let warrior = &mut self.warriors[w];
        warrior.currpc = slot;
        let num = warrior.num;
        let cpc = warrior.pcs[slot];
        let pc = cpc as usize;
        if !self.settings.exec_other && self.mem[pc].owner != num {
            self.kill(w);
        } else {
            let (a_addr, a_val) = self.load::<CORE>(cpc, self.mem[pc].a, self.mem[pc].modes & 7, num, false);
            let (b_addr, b_val) = self.load::<CORE>(cpc, self.mem[pc].b, self.mem[pc].modes >> 3 & 7, num, true);
            self.execute::<CORE>(w, cpc, a_addr, a_val, b_addr, b_val);
        }
        let warrior = &mut self.warriors[w];
        let slot = warrior.currpc;
        let pc = warrior.pcs[slot];
        if pc < core_size {
            warrior.pcs[slot] = add(pc, 1, core_size);
        }
        warrior.currpc = if slot + 1 >= queue_len { 0 } else { slot + 1 };
    }

    fn step_standard<const FETCHED_B: bool>(&mut self, w: usize) {
        let Some(pc) = self.warriors[w].fifo.pop_front() else { return };
        let num = self.warriors[w].num;
        let fetched = self.mem[pc as usize];
        let a = self.operand::<FETCHED_B>(pc, fetched.a, fetched.modes & 7, num, fetched, false);
        let b = self.operand::<FETCHED_B>(pc, fetched.b, fetched.modes >> 3 & 7, num, fetched, true);
        let m = fetched.modifier;
        let size = self.settings.core_size;
        let mut next = Some(add(pc, 1, size));
        let mut child = None;
        match fetched.op {
            0 => next = None,
            1 => {
                if m == modifier::I {
                    self.mem[b.addr as usize] = Cell { owner: num, ..a.cell };
                } else {
                    self.write_pair(b.addr, m, a.cell.a, a.cell.b, num);
                }
            }
            2 | 3 => self.arithmetic(b.addr, m, a.cell, b.cell, fetched.op, num),
            11..=13 => {
                if !self.arithmetic_extended(b.addr, m, a.cell, b.cell, fetched.op, num) {
                    next = None;
                }
            }
            4 => next = Some(a.addr),
            5 if test_zero(m, b.cell) => next = Some(a.addr),
            6 if test_nonzero(m, b.cell) => next = Some(a.addr),
            7 => {
                let (da, db) = decremented(m, self.mem[b.addr as usize], size);
                self.write_selected(b.addr, m, da, db, num);
                let (test_a, test_b) = decremented(m, b.cell, size);
                let tested = Cell { a: test_a, b: test_b, ..b.cell };
                if test_nonzero(m, tested) {
                    next = Some(a.addr);
                }
            }
            8 if compare(m, a.cell, b.cell) => next = Some(add(pc, 2, size)),
            14 if compare(m, a.cell, b.cell) => next = Some(add(pc, 2, size)),
            15 if !compare(m, a.cell, b.cell) => next = Some(add(pc, 2, size)),
            9 => child = Some(a.addr),
            10 if less_than(m, a.cell, b.cell) => next = Some(add(pc, 2, size)),
            _ => {}
        }
        let warrior = &mut self.warriors[w];
        if let Some(pc) = next {
            warrior.fifo.push_back(pc);
        }
        if let Some(pc) = child {
            if warrior.fifo.len() < self.settings.queue_len as usize {
                warrior.fifo.push_back(pc);
            }
        }
        warrior.pcnum = warrior.fifo.len() as u16;
        if warrior.pcnum == 0 {
            self.alive -= 1;
        }
    }

    fn operand<const FETCHED_B: bool>(
        &mut self,
        pc: u16,
        field: u16,
        mode: u8,
        num: u16,
        fetched: Cell,
        is_b: bool,
    ) -> Operand {
        let size = self.settings.core_size;
        if mode == 0 {
            let cell = if is_b && FETCHED_B { fetched } else { self.mem[pc as usize] };
            return Operand { addr: pc, cell };
        }
        let pointer = add(pc, field, size);
        if mode == 1 {
            return Operand { addr: pointer, cell: self.mem[pointer as usize] };
        }
        let use_a = matches!(mode, 2 | 4 | 6);
        if matches!(mode, 4 | 5) {
            let cell = &mut self.mem[pointer as usize];
            let value = if use_a { &mut cell.a } else { &mut cell.b };
            *value = dec(*value, size);
            cell.owner = num;
        }
        let offset = if use_a { self.mem[pointer as usize].a } else { self.mem[pointer as usize].b };
        let addr = add(pointer, offset, size);
        let cell = self.mem[addr as usize];
        if matches!(mode, 6 | 7) {
            let pointer_cell = &mut self.mem[pointer as usize];
            let value = if use_a { &mut pointer_cell.a } else { &mut pointer_cell.b };
            *value = add(*value, 1, size);
            pointer_cell.owner = num;
        }
        Operand { addr, cell }
    }

    fn write_pair(&mut self, addr: u16, modifier: u8, a: u16, b: u16, num: u16) {
        let dest = &mut self.mem[addr as usize];
        match modifier {
            modifier::A => dest.a = a,
            modifier::B => dest.b = b,
            modifier::AB => dest.b = a,
            modifier::BA => dest.a = b,
            modifier::F | modifier::I => {
                dest.a = a;
                dest.b = b;
            }
            modifier::X => {
                dest.a = b;
                dest.b = a;
            }
            _ => {}
        }
        dest.owner = num;
    }

    fn write_selected(&mut self, addr: u16, modifier: u8, a: u16, b: u16, num: u16) {
        let dest = &mut self.mem[addr as usize];
        match modifier {
            modifier::A | modifier::BA => dest.a = a,
            modifier::B | modifier::AB => dest.b = b,
            _ => {
                dest.a = a;
                dest.b = b;
            }
        }
        dest.owner = num;
    }

    fn arithmetic(&mut self, addr: u16, modifier: u8, source: Cell, target: Cell, op: u8, num: u16) {
        let size = self.settings.core_size;
        let calc = |x: u16, y: u16| {
            if op == 2 {
                add(x, y, size)
            } else {
                ((y as u32 + size as u32 - x as u32) % size as u32) as u16
            }
        };
        let dest = &mut self.mem[addr as usize];
        match modifier {
            modifier::A => dest.a = calc(source.a, target.a),
            modifier::B => dest.b = calc(source.b, target.b),
            modifier::AB => dest.b = calc(source.a, target.b),
            modifier::BA => dest.a = calc(source.b, target.a),
            modifier::F | modifier::I => {
                dest.a = calc(source.a, target.a);
                dest.b = calc(source.b, target.b);
            }
            modifier::X => {
                dest.a = calc(source.b, target.a);
                dest.b = calc(source.a, target.b);
            }
            _ => {}
        }
        dest.owner = num;
    }

    fn arithmetic_extended(&mut self, addr: u16, modifier: u8, source: Cell, target: Cell, op: u8, num: u16) -> bool {
        let size = self.settings.core_size as u32;
        let calc = |x: u16, y: u16| -> Option<u16> {
            match op {
                11 => Some((x as u32 * y as u32 % size) as u16),
                12 if x != 0 => Some(y / x),
                13 if x != 0 => Some(y % x),
                _ => None,
            }
        };
        let fields = match modifier {
            modifier::A => (calc(source.a, target.a), None),
            modifier::B => (None, calc(source.b, target.b)),
            modifier::AB => (None, calc(source.a, target.b)),
            modifier::BA => (calc(source.b, target.a), None),
            modifier::X => (calc(source.b, target.a), calc(source.a, target.b)),
            _ => (calc(source.a, target.a), calc(source.b, target.b)),
        };
        let dest = &mut self.mem[addr as usize];
        if let Some(value) = fields.0 {
            dest.a = value;
            dest.owner = num;
        }
        if let Some(value) = fields.1 {
            dest.b = value;
            dest.owner = num;
        }
        let selected = if matches!(modifier, modifier::A | modifier::BA | modifier::B | modifier::AB) { 1 } else { 2 };
        fields.0.is_some() as u8 + fields.1.is_some() as u8 == selected
    }

    /// `LOADA` / `LOADB`: the address and the value of a parameter. The value is the B field of
    /// the addressed cell, or the field itself for `#`. Indirection always goes through B fields.
    #[inline(always)]
    fn load<const CORE: u16>(&mut self, cpc: u16, field: u16, mode: u8, num: u16, is_b: bool) -> (u16, u16) {
        let core_size = if CORE == 0 { self.settings.core_size } else { CORE };
        let mem = &mut self.mem;
        match mode {
            0 => (cpc, if is_b { mem[cpc as usize].b } else { mem[cpc as usize].a }),
            1 => {
                let addr = add(field, cpc, core_size);
                (addr, mem[addr as usize].b)
            }
            3 => {
                let t = add(field, cpc, core_size);
                let addr = add(t, mem[t as usize].b, core_size);
                (addr, mem[addr as usize].b)
            }
            _ => {
                let t = add(field, cpc, core_size) as usize;
                mem[t].owner = num;
                mem[t].b = dec(mem[t].b, core_size);
                let addr = add(t as u16, mem[t].b, core_size);
                (addr, mem[addr as usize].b)
            }
        }
    }

    #[inline(always)]
    fn execute<const CORE: u16>(&mut self, w: usize, cpc: u16, a_addr: u16, a_val: u16, b_addr: u16, b_val: u16) {
        let core_size = if CORE == 0 { self.settings.core_size } else { CORE };
        let num = self.warriors[w].num;
        let b = b_addr as usize;
        match self.mem[cpc as usize].op {
            0 => self.kill(w),
            1 => {
                self.mem[b].owner = num;
                if self.mem[cpc as usize].modes & 3 == 0 {
                    self.mem[b].b = a_val;
                } else {
                    let src = self.mem[a_addr as usize];
                    self.set_cell(b, Cell { owner: num, ..src });
                }
            }
            2 => self.write_b(b, add(a_val, b_val, core_size), num),
            3 => self.write_b(b, ((b_val as u32 + core_size as u32 - a_val as u32) % core_size as u32) as u16, num),
            4 => self.jump(w, a_addr, core_size),
            5 if b_val == 0 => self.jump(w, a_addr, core_size),
            6 if b_val != 0 => self.jump(w, a_addr, core_size),
            7 => {
                let v = dec(b_val, core_size);
                self.write_b(b, v, num);
                if v != 0 {
                    self.jump(w, a_addr, core_size);
                }
            }
            8 if if self.mem[cpc as usize].modifier == modifier::I && self.mem[cpc as usize].modes & 7 != 0 {
                let a = self.mem[a_addr as usize];
                let b = self.mem[b_addr as usize];
                (a.op, a.modes, a.a, a.b) == (b.op, b.modes, b.a, b.b)
            } else {
                a_val == b_val
            } =>
            {
                let warrior = &mut self.warriors[w];
                warrior.set_pc(warrior.currpc, add(cpc, 1, core_size), core_size);
            }
            9 => {
                let queue_len = self.settings.queue_len;
                let warrior = &mut self.warriors[w];
                if warrior.pcnum < queue_len {
                    if let Some(slot) = warrior.find(0, queue_len as usize, false) {
                        warrior.set_pc(slot, a_addr, core_size);
                        warrior.pcnum += 1;
                    }
                }
            }
            _ => {}
        }
    }

    #[inline(always)]
    fn write_b(&mut self, addr: usize, value: u16, num: u16) {
        self.mem[addr].b = value;
        self.mem[addr].owner = num;
    }

    /// The common step adds one, so a jump stores the target minus one.
    #[inline(always)]
    fn jump(&mut self, w: usize, target: u16, core_size: u16) {
        let warrior = &mut self.warriors[w];
        warrior.set_pc(warrior.currpc, dec(target, core_size), core_size);
    }

    /// `C_DAT`: the process ends.
    fn kill(&mut self, w: usize) {
        let warrior = &mut self.warriors[w];
        warrior.set_pc(warrior.currpc, INACTIVE, self.settings.core_size);
        warrior.pcnum -= 1;
        if warrior.pcnum == 0 {
            self.alive -= 1;
        }
    }

    /// Number of programs with processes left.
    pub fn alive(&self) -> usize {
        self.alive
    }
}
