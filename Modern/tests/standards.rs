use std::process::Command;

use mars::assembler::{compile_with_context, AssemblyContext};
use mars::{Engine, MessageKind, Rng, Settings, Standard};

#[test]
fn defaults_and_assembler_quirks() {
    let dos = mars::compile_hu93_clean(b"MOV 0 1\r\n\x1aignored", 8000);
    assert!(dos.is_ok());
    assert!(compile_with_context(b"; Latin-1 comment: \xff\nJMP 0\n", &Settings::pmars(), AssemblyContext::default())
        .is_ok());
    let settings = Settings::default();
    assert_eq!(settings.standard, Standard::Pmars);
    assert!(!settings.quirks);
    assert!(settings.rotate);
    let context = AssemblyContext { warriors: 3, rounds: 7, first: true };
    let source = b"DAT #w, #s\nDAT #WARRIORS, #ROUNDS\nDAT #0, #1-2*3+4\nNOP 0\n";
    let plain = compile_with_context(source, &settings, context);
    assert!(plain.is_ok(), "{:?}", plain.messages);
    assert_eq!((plain.program.code[0].a, plain.program.code[0].b), (0, 0));
    assert_eq!((plain.program.code[1].a, plain.program.code[1].b), (3, 7));
    assert_eq!(plain.program.code[2].b, 7999);
    assert_eq!(plain.program.code[3].modifier, 4);
    let quirks = Settings { quirks: true, ..settings.clone() };
    let first = compile_with_context(source, &quirks, context);
    let second = compile_with_context(source, &quirks, AssemblyContext { first: false, ..context });
    assert!(first.is_ok() && second.is_ok());
    assert_eq!((first.program.code[0].a, first.program.code[0].b), (3, 3));
    assert_eq!((second.program.code[0].a, second.program.code[0].b), (0, 0));
    assert_eq!(first.program.code[2].b, 7991);
    assert!(!compile_with_context(b"DAT #0, #1 2\n", &settings, context).is_ok());
    assert_eq!(compile_with_context(b"DAT #0, #1 2\n", &quirks, context).program.code[0].b, 12);
    assert!(!compile_with_context(b"x DAT 0,0\nx DAT 1,2\n", &settings, context).is_ok());
    assert_eq!(compile_with_context(b"x DAT 0,0\nx DAT 1,2\n", &quirks, context).program.code.len(), 1);
}

#[test]
fn pmars_line_reading_and_messages() {
    let compile = |source: &str, quirks: bool| {
        let settings = Settings { quirks, ..Settings::pmars() };
        compile_with_context(source.as_bytes(), &settings, AssemblyContext::default())
    };
    let kinds = |source: &str| compile(source, false).messages.iter().map(|m| (m.kind, m.line)).collect::<Vec<_>>();
    let split = format!("JMP 0 ;{}DAT 7,7\n", "x".repeat(248));
    for quirks in [false, true] {
        // A label in a FOR count is relative, so this runs four times.
        assert_eq!(compile("xx DAT 0\nJMP 0\ncnt FOR 2-xx\nDAT 1\nROF\n", quirks).program.code.len(), 6);
        assert!(compile("MOV#1,2 \nMOV.AB@1,{2 ; glued\nMOV .AB#1,2\n", quirks).is_ok());
        assert_eq!(compile(&split, quirks).program.code.len(), if quirks { 2 } else { 1 });
    }
    assert!(compile("MOV#1,2\nx MOV.AB{1,2\n", false).is_ok());
    assert!(!compile("MOV#1,2\n", true).is_ok());
    assert!(!compile(&format!(";{}\nJMP 0\n", "x".repeat(4097)), false).is_ok());
    assert_eq!(kinds("JMP 0\n;assert CORESIZE==55\n"), [(MessageKind::AssertionFailed, 2)]);
    assert_eq!(kinds("ORG 2\nJMP 0\nJMP 0\n"), [(MessageKind::StartOutside, 1)]);
    assert_eq!(kinds("JMP 0\nEND -1\n"), [(MessageKind::StartOutside, 2)]);
    assert!(compile("ORG 1\nJMP 0\nJMP 0\nEND 0\n", false).is_ok());
}

