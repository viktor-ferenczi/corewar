use std::path::PathBuf;
use std::process::ExitCode;

use mars::report::{self, Source};
use mars::tournament::{self, Backend, Entry, Format};
use mars::{compile, Rng, Settings};

const USAGE: &str = "\
CoreWar MARS Rust V1.0 - Viktor Ferenczi 2026
The compiler and simulator of MARS.COM (CoreWar MARS V1.0, 1993), reimplemented

Usage:
  mars compile FILE
      Print the compiled program and the error messages of MARS.
  mars run [OPTIONS] FILE...
      Play wars between the programs and print the statistics like MARS /P.
  mars tournament [OPTIONS] --out PATH FILE...
      Pairwise tournament of 2 to 256 programs, every pair in both start orders.
      Writes one JSON object per run to the file PATH, or with --format sta
      PATH/results/runs.csv and PATH/raw/FIRST_vs_SECOND.sta.
  mars gpus
      List the GPUs --gpu can use.

Options:
  --wars N            run: wars to play (1)
  --games N           tournament: games per pair, both start orders (1000)
  --format F          tournament: jsonl (default) or sta
  --cpu               tournament: play on CPU threads (default)
  --gpu [LIST]        tournament: play on GPUs, the first one by default, or a
                      list like 0,1, or all (all GPUs of the best kind present)
  --steps N           steps per war, both programs counted, 0 = 2^32 (600000)
  --queue N           processes per program, 1..256 (64)
  --no-exec-other     programs die when executing a cell another program wrote
  --no-dat-test       do not end a war when no DAT is left (MARS without /P)
  --seed N            run: BIOS tick count to seed from (default: time of day),
                      tournament: master seed (0)
  --log FILE          run: write the binary statistics log of MARS /F
  --jobs N            tournament: CPU threads (all cores)
  --out PATH          tournament: output file (jsonl) or folder (sta)
";

struct Args {
    command: String,
    files: Vec<PathBuf>,
    wars: Option<u16>,
    games: u32,
    format: Format,
    gpu: Option<String>,
    settings: Settings,
    seed: Option<u64>,
    log: Option<PathBuf>,
    jobs: usize,
    out: Option<PathBuf>,
}

fn parse() -> Result<Args, String> {
    let mut it = std::env::args().skip(1).peekable();
    let command = it.next().ok_or("missing command")?;
    if command == "-h" || command == "--help" {
        return Err(String::new());
    }
    let mut args = Args {
        command,
        files: Vec::new(),
        wars: None,
        games: 1000,
        format: Format::Jsonl,
        gpu: None,
        settings: Settings::default(),
        seed: None,
        log: None,
        jobs: std::thread::available_parallelism().map_or(1, |n| n.get()),
        out: None,
    };
    while let Some(arg) = it.next() {
        if arg == "--gpu" {
            let list =
                it.next_if(|v| v == "all" || (!v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == ',')));
            args.gpu = Some(list.unwrap_or_else(|| "0".into()));
            continue;
        }
        let mut value = || it.next().ok_or(format!("{arg} needs a value"));
        let number = |v: String| v.parse::<u64>().map_err(|_| format!("{arg}: not a number: {v}"));
        match arg.as_str() {
            "--wars" => args.wars = Some(number(value()?)?.clamp(1, 65535) as u16),
            "--steps" => {
                args.settings.max_steps = u32::try_from(number(value()?)?).map_err(|_| "--steps is too large")?
            }
            "--games" => args.games = u32::try_from(number(value()?)?).map_err(|_| "--games is too large")?,
            "--format" => {
                args.format = match value()?.as_str() {
                    "jsonl" => Format::Jsonl,
                    "sta" => Format::Sta,
                    other => return Err(format!("unknown format {other}, use jsonl or sta")),
                }
            }
            "--cpu" => args.gpu = None,
            "--queue" => args.settings.queue_len = number(value()?)?.clamp(1, 256) as u16,
            "--no-exec-other" => args.settings.exec_other = false,
            "--no-dat-test" => args.settings.dat_test = false,
            "--seed" => args.seed = Some(number(value()?)?),
            "--log" => args.log = Some(value()?.into()),
            "--jobs" => args.jobs = number(value()?)? as usize,
            "--out" => args.out = Some(value()?.into()),
            "-h" | "--help" => return Err(String::new()),
            _ if arg.starts_with("--") => return Err(format!("unknown option {arg}")),
            _ => args.files.push(arg.into()),
        }
    }
    if args.files.is_empty() && args.command != "gpus" {
        return Err("no program given".into());
    }
    Ok(args)
}

