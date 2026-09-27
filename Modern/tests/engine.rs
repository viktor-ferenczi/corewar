//! Simulator behavior on hand placed programs. `golden.rs` checks random battles against MARS.COM.

use mars::engine::Cell;
use mars::{compile, Engine, Outcome, Program, Rng, Settings};

fn program(source: &str) -> Program {
    let compiled = compile(source.replace('\n', "\r\n").as_bytes()).unwrap();
    assert!(compiled.is_ok(), "{:?}", compiled.messages);
    compiled.program
}

fn engine(settings: Settings, sources: &[&str], positions: &[u16]) -> Engine {
    let programs: Vec<Program> = sources.iter().map(|s| program(s)).collect();
    let mut engine = Engine::new(settings, &programs, Rng::from_ticks(0));
    engine.place(Some(positions)).unwrap();
    engine
}

fn text(engine: &Engine, addr: usize) -> String {
    engine.mem[addr].instruction().to_string()
}

const IDLE: &str = "START JMP START\n";

#[test]
fn imp_copies_itself_forward() {
    let mut e = engine(Settings::default(), &["START MOV 0 1\n"], &[100]);
    e.step(0);
    assert_eq!(text(&e, 101), "MOV $0 $1");
    assert_eq!(e.mem[101].owner, 1);
    assert_eq!(e.warriors[0].pcs[0], 101);
    e.step(0);
    assert_eq!(text(&e, 102), "MOV $0 $1");
}

#[test]
fn addresses_wrap_around_the_arena() {
    let mut e = engine(Settings::default(), &["START MOV 0 1\n"], &[7999]);
    e.step(0);
    assert_eq!(text(&e, 0), "MOV $0 $1");
    assert_eq!(e.warriors[0].pcs[0], 0);
}

#[test]
fn dat_ends_the_process_and_the_program() {
    let mut e = engine(Settings::default(), &["START DAT 0\n", IDLE], &[0, 1000]);
    e.step(0);
    assert_eq!(e.warriors[0].pcnum, 0);
    assert_eq!(e.alive(), 1);
}

#[test]
fn operands_are_evaluated_even_for_dat() {
    let mut e = engine(Settings::default(), &["START DAT <1 <2\n DAT 5\n DAT 7\n", IDLE], &[0, 1000]);
    e.step(0);
    assert_eq!((e.mem[1].b, e.mem[2].b), (4, 6));
}

#[test]
fn mov_immediate_writes_only_the_b_field() {
    let mut e = engine(Settings::default(), &["START MOV #7 1\n SPL 3 4\n"], &[0]);
    e.step(0);
    assert_eq!(text(&e, 1), "SPL $3 #7");
}

#[test]
fn add_and_sub_work_on_b_fields_only() {
    let mut e = engine(Settings::default(), &["START ADD 2 3\n SUB 1 2\n DAT 5 10\n DAT 7 20\n"], &[0]);
    e.step(0);
    assert_eq!(text(&e, 3), "DAT #7 #30");
    e.step(0);
    // B - A: 30 - 10
    assert_eq!(text(&e, 3), "DAT #7 #20");
    let mut e = engine(Settings::default(), &["START SUB 1 2\n DAT 0 10\n DAT 0 3\n"], &[0]);
    e.step(0);
    assert_eq!(text(&e, 2), "DAT #0 #7993");
}

#[test]
fn indirect_goes_through_the_b_field() {
    let mut e = engine(Settings::default(), &["START MOV 3 @1\n DAT 9 2\n DAT 0\n DAT 1 42\n"], &[0]);
    e.step(0);
    assert_eq!(text(&e, 3), "DAT #1 #42");
    let mut e = engine(Settings::default(), &["START MOV @1 2\n DAT 9 2\n DAT 0\n DAT 1 42\n"], &[0]);
    e.step(0);
    assert_eq!(text(&e, 2), "DAT #1 #42");
}

#[test]
fn predecrement_changes_the_pointer_first() {
    let mut e = engine(Settings::default(), &["START MOV 3 <1\n DAT 0 3\n DAT 0\n DAT 1 42\n", IDLE], &[0, 1000]);
    e.step(0);
    assert_eq!(e.mem[1].b, 2);
    assert_eq!(e.mem[1].owner, 1);
    assert_eq!(text(&e, 3), "DAT #1 #42");
    assert_eq!(text(&e, 3), text(&e, 1 + 2));
}

#[test]
fn a_value_is_read_before_the_b_predecrement() {
    // A and B point at the same cell: ADD takes the A value (5), then B decrements it to 4.
    let mut e = engine(Settings::default(), &["START ADD 1 <1\n DAT 0 5\n", IDLE], &[0, 1000]);
    e.step(0);
    // B: cell 1's B becomes 4, the target is 1 + 4 = 5, its B field is 0, 0 + 5 = 5.
    assert_eq!(e.mem[1].b, 4);
    assert_eq!(e.mem[5].b, 5);
}

