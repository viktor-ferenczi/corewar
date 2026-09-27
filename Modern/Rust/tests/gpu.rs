//! The GPU shader against the CPU engine, war by war, on every adapter found (llvmpipe included).
//! Without any adapter there is nothing to compare and the tests pass.
#![cfg(feature = "gpu")]

use std::path::Path;

use mars::report::{self, Session};
use mars::tournament::{self, Progress, War, WarResult};
use mars::{gpu, Engine, Program, Rng, Settings};

/// The historical programs and the test programs written for edge cases.
fn programs() -> Vec<Program> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut paths: Vec<_> =
        std::fs::read_dir(root.join("tests/golden/progs")).unwrap().map(|e| e.unwrap().path()).collect();
    for name in
        ["IMP.CWR", "MICE.CWR", "CHANG.CWR", "TORPE.CWR", "KILLER.CWR", "KILLER2.CWR", "PRB004.CWR", "Y.CWR", "ARTUR-2"]
    {
        paths.push(root.join("../../Historical").join(name));
    }
    paths.sort();
    paths.iter().map(|p| report::compile_clean(&p.display().to_string(), &std::fs::read(p).unwrap()).unwrap()).collect()
}

/// Wars of every pair of programs, placed like MARS would place them.
fn wars(programs: &[Program], per_pair: u16) -> Vec<War> {
    let mut wars = Vec::new();
    for a in 0..programs.len() {
        for b in 0..programs.len() {
            let pair = [programs[a].clone(), programs[b].clone()];
            let seed = (a * 1000 + b) as u32;
            for p in report::place_run(&mut Session::new(), &pair, Rng::from_ticks(seed), per_pair).unwrap() {
                wars.push(War { programs: [a as u16, b as u16], positions: [p[0], p[1]] });
            }
        }
    }
    wars
}

fn cpu(programs: &[Program], wars: &[War], settings: &Settings) -> Vec<WarResult> {
    wars.iter()
        .map(|war| {
            let pair = war.programs.map(|p| programs[p as usize].clone());
            tournament::fight_one(&mut Engine::new(settings.clone(), &pair, Rng::from_ticks(0)), war)
        })
        .collect()
}

fn compare(settings: Settings, per_pair: u16) {
    let programs = programs();
    let wars = wars(&programs, per_pair);
    let expected = cpu(&programs, &wars, &settings);
    let adapters = gpu::adapters();
    for (d, info) in adapters.iter().enumerate() {
        let progress = Progress::new(wars.len() as u64, false);
        let results = gpu::fight(&programs, &wars, &settings, &[d], &progress).unwrap();
        let differ: Vec<usize> = (0..wars.len()).filter(|&i| results[i] != expected[i]).collect();
        assert!(
            differ.is_empty(),
            "{}: {} of {} wars differ, first {:?}: GPU {:?}, CPU {:?}",
            info.name,
            differ.len(),
            wars.len(),
            wars[differ[0]],
            results[differ[0]],
            expected[differ[0]]
        );
    }
}

#[test]
fn default_settings() {
    compare(Settings { max_steps: 30000, ..Settings::default() }, 2);
}

#[test]
fn queue_lengths() {
    for queue_len in [1, 2, 8, 256] {
        compare(Settings { queue_len, max_steps: 20000, ..Settings::default() }, 1);
    }
}

#[test]
fn no_exec_other() {
    compare(Settings { exec_other: false, max_steps: 20000, ..Settings::default() }, 1);
}

#[test]
fn step_limits_and_the_dat_test() {
    for max_steps in [1, 2, 999, 1000, 16001] {
        compare(Settings { max_steps, ..Settings::default() }, 1);
    }
    compare(Settings { max_steps: 40000, dat_test: false, ..Settings::default() }, 1);
}

#[test]
fn full_wars() {
    // A few whole wars of 600000 steps, imps included, where the DAT test ends wars early.
    compare(Settings::default(), 1);
}

#[test]
fn several_gpus_give_the_same_results() {
    let adapters = gpu::adapters();
    if adapters.len() < 2 {
        return;
    }
    let programs = programs();
    let wars = wars(&programs, 3);
    let settings = Settings { max_steps: 10000, ..Settings::default() };
    let devices: Vec<usize> = (0..adapters.len()).collect();
    let progress = Progress::new(wars.len() as u64, false);
    assert_eq!(gpu::fight(&programs, &wars, &settings, &devices, &progress).unwrap(), cpu(&programs, &wars, &settings));
}
