// Wars of two programs on the GPU, a port of Engine::fight and Engine::step in engine.rs.
//
// Every thread plays one war at a time in its own 8000 cell arena. When its war ends, it writes
// the result and takes the next war from an atomic counter. A dispatch runs at most
// params.steps_per_dispatch steps per thread, then saves the war state to continue with the next
// dispatch, which keeps every dispatch short enough for the driver's watchdog.
//
// Wars stream through a ring of params.slots chunks of params.chunk wars. War number g of the
// stream is at g % (chunk * slots) in the wars and results buffers. The host refills a slot once
// all its wars are finished and raises the limit, while the threads go on with the other slots.

const ARENALEN: u32 = 8000u;
// The DAT test does not see a DAT in the last cell, see Engine::dats.
const DAT_SCAN_LEN: u32 = 7999u;
const NONE: u32 = 0xFFFFFFFFu;
const QUEUELEN: u32 = 256u;
const MASK_WORDS: u32 = 8u;
const PROGRAM_WORDS: u32 = 202u;

// Per thread state words: header, then the process slot masks and slots of both programs.
const S_WAR: u32 = 0u;
const S_REMAINING: u32 = 1u;
const S_STEPS: u32 = 2u;
const S_COUNTER_LO: u32 = 3u;
const S_COUNTER_HI: u32 = 4u;
const S_TURN: u32 = 5u;
const S_ALIVE: u32 = 6u;
const S_DATS: u32 = 7u;
const S_CURRPC: u32 = 8u;  // 2 words
const S_PCNUM: u32 = 10u;  // 2 words
const S_MASK: u32 = 16u;   // 2 * MASK_WORDS words
const S_PCS: u32 = 32u;    // 2 * QUEUELEN words
const STATE_WORDS: u32 = 544u;

struct Params {
    queue_len: u32,
    exec_other: u32,
    max_steps: u32,
    dat_test: u32,
    chunk: u32,
    slots: u32,
    steps_per_dispatch: u32,
    threads: u32,
}

@group(0) @binding(0) var<uniform> params: Params;
// Per program: length, start, then 100 cells of (op | modes << 8 | a << 16, b).
@group(0) @binding(1) var<storage, read> programs: array<u32>;
// Per war: (program A | program B << 16, position A | position B << 16).
@group(0) @binding(2) var<storage, read> wars: array<vec2<u32>>;
// Per war: (processes of A | processes of B << 16, steps).
@group(0) @binding(3) var<storage, read_write> results: array<vec2<u32>>;
// Per thread 8000 cells of (op | modes << 8 | a << 16, b | owner << 16).
@group(0) @binding(4) var<storage, read_write> arena: array<vec2<u32>>;
@group(0) @binding(5) var<storage, read_write> state: array<u32>;
// 0: next war of the stream, 1: wars available (the limit), 2..: wars finished per slot.
@group(0) @binding(6) var<storage, read_write> control: array<atomic<u32>>;

// Registers of the running war.
var<private> base: u32;
var<private> sbase: u32;
var<private> currpc: array<u32, 2>;
var<private> pcnum: array<u32, 2>;
var<private> mask: array<u32, 16>;
var<private> alive: u32;
var<private> dats: u32;

fn add(x: u32, y: u32) -> u32 {
    let s = x + y;
    return select(s, s - ARENALEN, s >= ARENALEN);
}

fn dec(x: u32) -> u32 {
    return select(x - 1u, ARENALEN - 1u, x == 0u);
}

fn cell(addr: u32) -> vec2<u32> {
    return arena[base + addr];
}

fn put(addr: u32, value: vec2<u32>) {
    arena[base + addr] = value;
}

fn put_y(addr: u32, y: u32) {
    arena[base + addr].y = y;
}

fn get_b(addr: u32) -> u32 {
    return cell(addr).y & 0xFFFFu;
}

fn get_pc(w: u32, slot: u32) -> u32 {
    return state[sbase + S_PCS + w * QUEUELEN + slot];
}

fn set_pc(w: u32, slot: u32, pc: u32) {
    state[sbase + S_PCS + w * QUEUELEN + slot] = pc;
    let i = w * MASK_WORDS + slot / 32u;
    let bit = 1u << (slot % 32u);
    mask[i] = select(mask[i] & ~bit, mask[i] | bit, pc < ARENALEN);
}

fn is_active(w: u32, slot: u32) -> bool {
    return (mask[w * MASK_WORDS + slot / 32u] & (1u << (slot % 32u))) != 0u;
}

