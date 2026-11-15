//! `DrawFrame`'s lists from the visibility walk (`OccludeBsp` links each draw list into one of three frame lists and
//! each sprite into the frame's sprite list, both at the head). Set `DEUSEX_DIR` to the game folder.

use dx_play::Session;
use std::path::PathBuf;

const PF_SEE_THROUGH: u32 = 0x47;
const PF_FLAG_MIRROR_DRAWN: u32 = 0x8000_0000;

fn open(map: &str) -> Option<Session> {
    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    let mut s = Session::open(&g, map).unwrap();
    for _ in 0..3 {
        s.step_bound(1.0 / 30.0);
    }
    Some(s)
}

/// Engine rules (OccludeBsp 10b18208..10b195fc, 10b247ec): a surface's list is `(flags & 0x47 != 0) + 1` unless the
/// mirror path took it (lists 0 or 1); a sprite is linked once, on its first visible piece; `DrawFrame` walks the
/// sprite list from the head (the last linked first) and the third surface list from the head (newest piece first).
#[test]
fn frame_lists_follow_the_walk() {
    let mut blended = 0;
    let mut drawn_sprites = 0;
    for map in ["01_NYC_UNATCOIsland", "09_NYC_Dockyard", "14_OceanLab_Lab", "06_HongKong_TongBase"] {
        let Some(mut s) = open(map) else { return };
        let mut cam = s.camera();
        for k in 0..8 {
            cam.yaw = k * 8192;
            let (_, frames) = s.compute_visibility(&cam, 800, 600);
            for f in &frames {
                let v = &f.vis;
                assert_eq!(v.surfaces.len(), v.surface_lists.len());
                for (&(_, flags, _), &list) in v.surfaces.iter().zip(&v.surface_lists) {
                    if flags & PF_FLAG_MIRROR_DRAWN == 0 {
                        assert_eq!(list, (flags & PF_SEE_THROUGH != 0) as u8 + 1, "{map} flags {flags:#x}");
                    } else {
                        assert!(list <= 1, "{map} mirror flags {flags:#x} list {list}");
                    }
                }
                // the sprite list holds every drawn sprite once
                let mut order = v.sprite_order.clone();
                order.sort();
                order.dedup();
                assert_eq!(order.len(), v.sprite_order.len(), "{map}: a sprite linked twice");
                let drawn: Vec<usize> = (0..v.sprites.len()).filter(|&i| v.sprites[i]).collect();
                assert_eq!(order, drawn, "{map}");
            }
            let main = &frames[0];
            blended += main.vis.surface_lists.iter().filter(|&&l| l == 2).count();
            drawn_sprites += main.vis.sprite_order.len();
            // the last sprite linked is the first drawn
            let ranks = Session::sprite_ranks(main);
            if let Some(&last) = main.vis.sprite_order.last() {
                let sp = &main.sprites[last];
                assert_eq!(ranks[&(sp.id, sp.item)], 0);
            }
        }
        // the renderer's third list: the main frame's non-occluding pieces, newest first
        s.step_bound(1.0 / 30.0);
        let main = &s.vis_frames()[0];
        let want: Vec<u32> = main.vis.surfaces.iter().zip(&main.vis.surface_lists).filter(|(_, &l)| l == 2).map(|(&(n, _, _), _)| n).rev().collect();
        assert_eq!(s.blended_order().unwrap(), want);
    }
    println!("{blended} non-occluding pieces, {drawn_sprites} sprites in the main views");
    assert!(blended > 0 && drawn_sprites > 0);
}
