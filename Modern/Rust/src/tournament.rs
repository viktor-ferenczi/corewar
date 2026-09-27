//! Pairwise tournament of 2 to 256 programs, played like `Reproduction/mars.py tournament`.
//!
//! Every pair plays two MARS runs, one per start order, each with its own random seed derived from
//! a master seed, so a tournament can be repeated exactly. Like in `mars.py`, both runs of a pair
//! share one DOS session: the second run starts with the memory the first one left behind.
//!
//! Placement is the only part of a run that goes from war to war (the random generator and the
//! cells after the arena), and it does not depend on how the wars end. So the positions of all wars
//! are worked out first, then every war is fought on its own, on CPU threads or on GPUs, and the
//! results are summed up per run again. The results are the same as playing each run in order.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::engine::Stats;
use crate::report::{self, Session};
use crate::{Engine, Program, Rng, Settings};

/// BIOS ticks per day, the range of the tick counter MARS seeds from.
const TICKS_PER_DAY: u64 = 0x1800B0;
pub const MAX_PROGRAMS: usize = 256;

pub struct Entry {
    /// Name used in the results, the file name without `.CWR`.
    pub name: String,
    /// The file name, which MARS prints in its statistics.
    pub file: String,
    pub program: Program,
}

impl Entry {
    pub fn load(path: &Path) -> Result<Self, String> {
        let file = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let name = file.strip_suffix(".CWR").unwrap_or(&file).to_string();
        let source = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Entry { program: report::compile_clean(&file, &source)?, name, file })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// One JSON object per run and line.
    Jsonl,
    /// `results/runs.csv` and `raw/FIRST_vs_SECOND.sta` in a folder, the layout of `Reproduction`.
    Sta,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Backend {
    /// CPU threads.
    Cpu { jobs: usize },
    /// GPUs by their index in `gpu::adapters()`.
    Gpu { devices: Vec<usize> },
}

pub struct Options {
    /// Games per pair, split between the start orders, the first order gets the odd one.
    pub games: u32,
    pub settings: Settings,
    pub backend: Backend,
    pub seed: u64,
    pub format: Format,
    /// The JSONL file, or the folder for the `Sta` format.
    pub out: PathBuf,
    /// Print progress to standard error.
    pub progress: bool,
}

/// A war to fight: the indexes of the two programs and their positions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct War {
    pub programs: [u16; 2],
    pub positions: [u16; 2],
}

/// How a war ended: the processes left to both programs, and the steps played.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WarResult {
    pub pcs: [u16; 2],
    pub steps: u32,
}

/// One MARS run of the tournament and its results.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunResult {
    pub first: usize,
    pub second: usize,
    pub seed: u32,
    pub wars: u16,
    pub stats: [Stats; 2],
    pub steps: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    pub runs: Vec<RunResult>,
    pub wars: u64,
    pub steps: u64,
    pub seconds: f64,
}

/// Tick count for one run, from a SplitMix64 hash of the master seed and the run number.
pub fn run_seed(master: u64, run: u64) -> u32 {
    let mut z = master.wrapping_add(run.wrapping_add(1).wrapping_mul(0x9E3779B97F4A7C15));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    ((z ^ (z >> 31)) % TICKS_PER_DAY) as u32
}

/// Play the tournament and write its results.
pub fn play(entries: &[Entry], options: &Options) -> Result<Summary, String> {
    let summary = compute(entries, options)?;
    match options.format {
        Format::Jsonl => std::fs::write(&options.out, jsonl(entries, &options.settings, &summary.runs)),
        Format::Sta => write_sta(entries, &options.settings, &summary.runs, &options.out),
    }
    .map_err(|e| format!("{}: {e}", options.out.display()))?;
    Ok(summary)
}

