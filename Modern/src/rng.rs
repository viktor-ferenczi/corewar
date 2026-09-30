//! Random number generator of MARS (`RANDOMIZE`, `RANDOM`), used only to place the programs.

use crate::modulo;

/// A 31 bit shift register, SEEDL is the low word and SEEDH the high one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rng {
    lo: u16,
    hi: u16,
}

impl Rng {
    /// `RANDOMIZE`: seeded from the BIOS tick counter (dword at 0:046C, 18.2 ticks per second
    /// since midnight), then run for two short, seed dependent stretches.
    pub fn from_ticks(ticks: u32) -> Self {
        let (tick_lo, tick_hi) = (ticks as u16, (ticks >> 16) as u16);
        let lo = tick_lo ^ tick_hi;
        let mut rng = Rng { lo, hi: tick_hi.rotate_right(7).wrapping_add(lo) };
        let mut ax = rng.next();
        for _ in 0..2 {
            for _ in 0..(ax & 0xFF).max(1) {
                ax = rng.next();
            }
        }
        rng
    }

    /// A seed for the current time of day, the way MARS.COM gets one.
    pub fn from_clock() -> Self {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        let seconds = (now.as_secs() % 86400) as f64 + now.subsec_nanos() as f64 * 1e-9;
        Self::from_ticks((seconds * 1_193_180.0 / 65_536.0) as u32)
    }

    /// `RANDOM`: the next arena position, 0..ARENALEN.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> u16 {
        modulo(self.advance())
    }

    pub fn next_in(&mut self, core_size: u16) -> u16 {
        if core_size == 8000 {
            return self.next();
        }
        self.advance() % core_size
    }

    fn advance(&mut self) -> u16 {
        let x = (self.hi << 1) ^ self.hi;
        // ROL CX,2 leaves bit 14 of CX in the carry, which RCL shifts into SEEDL.
        let feedback = x >> 14 & 1;
        let lo = self.lo << 1 | feedback;
        let mut hi = (self.hi << 1 | self.lo >> 15) & 0x7FFF;
        if lo == 0 && hi == 0 {
            hi = 1;
        }
        self.lo = lo;
        self.hi = hi;
        lo
    }
}
