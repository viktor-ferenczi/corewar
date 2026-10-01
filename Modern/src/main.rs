use std::path::PathBuf;
use std::process::ExitCode;

use mars::assembler::{assemble, AssemblyContext};
use mars::report::{self, Source};
use mars::tournament::{self, Backend, Entry, Format};
use mars::{Rng, Settings, Standard};

const USAGE: &str = "\
CoreWar MARS Rust V1.0 - Viktor Ferenczi 2026
Redcode assembler and simulator, with ICWS standards and historical MARS.COM rules

Usage:
  mars compile [OPTIONS] FILE...
      Print the compiled program and the error messages of MARS.
      With --json one JSON object per file and line, and a file that fails
      does not stop the rest.
  mars run [OPTIONS] FILE...
      Play wars between the programs and print the statistics like MARS /P.
  mars tournament [OPTIONS] --out PATH FILE...
      Pairwise tournament of 2 to 256 programs, with rotating first movers.
      Writes one JSON object per run to the file PATH, or with --format sta
      PATH/results/runs.csv and PATH/raw/FIRST_vs_SECOND.sta.
      With --against every FILE plays every opponent instead (a gauntlet),
      and neither group plays among itself. Up to 16384 programs in total.
  mars gpus
      List the GPUs --gpu can use.

Options:
  --wars N            run: wars to play (1)
  --games N           tournament: total games per pair (1000)
  --format F          tournament: jsonl (default) or sta
  --against FILE...   tournament: the programs after it are the opponents
  --json              compile: machine readable output
  --cpu               tournament: play on CPU threads (default)
  --gpu [LIST]        tournament: play on GPUs, the first one by default, or a
                      list like 0,1, or all (all GPUs of the best kind present)
  --steps N           cycles per warrior (80000); hu93: shared steps (600000)
                      0 means 2^32 cycles or shared steps
  --core N            arena size in cells (8000)
  --standard S        pmars (default), hu93, 88 or 94
  --syntax hu93       compile MARS.COM sources for another standard
  --quirks            reproduce the reference implementation's bugs
  --norotate          keep the same first mover in every war
  --length N          maximum program length (100)
  --distance N        minimum start distance (0 for hu93, 100 otherwise)
  --queue N           processes per program (8000; hu93: 64, at most 256)
  --no-exec-other     hu93: die when executing another program's cell
  --no-dat-test       hu93: do not end a war when no DAT is left
  --seed N            run: BIOS tick count to seed from (default: time of day),
                      tournament: master seed (0)
  --log FILE          run: write the binary statistics log of MARS /F
  --jobs N            tournament: CPU threads (all cores)
  --out PATH          tournament: output file (jsonl) or folder (sta)
";

