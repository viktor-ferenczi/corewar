//! The GPU shader against the CPU engine, war by war, on discrete and software adapters.
//! Without any adapter there is nothing to compare and the tests pass.
#![cfg(feature = "gpu")]

use std::path::Path;

use mars::report::{self, Session};
use mars::tournament::{self, Progress, War, WarResult};
use mars::{compile_88, compile_94};
use mars::{gpu, Engine, Program, Rng, Settings};

fn test_adapters() -> Vec<usize> {
    let adapters = gpu::adapters();
    if let Ok(pinned) = std::env::var("MARS_TEST_GPUS") {
        return pinned
            .split(',')
            .map(|index| index.parse::<usize>().expect("MARS_TEST_GPUS must contain adapter indexes"))
            .inspect(|&index| assert!(index < adapters.len(), "no GPU adapter {index}"))
            .collect();
    }
    adapters
        .iter()
        .enumerate()
        .filter(|(_, adapter)| adapter.device_type != wgpu::DeviceType::IntegratedGpu)
        .map(|(index, _)| index)
        .collect()
}

/// The historical programs and the test programs written for edge cases.
fn programs() -> Vec<Program> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut paths: Vec<_> =
        std::fs::read_dir(root.join("tests/golden/progs")).unwrap().map(|e| e.unwrap().path()).collect();
    for name in
        ["IMP.CWR", "MICE.CWR", "CHANG.CWR", "TORPE.CWR", "KILLER.CWR", "KILLER2.CWR", "PRB004.CWR", "Y.CWR", "ARTUR-2"]
    {
        paths.push(root.join("../Historical").join(name));
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
            for p in report::place_run(&mut Session::new(), &pair, &Settings::hu93(), Rng::from_ticks(seed), per_pair)
                .unwrap()
            {
                wars.push(War { programs: [a as u16, b as u16], positions: [p[0], p[1]], first_mover: 0 });
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
    for d in test_adapters() {
        let info = &adapters[d];
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
    compare(Settings { max_steps: 30000, ..Settings::hu93() }, 2);
}

#[test]
fn queue_lengths() {
    for queue_len in [1, 2, 8, 256] {
        compare(Settings { queue_len, max_steps: 20000, ..Settings::hu93() }, 1);
    }
}

#[test]
fn no_exec_other() {
    compare(Settings { exec_other: false, max_steps: 20000, ..Settings::hu93() }, 1);
}

#[test]
fn step_limits_and_the_dat_test() {
    for max_steps in [1, 2, 999, 1000, 16001] {
        compare(Settings { max_steps, ..Settings::hu93() }, 1);
    }
    compare(Settings { max_steps: 40000, dat_test: false, ..Settings::hu93() }, 1);
}

#[test]
fn full_wars() {
    // A few whole wars of 600000 steps, imps included, where the DAT test ends wars early.
    compare(Settings::hu93(), 1);
}

#[test]
fn rotating_first_mover_matches_cpu() {
    let program = report::compile_clean("DAT.CWR", b"START DAT 0\n").unwrap();
    let programs = [program.clone(), program];
    let wars = [
        War { programs: [0, 1], positions: [0, 100], first_mover: 0 },
        War { programs: [0, 1], positions: [0, 100], first_mover: 1 },
    ];
    let settings = Settings { rotate: true, ..Settings::hu93() };
    let expected = cpu(&programs, &wars, &settings);
    assert_ne!(expected[0], expected[1]);
    for adapter in test_adapters() {
        let progress = Progress::new(wars.len() as u64, false);
        assert_eq!(gpu::fight(&programs, &wars, &settings, &[adapter], &progress).unwrap(), expected);
    }
}

#[test]
fn smaller_core_matches_cpu() {
    let program = report::compile_clean("JMP.CWR", b"START JMP 0\n").unwrap();
    let programs = [program.clone(), program];
    let wars = [War { programs: [0, 1], positions: [0, 100], first_mover: 0 }];
    let settings = Settings { core_size: 1024, max_steps: 100, ..Settings::hu93() };
    let expected = cpu(&programs, &wars, &settings);
    for adapter in test_adapters() {
        let progress = Progress::new(wars.len() as u64, false);
        assert_eq!(gpu::fight(&programs, &wars, &settings, &[adapter], &progress).unwrap(), expected);
    }
}

#[test]
fn clean_hu93_wrapping_matches_cpu() {
    let settings = Settings { quirks: false, core_size: 100, max_steps: 200, ..Settings::hu93() };
    let programs = [
        report::compile_clean_with_settings("A.CWR", b"START MOV 0 1\n DAT 0\n", &settings).unwrap(),
        report::compile_clean_with_settings("B.CWR", b"START JMP 0\n", &settings).unwrap(),
    ];
    let wars = [War { programs: [0, 1], positions: [99, 50], first_mover: 0 }];
    let expected = cpu(&programs, &wars, &settings);
    for adapter in test_adapters() {
        let progress = Progress::new(wars.len() as u64, false);
        assert_eq!(gpu::fight(&programs, &wars, &settings, &[adapter], &progress).unwrap(), expected);
    }
}

#[test]
fn clean_hu93_cmp_matches_cpu() {
    let settings = Settings { quirks: false, max_steps: 10, ..Settings::hu93() };
    let programs = [
        report::compile_clean_with_settings("A.CWR", b"START CMP 3 4\n DAT 0\n JMP 0\n DAT 1 5\n DAT 2 5\n", &settings)
            .unwrap(),
        report::compile_clean_with_settings("B.CWR", b"START JMP 0\n", &settings).unwrap(),
    ];
    let wars = [War { programs: [0, 1], positions: [0, 100], first_mover: 0 }];
    let expected = cpu(&programs, &wars, &settings);
    assert_eq!(expected[0].pcs[0], 0);
    for adapter in test_adapters() {
        let progress = Progress::new(wars.len() as u64, false);
        assert_eq!(gpu::fight(&programs, &wars, &settings, &[adapter], &progress).unwrap(), expected);
    }
}

#[test]
fn icws88_fifo_and_arithmetic_match_cpu() {
    let settings = Settings { max_steps: 200, ..Settings::icws88() };
    let source = b"START SPL child\nADD 3, 2\nJMP 0\nchild MOV #1, 2\nDAT #0, #0\nDAT #1, #1\n";
    let first = compile_88(source, &settings);
    assert!(first.is_ok(), "{:?}", first.messages);
    let second = compile_88(b"START JMP 0\n", &settings);
    let programs = [first.program, second.program];
    let wars = [
        War { programs: [0, 1], positions: [0, 100], first_mover: 0 },
        War { programs: [0, 1], positions: [0, 100], first_mover: 1 },
    ];
    let expected = cpu(&programs, &wars, &settings);
    for adapter in test_adapters() {
        let progress = Progress::new(wars.len() as u64, false);
        assert_eq!(gpu::fight(&programs, &wars, &settings, &[adapter], &progress).unwrap(), expected);
    }
}

#[test]
fn icws94_modes_and_arithmetic_match_cpu() {
    let settings = Settings { max_steps: 200, ..Settings::icws94() };
    let first = compile_94(
        b"start SPL child\nMOV.I 0, >2\nDIV.F #2, 1\nchild MUL.X #3, 2\nSNE.I 1, 2\nJMP 0\nDAT 0, 0\n",
        &settings,
    );
    let second = compile_94(b"start JMP 0\n", &settings);
    assert!(first.is_ok(), "{:?}", first.messages);
    assert!(second.is_ok(), "{:?}", second.messages);
    let programs = [first.program, second.program];
    let wars = [
        War { programs: [0, 1], positions: [0, 4000], first_mover: 0 },
        War { programs: [0, 1], positions: [0, 4000], first_mover: 1 },
    ];
    let expected = cpu(&programs, &wars, &settings);
    for adapter in test_adapters() {
        let progress = Progress::new(wars.len() as u64, false);
        assert_eq!(gpu::fight(&programs, &wars, &settings, &[adapter], &progress).unwrap(), expected);
    }
}

#[test]
fn generated_standard_cells_match_cpu() {
    use mars::{Instruction, Standard};
    for (standard, quirks) in [
        (Standard::Icws88, false),
        (Standard::Icws88, true),
        (Standard::Icws94, false),
        (Standard::Pmars, false),
        (Standard::Pmars, true),
    ] {
        let settings = Settings {
            quirks,
            core_size: 256,
            program_len: 120,
            queue_len: 16,
            max_steps: 100,
            min_distance: 0,
            ..Settings::for_standard(standard)
        };
        let mut random = 0x1994u32;
        let mut next = || {
            random = random.wrapping_mul(1664525).wrapping_add(1013904223);
            random >> 16
        };
        let mut programs = vec![Program {
            start: 0,
            code: vec![Instruction { op: 4, modifier: 1, modes: 9, wide_modes: true, a: 0, b: 0 }],
        }];
        for _ in 0..48 {
            let mut code = vec![Instruction { op: 9, modifier: 1, modes: 9, wide_modes: true, a: 1, b: 0 }; 2];
            for _ in 0..16 {
                code.push(Instruction {
                    op: (next() % 17) as u8,
                    modifier: (next() % 7) as u8,
                    modes: (next() % 64) as u8,
                    wide_modes: true,
                    a: (next() as u16).wrapping_sub(8) % 256,
                    b: (next() as u16).wrapping_sub(8) % 256,
                });
            }
            programs.push(Program { start: 0, code });
        }
        for source in [
            "ADD.B >0,#0\nJMZ.B 2,-1\nJMP 0\nDAT 0,0\n",
            "JMN.F 2,3\nDAT 0,0\nJMP 0\nDAT 0,1\n",
            "DJN.B 2,>0\nDAT 0,0\nJMP 0\n",
        ] {
            programs.push(
                compile_94(source.as_bytes(), &Settings { standard: Standard::Pmars, ..settings.clone() }).program,
            );
        }
        let wars: Vec<_> = (1..programs.len())
            .flat_map(|index| {
                [0, 1].map(move |first_mover| War { programs: [index as u16, 0], positions: [250, 100], first_mover })
            })
            .collect();
        let expected = cpu(&programs, &wars, &settings);
        for adapter in test_adapters() {
            let progress = Progress::new(wars.len() as u64, false);
            assert_eq!(
                gpu::fight(&programs, &wars, &settings, &[adapter], &progress).unwrap(),
                expected,
                "{standard:?}, quirks={quirks}"
            );
        }
    }
}

#[test]
fn large_core_program_and_cycle_limit_match_cpu() {
    let settings =
        Settings { core_size: 65535, program_len: 120, max_steps: 1 << 31, queue_len: 8, ..Settings::pmars() };
    let first = compile_94(format!("{}DAT 0,0\n", "NOP 0\n".repeat(119)).as_bytes(), &settings);
    let second = compile_94(b"JMP 0\n", &settings);
    assert!(first.is_ok());
    let programs = [first.program, second.program];
    let wars = [War { programs: [0, 1], positions: [65530, 1000], first_mover: 0 }];
    let expected = cpu(&programs, &wars, &settings);
    assert_eq!(expected[0].steps, 239);
    for adapter in test_adapters() {
        assert_eq!(gpu::fight(&programs, &wars, &settings, &[adapter], &Progress::new(1, false)).unwrap(), expected);
    }
}

#[test]
fn packed_fifo_queue_caps_match_cpu() {
    for queue_len in [1, 3, 17, 8000] {
        let settings = Settings { queue_len, max_steps: 100, ..Settings::pmars() };
        let programs = [compile_94(b"SPL 0\nJMP -1\n", &settings).program, compile_94(b"JMP 0\n", &settings).program];
        let wars = [War { programs: [0, 1], positions: [0, 4000], first_mover: 0 }];
        for adapter in test_adapters() {
            assert_eq!(
                gpu::fight(&programs, &wars, &settings, &[adapter], &Progress::new(1, false)).unwrap(),
                cpu(&programs, &wars, &settings),
                "queue {queue_len}"
            );
        }
    }
}

#[test]
fn several_gpus_give_the_same_results() {
    let devices = test_adapters();
    if devices.len() < 2 {
        return;
    }
    let programs = programs();
    let wars = wars(&programs, 3);
    let settings = Settings { max_steps: 10000, ..Settings::hu93() };
    let progress = Progress::new(wars.len() as u64, false);
    assert_eq!(gpu::fight(&programs, &wars, &settings, &devices, &progress).unwrap(), cpu(&programs, &wars, &settings));
}
