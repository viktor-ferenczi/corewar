//! The simulator, a port of `WAR`, `WAR1`, `LOADA`, `LOADB` and the `C_xxx` instruction routines.

use crate::{Instruction, Program, Rng, ARENALEN, MAXLEN, QUEUELEN};

/// Programs are copied into memory without wrapping around the end of the arena, so memory
/// continues for up to 99 cells after it. Nothing can address those cells.
pub const MEMLEN: usize = ARENALEN as usize + MAXLEN - 1;
const INACTIVE: u16 = 0xFFFF;
/// Steps between two DAT tests, in thousands.
const DAT_TEST_THOUSANDS: u16 = 16;
const DAT_SCAN_LEN: usize = ARENALEN as usize - 1;

/// A memory cell (`ITEM`): the instruction and the number of the program that wrote it last,
/// 0 for none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cell {
    pub op: u8,
    pub modes: u8,
    pub a: u16,
    pub b: u16,
    pub owner: u16,
}

impl Cell {
    pub fn instruction(&self) -> Instruction {
        Instruction { op: self.op, modes: self.modes, a: self.a, b: self.b }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Process slots used per program (`/Q`), 1..=256.
    pub queue_len: u16,
    /// Programs may execute cells written by others (not `/E`).
    pub exec_other: bool,
    /// Steps per war (`/M`), both programs' steps counted; 0 means 2^32.
    pub max_steps: u32,
    /// End a war as a draw when no DAT is left in the arena, checked every 16000 steps. MARS does
    /// this only in statistics mode (`/P`).
    pub dat_test: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { queue_len: 64, exec_other: true, max_steps: 600_000, dat_test: true }
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
    /// Bit i is set when slot i holds an address below ARENALEN.
    active: [u64; QUEUELEN / 64],
    pub currpc: usize,
    /// Process count (`PCNUM`). It counts the starting process even when it could not start.
    pub pcnum: u16,
    pub stats: Stats,
}

impl Warrior {
    fn new(num: u16, program: &Program) -> Self {
        assert!(!program.code.is_empty() && program.code.len() <= MAXLEN, "Program length error !");
        Warrior {
            num,
            code: program.code.clone(),
            start: program.start,
            pcs: [INACTIVE; QUEUELEN],
            active: [0; QUEUELEN / 64],
            currpc: 0,
            pcnum: 0,
            stats: Stats::default(),
        }
    }

