//! A whole MARS.COM run in statistics mode (`/P`): the text it prints and its binary log (`/F`).

use crate::engine::{Cell, PlaceError, Stats, MEMLEN};
use crate::{compile, Engine, Fatal, Program, Rng, Settings};

/// First line of the output. MARS.COM prints "CoreWar MARS V1.0 by GM 1993" there, the rest of the
/// output has the same format.
pub const BANNER: &str = "CoreWar MARS Rust V1.0 - Viktor Ferenczi 2026";

/// A source to compile, with the name MARS prints for it.
#[derive(Clone, Debug)]
pub struct Source {
    pub name: String,
    /// `None` when the file could not be opened.
    pub bytes: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    /// What MARS prints to standard output, with LF line ends instead of CR LF and `BANNER` as the
    /// first line.
    pub text: String,
    /// The binary statistics log, empty when no war was played.
    pub log: Vec<u8>,
    /// False when MARS stopped before the wars: errors in a program or no room in the arena.
    pub played: bool,
    /// Per program statistics of the wars played, empty when none was.
    pub stats: Vec<Stats>,
}

/// Memory of a DOS session above MARS's own code and data, in 8 byte cells.
///
/// MARS keeps a 1856 byte structure per program there, then the arena. It clears the arena for
/// every war, but not the up to 99 cells after it, where programs placed near the end of the arena
/// spill over. DOS does not clear memory between programs either, so these cells start with what
/// the previous MARS run in the same session left there: its spilled cells, or with fewer
/// programs, the end of its arena. MARS takes a cell there as occupied when its owner field is
/// not 0, which changes where programs can be placed.
#[derive(Clone, Debug)]
pub struct Session {
    cells: Vec<Cell>,
}

/// `(SIZE PROGDATA + 15) / 16` paragraphs, in cells.
const PROGDATA_CELLS: usize = 1856 / 8;

impl Session {
    /// A fresh DOSBox: all memory is zero.
    pub fn new() -> Self {
        Session { cells: Vec::new() }
    }

