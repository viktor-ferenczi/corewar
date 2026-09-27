//! Comparison with outputs recorded from the original MARS.COM under DOSBox by `tools/golden.py`.

use std::path::{Path, PathBuf};

use mars::report::{self, Session, Source, BANNER};
use mars::{compile, Rng, Settings};

fn golden() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn historical() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Historical")
}

fn word(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

/// Compile one source like MARS.COM does and compare with the recorded standard output, the dump
/// of the compiled program (`.BIN`), or the hang marker (`.HNG`).
fn check_compile(name: &str, source: &Path, stem: &Path) -> Result<(), String> {
    let bytes = std::fs::read(source).unwrap();
    let compiled = compile(&bytes);
    if stem.with_extension("HNG").exists() {
        return match compiled {
            Err(_) => Ok(()),
            Ok(c) => Err(format!("{name}: MARS.COM hangs, but compiled to {c:?}")),
        };
    }
    let compiled = compiled.map_err(|e| format!("{name}: {e}"))?;
    let mut text = format!("CoreWar MARS V1.0 by GM 1993\n\n{name}\n");
    for m in &compiled.messages {
        text += &m.text(name);
        text.push('\n');
    }
    if !compiled.is_ok() {
        text += "Cannot execute war, while there are any errors !\n";
    }
    let expected = String::from_utf8_lossy(&std::fs::read(stem.with_extension("OUT")).unwrap()).into_owned();
    if text != expected {
        return Err(format!("{name}: output differs\n--- MARS.COM\n{expected}--- Rust\n{text}"));
    }
    let dump = stem.with_extension("BIN");
    if !dump.exists() {
        return if compiled.is_ok() { Err(format!("{name}: MARS.COM did not dump the program")) } else { Ok(()) };
    }
    let dump = std::fs::read(dump).unwrap();
    let (len, start) = (word(&dump, 6) as usize, word(&dump, 26));
    let program = &compiled.program;
    if len != program.code.len() || start != program.start {
        return Err(format!("{name}: length/start {len}/{start}, Rust {}/{}", program.code.len(), program.start));
    }
    for (i, ins) in program.code.iter().enumerate() {
        let item = &dump[28 + 8 * i..];
        let expected = (item[0], item[1], word(item, 2), word(item, 4));
        if expected != (ins.op, ins.modes, ins.a, ins.b) {
            return Err(format!("{name}: instruction {i} is {expected:?}, Rust {ins:?}"));
        }
    }
    Ok(())
}

#[test]
fn compiler_matches_mars_com() {
    let dir = golden().join("compile");
    let mut cases: Vec<(String, PathBuf, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "CWR") {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            cases.push((name, path.clone(), path.with_extension("")));
        }
    }
    for entry in std::fs::read_dir(dir.join("historical")).unwrap() {
        let stem = entry.unwrap().path().with_extension("");
        let file = stem.file_name().unwrap().to_string_lossy().into_owned();
        // golden.py stores ARTUR-1 as ARTUR_1, the extensionless sources have no .CWR.
        let name = if historical().join(format!("{file}.CWR")).exists() {
            format!("{file}.CWR")
        } else {
            file.replace('_', "-")
        };
        if !cases.iter().any(|(n, ..)| *n == name) {
            cases.push((name.clone(), historical().join(&name), stem));
        }
    }
    assert!(cases.len() > 70, "only {} compile cases", cases.len());
    let failures: Vec<String> =
        cases.iter().filter_map(|(name, source, stem)| check_compile(name, source, stem).err()).collect();
    assert!(failures.is_empty(), "{} of {} differ:\n{}", failures.len(), cases.len(), failures.join("\n"));
}

struct Case {
    id: String,
    seed: u32,
    wars: u16,
    settings: Settings,
    programs: Vec<String>,
}

fn parse_cases() -> Vec<Case> {
    let text = std::fs::read_to_string(golden().join("battle/CASES.txt")).unwrap();
    let mut cases = Vec::new();
    for line in text.lines().filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let mut tokens = line.split_whitespace();
        let id = tokens.next().unwrap().to_string();
        let seed = tokens.next().unwrap().parse().unwrap();
        let mut case = Case { id, seed, wars: 1, settings: Settings::default(), programs: Vec::new() };
        for token in tokens {
            let value = || token[3..].parse::<u32>().unwrap();
            match token.get(..2) {
                Some("/P") => case.wars = value() as u16,
                Some("/M") => case.settings.max_steps = value(),
                Some("/Q") => case.settings.queue_len = value() as u16,
                Some("/E") => case.settings.exec_other = false,
                _ => case.programs.push(token.to_string()),
            }
        }
        cases.push(case);
    }
    cases
}

fn check_battle(session: &mut Session, case: &Case) -> Result<(), String> {
    let dir = golden().join("battle");
    let sources: Vec<Source> = case
        .programs
        .iter()
        .map(|name| {
            let special = golden().join("progs").join(name);
            let path = if special.exists() { special } else { historical().join(name) };
            Source { name: name.clone(), bytes: Some(std::fs::read(path).unwrap()) }
        })
        .collect();
    let run = report::run_in(session, &sources, &case.settings, Rng::from_ticks(case.seed), case.wars)
        .map_err(|(e, _)| e.to_string())?;
    // Everything but the first line is the same as MARS.COM prints.
    let recorded = std::fs::read_to_string(dir.join(format!("{}.OUT", case.id))).unwrap();
    let expected = recorded.replacen("CoreWar MARS V1.0 by GM 1993", BANNER, 1);
    let log = std::fs::read(dir.join(format!("{}.LOG", case.id))).unwrap();
    if run.text != expected || run.log != log {
        return Err(format!("{}: differs\n--- MARS.COM\n{expected}--- Rust\n{}", case.id, run.text));
    }
    Ok(())
}

/// Plays the recorded DOSBox sessions, each in order in its own thread.
#[test]
fn battles_match_mars_com() {
    let cases = parse_cases();
    assert!(cases.len() > 150);
    let text = std::fs::read_to_string(golden().join("battle/SESSIONS.txt")).unwrap();
    let sessions: Vec<Vec<&Case>> = text
        .lines()
        .map(|line| line.split_whitespace().map(|id| cases.iter().find(|c| c.id == id).unwrap()).collect())
        .collect();
    assert_eq!(sessions.iter().map(Vec::len).sum::<usize>(), cases.len());
    let failures = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for session_cases in &sessions {
            let failures = &failures;
            s.spawn(move || {
                let mut session = Session::new();
                for case in session_cases {
                    if let Err(e) = check_battle(&mut session, case) {
                        failures.lock().unwrap().push(e);
                    }
                }
            });
        }
    });
    let mut failures = failures.into_inner().unwrap();
    failures.sort();
    assert!(failures.is_empty(), "{} of {} differ:\n{}", failures.len(), cases.len(), failures.join("\n"));
}
