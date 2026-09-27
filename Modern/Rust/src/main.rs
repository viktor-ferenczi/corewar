use std::path::PathBuf;
use std::process::ExitCode;

use mars::report::{self, Source};
use mars::tournament::{self, Entry};
use mars::{compile, Rng, Settings};

const USAGE: &str = "\
CoreWar MARS V1.0 (1993) compiler and simulator, native reimplementation

Usage:
  mars compile FILE
      Print the compiled program and the error messages of MARS.
  mars run [OPTIONS] FILE...
      Play wars between the programs and print the statistics like MARS /P.
  mars tournament [OPTIONS] --out DIR FILE...
      Round robin of the programs, every pair in both start orders.
      Writes DIR/results/runs.csv and DIR/raw/FIRST_vs_SECOND.sta.

Options:
  --wars N            wars to play (run: 1, tournament: 500 per start order)
  --steps N           steps per war, both programs counted, 0 = 2^32 (600000)
  --queue N           processes per program, 1..256 (64)
  --no-exec-other     programs die when executing a cell another program wrote
  --no-dat-test       do not end a war when no DAT is left (MARS without /P)
  --seed N            run: BIOS tick count to seed from (default: time of day),
                      tournament: master seed (0)
  --log FILE          run: write the binary statistics log of MARS /F
  --jobs N            tournament: parallel threads (all cores)
  --out DIR           tournament: output folder
";

struct Args {
    command: String,
    files: Vec<PathBuf>,
    wars: Option<u16>,
    settings: Settings,
    seed: Option<u64>,
    log: Option<PathBuf>,
    jobs: usize,
    out: Option<PathBuf>,
}

fn parse() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let command = it.next().ok_or("missing command")?;
    if command == "-h" || command == "--help" {
        return Err(String::new());
    }
    let mut args = Args {
        command,
        files: Vec::new(),
        wars: None,
        settings: Settings::default(),
        seed: None,
        log: None,
        jobs: std::thread::available_parallelism().map_or(1, |n| n.get()),
        out: None,
    };
    while let Some(arg) = it.next() {
        let mut value = || it.next().ok_or(format!("{arg} needs a value"));
        let number = |v: String| v.parse::<u64>().map_err(|_| format!("{arg}: not a number: {v}"));
        match arg.as_str() {
            "--wars" => args.wars = Some(number(value()?)?.clamp(1, 65535) as u16),
            "--steps" => {
                args.settings.max_steps = u32::try_from(number(value()?)?).map_err(|_| "--steps is too large")?
            }
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
    if args.files.is_empty() {
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
    let entries = args
        .files
        .iter()
        .map(|p| Entry::load(p).map_err(|e| format!("{}: {e}", p.display())))
        .collect::<Result<Vec<_>, _>>()?;
    let options = tournament::Options {
        wars_per_order: args.wars.unwrap_or(500),
        settings: args.settings.clone(),
        jobs: args.jobs,
        seed: args.seed.unwrap_or(0),
        out: args.out.clone().ok_or("--out is required")?,
    };
    let wall = tournament::play(&entries, &options)?;
    println!("Finished in {wall:.1} s");
    Ok(ExitCode::SUCCESS)
}