#[test]
fn multi_warrior_cycle_counting() {
    for quirks in [false, true] {
        let settings = Settings { quirks, rotate: false, max_steps: 5, ..Settings::pmars() };
        let programs: Vec<_> = ["DAT 0,0\n", "JMP 0\n", "JMP 0\n"]
            .iter()
            .map(|source| compile_with_context(source.as_bytes(), &settings, AssemblyContext::default()).program)
            .collect();
        let mut engine = Engine::new(settings, &programs, Rng::from_ticks(0));
        engine.place(Some(&[0, 2000, 4000])).unwrap();
        assert_eq!(engine.fight().steps, if quirks { 10 } else { 11 });
    }
}

#[test]
fn plain_hu93_cmp_matches_an_empty_dat_cell() {
    let settings = Settings { quirks: false, max_steps: 10, ..Settings::hu93() };
    let program = mars::compile_hu93_clean(b"START CMP 3 100\nDAT 0\nJMP 0\nDAT 0\n", 8000).program;
    let mut engine = Engine::new(settings, &[program], Rng::from_ticks(0));
    engine.place(Some(&[0])).unwrap();
    engine.step(0);
    assert_eq!(engine.warriors[0].pcs[0], 2);
}

#[test]
fn cli_rejects_incompatible_settings() {
    for args in [
        vec!["--standard", "94", "--quirks"],
        vec!["--no-exec-other"],
        vec!["--log", "unused"],
        vec!["--core", "100", "--length", "101"],
        vec!["--standard", "hu93", "--quirks", "--core", "1000"],
    ] {
        let output =
            Command::new(env!("CARGO_BIN_EXE_mars")).arg("compile").args(args).arg("missing.red").output().unwrap();
        assert_eq!(output.status.code(), Some(2));
    }
}

#[test]
fn historical_sources_respect_configured_length() {
    for settings in [
        Settings { program_len: 1, ..Settings::hu93() },
        Settings { program_len: 1, hu93_syntax: true, ..Settings::pmars() },
    ] {
        let compiled =
            mars::assembler::assemble(b"START JMP 0\nDAT 0\n", &settings, AssemblyContext::default()).unwrap();
        assert!(!compiled.is_ok());
        assert_eq!(compiled.messages.last().unwrap().kind, mars::MessageKind::TooLong);
    }
}

#[test]
fn run_compiles_only_first_warrior_with_preset_registers() {
    let source = mars::report::Source { name: "register.red".into(), bytes: Some(b"JMP w-2,0\n".to_vec()) };
    let settings = Settings { quirks: true, max_steps: 10, ..Settings::pmars() };
    let run = mars::report::run(&[source.clone(), source], &settings, Rng::from_ticks(0), 2).unwrap();
    assert!(run.played);
    assert_eq!(run.stats[0].wins, 2);
    assert_eq!(run.stats[1].losses, 2);
}

#[test]
fn tournament_assembles_roles_and_round_counts() {
    use mars::tournament::{compute, Backend, Entry, Format, Options};
    let source = b";assert ROUNDS==3 || ROUNDS==2\nJMP w-2,0\n";
    let settings = Settings { quirks: true, rotate: false, max_steps: 10, ..Settings::pmars() };
    let context = AssemblyContext { warriors: 2, rounds: 3, first: true };
    let program = compile_with_context(source, &settings, context).program;
    let entries: Vec<_> = ["a", "b"]
        .map(|name| Entry {
            name: name.into(),
            file: name.into(),
            program: program.clone(),
            source: Some(source.to_vec()),
        })
        .into();
    let options = Options {
        settings,
        games: 5,
        backend: Backend::Cpu { jobs: 1 },
        seed: 0,
        format: Format::Jsonl,
        out: "unused".into(),
        progress: false,
        against: 0,
    };
    let summary = compute(&entries, &options).unwrap();
    assert_eq!(summary.runs[0].stats[0].wins, 3);
    assert_eq!(summary.runs[1].stats[0].wins, 2);
    assert_eq!(summary.runs[0].stats[1].losses, 3);
    assert_eq!(summary.runs[1].stats[1].losses, 2);
}