// First slot at or after `first` and before `limit` that is active (or free).
fn find(w: u32, first: u32, limit: u32, want_active: bool) -> u32 {
    var slot = first;
    while slot < limit {
        var word = mask[w * MASK_WORDS + slot / 32u];
        if !want_active {
            word = ~word;
        }
        let bits = word >> (slot % 32u);
        if bits != 0u {
            let found = slot + countTrailingZeros(bits);
            return select(NONE, found, found < limit);
        }
        slot = (slot / 32u + 1u) * 32u;
    }
    return NONE;
}

fn kill(w: u32) {
    set_pc(w, currpc[w], NONE);
    pcnum[w] -= 1u;
    if pcnum[w] == 0u {
        alive -= 1u;
    }
}

fn jump(w: u32, to: u32) {
    set_pc(w, currpc[w], dec(to));
}

fn write_b(addr: u32, value: u32, num: u32) {
    put_y(addr, value | (num << 16u));
}

// LOADA / LOADB: (address, value) of a parameter.
fn load(cpc: u32, field: u32, mode: u32, num: u32, is_b: bool) -> vec2<u32> {
    if mode == 0u {
        let c = cell(cpc);
        return vec2(cpc, select(c.x >> 16u, c.y & 0xFFFFu, is_b));
    }
    let t = add(field, cpc);
    if mode == 1u {
        return vec2(t, get_b(t));
    }
    if mode == 2u {
        let addr = add(t, get_b(t));
        return vec2(addr, get_b(addr));
    }
    let b = dec(get_b(t));
    write_b(t, b, num);
    let addr = add(t, b);
    return vec2(addr, get_b(addr));
}

fn step(w: u32) {
    let queue_len = params.queue_len;
    var slot = find(w, currpc[w], queue_len, true);
    if slot == NONE {
        slot = find(w, 0u, currpc[w], true);
        if slot == NONE {
            return;
        }
    }
    currpc[w] = slot;
    let num = w + 1u;
    let cpc = get_pc(w, slot);
    let current = cell(cpc);
    let op = current.x & 0xFFu;
    let modes = (current.x >> 8u) & 0xFFu;
    if params.exec_other == 0u && (current.y >> 16u) != num {
        kill(w);
    } else {
        let a = load(cpc, current.x >> 16u, modes & 3u, num, false);
        let b = load(cpc, get_b(cpc), (modes >> 2u) & 3u, num, true);
        switch op {
            case 0u: {
                kill(w);
            }
            case 1u: {
                if (modes & 3u) == 0u {
                    put_y(b.x, a.y | (num << 16u));
                } else {
                    // The owner is not copied, so reading the source before writing is the same.
                    let old = cell(b.x);
                    let src = cell(a.x);
                    put(b.x, vec2(src.x, (src.y & 0xFFFFu) | (num << 16u)));
                    if b.x < DAT_SCAN_LEN {
                        dats = dats - select(0u, 1u, (old.x & 0xFFu) == 0u) + select(0u, 1u, (src.x & 0xFFu) == 0u);
                    }
                }
            }
            case 2u: {
                write_b(b.x, add(a.y, b.y), num);
            }
            case 3u: {
                write_b(b.x, select(b.y + ARENALEN - a.y, b.y - a.y, b.y >= a.y), num);
            }
            case 4u: {
                jump(w, a.x);
            }
            case 5u: {
                if b.y == 0u {
                    jump(w, a.x);
                }
            }
            case 6u: {
                if b.y != 0u {
                    jump(w, a.x);
                }
            }
            case 7u: {
                let v = dec(b.y);
                write_b(b.x, v, num);
                if v != 0u {
                    jump(w, a.x);
                }
            }
            case 8u: {
                if a.y == b.y {
                    set_pc(w, currpc[w], add(cpc, 1u));
                }
            }
            case 9u: {
                if pcnum[w] < queue_len {
                    let free = find(w, 0u, queue_len, false);
                    if free != NONE {
                        set_pc(w, free, a.x);
                        pcnum[w] += 1u;
                    }
                }
            }
            default: {}
        }
    }
    let s = currpc[w];
    if is_active(w, s) {
        state[sbase + S_PCS + w * QUEUELEN + s] = add(get_pc(w, s), 1u);
    }
    currpc[w] = select(s + 1u, 0u, s + 1u >= queue_len);
}

fn ring(war: u32) -> u32 {
    return war % (params.chunk * params.slots);
}