/// Play the tournament without writing anything.
pub fn compute(entries: &[Entry], options: &Options) -> Result<Summary, String> {
    if !(2..=MAX_PROGRAMS).contains(&entries.len()) {
        return Err(format!("a tournament needs 2 to {MAX_PROGRAMS} programs, got {}", entries.len()));
    }
    for (i, e) in entries.iter().enumerate() {
        if entries[..i].iter().any(|o| o.name == e.name) {
            return Err(format!("two programs are called {}", e.name));
        }
    }
    if !(2..=2 * 65535).contains(&options.games) {
        return Err(format!("games per pair must be 2 to 131070, got {}", options.games));
    }
    let started = Instant::now();
    let jobs = match &options.backend {
        Backend::Cpu { jobs } => (*jobs).max(1),
        Backend::Gpu { .. } => std::thread::available_parallelism().map_or(1, |n| n.get()),
    };
    let (runs, wars) = plan(entries, options, jobs)?;
    let programs: Vec<Program> = entries.iter().map(|e| e.program.clone()).collect();
    let progress = Progress::new(wars.len() as u64, options.progress);
    let results = match &options.backend {
        Backend::Cpu { jobs } => fight_cpu(&programs, &wars, &options.settings, *jobs, &progress),
        #[cfg(feature = "gpu")]
        Backend::Gpu { devices } => crate::gpu::fight(&programs, &wars, &options.settings, devices, &progress)?,
        #[cfg(not(feature = "gpu"))]
        Backend::Gpu { .. } => return Err("this build has no GPU support (cargo feature gpu)".into()),
    };
    progress.finish();
    let mut offset = 0;
    let runs: Vec<RunResult> = runs
        .into_iter()
        .map(|mut run| {
            for r in &results[offset..offset + run.wars as usize] {
                for i in 0..2 {
                    let s = &mut run.stats[i];
                    s.pcs += r.pcs[i] as u32;
                    s.wins += (r.pcs[i] != 0 && r.pcs[1 - i] == 0) as u16;
                    s.losses += (r.pcs[i] == 0) as u16;
                }
                run.steps += r.steps as u64;
            }
            offset += run.wars as usize;
            run
        })
        .collect();
    let steps = runs.iter().map(|r| r.steps).sum();
    Ok(Summary { runs, wars: wars.len() as u64, steps, seconds: started.elapsed().as_secs_f64() })
}

/// The runs in pair order, and the wars of all runs in the same order.
fn plan(entries: &[Entry], options: &Options, jobs: usize) -> Result<(Vec<RunResult>, Vec<War>), String> {
    let mut pairs = Vec::new();
    for a in 0..entries.len() {
        for b in a + 1..entries.len() {
            pairs.push((a, b));
        }
    }
    let games = [options.games.div_ceil(2) as u16, (options.games / 2) as u16];
    let next = AtomicUsize::new(0);
    let planned = Mutex::new(vec![None; pairs.len()]);
    std::thread::scope(|scope| {
        for _ in 0..jobs {
            scope.spawn(|| loop {
                let n = next.fetch_add(1, Ordering::Relaxed);
                let Some(&(a, b)) = pairs.get(n) else { break };
                let mut session = Session::new();
                let mut pair = Vec::new();
                for (order, (first, second)) in [(a, b), (b, a)].into_iter().enumerate() {
                    let seed = run_seed(options.seed, 2 * n as u64 + order as u64);
                    let programs = [entries[first].program.clone(), entries[second].program.clone()];
                    let positions = report::place_run(&mut session, &programs, Rng::from_ticks(seed), games[order]);
                    pair.push((first, second, seed, positions));
                }
                planned.lock().unwrap()[n] = Some(pair);
            });
        }
    });
    let mut runs = Vec::new();
    let mut wars = Vec::new();
    for pair in planned.into_inner().unwrap() {
        for (first, second, seed, positions) in pair.unwrap() {
            let positions = positions.map_err(|_| "Cannot place many programs into arena !".to_string())?;
            runs.push(RunResult {
                first,
                second,
                seed,
                wars: positions.len() as u16,
                stats: Default::default(),
                steps: 0,
            });
            wars.extend(
                positions.iter().map(|p| War { programs: [first as u16, second as u16], positions: [p[0], p[1]] }),
            );
        }
    }
    Ok((runs, wars))
}

/// Play one war from its positions on the CPU.
pub fn fight_one(engine: &mut Engine, war: &War) -> WarResult {
    engine.place(Some(&war.positions)).expect("given positions");
    let outcome = engine.fight();
    WarResult { pcs: [engine.warriors[0].pcnum, engine.warriors[1].pcnum], steps: outcome.steps as u32 }
}

