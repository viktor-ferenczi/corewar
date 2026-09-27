//! Wars of two programs on GPUs through wgpu, running `gpu.wgsl`.
//!
//! Every GPU gets its own host thread. The threads take chunks of wars from a shared counter, so a
//! faster GPU plays more of them. A GPU holds a ring of a few chunks: the shader threads take one
//! war after the other, and the host refills a chunk as soon as all its wars are finished. It keeps
//! dispatching until no war is left, each dispatch short enough for the driver's watchdog.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use crate::tournament::{Progress, War, WarResult};
use crate::{Program, Settings, ARENALEN, MAXLEN};

const ARENA_BYTES: u64 = ARENALEN as u64 * 8;
/// `STATE_WORDS` in the shader.
const STATE_BYTES: u64 = 544 * 4;
/// `PROGRAM_WORDS` in the shader: length, start and the cells.
const PROGRAM_WORDS: usize = 2 + 2 * MAXLEN;
const WORKGROUP: u32 = 64;
/// Chunks in the ring of wars on the GPU, see gpu.wgsl.
const SLOTS: usize = 4;
const CONTROL_WORDS: usize = 2 + SLOTS;
/// Aim for dispatches of about this long, far below the watchdog of the drivers.
const DISPATCH_SECONDS: f64 = 0.05;

pub use wgpu::AdapterInfo;

/// One instance for the whole process: the Vulkan loader crashes when threads create instances at
/// the same time.
fn instance() -> &'static Mutex<wgpu::Instance> {
    static INSTANCE: OnceLock<Mutex<wgpu::Instance>> = OnceLock::new();
    INSTANCE.get_or_init(|| {
        Mutex::new(wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        }))
    })
}

fn rank(device_type: wgpu::DeviceType) -> u8 {
    match device_type {
        wgpu::DeviceType::DiscreteGpu => 0,
        wgpu::DeviceType::IntegratedGpu => 1,
        wgpu::DeviceType::VirtualGpu => 2,
        wgpu::DeviceType::Other => 3,
        wgpu::DeviceType::Cpu => 4,
    }
}

/// The adapters in the order `--gpu` numbers them: discrete GPUs first, software ones last,
/// otherwise in the order of the driver.
fn sorted_adapters() -> Vec<wgpu::Adapter> {
    let instance = instance().lock().unwrap();
    let mut adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::PRIMARY));
    adapters.sort_by_key(|a| rank(a.get_info().device_type));
    adapters
}

pub fn adapters() -> Vec<AdapterInfo> {
    sorted_adapters().iter().map(|a| a.get_info()).collect()
}

/// The indexes `--gpu all` means: every GPU of the best kind present, so all discrete GPUs if there
/// is one. A much slower GPU would hold up the end of a tournament with its last wars.
pub fn best_of_kind(adapters: &[AdapterInfo]) -> Vec<usize> {
    (0..adapters.len()).filter(|&i| adapters[i].device_type == adapters[0].device_type).collect()
}

/// Fight the wars on the given GPUs, results in the order of `wars`.
pub fn fight(
    programs: &[Program],
    wars: &[War],
    settings: &Settings,
    devices: &[usize],
    progress: &Progress,
) -> Result<Vec<WarResult>, String> {
    let adapters = sorted_adapters();
    let mut table = vec![0u32; programs.len() * PROGRAM_WORDS];
    for (p, program) in programs.iter().enumerate() {
        let words = &mut table[p * PROGRAM_WORDS..(p + 1) * PROGRAM_WORDS];
        words[0] = program.code.len() as u32;
        words[1] = program.start as u32;
        for (i, ins) in program.code.iter().enumerate() {
            words[2 + 2 * i] = ins.op as u32 | (ins.modes as u32) << 8 | (ins.a as u32) << 16;
            words[3 + 2 * i] = ins.b as u32;
        }
    }
    let next = AtomicUsize::new(0);
    let results = Mutex::new(vec![WarResult::default(); wars.len()]);
    let errors = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for &d in devices {
            let adapter = adapters.get(d).cloned();
            let (table, next, results, errors) = (&table, &next, &results, &errors);
            scope.spawn(move || {
                let outcome = adapter
                    .ok_or_else(|| format!("no GPU {d}"))
                    .and_then(|adapter| Gpu::new(adapter, table, settings, wars.len()))
                    .map(|mut gpu| gpu.run(wars, next, results, progress));
                if let Err(e) = outcome {
                    errors.lock().unwrap().push(e);
                }
            });
        }
    });
    let errors = errors.into_inner().unwrap();
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    Ok(results.into_inner().unwrap())
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    bind_group: wgpu::BindGroup,
    params: wgpu::Buffer,
    wars: wgpu::Buffer,
    results: wgpu::Buffer,
    control: wgpu::Buffer,
    readback: wgpu::Buffer,
    threads: u32,
    chunk: usize,
    settings: Settings,
}