#[test]
fn jumps() {
    let mut e = engine(Settings::default(), &["START JMP 5\n"], &[10]);
    e.step(0);
    assert_eq!(e.warriors[0].pcs[0], 15);

    let mut e = engine(Settings::default(), &["START JMZ 5 1\n DAT 0\n"], &[10]);
    e.step(0);
    assert_eq!(e.warriors[0].pcs[0], 15);
    let mut e = engine(Settings::default(), &["START JMZ 5 1\n DAT 1\n"], &[10]);
    e.step(0);
    assert_eq!(e.warriors[0].pcs[0], 11);

    let mut e = engine(Settings::default(), &["START JMN 5 1\n DAT 1\n"], &[10]);
    e.step(0);
    assert_eq!(e.warriors[0].pcs[0], 15);

    let mut e = engine(Settings::default(), &["START DJN 5 1\n DAT 2\n"], &[10]);
    e.step(0);
    assert_eq!((e.warriors[0].pcs[0], e.mem[11].b), (15, 1));
    let mut e = engine(Settings::default(), &["START DJN 5 1\n DAT 1\n"], &[10]);
    e.step(0);
    assert_eq!((e.warriors[0].pcs[0], e.mem[11].b), (11, 0));
}

#[test]
fn cmp_compares_values_and_skips() {
    let mut e = engine(Settings::default(), &["START CMP 2 3\n DAT 0\n DAT 1 5\n DAT 2 5\n"], &[0]);
    e.step(0);
    assert_eq!(e.warriors[0].pcs[0], 2);
    let mut e = engine(Settings::default(), &["START CMP #5 3\n DAT 0\n DAT 0\n DAT 2 6\n"], &[0]);
    e.step(0);
    assert_eq!(e.warriors[0].pcs[0], 1);
}

#[test]
fn spl_fills_the_first_free_slot() {
    let mut e = engine(Settings::default(), &["START SPL 2\n JMP 0\n DAT 0\n"], &[0]);
    e.step(0);
    let w = &e.warriors[0];
    assert_eq!((w.pcnum, w.pcs[0], w.pcs[1], w.currpc), (2, 1, 2, 1));
    // Slot 1 runs next and dies, then slot 0 again.
    e.step(0);
    assert_eq!(e.warriors[0].pcnum, 1);
    e.step(0);
    assert_eq!(e.warriors[0].pcs[0], 1);
    let mut e = engine(Settings::default(), &["START SPL 0\n JMP -1\n"], &[0]);
    for _ in 0..10 {
        e.step(0);
    }
    assert!(e.warriors[0].pcs[..e.warriors[0].pcnum as usize].iter().all(|&pc| pc < 2));
}

#[test]
fn spl_stops_at_the_queue_length() {
    let settings = Settings { queue_len: 4, ..Settings::default() };
    let mut e = engine(settings, &["START SPL 0\n JMP -1\n"], &[0]);
    for _ in 0..20 {
        e.step(0);
    }
    assert_eq!(e.warriors[0].pcnum, 4);
    assert!(e.warriors[0].pcs[4..].iter().all(|&pc| pc == 0xFFFF));
}

#[test]
fn processes_run_in_slot_order_from_the_current_slot() {
    // Three processes in slots 0, 1, 2 at 10, 20, 30.
    let mut e = engine(Settings::default(), &["START SPL 20\n SPL 29\n JMP -2\n"], &[10]);
    e.step(0); // slot 0: SPL -> slot 1 = 30
    e.step(0); // slot 1 at 30: DAT, dies
    e.step(0); // slot 0 at 11: SPL -> slot 1 = 40
    e.step(0); // slot 1 at 40: dies
    assert_eq!(e.warriors[0].pcnum, 1);
    assert_eq!(e.warriors[0].pcs[0], 12);
}

#[test]
fn start_past_the_arena_never_runs_and_never_loses() {
    let long: String = (0..99).map(|_| " DAT 0\n").collect::<String>() + "START JMP START\n";
    let mut e = engine(Settings { max_steps: 1000, ..Settings::default() }, &[&long, "START DAT 0\n"], &[7950, 0]);
    // Cells from 8000 on are outside the arena, 8049 is the start.
    assert_eq!(e.warriors[0].pcs[0], 8049);
    assert_eq!(e.mem[8049].instruction().to_string(), "JMP $0 #0");
    let outcome = e.fight();
    // The other program dies at once, this one never ran but still counts as alive.
    assert_eq!(outcome, Outcome { winner: Some(0), steps: 2 });
    assert_eq!(e.warriors[0].pcnum, 1);
}

