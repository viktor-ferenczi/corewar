//! The placement random generator. `golden.rs` checks its sequence against MARS.COM.

use mars::Rng;

#[test]
fn positions_are_in_the_arena() {
    let mut rng = Rng::from_ticks(0x0012_3456);
    assert!((0..100_000).all(|_| rng.next() < 8000));
}

#[test]
fn same_ticks_same_sequence() {
    let (mut a, mut b) = (Rng::from_ticks(99), Rng::from_ticks(99));
    let mut c = Rng::from_ticks(100);
    let (sa, sb, sc): (Vec<u16>, Vec<u16>, Vec<u16>) =
        (0..50).map(|_| (a.next(), b.next(), c.next())).fold((vec![], vec![], vec![]), |mut v, (x, y, z)| {
            v.0.push(x);
            v.1.push(y);
            v.2.push(z);
            v
        });
    assert_eq!(sa, sb);
    assert_ne!(sa, sc);
}

#[test]
fn zero_ticks_do_not_get_stuck() {
    // Ticks 0 seed an all zero state, RANDOM then sets the high word to 1.
    let mut rng = Rng::from_ticks(0);
    let values: Vec<u16> = (0..20).map(|_| rng.next()).collect();
    assert!(values.windows(2).any(|w| w[0] != w[1]));
}