impl Gpu {
    fn new(adapter: wgpu::Adapter, table: &[u32], settings: &Settings, total: usize) -> Result<Self, String> {
        let info = adapter.get_info();
        let limits = adapter.limits();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("mars"),
            required_limits: limits.clone(),
            ..Default::default()
        }))
        .map_err(|e| format!("{}: {e}", info.name))?;
        // Enough threads to keep the GPU busy, but not more: every thread has a 64 KB arena, and
        // with many more the caches stop holding them. An RTX 4090 played as fast with 32768
        // threads as with 65536 and half as fast with 131072; a Radeon AI PRO R9700 played fastest
        // with 16384. Fewer on integrated and software devices, which share the system memory.
        let wanted: u64 = match info.device_type {
            wgpu::DeviceType::Cpu => 1024,
            wgpu::DeviceType::IntegratedGpu => 8192,
            _ => 16384,
        };
        let fits = limits.max_storage_buffer_binding_size.min(limits.max_buffer_size) / ARENA_BYTES;
        let threads = (wanted.min(fits) as u32 / WORKGROUP * WORKGROUP).max(WORKGROUP);
        // The ring holds twice as many wars as there are threads, so there is still work while the
        // oldest chunk waits for its last war.
        let chunk = (threads as usize / 2).min(total.max(1));

        let descriptor = wgpu::ShaderModuleDescriptor {
            label: Some("mars"),
            source: wgpu::ShaderSource::Wgsl(include_str!("gpu.wgsl").into()),
        };
        let module = device.create_shader_module(descriptor);
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("mars"),
            layout: None,
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let buffer = |label, size: u64, usage| {
            device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false })
        };
        use wgpu::BufferUsages as U;
        let params = buffer("params", 32, U::UNIFORM | U::COPY_DST);
        let programs = buffer("programs", table.len() as u64 * 4, U::STORAGE | U::COPY_DST);
        let ring = (chunk * SLOTS) as u64 * 8;
        let wars = buffer("wars", ring, U::STORAGE | U::COPY_DST);
        let results = buffer("results", ring, U::STORAGE | U::COPY_SRC);
        let arena = buffer("arena", threads as u64 * ARENA_BYTES, U::STORAGE);
        let state = buffer("state", threads as u64 * STATE_BYTES, U::STORAGE | U::COPY_DST);
        let control = buffer("control", CONTROL_WORDS as u64 * 4, U::STORAGE | U::COPY_DST | U::COPY_SRC);
        let readback = buffer("readback", (chunk as u64 * 8).max(CONTROL_WORDS as u64 * 4), U::MAP_READ | U::COPY_DST);
        queue.write_buffer(&programs, 0, &words(table));
        // Every thread starts without a war (NONE in its first state word).
        let mut idle = vec![0u32; (STATE_BYTES / 4) as usize * threads as usize];
        idle.chunks_mut((STATE_BYTES / 4) as usize).for_each(|s| s[0] = u32::MAX);
        queue.write_buffer(&state, 0, &words(&idle));
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mars"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[&params, &programs, &wars, &results, &arena, &state, &control]
                .iter()
                .enumerate()
                .map(|(i, b)| wgpu::BindGroupEntry { binding: i as u32, resource: b.as_entire_binding() })
                .collect::<Vec<_>>(),
        });
        Ok(Gpu {
            device,
            queue,
            pipeline,
            bind_group,
            params,
            wars,
            results,
            control,
            readback,
            threads,
            chunk,
            settings: settings.clone(),
        })
    }

    fn run(&mut self, wars: &[War], next: &AtomicUsize, results: &Mutex<Vec<WarResult>>, progress: &Progress) {
        let chunk = self.chunk;
        // Chunks on the GPU: their first war in `wars` and their length. The k-th chunk of this GPU
        // is the k-th of its stream of wars and sits in slot k % SLOTS.
        let mut ring: VecDeque<(usize, usize)> = VecDeque::new();
        let mut taken = 0usize;
        let mut finished_chunks = 0usize;
        let mut done = 0u64;
        let mut reported = 0u64;
        self.queue.write_buffer(&self.control, 0, &words(&[0; CONTROL_WORDS]));
        let mut steps_per_dispatch = 2000u32;
        loop {
            while ring.len() < SLOTS {
                let start = next.fetch_add(chunk, Ordering::Relaxed);
                if start >= wars.len() {
                    break;
                }
                let part = &wars[start..(start + chunk).min(wars.len())];
                let input: Vec<u32> = part
                    .iter()
                    .flat_map(|w| {
                        [
                            w.programs[0] as u32 | (w.programs[1] as u32) << 16,
                            w.positions[0] as u32 | (w.positions[1] as u32) << 16,
                        ]
                    })
                    .collect();
                let slot = taken % SLOTS;
                self.queue.write_buffer(&self.wars, (slot * chunk * 8) as u64, &words(&input));
                self.queue.write_buffer(&self.control, (2 + slot) as u64 * 4, &words(&[0]));
                self.queue.write_buffer(&self.control, 4, &words(&[(taken * chunk + part.len()) as u32]));
                ring.push_back((start, part.len()));
                taken += 1;
            }
            if ring.is_empty() {
                return;
            }
            let began = Instant::now();
            self.write_params(steps_per_dispatch);
            let mut encoder = self.device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &self.bind_group, &[]);
                pass.dispatch_workgroups(self.threads / WORKGROUP, 1, 1);
            }
            encoder.copy_buffer_to_buffer(&self.control, 0, &self.readback, 0, CONTROL_WORDS as u64 * 4);
            self.queue.submit([encoder.finish()]);
            let control = self.read(CONTROL_WORDS as u64 * 4);
            let seconds = began.elapsed().as_secs_f64().max(1e-4);
            let scaled = steps_per_dispatch as f64 * (DISPATCH_SECONDS / seconds).clamp(0.5, 2.0);
            steps_per_dispatch = scaled.clamp(1000.0, 10_000_000.0) as u32;
            let in_ring: u64 = (0..ring.len()).map(|k| control[2 + (finished_chunks + k) % SLOTS] as u64).sum();
            progress.add(done + in_ring - reported);
            reported = done + in_ring;
            // Collect the oldest chunks that are complete.
            while let Some(&(start, len)) = ring.front() {
                let slot = finished_chunks % SLOTS;
                if (control[2 + slot] as usize) < len {
                    break;
                }
                let mut encoder = self.device.create_command_encoder(&Default::default());
                encoder.copy_buffer_to_buffer(
                    &self.results,
                    (slot * chunk * 8) as u64,
                    &self.readback,
                    0,
                    len as u64 * 8,
                );
                self.queue.submit([encoder.finish()]);
                let out = self.read(len as u64 * 8);
                let mut results = results.lock().unwrap();
                for (i, r) in out.chunks(2).enumerate() {
                    results[start + i] = WarResult { pcs: [r[0] as u16, (r[0] >> 16) as u16], steps: r[1] };
                }
                ring.pop_front();
                finished_chunks += 1;
                done += len as u64;
            }
        }
    }

    fn write_params(&self, steps_per_dispatch: u32) {
        let s = &self.settings;
        let params = [
            s.queue_len as u32,
            s.exec_other as u32,
            s.max_steps,
            s.dat_test as u32,
            self.chunk as u32,
            SLOTS as u32,
            steps_per_dispatch,
            self.threads,
        ];
        self.queue.write_buffer(&self.params, 0, &words(&params));
    }

    /// Wait for the GPU and read the first `bytes` of the readback buffer.
    fn read(&self, bytes: u64) -> Vec<u32> {
        let slice = self.readback.slice(..bytes);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map the readback buffer"));
        self.device.poll(wgpu::PollType::wait_indefinitely()).expect("wait for the GPU");
        let data = slice
            .get_mapped_range()
            .expect("mapped readback")
            .chunks(4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        self.readback.unmap();
        data
    }
}

fn words(data: &[u32]) -> Vec<u8> {
    data.iter().flat_map(|w| w.to_le_bytes()).collect()
}
