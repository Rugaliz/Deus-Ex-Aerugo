//! Fire checks: hand-derived cases, plus (ignored) byte-exact comparisons with vectors produced by running the original
//! engine's fire code (private files: `DX_FIRE_VECTORS`, `DX_SPARK_VECTORS`; see `docs/formats/fire-texture.md`).
use dx_fractal::{Fire, Shared};

#[test]
fn heat_spreads_up_and_fades() {
    // RenderHeat 255: table(i) = i/4 + 1, so four 100s become 101
    let mut f = Fire::new(8, 8, &[], 4, 255, false, 0);
    f.bitmap[5 * 8 + 3] = 100; // one hot pixel in row 5
    f.propagate();
    // the pixel above reads (left, self, right, below) = (0, 0, 0, 100): 100/4 + 1 = 26
    assert_eq!(f.bitmap[4 * 8 + 3], 26);
    // the hot pixel's row: its neighbours see it once (26), itself too
    assert_eq!(f.bitmap[5 * 8 + 3], 26);
    assert_eq!(f.bitmap[5 * 8 + 2], 26);
    // cold elsewhere: table(0) = 1
    assert_eq!(f.bitmap[0], 1);
}

#[test]
fn a_rising_fire_moves_up_a_row() {
    let mut f = Fire::new(8, 8, &[], 4, 255, true, 0);
    f.bitmap[5 * 8 + 3] = 100;
    f.propagate();
    assert_eq!(f.bitmap[3 * 8 + 3], 26); // two rows up: reads row y+1 and y+2
    assert_eq!(f.bitmap[4 * 8 + 3], 26);
}

#[test]
fn render_table_follows_render_heat() {
    let f = Fire::new(8, 8, &[], 4, 191, false, 0);
    // i/4 + 1 - (255 - 191)/16 = i/4 - 3
    assert_eq!(f.render_table[0], 0);
    assert_eq!(f.render_table[100], 22);
    assert_eq!(f.render_table[1023], 252);
}

#[test]
fn an_emitter_spawns_particles_that_burn_out() {
    // SPARK_Blaze (4) at the bottom: spawns type 0x20 particles that lose 5 heat per step
    let spark = [4u8, 200, 16, 30, 0, 0, 0, 0];
    let mut f = Fire::new(32, 32, &[spark], 64, 255, false, 0);
    let mut shared = Shared::new();
    let mut most = 0;
    for _ in 0..100 {
        f.redraw_sparks(&mut shared);
        most = most.max(f.num_sparks);
        assert!(f.num_sparks <= 64);
    }
    assert!(most > 10, "{most} sparks at most");
    assert!(f.sparks[1..f.num_sparks].iter().all(|s| s[0] == 0x20));
}

fn read_u16(d: &[u8], at: usize) -> usize {
    u16::from_le_bytes([d[at], d[at + 1]]) as usize
}

#[test]
#[ignore]
fn sparks_match_the_original_code() {
    let Ok(path) = std::env::var("DX_SPARK_VECTORS") else {
        eprintln!("DX_SPARK_VECTORS not set");
        return;
    };
    let d = std::fs::read(path).expect("vectors");
    let (mut at, mut cases) = (0, 0);
    while at < d.len() {
        let (u, v, limit, num) = (read_u16(&d, at), read_u16(&d, at + 2), read_u16(&d, at + 4), read_u16(&d, at + 6));
        let (freq, steps) = (d[at + 8], d[at + 9] as usize);
        let seed = u32::from_le_bytes(d[at + 10..at + 14].try_into().unwrap());
        at += 14;
        let sparks: Vec<[u8; 8]> = d[at..at + 8 * limit].chunks(8).map(|c| c.try_into().unwrap()).collect();
        at += 8 * limit;
        let mut f = Fire::new(u, v, &sparks, limit as i32, 255, false, freq);
        f.num_sparks = num;
        f.bitmap.copy_from_slice(&d[at..at + u * v]);
        at += u * v;
        let mut shared = Shared::new();
        shared.rng = dx_fractal::Rng::new(seed);
        for step in 0..steps {
            f.redraw_sparks(&mut shared);
            let want_num = read_u16(&d, at);
            assert_eq!(f.num_sparks, want_num, "num {u}x{v} seed {seed} step {step}");
            assert_eq!(f.aux_phase, d[at + 2], "aux {u}x{v} seed {seed} step {step}");
            at += 3;
            let diffs: Vec<usize> = d[at..at + 8 * limit].chunks(8).enumerate().filter(|(k, s)| &f.sparks[*k][..] != *s).map(|(k, _)| k).collect();
            if !diffs.is_empty() && std::env::var_os("DX_FIRE_DEBUG").is_some() {
                eprintln!("step {step}: num ours {} theirs {want_num}", f.num_sparks);
                for k in 0..f.num_sparks.max(want_num) {
                    let theirs = &d[at + 8 * k..at + 8 * k + 8];
                    eprintln!("{k:3} ours {:?} theirs {:?}{}", f.sparks[k], theirs, if &f.sparks[k][..] != theirs { "  <--" } else { "" });
                }
            }
            for (k, s) in d[at..at + 8 * limit].chunks(8).enumerate() {
                assert_eq!(&f.sparks[k][..], s, "spark {k} {u}x{v} seed {seed} step {step}");
            }
            at += 8 * limit;
            let bad = f.bitmap.iter().zip(&d[at..at + u * v]).filter(|(a, b)| a != b).count();
            assert_eq!(bad, 0, "{bad} pixels differ, {u}x{v} seed {seed} step {step}");
            at += u * v;
        }
        cases += 1;
    }
    assert!(cases > 0);
    eprintln!("{cases} cases match");
}

#[test]
#[ignore]
fn propagation_matches_the_original_code() {
    let Ok(path) = std::env::var("DX_FIRE_VECTORS") else {
        eprintln!("DX_FIRE_VECTORS not set");
        return;
    };
    let d = std::fs::read(path).expect("vectors");
    let (mut at, mut cases) = (0, 0);
    while at < d.len() {
        let (u, v, rising) = (read_u16(&d, at), read_u16(&d, at + 2), d[at + 4] != 0);
        at += 5;
        let mut f = Fire::new(u, v, &[], 4, 0, rising, 0);
        f.render_table = d[at..at + 1028].to_vec();
        at += 1028;
        f.bitmap.copy_from_slice(&d[at..at + u * v]);
        at += u * v;
        f.propagate();
        assert_eq!(f.bitmap, d[at..at + u * v].to_vec(), "{u}x{v} rising={rising}");
        at += u * v;
        cases += 1;
    }
    assert!(cases > 0);
    eprintln!("{cases} cases match");
}