struct Args {
    command: String,
    files: Vec<PathBuf>,
    /// Files after `--against`, the opponents of a gauntlet.
    against: Option<Vec<PathBuf>>,
    json: bool,
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
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let command = raw.first().ok_or("missing command")?.clone();
    if command == "-h" || command == "--help" {
        return Err(String::new());
    }
    let standard = raw
        .windows(2)
        .filter(|pair| pair[0] == "--standard")
        .map(|pair| pair[1].parse::<Standard>())
        .next_back()
        .transpose()?
        .unwrap_or(Standard::Pmars);
    let mut settings = Settings::for_standard(standard);
    settings.quirks = false;
    settings.rotate = true;
    let mut it = raw.into_iter().skip(1).peekable();
    let mut args = Args {
        command,
        files: Vec::new(),
        against: None,
        json: false,
        wars: None,
        games: 1000,
        format: Format::Jsonl,
        gpu: None,
        settings,
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
            "--core" => {
                args.settings.core_size =
                    u16::try_from(number(value()?)?).map_err(|_| "--core must be 100 to 65535")?;
                if args.settings.core_size < 100 {
                    return Err("--core must be 100 to 65535".into());
                }
            }
            "--standard" => args.settings.standard = value()?.parse::<Standard>()?,
            "--syntax" => {
                if value()? != "hu93" {
                    return Err("--syntax currently accepts only hu93".into());
                }
                args.settings.hu93_syntax = true;
            }
            "--quirks" => args.settings.quirks = true,
            "--norotate" => args.settings.rotate = false,
            "--length" => {
                args.settings.program_len = u16::try_from(number(value()?)?).map_err(|_| "--length is too large")?
            }
            "--distance" => {
                args.settings.min_distance = u16::try_from(number(value()?)?).map_err(|_| "--distance is too large")?
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
            "--against" => args.against = Some(Vec::new()),
            "--json" => args.json = true,
            "--queue" => {
                args.settings.queue_len = u16::try_from(number(value()?)?).map_err(|_| "--queue is too large")?
            }
            "--no-exec-other" => args.settings.exec_other = false,
            "--no-dat-test" => args.settings.dat_test = false,
            "--seed" => args.seed = Some(number(value()?)?),
            "--log" => args.log = Some(value()?.into()),
            "--jobs" => args.jobs = number(value()?)? as usize,
            "--out" => args.out = Some(value()?.into()),
            "-h" | "--help" => return Err(String::new()),
            _ if arg.starts_with("--") => return Err(format!("unknown option {arg}")),
            _ => args.against.as_mut().unwrap_or(&mut args.files).push(arg.into()),
        }
    }
    if args.files.is_empty() && args.command != "gpus" {
        return Err("no program given".into());
    }
    if args.command != "gpus" {
        if args.settings.queue_len == 0 || args.settings.queue_len > if standard == Standard::Hu93 { 256 } else { 8000 }
        {
            return Err(format!("--queue must be 1 to {}", if standard == Standard::Hu93 { 256 } else { 8000 }));
        }
        let max_length = if standard == Standard::Hu93 { 100 } else { 1000 };
        if args.settings.program_len == 0 || args.settings.program_len > max_length {
            return Err(format!("--length must be 1 to {max_length}"));
        }
        if args.settings.program_len > args.settings.core_size {
            return Err("--length must not exceed --core".into());
        }
        if !args.settings.exec_other && standard != Standard::Hu93 {
            return Err("--no-exec-other is only available with hu93".into());
        }
        if args.settings.min_distance > args.settings.core_size / 2 {
            return Err("--distance must be at most half the core size".into());
        }
        if args.settings.standard == Standard::Hu93 && args.settings.quirks && args.settings.core_size != 8000 {
            return Err("hu93 --quirks requires --core 8000".into());
        }
        if args.log.is_some() && standard != Standard::Hu93 {
            return Err("--log is only available with hu93".into());
        }
        match (args.settings.standard, args.settings.quirks) {
            (Standard::Hu93 | Standard::Icws88 | Standard::Pmars | Standard::Icws94, false) => {}
            (Standard::Hu93 | Standard::Icws88 | Standard::Pmars, true) => {}
            (Standard::Icws94, true) => return Err("94 does not support --quirks".into()),
        }
    }
    if args.against.as_ref().is_some_and(|opponents| opponents.is_empty() || args.command != "tournament") {
        return Err("--against needs opponents, and is for tournament only".into());
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

/// One line of `compile --json`: the instructions, or the errors that keep the program from playing.
fn compile_json(name: &str, source: std::io::Result<Vec<u8>>, settings: &Settings, context: AssemblyContext) -> String {
    use mars::tournament::json_string;
    let failed = |errors: Vec<String>| {
        let errors: Vec<String> = errors.iter().map(|e| json_string(e)).collect();
        format!("{{\"file\":{},\"ok\":false,\"errors\":[{}]}}", json_string(name), errors.join(","))
    };
    let compiled = match source
        .map_err(|e| e.to_string())
        .and_then(|s| assemble(&s, settings, context).map_err(|f| f.to_string()))
    {
        Ok(compiled) => compiled,
        Err(e) => return failed(vec![e]),
    };
    if !compiled.is_ok() {
        return failed(compiled.messages.iter().map(|m| m.text(name)).collect());
    }
    let instructions: Vec<String> = compiled
        .program
        .code
        .iter()
        .map(|ins| {
            let (op, modifier, a_mode, b_mode) = ins.fields();
            // A MARS.COM source played under another standard gets its modifier when it is loaded.
            let modifier = match settings.standard {
                Standard::Hu93 => modifier,
                _ => mars::assembler::MODIFIERS.get(mars::engine::load_modifier(settings, ins) as usize).copied(),
            };
            format!(
                "{{\"op\":\"{op}\",\"modifier\":{},\"a_mode\":\"{a_mode}\",\"a\":{},\"b_mode\":\"{b_mode}\",\"b\":{}}}",
                modifier.map_or("null".into(), |m| format!("\"{m}\"")),
                ins.a,
                ins.b
            )
        })
        .collect();
    format!(
        "{{\"file\":{},\"ok\":true,\"start\":{},\"instructions\":[{}]}}",
        json_string(name),
        compiled.program.start,
        instructions.join(",")
    )
}

fn compile_command(args: &Args) -> Result<ExitCode, String> {
    let mut ok = true;
    for (index, path) in args.files.iter().enumerate() {
        let name = path.display().to_string();
        if args.json {
            let context = AssemblyContext { warriors: 2, rounds: args.wars.unwrap_or(1), first: true };
            let line = compile_json(&name, std::fs::read(path), &args.settings, context);
            ok &= line.contains("\"ok\":true,");
            println!("{line}");
            continue;
        }
        let source = std::fs::read(path).map_err(|e| format!("{name}: {e}"))?;
        let compiled = assemble(
            &source,
            &args.settings,
            AssemblyContext { warriors: args.files.len(), rounds: args.wars.unwrap_or(1), first: index == 0 },
        )
        .map_err(|fatal| format!("{name}: {fatal}"))?;
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
    let rounds = if args.settings.rotate { args.games } else { args.games.div_ceil(2) } as u16;
    let opponents = args.against.as_deref().unwrap_or_default();
    let files: Vec<&PathBuf> = args.files.iter().chain(opponents).collect();
    let entries = files
        .iter()
        .enumerate()
        .map(|(index, path)| {
            Entry::load_with_context(
                path,
                &args.settings,
                AssemblyContext { warriors: 2, rounds, first: !args.settings.rotate || index + 1 < files.len() },
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
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
        against: opponents.len(),
    };
    let summary = tournament::play(&entries, &options)?;
    println!(
        "{} wars, {} steps in {:.1} s, {:.2} billion steps per second (standard {}, quirks {}, rotate {})",
        summary.wars,
        summary.steps,
        summary.seconds,
        summary.steps as f64 / summary.seconds / 1e9,
        args.settings.standard.name(),
        args.settings.quirks,
        args.settings.rotate,
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
