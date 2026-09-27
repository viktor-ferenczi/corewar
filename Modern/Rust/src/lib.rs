//! Native reimplementation of CoreWar MARS V1.0 (`Historical/MARS.COM`, 1993).
//!
//! The compiler and the simulator follow `MARS.ASM` instruction by instruction where it matters
//! for the results, quirks included, so that a battle played here with the same random seed ends
//! exactly like in MARS.COM.

pub mod compiler;
pub mod engine;
pub mod report;
pub mod rng;
pub mod tournament;

pub use compiler::{compile, Compiled, Fatal, Instruction, Message, MessageKind, Program};
pub use engine::{Engine, Outcome, Settings};
pub use rng::Rng;

/// Arena length in cells.
pub const ARENALEN: u16 = 8000;
/// Maximum program length in instructions.
pub const MAXLEN: usize = 100;
/// Size of the process slot table of a program, the upper limit of the queue length.
pub const QUEUELEN: usize = 256;

/// The `MODULO` macro: the 16 bit value is taken as signed and brought into 0..ARENALEN.
pub fn modulo(v: u16) -> u16 {
    (v as i16).rem_euclid(ARENALEN as i16) as u16
}
