//! Compiler behavior, quirks of MARS.COM included. `golden.rs` checks the same against MARS.COM.

use mars::{compile, Compiled, Fatal, Instruction, MessageKind};

fn ok(source: &str) -> Compiled {
    let compiled = compile(source.replace('\n', "\r\n").as_bytes()).unwrap();
    assert!(compiled.is_ok(), "{:?}", compiled.messages);
    compiled
}

fn listing(source: &str) -> Vec<String> {
    ok(source).program.code.iter().map(|i| i.to_string()).collect()
}

fn messages(source: &str) -> Vec<(MessageKind, u16)> {
    compile(source.replace('\n', "\r\n").as_bytes()).unwrap().messages.iter().map(|m| (m.kind, m.line)).collect()
}

#[test]
fn imp() {
    let c = ok("START MOV 0 1\n");
    assert_eq!(c.program.code, [Instruction { op: 1, modes: 0b0101, a: 0, b: 1 }]);
    assert_eq!(c.program.start, 0);
}

#[test]
fn default_modes() {
    assert_eq!(
        listing(" MOV 1 2\n JMP 3\n SPL 4\n JMZ 5 6\n CMP 7 8\n"),
        ["MOV $1 $2", "JMP $3 #0", "SPL $4 #0", "JMZ $5 $6", "CMP $7 $8"]
    );
}

#[test]
fn dat_with_one_parameter_stores_it_in_b() {
    assert_eq!(
        listing(" DAT 5\n DAT $5\n DAT <5\n DAT 5 6\n DAT\n DAT ;x\n"),
        ["DAT #0 #5", "DAT #0 $5", "DAT #0 <5", "DAT #5 #6", "DAT #0 #0", "DAT #0 #0"]
    );
}

#[test]
fn fields_are_reduced_modulo_8000() {
    assert_eq!(
        listing(" DAT -1\n DAT 8001\n DAT -8000\n DAT 32767\n"),
        ["DAT #0 #7999", "DAT #0 #1", "DAT #0 #0", "DAT #0 #767"]
    );
}

#[test]
fn expressions_are_evaluated_left_to_right_in_16_bits() {
    assert_eq!(
        listing(" DAT 1+2*3\n DAT 10-3-2\n DAT 300*300\n DAT 20000+20000\n DAT 7%3\n DAT 100/7*7\n"),
        [
            "DAT #0 #9",
            "DAT #0 #5",
            "DAT #0 #464",  // 90000 & 0xFFFF = 24464
            "DAT #0 #6464", // 40000 is -25536 in 16 bits
            "DAT #0 #1",
            "DAT #0 #98"
        ]
    );
}

#[test]
fn division_takes_the_dividend_as_unsigned() {
    // -7 is 65529 for IDIV after XOR DX,DX: 65529 / 2 = 32764.
    assert_eq!(listing(" DAT -7/2\n")[0], "DAT #0 #764");
    assert_eq!(compile(b" DAT -1/1\r\n"), Err(Fatal::DivideOverflow { line: 1 }));
    assert_eq!(messages(" DAT 5/0\n DAT 1\n"), [(MessageKind::DivideByZero, 1)]);
}

#[test]
fn unary_sign_applies_to_the_first_term_only() {
    assert_eq!(listing(" DAT -5+1\n DAT +5\n")[..], ["DAT #0 #7996", "DAT #0 #5"]);
    // A sign after an operator is read as a symbol name.
    assert_eq!(messages(" DAT 3*-2\n"), [(MessageKind::Undefined, 1)]);
}

#[test]
fn numbers_stop_at_32769() {
    assert_eq!(listing(" DAT 32769\n DAT 3277\n DAT 0003277\n")[..], ["DAT #0 #7233", "DAT #0 #3277", "DAT #0 #3277"]);
    assert_eq!(messages(" DAT 32770\n")[0], (MessageKind::ParameterError, 1));
}

#[test]
fn symbols_are_relative_start_is_absolute() {
    let c = ok("A DAT 0\nB DAT A\n DAT B-A\nSTART JMP A\nEND DAT END-START\n");
    let code: Vec<String> = c.program.code.iter().map(|i| i.to_string()).collect();
    // B-A is (1-2) - (0-2) = 1, END-START is (4-4) - (3-4) = 1.
    assert_eq!(code, ["DAT #0 #0", "DAT #0 #7999", "DAT #0 #1", "JMP $7997 #0", "DAT #0 #1"]);
    assert_eq!(c.program.start, 3);
}

#[test]
fn missing_start_means_zero() {
    assert_eq!(ok(" DAT 1\n MOV 0 1\n").program.start, 0);
    assert_eq!(ok(" DAT 1\n MOV 0 1\nSTART\n").program.start, 2);
}

#[test]
fn labels_before_and_on_lines() {
    let c = ok("FIRST\nSECOND THIRD MOV FIRST SECOND\n JMP THIRD\n");
    assert_eq!(c.program.code.iter().map(|i| i.to_string()).collect::<Vec<_>>(), ["MOV $0 $0", "JMP $7999 #0"]);
}

#[test]
fn a_mnemonic_is_exactly_three_characters() {
    let c = ok("MOVE DAT 1\nMO DAT 2\nDATA DAT 3\nSTART JMP MOVE\n JMP MO\n JMP DATA\n");
    assert_eq!(c.program.code.len(), 6);
    assert_eq!(c.program.code[3].a, 8000 - 3);
}