fn fight_cpu(
    programs: &[Program],
    wars: &[War],
    settings: &Settings,
    jobs: usize,
    progress: &Progress,
) -> Vec<WarResult> {
    const BLOCK: usize = 64;
    let next = AtomicUsize::new(0);
    let results = Mutex::new(vec![WarResult::default(); wars.len()]);
    std::thread::scope(|scope| {
        for _ in 0..jobs.max(1) {
            scope.spawn(|| {
                let mut engine: Option<(Engine, [u16; 2])> = None;
                loop {
                    let start = next.fetch_add(BLOCK, Ordering::Relaxed);
                    if start >= wars.len() {
                        break;
                    }
                    let block = &wars[start..(start + BLOCK).min(wars.len())];
                    let mut out = Vec::with_capacity(block.len());
                    for war in block {
                        if engine.as_ref().is_none_or(|(_, p)| *p != war.programs) {
                            let pair = war.programs.map(|p| programs[p as usize].clone());
                            engine = Some((Engine::new(settings.clone(), &pair, Rng::from_ticks(0)), war.programs));
                        }
                        out.push(fight_one(&mut engine.as_mut().unwrap().0, war));
                    }
                    results.lock().unwrap()[start..start + out.len()].copy_from_slice(&out);
                    progress.add(out.len() as u64);
                }
            });
        }
    });
    results.into_inner().unwrap()
}

/// Wars done so far, printed to standard error every few seconds.
pub struct Progress {
    total: u64,
    done: AtomicU64,
    started: Instant,
    last: Mutex<Instant>,
    enabled: bool,
}

impl Progress {
    pub fn new(total: u64, enabled: bool) -> Self {
        let now = Instant::now();
        Progress { total, done: AtomicU64::new(0), started: now, last: Mutex::new(now), enabled }
    }

    pub fn add(&self, wars: u64) {
        let done = self.done.fetch_add(wars, Ordering::Relaxed) + wars;
        if !self.enabled {
            return;
        }
        let mut last = self.last.lock().unwrap();
        if last.elapsed() >= Duration::from_secs(5) {
            *last = Instant::now();
            let seconds = self.started.elapsed().as_secs_f64();
            eprintln!("{done}/{} wars, {:.1}%, {seconds:.0} s", self.total, done as f64 * 100.0 / self.total as f64);
        }
    }

    fn finish(&self) {
        if self.enabled {
            eprintln!(
                "{}/{} wars in {:.1} s",
                self.done.load(Ordering::Relaxed),
                self.total,
                self.started.elapsed().as_secs_f64()
            );
        }
    }
}

fn json_string(s: &str) -> String {
    let mut out = String::from('"');
    for c in s.chars() {
        match c {
            '"' => out += "\\\"",
            '\\' => out += "\\\\",
            c if (c as u32) < 0x20 => write!(out, "\\u{:04x}", c as u32).unwrap(),
            c => out.push(c),
        }
    }
    out + "\""
}

/// One JSON object per run and line, in pair order.
pub fn jsonl(entries: &[Entry], settings: &Settings, runs: &[RunResult]) -> String {
    let mut text = String::new();
    for r in runs {
        let [f, s] = &r.stats;
        writeln!(
            text,
            "{{\"first\":{},\"second\":{},\"seed\":{},\"games\":{},\"first_wins\":{},\"second_wins\":{},\"draws\":{},\
             \"first_pcs\":{},\"second_pcs\":{},\"steps\":{},\"max_steps\":{},\"queue\":{},\"exec_other\":{}}}",
            json_string(&entries[r.first].name),
            json_string(&entries[r.second].name),
            r.seed,
            r.wars,
            f.wins,
            s.wins,
            r.wars - f.wins - s.wins,
            f.pcs,
            s.pcs,
            r.steps,
            settings.max_steps,
            settings.queue_len,
            settings.exec_other,
        )
        .unwrap();
    }
    text
}

/// `results/runs.csv` and the statistics text of every run in `raw`, like `mars.py` writes them.
fn write_sta(entries: &[Entry], settings: &Settings, runs: &[RunResult], out: &Path) -> std::io::Result<()> {
    let raw = out.join("raw");
    std::fs::create_dir_all(&raw)?;
    let mut csv =
        String::from("first,second,games,max_steps,first_wins,second_wins,draws,first_avg_pcs,second_avg_pcs,seed\n");
    for r in runs {
        let (first, second) = (&entries[r.first], &entries[r.second]);
        let text = report::run_text(settings, r.wars, &r.stats, &[&first.file, &second.file]);
        std::fs::write(raw.join(format!("{}_vs_{}.sta", first.name, second.name)), text)?;
        let [f, s] = &r.stats;
        writeln!(
            csv,
            "{},{},{},{},{},{},{},{},{},{}",
            first.name,
            second.name,
            r.wars,
            settings.max_steps,
            f.wins,
            s.wins,
            r.wars - f.wins - s.wins,
            report::average(f.pcs, r.wars),
            report::average(s.pcs, r.wars),
            r.seed
        )
        .unwrap();
    }
    let results = out.join("results");
    std::fs::create_dir_all(&results)?;
    std::fs::write(results.join("runs.csv"), csv)
}
