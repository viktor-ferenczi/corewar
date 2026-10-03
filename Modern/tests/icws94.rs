use mars::{compile_94, Settings};

#[test]
fn draft_modifiers_modes_and_text_equ() {
    let settings = Settings::icws94();
    let compiled = compile_94(b"two EQU 1+1\nORG start\nDAT #0, #two*2\nstart MOV.X *1, }2\nEND\n", &settings);
    assert!(compiled.is_ok(), "{:?}", compiled.messages);
    assert_eq!(compiled.program.start, 1);
    assert_eq!(compiled.program.code[0].b, 3);
    assert_eq!(compiled.program.code[1].modifier, 5);
    assert_eq!(compiled.program.code[1].modes, 2 | 6 << 3);
}

#[test]
fn draft_and_pmars_conventions() {
    // ICWS'94: SEQ is a synonym, NOP defaults to .B, and an absent B field is #0.
    let draft = compile_94(b"SEQ.I 1,2\nNOP 0\nJMP 0\n", &Settings::icws94());
    assert!(draft.is_ok(), "{:?}", draft.messages);
    assert_eq!(draft.program.code[0].op, 8);
    assert_eq!(draft.program.code[1].modifier, 1);
    assert_eq!(draft.program.code[2].modes >> 3, 0);
    let pmars = compile_94(b"SEQ.I 1,2\nNOP 0\nJMP 0\n", &Settings::pmars());
    assert!(pmars.is_ok(), "{:?}", pmars.messages);
    assert_eq!(pmars.program.code[0].op, 14);
    assert_eq!(pmars.program.code[1].modifier, 4);
    assert_eq!(pmars.program.code[2].modes >> 3, 1);
    let looped = compile_94(b"i FOR 3\nDAT #0, #i\nROF\n", &Settings::pmars());
    assert!(looped.is_ok(), "{:?}", looped.messages);
    assert_eq!(looped.program.code.iter().map(|ins| ins.b).collect::<Vec<_>>(), [1, 2, 3]);
    for source in [b"FOR 2\nDAT #0, #0\nROF\n".as_slice(), b"DAT #q, #0\n", b"PIN 1\nDAT 0,0\n"] {
        assert!(!compile_94(source, &Settings::icws94()).is_ok());
    }
}

#[test]
fn counter_or_equ_named_like_a_modifier() {
    // The modifier glued to an opcode is not substituted; detached, it is (pMARS rejects that too).
    let looped = compile_94(b"i FOR 2\nMOV.I 0, 1\nROF\nab FOR 1\nMOV.ab 0, 1\nROF\n", &Settings::pmars());
    assert!(looped.is_ok(), "{:?}", looped.messages);
    assert_eq!(looped.program.code.iter().map(|ins| ins.modifier).collect::<Vec<_>>(), [6, 6, 2]);
    assert!(!compile_94(b"ab FOR 1\nMOV . ab 0, 1\nROF\n", &Settings::pmars()).is_ok());
    for settings in [Settings::pmars(), Settings::icws94()] {
        let equ = compile_94(b"x EQU 3*(2+1)\nb EQU 4\nMUL.X #x, #b\nADD.b #b, 1\n", &settings);
        assert!(equ.is_ok(), "{:?}", equ.messages);
        assert_eq!(equ.program.code.iter().map(|ins| (ins.modifier, ins.a)).collect::<Vec<_>>(), [(5, 9), (1, 4)]);
    }
}