#[test]
fn words_of_a_comment_after_a_label_are_labels() {
    // "; the jmp x" after the label X is read as the labels ";" and "THE", then a JMP.
    let c = ok("X ; the jmp x\nSTART MOV X THE\n");
    let code: Vec<String> = c.program.code.iter().map(|i| i.to_string()).collect();
    assert_eq!(code, ["JMP $0 #0", "MOV $7999 $7999"]);
    assert_eq!(c.program.start, 1);
    // So the same word in two such comments is a duplicate label.
    assert_eq!(messages("X ; a b\nY ; b\n DAT 1\n"), [(MessageKind::Duplicated, 2), (MessageKind::Duplicated, 2)]);
}

#[test]
fn label_of_instruction_59_comments_out_its_line_in_pass_1() {
    let mut source: String = (0..59).map(|i| format!(" DAT {i}\n")).collect();
    source += "L59 DAT 59\nL60 DAT 60\nSTART JMP L59\n";
    let c = ok(&source);
    // AL stays 59 after every label at instruction 59, so pass 1 skips the three lines: L60 and
    // START get 59 too, while pass 2 compiles all of them.
    assert_eq!(c.program.code.len(), 62);
    assert_eq!(c.program.start, 59);
    assert_eq!(c.program.code[61].a, 8000 - 2);
}

#[test]
fn errors_and_their_lines() {
    assert_eq!(
        messages("START MOV NOWHERE 1\n JMP\n MOV 1 #2\n DAT 5;x\nA DAT 1\nA DAT 2\n"),
        [
            (MessageKind::Duplicated, 6),
            (MessageKind::Undefined, 1),
            (MessageKind::MissingParameter, 2),
            (MessageKind::IllegalMode, 3),
            (MessageKind::ParameterError, 4),
        ]
    );
    assert_eq!(messages("; nothing\n"), [(MessageKind::ZeroLength, 2)]);
}

#[test]
fn message_texts() {
    let c = compile(b" MOV X 1\r\n").unwrap();
    assert_eq!(c.messages[0].text("D:\\A.CWR"), "Undefined symbol at line 1 in program D:\\A.CWR !");
}

#[test]
fn cr_lf_and_crlf_line_ends() {
    // MARS.COM reads an LF as a space, the port takes one without a CR before it as a line end.
    for source in [&b"; x\r\n DAT 1\r\n DAT X\r\n"[..], b"; x\n DAT 1\n DAT X\n", b"; x\r DAT 1\r DAT X\r"] {
        let c = compile(source).unwrap();
        assert_eq!((c.program.code.len(), c.messages[0].line), (2, 3), "{source:?}");
    }
    assert_eq!(compile(b"START MOV 0 1\n").unwrap(), compile(b"START MOV 0 1\r\n").unwrap());
}

#[test]
fn line_counting() {
    // Whitespace after a label skips the line end without counting it.
    assert_eq!(messages("LAB   \n\n DAT 1\n DAT X\n"), [(MessageKind::Undefined, 2)]);
}

#[test]
fn at_most_100_instructions() {
    let source: String = (0..100).map(|i| format!(" DAT {i}\n")).collect();
    assert_eq!(ok(&source).program.code.len(), 100);
    // Both passes report it.
    assert_eq!(messages(&(source + " DAT 100\n")), [(MessageKind::TooLong, 101), (MessageKind::TooLong, 101)]);
}

#[test]
fn at_most_100_symbols() {
    let source: String = (0..101).map(|i| format!("L{i} DAT {i}\n")).collect();
    assert_eq!(messages(&source)[0], (MessageKind::SymbolTableFull, 101));
}

#[test]
fn symbols_compare_on_16_characters() {
    assert_eq!(messages("ABCDEFGHIJKLMNOPQ DAT 1\nABCDEFGHIJKLMNOPR DAT 2\n"), [(MessageKind::Duplicated, 2)]);
}

#[test]
fn whitespace_after_a_label_at_the_end_of_the_file_hangs_mars() {
    assert_eq!(compile(b"START MOV 0 1\r\nEND \r\n"), Err(Fatal::Hang { line: 2 }));
    assert!(compile(b"START MOV 0 1\r\nEND\r\n").unwrap().is_ok());
}

#[test]
fn nul_ends_pass_1_and_pass_2_rereads_the_buffer() {
    // Pass 1 stops at the NUL. 15 characters after it are already in the window, what follows
    // stays in the read buffer, and pass 2 reads that before the file again from the start.
    let mut source = b"START MOV 0 1\r\n\x00".to_vec();
    source.extend_from_slice(b"AAAAAAAAAAAAAAA DAT 7\r\n");
    let c = compile(&source).unwrap();
    assert!(c.is_ok());
    assert_eq!(c.program.code.iter().map(|i| i.to_string()).collect::<Vec<_>>(), ["DAT #0 #7", "MOV $0 $1"]);
}

#[test]
fn lower_case_tabs_and_high_characters() {
    let c = compile(b"start\tmov\t0\t1 ; \xe9\r\n\tdat\t#-1\r\n").unwrap();
    assert!(c.is_ok());
    assert_eq!(c.program.code.len(), 2);
}

#[test]
fn files_larger_than_the_read_buffer() {
    let mut source: String = (0..99).map(|i| format!("L{i} DAT {i} ; {}\n", "x".repeat(90))).collect();
    source += "START JMP L3\n";
    assert!(source.len() > 2 * 4096);
    let c = ok(&source);
    assert_eq!(c.program.code.len(), 100);
    assert_eq!(c.program.code[99].a, 8000 - 96);
}
