use mars::{compile_hu93_clean, MessageKind};

#[test]
fn clean_expressions_and_long_labels() {
    let compiled = compile_hu93_clean(
        b"LongLabelBeyondSixteen DAT 1+2*3\nSTART DAT -7/2\n DAT LongLabelBeyondSixteen\n DAT 40000\n",
        8000,
    );
    assert!(compiled.is_ok(), "{:?}", compiled.messages);
    assert_eq!(compiled.program.start, 1);
    assert_eq!(compiled.program.code.iter().map(|i| i.b).collect::<Vec<_>>(), [7, 7997, 7998, 0]);
}

#[test]
fn comments_and_nulls_do_not_change_the_next_line() {
    let compiled = compile_hu93_clean(b"LABEL ; JMP 0\nSTART DAT LABEL\0\n DAT 3\n", 8000);
    assert!(compiled.is_ok(), "{:?}", compiled.messages);
    assert_eq!(compiled.program.code.len(), 2);
    assert_eq!(compiled.program.code[0].b, 0);
    assert_eq!(compiled.program.code[1].b, 3);
}

#[test]
fn errors_are_bounded_and_keep_the_first_label() {
    let compiled = compile_hu93_clean(b"X DAT 1\nX DAT 2\n DAT Y\n", 8000);
    assert_eq!(compiled.messages[0].kind, MessageKind::Duplicated);
    assert_eq!(compiled.messages[1].kind, MessageKind::Undefined);
    assert_eq!(compiled.program.code.len(), 3);
}
