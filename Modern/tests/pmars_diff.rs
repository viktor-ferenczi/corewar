//! Offline by default. PMARS_092_BIN and PMARS_095_BIN select binaries built from upstream source.

use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use mars::assembler::{compile_with_context, AssemblyContext, MODES, MODIFIERS, OPCODES};
use mars::{Engine, Instruction, Program, Rng, Settings, Standard};

const SIT: &str = ";redcode\n;assert 1\nJMP 0\n";
static CASE: AtomicUsize = AtomicUsize::new(0);

struct Reference {
    accepted: bool,
    programs: Vec<Program>,
    outcome: Vec<u32>,
}

fn reference(binary: &str, sources: &[&str], settings: &Settings, rounds: u16) -> Reference {
    let case = CASE.fetch_add(1, Ordering::Relaxed);
    let paths: Vec<_> = sources
        .iter()
        .enumerate()
        .map(|(index, source)| {
            let path = std::env::temp_dir().join(format!("mars-diff-{}-{case}-{index}.red", std::process::id()));
            std::fs::write(&path, source).unwrap();
            path
        })
        .collect();
    let mut command = Command::new("timeout");
    command.args([
        "2",
        binary,
        "-r",
        &rounds.to_string(),
        "-c",
        &settings.max_steps.to_string(),
        "-p",
        &settings.queue_len.to_string(),
        "-s",
        &settings.core_size.to_string(),
        "-l",
        &settings.program_len.to_string(),
        "-d",
        &settings.min_distance.to_string(),
    ]);
    if sources.len() == 2 {
        command.args(["-F", "4000"]);
    }
    if settings.standard == Standard::Icws88 {
        command.arg("-8");
    }
    command.args(&paths);
    let output = command.output().unwrap();
    for path in paths {
        std::fs::remove_file(path).unwrap();
    }
    assert_ne!(output.status.code(), Some(124), "pMARS timed out: {sources:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() {
        return Reference { accepted: false, programs: Vec::new(), outcome: Vec::new() };
    }
    let mut programs: Vec<Program> = Vec::new();
    let mut outcome = Vec::new();
    for line in stdout.lines() {
        if line.starts_with("Program ") {
            programs.push(Program { code: Vec::new(), start: 0 });
            continue;
        }
        if let Some(results) = line.trim_start().strip_prefix("Results:") {
            outcome.extend(results.split_whitespace().map(|value| value.parse::<u32>().unwrap()));
            continue;
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        let Some(at) = fields.iter().position(|field| OPCODES.contains(&field.split('.').next().unwrap_or(""))) else {
            continue;
        };
        if fields.len() < at + 5 {
            continue;
        }
        let (name, modifier) =
            fields[at].split_once('.').map_or((fields[at], None), |(name, modifier)| (name, Some(modifier)));
        let op = OPCODES.iter().position(|candidate| *candidate == name).unwrap() as u8;
        let mode = |field: &str| MODES.iter().position(|candidate| candidate.to_string() == field).unwrap() as u8;
        let ma = mode(fields[at + 1]);
        let mb = mode(fields[at + 3]);
        let modifier = modifier.map_or_else(
            || match op {
                0 => 4,
                1 | 8 => {
                    if ma == 0 {
                        2
                    } else if mb == 0 {
                        1
                    } else {
                        6
                    }
                }
                2 | 3 => {
                    if ma == 0 {
                        2
                    } else if mb == 0 {
                        1
                    } else {
                        4
                    }
                }
                10 => {
                    if ma == 0 {
                        2
                    } else {
                        1
                    }
                }
                _ => 1,
            },
            |name| MODIFIERS.iter().position(|candidate| *candidate == name).unwrap() as u8,
        );
        let value = |field: &str| {
            field.trim_end_matches(',').parse::<i64>().unwrap().rem_euclid(settings.core_size as i64) as u16
        };
        let program = programs.last_mut().unwrap();
        if fields.first() == Some(&"START") {
            program.start = program.code.len() as u16;
        }
        program.code.push(Instruction {
            op,
            modifier,
            modes: ma | mb << 3,
            wide_modes: true,
            a: value(fields[at + 2]),
            b: value(fields[at + 4]),
        });
    }
    Reference { accepted: true, programs, outcome }
}

fn compile_sources(sources: &[&str], settings: &Settings, rounds: u16) -> Vec<Program> {
    sources
        .iter()
        .enumerate()
        .map(|(index, source)| {
            let compiled = compile_with_context(
                source.as_bytes(),
                settings,
                AssemblyContext { warriors: sources.len(), rounds, first: index == 0 },
            );
            assert!(compiled.is_ok(), "{:?}\n{source}", compiled.messages);
            compiled.program
        })
        .collect()
}

fn outcomes(programs: &[Program], settings: &Settings, rounds: u16) -> Vec<u32> {
    let mut engine = Engine::new(settings.clone(), programs, Rng::from_ticks(0));
    let mut result = vec![0; 3];
    let mut seed = 4000u64 - settings.min_distance as u64;
    for _ in 0..rounds {
        let position =
            settings.min_distance as u64 + seed % (settings.core_size as u64 + 1 - 2 * settings.min_distance as u64);
        engine.place(Some(&[0, position as u16])).unwrap();
        // -F seeds the first position; later rounds use pMARS's Park-Miller generator.
        seed = seed * 16807 % 2147483647;
        let index = engine.fight().winner.unwrap_or(2);
        result[index] += 1;
    }
    result
}

fn compare(binary: &str, source: &str, settings: &Settings, rounds: u16) {
    let sit = if settings.standard == Standard::Icws94 { "JMP.B $0,$0\n" } else { SIT };
    let sources = [source, sit];
    let expected = reference(binary, &sources, settings, rounds);
    assert!(expected.accepted, "reference rejected\n{source}");
    let actual = compile_sources(&sources, settings, rounds);
    assert_eq!(actual, expected.programs, "listing\n{source}");
    assert_eq!(outcomes(&actual, settings, rounds), expected.outcome, "outcome\n{source}");
}

#[test]
fn pmars_092_assembler_sources() {
    let Ok(binary) = std::env::var("PMARS_092_BIN") else { return };
    let settings = Settings { quirks: true, max_steps: 100, ..Settings::pmars() };
    let sources = [
        ";redcode\n;assert CORESIZE==8000\nbase equ 1+2\nDAT #0, #base*2\nJMP 0\n",
        "DAT 99,99\n;redcode-94\n;assert 1\nDAT 1 2, 3 4\nJMP 0\n;redcode\nDAT 9,9\n",
        "x DAT 1,2\nx DAT 3,4\nDAT 5,6\n",
        "DAT #w,#s\nDAT #q=3,#q+1\nJMP 0\n",
        "DAT #0,#1-2*3+4\nDAT #0,#10-2*3-1\nJMP 0\n",
        "op EQU MOV\nop #1,2\nJMP 0\n",
        "mac EQU DAT 1,2\nEQU DAT 3,4\nmac\nJMP 0\n",
        "DAT #0,#future\nfuture EQU 2+3\nJMP 0\n",
        "i FOR 3\ncell&i DAT #i, #CURLINE\nROF\nJMP 0\n",
        "load0 z FOR 0\nthis is not valid\nROF\nDAT #0,#load0\nJMP 0\n",
        "i FOR 2\nj FOR 2\nDAT #i,#j\nROF\nROF\nJMP 0\n",
        "ORG 1\nDAT 0,0\nJMP 0\nEND 2\n",
        "ORG 0\nDAT 0,0\nJMP 0\nEND 1\n",
        ";assert unknown\nJMP 0\n",
        "MOV . AB # 5 , @ 3\nJMP 0\n",
        "DAT #0, #1+\\\n2\nJMP 0\n",
        "i FOR 2\nMOV.I 0,1\nROF\nab FOR 1\nMOV.ab 0,1\nROF\nJMP 0\n",
        "x EQU 3*(2+1)\nb EQU 4\nMUL.X #x,#b\nADD.b #b,1\nJMP 0\n",
    ];
    for source in sources {
        compare(&binary, source, &settings, 1);
    }
    for source in [
        ";assert 0\nJMP 0",
        "MOV 0",
        "DAT",
        "MOV 0,",
        "DAT #0,#1+2*3==7",
        "PIN 1\nJMP 0",
        "MOV.AZ 1,2",
        "ab FOR 1\nMOV . ab 0,1\nROF",
    ] {
        let expected = reference(&binary, &[source, SIT], &settings, 1);
        let actual = compile_with_context(source.as_bytes(), &settings, AssemblyContext::default());
        assert!(!expected.accepted || source.starts_with("PIN"), "reference accepted {source}");
        assert!(!actual.is_ok(), "accepted {source}");
    }
}

#[test]
fn fixed_position_opcode_and_timing_probes() {
    for (variable, standard, quirks) in
        [("PMARS_092_BIN", Standard::Pmars, true), ("PMARS_095_BIN", Standard::Pmars, false)]
    {
        let Ok(binary) = std::env::var(variable) else { continue };
        let settings = Settings { quirks, max_steps: 100, ..Settings::for_standard(standard) };
        for source in [
            "ADD.B >0,#0\nJMZ.B 2,-1\nJMP 0\nDAT 0,0\n",
            "JMN.F 2,3\nDAT 0,0\nJMP 0\nDAT 0,1\n",
            "DJN.F 2,3\nDAT 0,0\nJMP 0\nDAT 1,2\n",
            "DJN.B 2,>0\nDAT 0,0\nJMP 0\n",
            "SPL 2\nDIV.F 3,4\nCMP.F 2,3\nJMP 0\nDAT 0,2\nDAT 0,0\n",
            "SEQ.I a,b\nDAT 0,0\nJMP 0\na CMP.B 1,2\nb SEQ.B 1,2\n",
            "SPL 2\nDAT 0,0\nJMP 0\n",
        ] {
            compare(&binary, source, &settings, 2);
        }
    }
}

#[test]
fn generated_cells_and_battles() {
    let cases = std::env::var("MARS_DIFF_CASES").ok().map(|value| value.parse::<usize>().unwrap()).unwrap_or(96);
    for (variable, standard, quirks) in [
        ("PMARS_092_BIN", Standard::Pmars, true),
        ("PMARS_095_BIN", Standard::Icws94, false),
        ("PMARS_092_BIN", Standard::Icws88, true),
    ] {
        let Ok(binary) = std::env::var(variable) else { continue };
        let settings = Settings { quirks, max_steps: 100, queue_len: 64, ..Settings::for_standard(standard) };
        let mut random = 0x1994u32;
        let mut next = || {
            random = random.wrapping_mul(1664525).wrapping_add(1013904223);
            random >> 16
        };
        for _ in 0..cases {
            let mut source = String::from(";redcode\n;assert 1\nSPL.B $1,$0\nSPL.B $1,$0\n");
            if standard == Standard::Icws88 {
                source = String::from(";redcode\n;assert 1\nSPL 1\nSPL 1\n");
            }
            for _ in 0..12 {
                let mut op = next() as usize % if standard == Standard::Icws88 { 11 } else { 17 };
                if standard == Standard::Icws94 && op == 14 {
                    op = 8;
                }
                let modifier = MODIFIERS[next() as usize % 7];
                let mut ma = MODES[next() as usize % 8];
                let mut mb = MODES[next() as usize % 8];
                if standard == Standard::Icws88 {
                    ma = if op == 0 {
                        ['#', '<'][next() as usize % 2]
                    } else if matches!(op, 4 | 5 | 6 | 7 | 9) {
                        ['$', '@', '<'][next() as usize % 3]
                    } else {
                        ['#', '$', '@', '<'][next() as usize % 4]
                    };
                    mb = if op == 0 {
                        ['#', '<'][next() as usize % 2]
                    } else if matches!(op, 1 | 2 | 3 | 8) {
                        ['$', '@', '<'][next() as usize % 3]
                    } else {
                        ['#', '$', '@', '<'][next() as usize % 4]
                    };
                }
                let a = next() as i32 % 17 - 8;
                let b = next() as i32 % 17 - 8;
                source += &format!(
                    "{}{} {ma}{a}, {mb}{b}\n",
                    OPCODES[op],
                    if standard == Standard::Icws88 { String::new() } else { format!(".{modifier}") }
                );
            }
            compare(&binary, &source, &settings, 1);
        }
    }
}

#[test]
fn upstream_warrior_collection() {
    let Ok(binary) = std::env::var("PMARS_092_BIN") else { return };
    let Ok(directory) = std::env::var("PMARS_092_SRC") else { return };
    let settings = Settings { quirks: true, max_steps: 10000, ..Settings::pmars() };
    let mut collection = Vec::new();
    for name in ["aeka.red", "flashpaper.red", "rave.red", "validate.red"] {
        let source = std::fs::read_to_string(std::path::Path::new(&directory).join("warriors").join(name)).unwrap();
        compare(&binary, &source, &settings, 2);
        collection.push(source);
    }
    for first in &collection {
        for second in &collection {
            let sources = [first.as_str(), second.as_str()];
            let expected = reference(&binary, &sources, &settings, 4);
            assert!(expected.accepted);
            let programs = compile_sources(&sources, &settings, 4);
            assert_eq!(programs, expected.programs);
            assert_eq!(outcomes(&programs, &settings, 4), expected.outcome);
        }
    }
}

#[test]
fn multi_warrior_counting_matches_pmars_092() {
    let Ok(binary) = std::env::var("PMARS_092_BIN") else { return };
    for limit in 2..=8 {
        let settings = Settings { quirks: true, max_steps: limit, ..Settings::pmars() };
        for count in 1..=limit + 1 {
            let countdown = format!("DJN 0,#{count}\nDAT 0,0\n");
            let sources = ["DAT 0,0\n", SIT, countdown.as_str()];
            let reference = reference(&binary, &sources, &settings, 3);
            assert!(reference.accepted);
            let programs = compile_sources(&sources, &settings, 3);
            assert_eq!(programs, reference.programs);
            let mut engine = Engine::new(settings.clone(), &programs, Rng::from_ticks(0));
            let mut scores = vec![0; 12];
            for _ in 0..3 {
                engine.place(Some(&[0, 2000, 4000])).unwrap();
                engine.fight();
                let survivors = engine.warriors.iter().filter(|warrior| warrior.pcnum != 0).count();
                for (index, warrior) in engine.warriors.iter().enumerate() {
                    let column = if warrior.pcnum == 0 { 3 } else { survivors - 1 };
                    scores[index * 4 + column] += 1;
                }
            }
            assert_eq!(scores, reference.outcome, "limit {limit}, countdown {count}");
        }
    }
}

#[test]
fn generated_assembler_expressions() {
    let Ok(binary) = std::env::var("PMARS_092_BIN") else { return };
    let settings = Settings { quirks: true, max_steps: 10, ..Settings::pmars() };
    let mut random = 0x88u32;
    let mut next = || {
        random = random.wrapping_mul(1664525).wrapping_add(1013904223);
        random >> 16
    };
    let operators = ["+", "-", "*", "/", "%", "<", ">", "==", "!=", "&&", "||"];
    for _ in 0..96 {
        let mut expression = (next() % 9 + 1).to_string();
        for _ in 0..3 {
            expression += &format!("{}{}", operators[next() as usize % operators.len()], next() % 9 + 1);
        }
        let source = format!(";assert 1\nJMP 0\nDAT #0,#{expression}\n");
        let expected = reference(&binary, &[&source, SIT], &settings, 1);
        let actual = compile_with_context(source.as_bytes(), &settings, AssemblyContext::default());
        assert_eq!(actual.is_ok(), expected.accepted, "{expression}: {:?}", actual.messages);
        if expected.accepted {
            assert_eq!(actual.program, expected.programs[0], "{expression}");
        }
    }
}
