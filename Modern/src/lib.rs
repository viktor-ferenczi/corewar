//! Native reimplementation of CoreWar MARS V1.0 (`Historical/MARS.COM`, 1993).
//!
//! The compiler and the simulator follow `MARS.ASM` instruction by instruction where it matters
//! for the results under `Settings::hu93()`, so a battle with the same random seed ends
//! exactly like in MARS.COM. The default is pMARS without P-space; ICWS'88 and '94 are also supported.

pub mod assembler;
pub mod clean;
pub mod compiler;
pub mod engine;
#[cfg(feature = "gpu")]
pub mod gpu;
mod pmars_eval;
pub mod report;
pub mod rng;
pub mod tournament;

pub use assembler::{compile_88, compile_94};
pub use clean::compile_hu93_clean;
pub use compiler::{compile, Compiled, Fatal, Instruction, Message, MessageKind, Program};
pub use engine::{Engine, Outcome, Rules, Settings, Standard};
pub use rng::Rng;

/// Historical arena length; other standards use `Settings::core_size`.
pub const ARENALEN: u16 = 8000;
/// Historical program length limit; other standards use `Settings::program_len`.
pub const MAXLEN: usize = 100;
/// Historical process slot table size; FIFO standards allow up to 8000 processes.
pub const QUEUELEN: usize = 256;

/// The `MODULO` macro: the 16 bit value is taken as signed and brought into 0..ARENALEN.
pub fn modulo(v: u16) -> u16 {
    (v as i16).rem_euclid(ARENALEN as i16) as u16
}