fn main() -> ExitCode {
    let args = match parse() {
        Ok(args) => args,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("error: {e}\n");
            }
            eprint!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let result = match args.command.as_str() {
        "compile" => compile_command(&args),
        "run" => run_command(&args),
        "tournament" => tournament_command(&args),
        "gpus" => gpus_command(),
        other => Err(format!("unknown command {other}")),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn compile_command(args: &Args) -> Result<ExitCode, String> {
    let mut ok = true;
    for path in &args.files {
        let name = path.display().to_string();
        let source = std::fs::read(path).map_err(|e| format!("{name}: {e}"))?;
        let compiled = compile(&source).map_err(|e| format!("{name}: {e}"))?;
        println!("; {name}, {} instructions, START at {}", compiled.program.code.len(), compiled.program.start);
        for (i, ins) in compiled.program.code.iter().enumerate() {
            println!("{i:3}  {ins}");
        }
        for m in &compiled.messages {
            println!("{}", m.text(&name));
        }
        ok &= compiled.is_ok();
    }
    Ok(if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

fn run_command(args: &Args) -> Result<ExitCode, String> {
    let sources: Vec<Source> =
        args.files.iter().map(|p| Source { name: p.display().to_string(), bytes: std::fs::read(p).ok() }).collect();
    let rng = match args.seed {
        Some(ticks) => Rng::from_ticks(ticks as u32),
        None => Rng::from_clock(),
    };
    let run = report::run(&sources, &args.settings, rng, args.wars.unwrap_or(1)).map_err(|(fatal, text)| {
        print!("{text}");
        fatal.to_string()
    })?;
    print!("{}", run.text);
    if let (Some(path), true) = (&args.log, run.played) {
        std::fs::write(path, &run.log).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(if run.played { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

fn tournament_command(args: &Args) -> Result<ExitCode, String> {
    let entries = args.files.iter().map(|p| Entry::load(p)).collect::<Result<Vec<_>, _>>()?;
    let backend = match &args.gpu {
        None => Backend::Cpu { jobs: args.jobs },
        Some(list) => Backend::Gpu { devices: gpu_devices(list)? },
    };
    let options = tournament::Options {
        games: args.games,
        settings: args.settings.clone(),
        backend,
        seed: args.seed.unwrap_or(0),
        format: args.format,
        out: args.out.clone().ok_or("--out is required")?,
        progress: true,
    };
    let summary = tournament::play(&entries, &options)?;
    println!(
        "{} wars, {} steps in {:.1} s, {:.2} billion steps per second",
        summary.wars,
        summary.steps,
        summary.seconds,
        summary.steps as f64 / summary.seconds / 1e9
    );
    Ok(ExitCode::SUCCESS)
}

#[cfg(feature = "gpu")]
fn gpu_devices(list: &str) -> Result<Vec<usize>, String> {
    let adapters = mars::gpu::adapters();
    let count = adapters.len();
    if count == 0 {
        return Err("no usable GPU found".into());
    }
    if list == "all" {
        return Ok(mars::gpu::best_of_kind(&adapters));
    }
    list.split(',')
        .map(|d| match d.parse::<usize>() {
            Ok(i) if i < count => Ok(i),
            _ => Err(format!("no GPU {d}, see mars gpus")),
        })
        .collect()
}

#[cfg(not(feature = "gpu"))]
fn gpu_devices(_: &str) -> Result<Vec<usize>, String> {
    Err("this build has no GPU support (cargo feature gpu)".into())
}

#[cfg(feature = "gpu")]
fn gpus_command() -> Result<ExitCode, String> {
    for (i, info) in mars::gpu::adapters().iter().enumerate() {
        println!("{i}  {}  ({:?}, {:?}, {})", info.name, info.device_type, info.backend, info.driver);
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(not(feature = "gpu"))]
fn gpus_command() -> Result<ExitCode, String> {
    gpu_devices("").map(|_| ExitCode::SUCCESS)
}
