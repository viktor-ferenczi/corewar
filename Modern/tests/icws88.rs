use mars::{compile_88, Engine, MessageKind, Program, Rng, Settings};

fn program(source: &str, settings: &Settings) -> Program {
    let compiled = compile_88(source.as_bytes(), settings);
    assert!(compiled.is_ok(), "{:?}", compiled.messages);
    compiled.program
}

#[test]
fn operands_equ_and_end() {
    let settings = Settings::icws88();
    let compiled = compile_88(b"two EQU 1+1\nDAT #0, #two*2\nstart JMP 0\nEND start\n", &settings);
    assert!(compiled.is_ok(), "{:?}", compiled.messages);
    assert_eq!(compiled.program.start, 1);
    assert_eq!(compiled.program.code[0].b, 4);
    assert_eq!(program("MOV 0 -1\n", &settings).code[0].b, 7999);
    assert_eq!(program("ADD # 4 bomb\nbomb DAT #0 #0\n", &settings).code[0].a, 4);
}

#[test]
fn illegal_modes_and_modifiers_are_rejected() {
    let settings = Settings::icws88();
    for source in ["SLT 1, #2", "CMP 1, #2", "DAT 5", "JMP #1, 0", "MOV.I 0, 1"] {
        assert!(!compile_88(source.as_bytes(), &settings).is_ok(), "{source}");
    }
    let quirks = Settings { quirks: true, ..settings };
    assert!(compile_88(b"SLT 1, #2", &quirks).is_ok());
    assert_eq!(compile_88(b"Loop JMP loop", &quirks).messages[0].kind, MessageKind::Undefined);
}

#[test]
fn arithmetic_changes_both_fields_and_spl_uses_fifo() {
    let settings = Settings::icws88();
    let programs = [program("ADD 2, 1\nDAT #1, #1\nDAT #2, #3\n", &settings)];
    let mut engine = Engine::new(settings.clone(), &programs, Rng::from_ticks(0));
    engine.place(Some(&[0])).unwrap();
    engine.step(0);
    assert_eq!((engine.mem[1].a, engine.mem[1].b), (3, 4));

    let programs = [program("SPL 2\nJMP 0\nJMP 0\n", &settings)];
    let mut engine = Engine::new(settings, &programs, Rng::from_ticks(0));
    engine.place(Some(&[0])).unwrap();
    engine.step(0);
    assert_eq!(engine.warriors[0].fifo.iter().copied().collect::<Vec<_>>(), [1, 2]);
}

#[test]
fn empty_core_placement_and_cycle_limit() {
    let settings = Settings { max_steps: 9, ..Settings::icws88() };
    let programs = [program("JMP 0\n", &settings), program("JMP 0\n", &settings)];
    let mut engine = Engine::new(settings, &programs, Rng::from_ticks(7));
    let positions = engine.place(None).unwrap();
    assert_eq!(positions[0], 0);
    assert!((100..=7900).contains(&positions[1]));
    assert_eq!((engine.mem[500].op, engine.mem[500].modifier, engine.mem[500].modes), (0, 4, 9));
    assert_eq!(engine.fight().steps, 18);
}
