//! Round robin of programs, played like `Reproduction/mars.py tournament` but in parallel threads.
//!
//! Every pair plays two MARS runs, one per start order, each with its own random seed derived from
//! a master seed, so a tournament can be repeated exactly. Like in `mars.py`, both runs of a pair
//! share one DOS session: the second run starts with the memory the first one left behind.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use crate::report::{self, Session, Source};
use crate::{Rng, Settings};

/// BIOS ticks per day, the range of the tick counter MARS seeds from.
const TICKS_PER_DAY: u64 = 0x1800B0;

pub struct Entry {
    /// Name used in the results, the file name without `.CWR`.
    pub name: String,
    pub path: PathBuf,
    pub source: Vec<u8>,
}

impl Entry {
    pub fn load(path: &Path) -> std::io::Result<Self> {
        let file = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let name = file.strip_suffix(".CWR").unwrap_or(&file).to_string();
        Ok(Entry { name, path: path.to_path_buf(), source: std::fs::read(path)? })
    }
}

pub struct Options {
    pub wars_per_order: u16,
    pub settings: Settings,
    pub jobs: usize,
    pub seed: u64,
    pub out: PathBuf,
}

/// One row of `runs.csv`, same columns as the DOSBox tournament plus the seed.
struct Row {
    first: String,
    second: String,
    wars: u16,
    max_steps: u32,
    first_wins: u16,
    second_wins: u16,
    first_avg_pcs: u16,
    second_avg_pcs: u16,
    seconds: f64,
    seed: u32,
}

const HEADER: &str =
    "first,second,games,max_steps,first_wins,second_wins,draws,first_avg_pcs,second_avg_pcs,seconds,seed";

/// Tick count for one run, from a SplitMix64 hash of the master seed and the run number.
pub fn run_seed(master: u64, run: u64) -> u32 {
    let mut z = master.wrapping_add(run.wrapping_add(1).wrapping_mul(0x9E3779B97F4A7C15));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    ((z ^ (z >> 31)) % TICKS_PER_DAY) as u32
}

/// Play every pair in both start orders, write `results/runs.csv` and `raw/*.sta` into
/// `options.out`, the layout of `Reproduction`.
/// Returns the wall clock time in seconds.
pub fn play(entries: &[Entry], options: &Options) -> Result<f64, String> {
    for entry in entries {
        report::compile_clean(&entry.name, &entry.source)?;
    }
    let raw = options.out.join("raw");
    std::fs::create_dir_all(&raw).map_err(|e| e.to_string())?;
    let mut pairs = Vec::new();
    for a in 0..entries.len() {
        for b in a + 1..entries.len() {
            pairs.push((a, b));
        }
    }
    let rows = Mutex::new(Vec::new());
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let started = Instant::now();
    std::thread::scope(|scope| {
        for _ in 0..options.jobs.max(1) {
            scope.spawn(|| loop {
                let n = next.fetch_add(1, Ordering::Relaxed);
                let Some(&(a, b)) = pairs.get(n) else { break };
                let pair_started = Instant::now();
                let mut played = Vec::new();
                let mut session = Session::new();
                for (order, (first, second)) in [(a, b), (b, a)].into_iter().enumerate() {
                    let seed = run_seed(options.seed, 2 * n as u64 + order as u64);
                    played.push(play_run(&mut session, &entries[first], &entries[second], options, seed, &raw));
                }
                let seconds = pair_started.elapsed().as_secs_f64();
                let count = done.fetch_add(1, Ordering::Relaxed) + 1;
                println!("[{count}/{}] {} vs {} in {seconds:.2} s", pairs.len(), entries[a].name, entries[b].name);
                let mut rows = rows.lock().unwrap();
                for mut row in played {
                    row.seconds = seconds;
                    rows.push((n, row));
                }
            });
        }
    });
    let wall = started.elapsed().as_secs_f64();
    let mut rows = rows.into_inner().unwrap();
    rows.sort_by_key(|(n, _)| *n);
    let mut csv = String::from(HEADER);
    csv.push('\n');
    for (_, r) in rows {
        csv += &format!(
            "{},{},{},{},{},{},{},{},{},{:.2},{}\n",
            r.first,
            r.second,
            r.wars,
            r.max_steps,
            r.first_wins,
            r.second_wins,
            r.wars - r.first_wins - r.second_wins,
            r.first_avg_pcs,
            r.second_avg_pcs,
            r.seconds,
            r.seed
        );
    }
    let results = options.out.join("results");
    std::fs::create_dir_all(&results).map_err(|e| e.to_string())?;
    std::fs::write(results.join("runs.csv"), csv).map_err(|e| e.to_string())?;
    std::io::stdout().flush().ok();
    Ok(wall)
}

fn play_run(session: &mut Session, first: &Entry, second: &Entry, options: &Options, seed: u32, raw: &Path) -> Row {
    let sources = [first, second].map(|e| Source {
        name: e.path.file_name().unwrap().to_string_lossy().into_owned(),
        bytes: Some(e.source.clone()),
    });
    let run = report::run_in(session, &sources, &options.settings, Rng::from_ticks(seed), options.wars_per_order)
        .expect("programs were checked before");
    assert!(run.played, "{}", run.text);
    std::fs::write(raw.join(format!("{}_vs_{}.sta", first.name, second.name)), &run.text)
        .expect("write raw statistics");
    let wars = options.wars_per_order.max(1);
    let (f, s) = (&run.stats[0], &run.stats[1]);
    Row {
        first: first.name.clone(),
        second: second.name.clone(),
        wars,
        max_steps: options.settings.max_steps,
        first_wins: f.wins,
        second_wins: s.wins,
        first_avg_pcs: report::average(f.pcs, wars),
        second_avg_pcs: report::average(s.pcs, wars),
        seconds: 0.0,
        seed,
    }
}