#[test]
fn cells_after_the_arena_block_placement_in_later_wars() {
    let long: String = (0..99).map(|_| " DAT 0\n").collect::<String>() + "START JMP START\n";
    let programs = [program(&long)];
    let mut e = Engine::new(Settings { max_steps: 1, ..Settings::default() }, &programs, Rng::from_ticks(0));
    e.place(Some(&[7950])).unwrap();
    assert!(e.mem[8000..8050].iter().all(|c| c.owner == 1));
    e.place(Some(&[0])).unwrap();
    // The arena is cleared, the cells after it are not.
    assert!(e.mem[7950..8000].iter().all(|c| c.owner == 0));
    assert!(e.mem[8000..8050].iter().all(|c| c.owner == 1));
}

#[test]
fn exec_other_disabled_kills_on_foreign_cells() {
    let settings = Settings { exec_other: false, ..Settings::default() };
    let mut e = engine(settings.clone(), &["START JMP 5\n", IDLE], &[0, 1000]);
    e.step(0);
    e.step(0);
    assert_eq!(e.warriors[0].pcnum, 0);
    // An imp runs on its own copies.
    let mut e = engine(settings, &["START MOV 0 1\n"], &[0]);
    for _ in 0..100 {
        e.step(0);
    }
    assert_eq!(e.warriors[0].pcnum, 1);
}

#[test]
fn steps_count_both_programs_and_skip_dead_ones() {
    let mut e = engine(Settings { max_steps: 7, ..Settings::default() }, &[IDLE, IDLE], &[0, 100]);
    assert_eq!(e.fight(), Outcome { winner: None, steps: 7 });
    // With three programs the war goes on after one dies, without its turns.
    let settings = Settings { max_steps: 10, ..Settings::default() };
    let mut e = engine(settings, &[IDLE, "START DAT 0\n", IDLE], &[0, 100, 200]);
    assert_eq!(e.fight(), Outcome { winner: None, steps: 10 });
    assert_eq!(e.warriors[0].stats.pcs + e.warriors[2].stats.pcs, 2);
    assert_eq!(e.warriors[1].stats.losses, 1);
}

#[test]
fn statistics_of_a_war() {
    let mut e = engine(Settings::default(), &["START SPL 0\n JMP -1\n", "START DAT 0\n"], &[0, 100]);
    assert_eq!(e.fight(), Outcome { winner: Some(0), steps: 2 });
    assert_eq!((e.warriors[0].stats.wins, e.warriors[0].stats.pcs), (1, 2));
    assert_eq!((e.warriors[1].stats.losses, e.warriors[1].stats.pcs), (1, 0));
    assert_eq!(e.wars, 1);
}

#[test]
fn dat_test_ends_a_war_without_dats() {
    let mut e = engine(Settings::default(), &[IDLE, IDLE], &[0, 100]);
    let imp = Cell { op: 1, modes: 0b0101, a: 0, b: 1, owner: 0 };
    for addr in (0..8000).filter(|&a| a != 0 && a != 100) {
        e.set_cell(addr, imp);
    }
    // Tested before step 16000, which still runs.
    assert_eq!(e.fight(), Outcome { winner: None, steps: 16000 });
}

#[test]
fn dat_test_misses_a_dat_in_the_last_cell() {
    let mut e = engine(Settings::default(), &[IDLE, IDLE], &[0, 100]);
    let imp = Cell { op: 1, modes: 0b0101, a: 0, b: 1, owner: 0 };
    for addr in (0..7999).filter(|&a| a != 0 && a != 100) {
        e.set_cell(addr, imp);
    }
    assert_eq!(e.mem[7999].op, 0);
    assert_eq!(e.fight().steps, 16000);
    // One DAT anywhere else keeps the war going.
    let mut e = engine(Settings::default(), &[IDLE, IDLE], &[0, 100]);
    for addr in (0..8000).filter(|&a| a != 0 && a != 100 && a != 7998) {
        e.set_cell(addr, imp);
    }
    assert_eq!(e.fight().steps, 600_000);
    // Without the test (MARS outside statistics mode) the war lasts to the end.
    let mut e = engine(Settings { dat_test: false, ..Settings::default() }, &[IDLE, IDLE], &[0, 100]);
    for addr in (0..8000).filter(|&a| a != 0 && a != 100) {
        e.set_cell(addr, imp);
    }
    assert_eq!(e.fight().steps, 600_000);
}

#[test]
fn random_placement_does_not_overlap() {
    let programs: Vec<Program> = (0..5).map(|_| program(&" DAT 0\n".repeat(100))).collect();
    let mut e = Engine::new(Settings { max_steps: 1, ..Settings::default() }, &programs, Rng::from_ticks(12345));
    for _ in 0..50 {
        e.place(None).unwrap();
        let mut positions: Vec<u16> = e.warriors.iter().map(|w| w.pcs[0]).collect();
        positions.sort();
        assert!(positions.windows(2).all(|p| p[1] - p[0] >= 100), "{positions:?}");
    }
}
