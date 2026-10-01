//! The tournament: same results as playing the MARS runs one by one, both output formats, limits.

use std::path::{Path, PathBuf};

use mars::report::{self, Session, Source};
use mars::tournament::{self, Backend, Entry, Format, Options, RunResult};
use mars::{Rng, Settings};

fn historical(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../Historical").join(name)
}

fn entries(names: &[&str]) -> Vec<Entry> {
    names.iter().map(|n| Entry::load(&historical(n)).unwrap()).collect()
}

fn options(games: u32, out: PathBuf, format: Format) -> Options {
    Options {
        games,
        settings: Settings { max_steps: 20000, ..Settings::hu93() },
        backend: Backend::Cpu { jobs: 4 },
        seed: 5,
        format,
        out,
        progress: false,
        against: 0,
    }
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mars-tournament-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const PROGRAMS: [&str; 4] = ["IMP.CWR", "MICE.CWR", "TORPE.CWR", "KILLER2.CWR"];

#[test]
fn same_results_as_playing_the_runs_in_order() {
    let entries = entries(&PROGRAMS);
    let options = options(41, temp("same").join("t.jsonl"), Format::Jsonl);
    let summary = tournament::compute(&entries, &options).unwrap();
    assert_eq!(summary.runs.len(), 12);
    // Each pair in one DOS session, first order first, with the seeds of the tournament.
    for pair in summary.runs.chunks(2) {
        let mut session = Session::new();
        for run in pair {
            let sources = [run.first, run.second].map(|i| Source {
                name: entries[i].file.clone(),
                bytes: Some(std::fs::read(historical(&entries[i].file)).unwrap()),
            });
            let played =
                report::run_in(&mut session, &sources, &options.settings, Rng::from_ticks(run.seed), run.wars).unwrap();
            assert_eq!(played.stats, run.stats, "{} against {}", entries[run.first].name, entries[run.second].name);
        }
    }
    // 41 games per pair: 21 with the first program starting, 20 with the other.
    assert_eq!(summary.runs.iter().map(|r| r.wars).collect::<Vec<_>>()[..2], [21, 20]);
    assert_eq!(summary.wars, 6 * 41);
    assert_eq!(summary.steps, summary.runs.iter().map(|r| r.steps).sum::<u64>());
}

#[test]
fn every_pair_in_both_orders_repeatably() {
    let entries = entries(&PROGRAMS[..3]);
    let options = options(10, temp("pairs").join("t.jsonl"), Format::Jsonl);
    let a = tournament::compute(&entries, &options).unwrap();
    let names: Vec<(&str, &str)> =
        a.runs.iter().map(|r| (entries[r.first].name.as_str(), entries[r.second].name.as_str())).collect();
    assert_eq!(
        names,
        [("IMP", "MICE"), ("MICE", "IMP"), ("IMP", "TORPE"), ("TORPE", "IMP"), ("MICE", "TORPE"), ("TORPE", "MICE")]
    );
    assert_eq!(a.runs, tournament::compute(&entries, &options).unwrap().runs);
    let other_seed = Options { seed: 6, ..options };
    assert_ne!(a.runs, tournament::compute(&entries, &other_seed).unwrap().runs);
}

#[test]
fn rotating_tournament_has_one_run_per_pair() {
    let entries = entries(&PROGRAMS[..3]);
    let mut options = options(5, PathBuf::new(), Format::Jsonl);
    options.settings.rotate = true;
    let summary = tournament::compute(&entries, &options).unwrap();
    assert_eq!(summary.runs.len(), 3);
    assert_eq!(summary.wars, 15);
    assert!(summary.runs.iter().all(|run| run.wars == 5));
}

#[test]
fn jsonl_output() {
    let entries = entries(&PROGRAMS[..2]);
    let path = temp("jsonl").join("t.jsonl");
    let summary = tournament::play(&entries, &options(10, path.clone(), Format::Jsonl)).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    let RunResult { seed, stats, steps, .. } = &summary.runs[0];
    assert_eq!(
        lines[0],
        format!(
            "{{\"first\":\"IMP\",\"second\":\"MICE\",\"standard\":\"hu93\",\"quirks\":true,\"rotate\":false,\"seed\":{seed},\"games\":5,\"first_wins\":{},\"second_wins\":{},\
             \"draws\":{},\"first_pcs\":{},\"second_pcs\":{},\"steps\":{steps},\"max_steps\":20000,\"queue\":64,\
             \"exec_other\":true}}",
            stats[0].wins,
            stats[1].wins,
            5 - stats[0].wins - stats[1].wins,
            stats[0].pcs,
            stats[1].pcs
        )
    );
}

#[test]
fn names_are_escaped_in_json() {
    let mut entries = entries(&PROGRAMS[..2]);
    entries[0].name = "A \"B\" \\ C\u{1}".into();
    let summary = tournament::compute(&entries, &options(2, PathBuf::new(), Format::Jsonl)).unwrap();
    let text = tournament::jsonl(&entries, &Settings::hu93(), &summary.runs);
    assert!(text.starts_with(r#"{"first":"A \"B\" \\ C\u0001","second":"MICE""#), "{text}");
}

#[test]
fn sta_output() {
    let entries = entries(&PROGRAMS[..2]);
    let dir = temp("sta");
    let summary = tournament::play(&entries, &options(10, dir.clone(), Format::Sta)).unwrap();
    let csv = std::fs::read_to_string(dir.join("results/runs.csv")).unwrap();
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(lines[0], "first,second,games,max_steps,first_wins,second_wins,draws,first_avg_pcs,second_avg_pcs,seed");
    assert_eq!(lines.len(), 3);
    assert!(lines[2].starts_with("MICE,IMP,5,20000,"));
    let run = &summary.runs[1];
    let sta = std::fs::read_to_string(dir.join("raw/MICE_vs_IMP.sta")).unwrap();
    assert_eq!(
        sta,
        report::run_text(&options(10, dir, Format::Sta).settings, run.wars, &run.stats, &["MICE.CWR", "IMP.CWR"])
    );
}

#[test]
fn limits() {
    let one = entries(&PROGRAMS[..1]);
    let two = entries(&PROGRAMS[..2]);
    let out = PathBuf::new();
    assert!(tournament::compute(&one, &options(10, out.clone(), Format::Jsonl)).unwrap_err().contains("2 to 256"));
    assert!(tournament::compute(&two, &options(1, out.clone(), Format::Jsonl)).unwrap_err().contains("games"));
    assert!(tournament::compute(&two, &options(131071, out.clone(), Format::Jsonl)).unwrap_err().contains("games"));
    let mut same = entries(&PROGRAMS[..2]);
    same[1].name = same[0].name.clone();
    assert!(tournament::compute(&same, &options(10, out.clone(), Format::Jsonl)).unwrap_err().contains("two programs"));
    let many: Vec<Entry> = (0..257)
        .map(|i| {
            let mut e = Entry::load(&historical("IMP.CWR")).unwrap();
            e.name = format!("IMP{i}");
            e
        })
        .collect();
    assert!(tournament::compute(&many, &options(2, out, Format::Jsonl)).unwrap_err().contains("2 to 256"));
}

#[test]
fn a_tournament_of_256_programs() {
    // 32640 pairs of very short wars: the limit works and every pair shows up once per order.
    let entries: Vec<Entry> = (0..256)
        .map(|i| {
            let mut e = Entry::load(&historical(PROGRAMS[i % 4])).unwrap();
            e.name = format!("P{i:03}");
            e
        })
        .collect();
    let options = Options {
        settings: Settings { max_steps: 2, ..Settings::hu93() },
        ..options(2, PathBuf::new(), Format::Jsonl)
    };
    let summary = tournament::compute(&entries, &options).unwrap();
    assert_eq!(summary.runs.len(), 256 * 255);
    assert_eq!(summary.wars, 256 * 255);
    assert!(summary.runs.iter().all(|r| r.wars == 1 && r.first != r.second));
}

/// The runs of the full tournament between a candidate and an opponent.
fn gauntlet_pairs(runs: &[RunResult], candidates: usize) -> Vec<RunResult> {
    runs.iter().filter(|r| (r.first < candidates) != (r.second < candidates)).cloned().collect()
}

#[test]
fn a_gauntlet_equals_its_pairs_of_the_full_tournament() {
    // Two candidates against two opponents, with fixed first movers (two runs per pair).
    let entries = entries(&PROGRAMS);
    let full = options(11, PathBuf::new(), Format::Jsonl);
    let all = tournament::compute(&entries, &full).unwrap();
    let gauntlet = tournament::compute(&entries, &Options { against: 2, ..full }).unwrap();
    assert_eq!(gauntlet.runs.len(), 2 * 2 * 2);
    assert_eq!(gauntlet.runs, gauntlet_pairs(&all.runs, 2));
    assert_eq!(gauntlet.wars, 4 * 11);
}

#[test]
fn a_pmars_gauntlet_equals_its_pairs_of_the_full_tournament() {
    let sources: [(&str, &[u8]); 5] = [
        ("imp", b"MOV 0, 1\n"),
        ("dwarf", b"ADD #4, 3\nMOV 2, @2\nJMP -2\nDAT #0, #0\n"),
        ("first", b"DAT #0, #(ROUNDS)\nstart SPL 0\nMOV -2, <-2\nJMP start\nEND start\n"),
        ("split", b"SPL 0\nMOV.I #0, 1\n"),
        ("clear", b"MOV 2, <-1\nJMP -1\nDAT #0, #-5\n"),
    ];
    let settings = Settings { max_steps: 2000, ..Settings::pmars() };
    let entries: Vec<Entry> = sources
        .iter()
        .map(|(name, source)| Entry {
            name: name.to_string(),
            file: name.to_string(),
            program: mars::assembler::compile_with_context(source, &settings, Default::default()).program,
            source: Some(source.to_vec()),
        })
        .collect();
    let full = Options { settings, ..options(9, PathBuf::new(), Format::Jsonl) };
    let all = tournament::compute(&entries, &full).unwrap();
    assert_eq!(all.runs.len(), 10);
    let gauntlet = tournament::compute(&entries, &Options { against: 2, ..full }).unwrap();
    assert_eq!(gauntlet.runs.len(), 3 * 2);
    assert_eq!(gauntlet.runs, gauntlet_pairs(&all.runs, 3));
}

#[test]
fn gauntlet_limits() {
    let two = entries(&PROGRAMS[..2]);
    let no_candidate = Options { against: 2, ..options(10, PathBuf::new(), Format::Jsonl) };
    assert!(tournament::compute(&two, &no_candidate).unwrap_err().contains("candidate"));
    // More than a full tournament takes: 300 candidates against one opponent.
    let many: Vec<Entry> = (0..301)
        .map(|i| {
            let mut e = Entry::load(&historical(PROGRAMS[i % 4])).unwrap();
            e.name = format!("P{i:03}");
            e
        })
        .collect();
    let options = Options {
        settings: Settings { max_steps: 2, ..Settings::hu93() },
        against: 1,
        ..options(2, PathBuf::new(), Format::Jsonl)
    };
    let summary = tournament::compute(&many, &options).unwrap();
    assert_eq!(summary.runs.len(), 300 * 2);
    assert!(summary.runs.iter().all(|r| r.first == 300 || r.second == 300));
}

#[test]
fn cli_gauntlet_and_compile_json() {
    use std::process::Command;
    let dir = temp("cli");
    let write = |name: &str, text: &str| {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        path
    };
    let imp = write("imp.red", "MOV 0, 1\n");
    let dwarf = write("dwarf.red", "ADD #4, 3\nMOV 2, @2\nJMP -2\nDAT #0, #0\n");
    let clear = write("clear.red", "ORG 1\nDAT #0, #-5\nMOV -1, <-1\nJMP -1\n");
    let broken = write("broken.red", "MOV 0, nowhere\n");
    let mars = || Command::new(env!("CARGO_BIN_EXE_mars"));

    let out = dir.join("g.jsonl");
    let status = mars()
        .args(["tournament", "--cpu", "--games", "4", "--steps", "500", "--out"])
        .arg(&out)
        .args([&imp, &dwarf])
        .arg("--against")
        .arg(&clear)
        .output()
        .unwrap();
    assert!(status.status.success(), "{}", String::from_utf8_lossy(&status.stderr));
    let text = std::fs::read_to_string(&out).unwrap();
    let pairs: Vec<&str> = text.lines().map(|l| &l[..l.find(",\"standard\"").unwrap()]).collect();
    assert_eq!(pairs, [r#"{"first":"imp.red","second":"clear.red""#, r#"{"first":"dwarf.red","second":"clear.red""#]);
    let rejected = mars().args(["tournament", "--out", "x"]).arg(&imp).arg("--against").output().unwrap();
    assert_eq!(rejected.status.code(), Some(2));

    let output = mars().args(["compile", "--json"]).args([&clear, &broken, &dir.join("missing.red")]).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let text = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(
        lines[0],
        format!(
            "{{\"file\":\"{}\",\"ok\":true,\"start\":1,\"instructions\":[\
             {{\"op\":\"DAT\",\"modifier\":\"F\",\"a_mode\":\"#\",\"a\":0,\"b_mode\":\"#\",\"b\":7995}},\
             {{\"op\":\"MOV\",\"modifier\":\"I\",\"a_mode\":\"$\",\"a\":7999,\"b_mode\":\"<\",\"b\":7999}},\
             {{\"op\":\"JMP\",\"modifier\":\"B\",\"a_mode\":\"$\",\"a\":7999,\"b_mode\":\"$\",\"b\":0}}]}}",
            clear.display()
        )
    );
    assert!(lines[1].contains("\"ok\":false,\"errors\":[\"Undefined symbol at line 1"), "{}", lines[1]);
    assert!(lines[2].contains("\"ok\":false,\"errors\":[\""), "{}", lines[2]);
}
