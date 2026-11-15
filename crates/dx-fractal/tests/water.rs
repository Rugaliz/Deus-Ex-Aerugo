//! Water simulation checks: invariants derived by hand, plus (ignored) a byte-exact comparison with vectors produced
//! by running the original engine's water code (private; `DX_WATER_VECTORS=<file>`, see `docs/formats/water-texture.md`).
use dx_fractal::water::{wet_source, Water};
use dx_fractal::{Drop, Shared};

#[test]
fn level_water_stays_level() {
    let mut w = Water::new(32, 16, Vec::new());
    w.set_refraction(128);
    for _ in 0..10 {
        w.calculate();
    }
    assert!(w.field().iter().all(|&h| h == 128));
    // flat water has no slope: every pixel is the table's value for 0, trunc(1 * 128 / 512) = 0
    assert!(w.bitmap.iter().all(|&p| p == 0));
}

#[test]
fn a_raised_cell_spreads_to_its_hexagonal_neighbours() {
    // even rows: a cell's neighbours are x-1 and x in the rows above and below; odd rows: x and x+1
    let (u, v) = (16, 16);
    let w8 = u / 2;
    let mut w = Water::new(u, v, Vec::new());
    w.field_mut()[5 * w8 + 3] = 200; // odd row 5, x 3
    w.pass(false); // the even rows take the new heights
    let f = w.field();
    for (y, x) in [(4, 3), (4, 4), (6, 3), (6, 4)] {
        // (200 + 3 * 128 - 2 * 128) / 2 = 164
        assert_eq!(f[y * w8 + x], 164, "row {y} x {x}");
    }
    assert_eq!(f[4 * w8 + 2], 128);
    assert_eq!(f[6 * w8 + 5], 128);
}

#[test]
fn wet_pixels_come_from_the_source_shifted_right() {
    let (u, v) = (8, 8);
    let mut w = Water::new(u, v, Vec::new());
    let source: Vec<u8> = (0..64).collect();
    w.bitmap = vec![2; 64];
    w.bitmap[7] = 0xff; // -1
    w.apply_wet(&source);
    assert_eq!(w.bitmap[0], 2);
    assert_eq!(w.bitmap[6], 0); // 6 + 2 wraps to 0
    assert_eq!(w.bitmap[7], 6);
    assert_eq!(w.bitmap[8], 10);
}

#[test]
fn a_small_source_is_enlarged() {
    let src = [1, 2, 3, 4];
    let big = wet_source(4, 4, 2, 2, &src).unwrap();
    assert_eq!(big, vec![1, 1, 2, 2, 1, 1, 2, 2, 3, 3, 4, 4, 3, 3, 4, 4]);
    assert!(wet_source(2, 2, 4, 4, &[0; 16]).is_none());
}

#[test]
fn phase_spot_and_tap_drops() {
    let mut shared = Shared::new();
    let drops = vec![
        Drop { kind: 1, depth: 0, x: 2, y: 3, d: 64, ..Default::default() },
        Drop { kind: 18, depth: 200, x: 5, y: 1, a: 250, d: 10, ..Default::default() },
    ];
    let mut w = Water::new(16, 16, drops);
    w.redraw_drops(&mut shared);
    let wd = 8;
    // phase 0 + 64 = a quarter turn: the sine's peak, in rows 6 and 7
    assert_eq!(w.field()[6 * wd + 2], 255);
    assert_eq!(w.field()[7 * wd + 2], 255);
    assert_eq!(w.drops[0].depth, 64);
    // the tap's counter wraps (250 + 10 = 4 <= 10): a drip, the lower cell inverted
    assert_eq!(w.field()[2 * wd + 5], 200);
    assert_eq!(w.field()[3 * wd + 5], !200u8);
    assert_eq!(w.drops[1].a, 4);
}

#[test]
#[ignore]
fn matches_the_original_water_code() {
    let Ok(path) = std::env::var("DX_WATER_VECTORS") else {
        eprintln!("DX_WATER_VECTORS not set");
        return;
    };
    let data = std::fs::read(path).expect("vectors");
    let mut at = 0;
    let mut cases = 0;
    while at < data.len() {
        let u = u16::from_le_bytes([data[at], data[at + 1]]) as usize;
        let v = u16::from_le_bytes([data[at + 2], data[at + 3]]) as usize;
        let odd = data[at + 4] != 0;
        at += 5;
        let mut take = |n: usize| {
            let s = &data[at..at + n];
            at += n;
            s.to_vec()
        };
        let (field, table, want_field, want_bitmap) = (take(u * v / 2), take(1028), take(u * v / 2), take(u * v));
        let mut w = Water::new(u, v, Vec::new());
        w.field_mut().copy_from_slice(&field);
        w.render_table = table;
        w.pass(odd);
        assert_eq!(w.field(), &want_field[..], "field {u}x{v} odd={odd}");
        assert_eq!(w.bitmap, want_bitmap, "bitmap {u}x{v} odd={odd}");
        cases += 1;
    }
    assert!(cases > 0);
    eprintln!("{cases} cases match");
}

#[test]
#[ignore]
fn drops_match_the_original_code() {
    let Ok(path) = std::env::var("DX_DROP_VECTORS") else {
        eprintln!("DX_DROP_VECTORS not set");
        return;
    };
    let data = std::fs::read(path).expect("vectors");
    let mut at = 0;
    let mut cases = 0;
    while at < data.len() {
        let rd16 = |p: usize| u16::from_le_bytes([data[p], data[p + 1]]) as usize;
        let (u, v, n, steps) = (rd16(at), rd16(at + 2), rd16(at + 4), data[at + 6] as usize);
        let seed = u32::from_le_bytes(data[at + 7..at + 11].try_into().unwrap());
        at += 11;
        let to_drops = |b: &[u8]| -> Vec<Drop> {
            b.chunks(8).map(|c| Drop { kind: c[0], depth: c[1], x: c[2], y: c[3], a: c[4], b: c[5], c: c[6], d: c[7] }).collect()
        };
        let drops = to_drops(&data[at..at + 8 * n]);
        at += 8 * n;
        let fs = u * v / 2;
        let mut w = Water::new(u, v, drops);
        w.field_mut().copy_from_slice(&data[at..at + fs]);
        at += fs;
        let mut shared = Shared::new();
        shared.rng = dx_fractal::Rng::new(seed);
        for step in 0..steps {
            w.redraw_drops(&mut shared);
            assert_eq!(w.field(), &data[at..at + fs], "field {u}x{v} seed {seed} step {step}");
            at += fs;
            assert_eq!(w.drops, to_drops(&data[at..at + 8 * n]), "drops {u}x{v} seed {seed} step {step}");
            at += 8 * n;
        }
        cases += 1;
    }
    assert!(cases > 0);
    eprintln!("{cases} cases match");
}
