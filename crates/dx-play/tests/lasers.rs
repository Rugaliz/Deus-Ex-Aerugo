//! Laser beams (`ULaserIterator::CurrentItem`, DeusEx 1001a1a0): a laser emitter's render iterator yields its proxy
//! once per beam segment, moved along the beam and turned with it, and once more at a saved point. Set `DEUSEX_DIR`
//! to the game folder.

use dx_play::Session;
use std::path::PathBuf;

#[test]
fn laser_emitters_draw_their_proxy_along_each_beam() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "02_NYC_Warehouse").unwrap();
    for _ in 0..10 {
        s.step_bound(1.0 / 30.0);
    }
    let class = s.vm.reg.class_by_name("LaserEmitter").unwrap();
    let emitters: Vec<u32> = s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).is_some_and(|c| s.vm.is_a(c, class))).collect();
    let player = s.player;
    let mut checked = 0;
    for e in emitters {
        if !s.vm.bp(e, "bIsOn") || s.vm.bp(e, "bRandomBeam") || s.vm.bp(e, "bHiddenBeam") {
            continue;
        }
        // far from the player the emitter is frozen (its `Tick`); as when the player comes near: traced again
        s.vm.set_bp(e, "bFrozen", false);
        s.vm.call_by_name(e, "CalcTrace", vec![ue_vm::Value::Float(0.0)]).unwrap();
        let Some(it) = s.vm.render_iterator_begin(e, player).unwrap() else { continue };
        let max = s.vm.ip(it, "MaxItems");
        assert!(max >= 2, "an emitter that is on has segments (MaxItems {max})");
        let proxy = s.vm.op(it, "proxy");
        let start = s.vm.vp(e, "Location");
        let mut items = Vec::new();
        while !s.vm.render_iterator_done(it) {
            let r = s.vm.render_iterator_item(it).unwrap();
            assert_eq!(r, proxy, "every item is the proxy");
            items.push(s.vm.vp(proxy, "Location"));
            s.vm.render_iterator_next(it);
        }
        assert_eq!(items.len() as i32, max);
        assert_eq!(s.vm.ip(it, "NextItem"), max, "NextItem counts the items");
        // all but the last on the first beam's line, at most one segment (16 units) apart, moving away from its start
        let mut last_d = -1.0f32;
        for (k, p) in items[..items.len() - 1].iter().enumerate() {
            let d = ((p[0] - start[0]).powi(2) + (p[1] - start[1]).powi(2) + (p[2] - start[2]).powi(2)).sqrt();
            if k > 0 && d < last_d {
                // a reflected beam starts again near its mirror point; only the first beam is checked
                break;
            }
            assert!(k == 0 || d - last_d <= 17.0, "item {k}: {d} after {last_d}");
            last_d = d;
        }
        // the last item shows the proxy at the saved point (one of the items, or the emitter's place)
        let last = items[items.len() - 1];
        let saved = s.vm.vp(it, "savedLoc");
        assert_eq!(last, saved);
        assert!(items[..items.len() - 1].contains(&saved) || saved == start, "the saved point is an item or the emitter");
        // the draws: one per item, where the walk put the proxy
        let draws = s.vm.particle_draws(e);
        assert_eq!(draws.len(), items.len());
        for ((i, d), p) in draws.iter().zip(&items) {
            assert_eq!(d.location, *p, "draw {i}");
        }
        checked += 1;
    }
    eprintln!("{checked} laser emitters checked");
    assert!(checked > 0, "the map has lasers that are on");
}