    fn set_pc(&mut self, slot: usize, pc: u16) {
        self.pcs[slot] = pc;
        let bit = 1u64 << (slot % 64);
        if pc < ARENALEN {
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
    pub warriors: Vec<Warrior>,
    pub mem: Vec<Cell>,
    pub rng: Rng,
    /// Wars played (`STATCTR0`).
    pub wars: u16,
    /// Programs with processes left (`NPROG`).
    alive: usize,
    /// Cells holding a DAT, not counting the last one: the `LOOPNZ` scan of `WAR_DATTEST` finds
    /// a DAT in cell 7999 with CX already 0 and takes it for none.
    dats: u32,
}

#[inline(always)]
fn add(x: u16, y: u16) -> u16 {
    let s = x + y;
    if s >= ARENALEN {
        s - ARENALEN
    } else {
        s
    }
}

#[inline(always)]
fn dec(x: u16) -> u16 {
    if x == 0 {
        ARENALEN - 1
    } else {
        x - 1
    }
}

impl Engine {
    /// An engine with zeroed memory, like the first MARS run after DOS started.
    pub fn new(settings: Settings, programs: &[Program], rng: Rng) -> Self {
        Self::with_memory(settings, programs, rng, vec![Cell::default(); MEMLEN])
    }

    /// An engine whose memory, the arena and the cells after it, starts with what an earlier
    /// program left there. See `report::Session`.
    pub fn with_memory(settings: Settings, programs: &[Program], rng: Rng, mem: Vec<Cell>) -> Self {
        assert!((1..=QUEUELEN as u16).contains(&settings.queue_len));
        assert_eq!(mem.len(), MEMLEN);
        let warriors = programs.iter().enumerate().map(|(i, p)| Warrior::new(i as u16 + 1, p)).collect();
        Engine { settings, warriors, mem, rng, wars: 0, alive: 0, dats: 0 }
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
        self.mem[..ARENALEN as usize].fill(Cell::default());
        self.dats = DAT_SCAN_LEN as u32;
        for w in 0..self.warriors.len() {
            let warrior = &mut self.warriors[w];
            warrior.pcs = [INACTIVE; QUEUELEN];
            warrior.active = [0; QUEUELEN / 64];
            warrior.currpc = 0;
            warrior.pcnum = 1;
            let len = warrior.code.len();
            let pos = match positions {
                Some(p) => p[w],
                None => {
                    let free = |mem: &[Cell], pos: usize| mem[pos..pos + len].iter().all(|c| c.owner == 0);
                    (0..ARENALEN)
                        .map(|_| self.rng.next())
                        .find(|&pos| free(&self.mem, pos as usize))
                        .ok_or(PlaceError::NoPlace)?
                }
            };
            // The start address is not reduced modulo ARENALEN. Past the arena the process can
            // never run, but it still counts, so the program can not lose.
            placed.push(pos);
            let warrior = &mut self.warriors[w];
            warrior.set_pc(0, warrior.start + pos);
            let num = warrior.num;
            for i in 0..len {
                let ins = self.warriors[w].code[i];
                let cell = Cell { op: ins.op, modes: ins.modes, a: ins.a, b: ins.b, owner: num };
                self.set_cell(pos as usize + i, cell);
            }
        }
        self.alive = self.warriors.len();
        Ok(placed)
    }

    /// Write a memory cell, keeping the DAT count of the DAT test up to date.
    pub fn set_cell(&mut self, addr: usize, cell: Cell) {
        if addr < DAT_SCAN_LEN {
            self.dats -= (self.mem[addr].op == 0) as u32;
            self.dats += (cell.op == 0) as u32;
        }
        self.mem[addr] = cell;
    }

    /// Run the war placed by `place` to its end and add it to the statistics (`WAR_WAR`).
    pub fn fight(&mut self) -> Outcome {
        let n = self.warriors.len();
        let min_alive = if n >= 2 { 2 } else { 1 };
        let mut remaining: u64 = if self.settings.max_steps == 0 { 1 << 32 } else { self.settings.max_steps as u64 };
        let mut steps = 0;
        let (mut counter_lo, mut counter_hi) = (0u16, 0u16);
        let mut w = 0;
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
                self.step(w);
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
            let (a_addr, a_val) = self.load(cpc, self.mem[pc].a, self.mem[pc].modes & 3, num, false);
            let (b_addr, b_val) = self.load(cpc, self.mem[pc].b, self.mem[pc].modes >> 2 & 3, num, true);
            self.execute(w, cpc, a_addr, a_val, b_addr, b_val);
        }
        let warrior = &mut self.warriors[w];
        let slot = warrior.currpc;
        let pc = warrior.pcs[slot];
        if pc < ARENALEN {
            warrior.pcs[slot] = add(pc, 1);
        }
        warrior.currpc = if slot + 1 >= queue_len { 0 } else { slot + 1 };
    }

    /// `LOADA` / `LOADB`: the address and the value of a parameter. The value is the B field of
    /// the addressed cell, or the field itself for `#`. Indirection always goes through B fields.
    #[inline(always)]
    fn load(&mut self, cpc: u16, field: u16, mode: u8, num: u16, is_b: bool) -> (u16, u16) {
        let mem = &mut self.mem;
        match mode {
            0 => (cpc, if is_b { mem[cpc as usize].b } else { mem[cpc as usize].a }),
            1 => {
                let addr = add(field, cpc);
                (addr, mem[addr as usize].b)
            }
            2 => {
                let t = add(field, cpc);
                let addr = add(t, mem[t as usize].b);
                (addr, mem[addr as usize].b)
            }
            _ => {
                let t = add(field, cpc) as usize;
                mem[t].owner = num;
                mem[t].b = dec(mem[t].b);
                let addr = add(t as u16, mem[t].b);
                (addr, mem[addr as usize].b)
            }
        }
    }

    #[inline(always)]
    fn execute(&mut self, w: usize, cpc: u16, a_addr: u16, a_val: u16, b_addr: u16, b_val: u16) {
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
            2 => self.write_b(b, add(a_val, b_val), num),
            3 => self.write_b(b, if b_val >= a_val { b_val - a_val } else { b_val + ARENALEN - a_val }, num),
            4 => self.jump(w, a_addr),
            5 if b_val == 0 => self.jump(w, a_addr),
            6 if b_val != 0 => self.jump(w, a_addr),
            7 => {
                let v = dec(b_val);
                self.write_b(b, v, num);
                if v != 0 {
                    self.jump(w, a_addr);
                }
            }
            8 if a_val == b_val => {
                let warrior = &mut self.warriors[w];
                warrior.set_pc(warrior.currpc, add(cpc, 1));
            }
            9 => {
                let queue_len = self.settings.queue_len;
                let warrior = &mut self.warriors[w];
                if warrior.pcnum < queue_len {
                    if let Some(slot) = warrior.find(0, queue_len as usize, false) {
                        warrior.set_pc(slot, a_addr);
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
    fn jump(&mut self, w: usize, target: u16) {
        let warrior = &mut self.warriors[w];
        warrior.set_pc(warrior.currpc, dec(target));
    }

    /// `C_DAT`: the process ends.
    fn kill(&mut self, w: usize) {
        let warrior = &mut self.warriors[w];
        warrior.set_pc(warrior.currpc, INACTIVE);
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