// Clear the arena and load both programs of the war, like Engine::place with given positions.
fn start_war(war: u32) {
    for (var i = 0u; i < ARENALEN; i++) {
        put(i, vec2(0u, 0u));
    }
    dats = DAT_SCAN_LEN;
    let pair = wars[ring(war)];
    for (var w = 0u; w < 2u; w++) {
        let program = ((pair.x >> (16u * w)) & 0xFFFFu) * PROGRAM_WORDS;
        let pos = (pair.y >> (16u * w)) & 0xFFFFu;
        let len = programs[program];
        for (var k = 0u; k < MASK_WORDS; k++) {
            mask[w * MASK_WORDS + k] = 0u;
        }
        set_pc(w, 0u, programs[program + 1u] + pos);
        currpc[w] = 0u;
        pcnum[w] = 1u;
        for (var i = 0u; i < len; i++) {
            let addr = pos + i;
            // Cells past the arena can not be reached by anything.
            if addr < ARENALEN {
                let x = programs[program + 2u + 2u * i];
                if addr < DAT_SCAN_LEN {
                    dats = dats - select(0u, 1u, (cell(addr).x & 0xFFu) == 0u) + select(0u, 1u, (x & 0xFFu) == 0u);
                }
                put(addr, vec2(x, programs[program + 3u + 2u * i] | ((w + 1u) << 16u)));
            }
        }
    }
    alive = 2u;
    state[sbase + S_WAR] = war;
    state[sbase + S_REMAINING] = params.max_steps;
    state[sbase + S_STEPS] = 0u;
    state[sbase + S_COUNTER_LO] = 0u;
    state[sbase + S_COUNTER_HI] = 0u;
    state[sbase + S_TURN] = 0u;
}

// Take the next war, false when there is none available.
fn next_war() -> bool {
    var war = atomicLoad(&control[0]);
    while war < atomicLoad(&control[1]) {
        let taken = atomicCompareExchangeWeak(&control[0], war, war + 1u);
        if taken.exchanged {
            start_war(war);
            return true;
        }
        war = taken.old_value;
    }
    state[sbase + S_WAR] = NONE;
    return false;
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let t = id.x;
    if t >= params.threads {
        return;
    }
    base = t * ARENALEN;
    sbase = t * STATE_WORDS;
    if state[sbase + S_WAR] == NONE {
        if !next_war() {
            return;
        }
    } else {
        for (var w = 0u; w < 2u; w++) {
            currpc[w] = state[sbase + S_CURRPC + w];
            pcnum[w] = state[sbase + S_PCNUM + w];
        }
        for (var k = 0u; k < 16u; k++) {
            mask[k] = state[sbase + S_MASK + k];
        }
        alive = state[sbase + S_ALIVE];
        dats = state[sbase + S_DATS];
    }
    var remaining = state[sbase + S_REMAINING];
    var steps = state[sbase + S_STEPS];
    var counter_lo = state[sbase + S_COUNTER_LO];
    var counter_hi = state[sbase + S_COUNTER_HI];
    var w = state[sbase + S_TURN];
    var budget = params.steps_per_dispatch;
    loop {
        var over = false;
        if pcnum[w] != 0u {
            if budget == 0u {
                break;
            }
            counter_lo += 1u;
            if counter_lo >= 1000u {
                counter_lo = 0u;
                counter_hi = (counter_hi + 1u) & 0xFFFFu;
                if counter_hi % 16u == 0u && params.dat_test != 0u && dats == 0u {
                    remaining = 1u;
                }
            }
            step(w);
            steps += 1u;
            budget -= 1u;
            // 0 steps stands for 2^32: the first decrement wraps around.
            remaining -= 1u;
            over = remaining == 0u;
        }
        if !over {
            w ^= 1u;
            over = alive < 2u;
        }
        if over {
            let war = state[sbase + S_WAR];
            results[ring(war)] = vec2(pcnum[0] | (pcnum[1] << 16u), steps);
            atomicAdd(&control[2u + (war / params.chunk) % params.slots], 1u);
            if !next_war() {
                return;
            }
            remaining = params.max_steps;
            steps = 0u;
            counter_lo = 0u;
            counter_hi = 0u;
            w = 0u;
        }
    }
    state[sbase + S_REMAINING] = remaining;
    state[sbase + S_STEPS] = steps;
    state[sbase + S_COUNTER_LO] = counter_lo;
    state[sbase + S_COUNTER_HI] = counter_hi;
    state[sbase + S_TURN] = w;
    state[sbase + S_ALIVE] = alive;
    state[sbase + S_DATS] = dats;
    for (var k = 0u; k < 2u; k++) {
        state[sbase + S_CURRPC + k] = currpc[k];
        state[sbase + S_PCNUM + k] = pcnum[k];
    }
    for (var k = 0u; k < 16u; k++) {
        state[sbase + S_MASK + k] = mask[k];
    }
}
