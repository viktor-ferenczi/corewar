//! MARS output, DOS session memory and the tournament runner.

use std::path::{Path, PathBuf};

use mars::report::{self, average, Session, Source};
use mars::tournament::{self, Entry, Options};
use mars::{Rng, Settings};

fn source(name: &str, text: &str) -> Source {
    Source { name: name.into(), bytes: Some(text.replace('\n', "\r\n").into_bytes()) }
}

fn historical(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Historical").join(name)
}

#[test]
fn average_rounds_half_down() {
    assert_eq!(average(5, 2), 2);
    assert_eq!(average(7, 4), 2);
    assert_eq!(average(6, 4), 1);
    assert_eq!(average(128, 1), 128);
    // Twice the remainder is taken in 16 bits.
    assert_eq!(average(40000 + 39999, 40000), 1);
}

#[test]
fn statistics_text() {
    let sources = [source("D:\\IMP.CWR", "START MOV 0 1\n"), source("D:\\DIE.CWR", "START DAT 0\n")];
    let run = report::run(&sources, &Settings::default(), Rng::from_ticks(1), 3).unwrap();
    assert!(run.played);
    assert_eq!(
        run.text,
        "CoreWar MARS V1.0 by GM 1993\n\nD:\\IMP.CWR\n\nD:\\DIE.CWR\n
CoreWar MARS V1.0 Statistics:

Number of full wars         = 3
Maximal war length in steps = 600000
Queue length (Max. PCs)     = 64
Execute each other was enabled.

ProgNum   Average PC  Win     Lose    Progam name
1         1           3       0       D:\\IMP.CWR
2         0           0       3       D:\\DIE.CWR
"
    );
    let words: Vec<u16> = run.log.chunks(2).map(|w| u16::from_le_bytes([w[0], w[1]])).collect();
    assert_eq!(words, [0x100, 3, 0x27C0, 9, 64, 1, 1, 3, 0, 1, 3, 0, 2, 0, 0, 0, 0, 3]);
}

#[test]
fn errors_stop_mars_before_the_war() {
    let sources = [source("A.CWR", " MOV X 1\n"), Source { name: "B.CWR".into(), bytes: None }];
    let run = report::run(&sources, &Settings::default(), Rng::from_ticks(1), 3).unwrap();
    assert!(!run.played);
    assert_eq!(
        run.text,
        "CoreWar MARS V1.0 by GM 1993\n\nA.CWR\nUndefined symbol at line 1 in program A.CWR !\nCan't open B.CWR !\n\
         Cannot execute war, while there are any errors !\n"
    );
    assert!(run.log.is_empty());
}

#[test]
fn a_fatal_source_returns_the_text_so_far() {
    let (fatal, text) =
        report::run(&[source("H.CWR", "START MOV 0 1\nX ")], &Settings::default(), Rng::from_ticks(1), 1).unwrap_err();
    assert_eq!(fatal, mars::Fatal::Hang { line: 2 });
    assert_eq!(text, "CoreWar MARS V1.0 by GM 1993\n\nH.CWR\n");
}

#[test]
fn a_session_carries_the_memory_after_the_arena_to_the_next_run() {
    let long = " DAT 0\n".repeat(99) + "START JMP START\n";
    let sources = [source("L.CWR", &long), source("M.CWR", &long)];
    let settings = Settings { max_steps: 2, ..Settings::default() };
    let mut session = Session::new();
    let first = report::run_in(&mut session, &sources, &settings, Rng::from_ticks(7), 2000).unwrap();
    assert_eq!(first, report::run(&sources, &settings, Rng::from_ticks(7), 2000).unwrap());
    // After 2 structures of 232 cells and the arena, some of 2000 wars surely spilled over.
    let spilled = &session.cells()[2 * 232 + 8000..];
    assert!(spilled.iter().any(|c| c.owner != 0));
    // With one program more the arena starts 232 cells higher.
    let three = [source("L.CWR", &long), source("M.CWR", &long), source("N.CWR", &long)];
    report::run_in(&mut session, &three, &settings, Rng::from_ticks(7), 1).unwrap();
    assert_eq!(session.cells().len(), 3 * 232 + 8099);
}

fn tournament(out: &Path, seed: u64) -> String {
    let entries: Vec<Entry> =
        ["IMP.CWR", "MICE.CWR", "TORPE.CWR"].iter().map(|n| Entry::load(&historical(n)).unwrap()).collect();
    let options = Options {
        wars_per_order: 10,
        settings: Settings { max_steps: 20000, ..Settings::default() },
        jobs: 2,
        seed,
        out: out.to_path_buf(),
    };
    tournament::play(&entries, &options).unwrap();
    std::fs::read_to_string(out.join("results/runs.csv")).unwrap()
}

#[test]
fn tournament_plays_every_pair_in_both_orders_repeatably() {
    let dir = std::env::temp_dir().join(format!("mars-tournament-{}", std::process::id()));
    let csv = tournament(&dir.join("a"), 5);
    let rows: Vec<Vec<&str>> = csv.lines().map(|l| l.split(',').collect()).collect();
    assert_eq!(
        rows[0].join(","),
        "first,second,games,max_steps,first_wins,second_wins,draws,first_avg_pcs,second_avg_pcs,seconds,seed"
    );
    let pairs: Vec<(&str, &str)> = rows[1..].iter().map(|r| (r[0], r[1])).collect();
    assert_eq!(
        pairs,
        [("IMP", "MICE"), ("MICE", "IMP"), ("IMP", "TORPE"), ("TORPE", "IMP"), ("MICE", "TORPE"), ("TORPE", "MICE")]
    );
    for r in &rows[1..] {
        let n: Vec<u32> = [2, 4, 5, 6].iter().map(|&i| r[i].parse().unwrap()).collect();
        assert_eq!(n[0], 10);
        assert_eq!(n[1] + n[2] + n[3], 10);
    }
    assert!(dir.join("a/raw/MICE_vs_IMP.sta").exists());
    let without_time = |csv: &str| {
        csv.lines()
            .map(|l| l.split(',').enumerate().filter(|(i, _)| *i != 9).map(|(_, f)| f).collect::<Vec<_>>().join(","))
            .collect::<Vec<_>>()
    };
    assert_eq!(without_time(&csv), without_time(&tournament(&dir.join("b"), 5)));
    std::fs::remove_dir_all(dir).unwrap();
}