    /// The memory, cell 0 is where the structure of the first program starts.
    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    fn window(&mut self, programs: usize) -> std::ops::Range<usize> {
        let base = programs * PROGDATA_CELLS;
        if self.cells.len() < base + MEMLEN {
            self.cells.resize(base + MEMLEN, Cell::default());
        }
        base..base + MEMLEN
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

/// Compile the sources and play `wars` wars, like `MARS prg_1 ... prg_n /P=wars /V` as the first
/// program in a fresh DOSBox. Fails where MARS.COM itself would hang or abort while compiling.
pub fn run(sources: &[Source], settings: &Settings, rng: Rng, wars: u16) -> Result<Run, (Fatal, String)> {
    run_in(&mut Session::new(), sources, settings, rng, wars)
}

/// Like `run`, as the next program in a DOS session.
pub fn run_in(
    session: &mut Session,
    sources: &[Source],
    settings: &Settings,
    rng: Rng,
    wars: u16,
) -> Result<Run, (Fatal, String)> {
    let mut text = format!("{BANNER}\n");
    let mut programs = Vec::new();
    let mut errors = 0;
    for source in sources {
        let Some(bytes) = &source.bytes else {
            text += &format!("Can't open {} !\n", source.name);
            errors += 1;
            continue;
        };
        text += &format!("\n{}\n", source.name);
        let compiled = compile(bytes).map_err(|fatal| (fatal, text.clone()))?;
        for message in &compiled.messages {
            text += &message.text(&source.name);
            text.push('\n');
        }
        errors += compiled.messages.len();
        programs.push(compiled.program);
    }
    if errors != 0 {
        text += "Cannot execute war, while there are any errors !\n";
        return Ok(Run { text, log: Vec::new(), played: false, stats: Vec::new() });
    }
    let window = session.window(programs.len());
    let mut engine = Engine::with_memory(settings.clone(), &programs, rng, session.cells[window.clone()].to_vec());
    for _ in 0..wars.max(1) {
        if let Err(PlaceError::NoPlace) = engine.war() {
            session.cells[window].copy_from_slice(&engine.mem);
            text += "Cannot place many programs into arena !\n";
            return Ok(Run { text, log: Vec::new(), played: false, stats: Vec::new() });
        }
    }
    session.cells[window].copy_from_slice(&engine.mem);
    let names: Vec<&str> = sources.iter().map(|s| s.name.as_str()).collect();
    let stats: Vec<Stats> = engine.warriors.iter().map(|w| w.stats.clone()).collect();
    text += &statistics(settings, engine.wars, &stats, &names);
    Ok(Run { text, log: log(settings, engine.wars, &stats), played: true, stats })
}

/// The positions `run_in` would place the programs at in each of its wars, without playing them.
/// This is possible because only placement uses the random generator and the cells after the arena,
/// and no war can change them. Leaves the session like `run_in` would, as far as placement goes.
pub fn place_run(
    session: &mut Session,
    programs: &[Program],
    rng: Rng,
    wars: u16,
) -> Result<Vec<Vec<u16>>, PlaceError> {
    let window = session.window(programs.len());
    let settings = Settings::default();
    let mut engine = Engine::with_memory(settings, programs, rng, session.cells[window.clone()].to_vec());
    let placed = (0..wars.max(1)).map(|_| engine.place(None)).collect();
    session.cells[window].copy_from_slice(&engine.mem);
    placed
}

/// The text of a run that compiled and played without errors: what `run` returns in `Run::text`.
pub fn run_text(settings: &Settings, wars: u16, stats: &[Stats], names: &[&str]) -> String {
    let mut text = format!("{BANNER}\n");
    for name in names {
        text += &format!("\n{name}\n");
    }
    text + &statistics(settings, wars, stats, names)
}

/// Compile a source that must have no errors.
pub fn compile_clean(name: &str, bytes: &[u8]) -> Result<Program, String> {
    let compiled = compile(bytes).map_err(|fatal| format!("{name}: {fatal}"))?;
    if let Some(message) = compiled.messages.first() {
        return Err(message.text(name));
    }
    Ok(compiled.program)
}

/// Average process count, `DIV` rounded up when twice the remainder, taken in 16 bits, is above
/// the number of wars.
pub fn average(pcs: u32, wars: u16) -> u16 {
    let wars = wars as u32;
    let (q, r) = (pcs / wars, pcs % wars);
    (q + (wars < ((r * 2) & 0xFFFF)) as u32) as u16
}

/// The statistics block `WAR_QUIT` prints. `OUTSPACES` pads to a column but always prints at
/// least one space.
pub fn statistics(s: &Settings, wars: u16, stats: &[Stats], names: &[&str]) -> String {
    let mut text = format!(
        "\nCoreWar MARS V1.0 Statistics:\n\nNumber of full wars         = {}\nMaximal war length in steps = {}\nQueue length (Max. PCs)     = {}\n{}\n\nProgNum   Average PC  Win     Lose    Progam name\n",
        wars,
        s.max_steps,
        s.queue_len,
        if s.exec_other { "Execute each other was enabled." } else { "Execute each other was disabled." },
    );
    for (i, (stats, name)) in stats.iter().zip(names).enumerate() {
        let mut line = String::new();
        let pad = |line: &mut String, column: usize| {
            line.push(' ');
            while line.len() < column {
                line.push(' ');
            }
        };
        line += &(i + 1).to_string();
        pad(&mut line, 10);
        if wars != 0 {
            line += &average(stats.pcs, wars).to_string();
        }
        pad(&mut line, 22);
        line += &stats.wins.to_string();
        pad(&mut line, 30);
        line += &stats.losses.to_string();
        pad(&mut line, 38);
        text += &line;
        text += name;
        text.push('\n');
    }
    text
}

/// The binary log, 16 bit little endian words.
pub fn log(s: &Settings, wars: u16, stats: &[Stats]) -> Vec<u8> {
    let mut words =
        vec![0x0100, wars, s.max_steps as u16, (s.max_steps >> 16) as u16, s.queue_len, s.exec_other as u16];
    for (i, w) in stats.iter().enumerate() {
        let pcs = w.pcs;
        words.extend([i as u16 + 1, pcs as u16, (pcs >> 16) as u16, average(pcs, wars), w.wins, w.losses]);
    }
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}
