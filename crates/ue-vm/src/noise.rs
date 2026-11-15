//! Noise and hearing: `MakeNoise` (`AActor::CheckNoiseHearing`) and `APawn::CanHear`. Ported from the original code's
//! behaviour; the team-game branch of `CanHear` (multiplayer only) is left out.

use crate::collision::*;
use crate::{Value, Vm, VmError};

type Res<T> = Result<T, VmError>;

fn dist_sq(a: Vec3, b: Vec3) -> f32 {
    (0..3).map(|i| (a[i] - b[i]) * (a[i] - b[i])).sum()
}

impl Vm {
    /// `Level.PawnList`, in list order (the newest pawn first); dead entries are skipped.
    pub(crate) fn pawn_list(&mut self) -> Vec<u32> {
        let mut out = Vec::new();
        if self.level == 0 {
            return out;
        }
        let mut p = self.op(self.level, "PawnList");
        while p != 0 && out.len() < 4096 {
            if self.is_live(p) {
                out.push(p);
            }
            p = self.op(p, "nextPawn");
        }
        out
    }

    /// `APawn::CanHear`: is a noise of `loudness` at `pos` loud enough for `pawn` and in line of sight?
    pub fn can_hear(&mut self, pawn: u32, pos: Vec3, loudness: f32) -> bool {
        let o = self.offsets();
        let loc = self.vec_at(pawn, o.location);
        let d = dist_sq(loc, pos);
        if loudness * loudness * 4_000_000.0 < d {
            return false;
        }
        if d != 0.0 {
            let x = 1_200_000.0 / d;
            let (near, far) = (x.min(2.0), x.min(0.5));
            let stimulus = near * loudness + far * self.fp(pawn, "Alertness");
            self.set_fp(pawn, "Stimulus", stimulus);
            if stimulus < self.fp(pawn, "HearingThreshold") {
                return false;
            }
        } else {
            return true;
        }
        self.bsp.as_ref().map(|b| b.fast_line_check(pos, loc)).unwrap_or(true)
    }

    /// `MakeNoise(Loudness)`: remembers the two most recent distinct noises on the instigator and offers the noise to
    /// the pawns that can hear it (`HearNoise` event).
    pub fn make_noise(&mut self, a: u32, loudness: f32) -> Res<()> {
        let o = self.offsets();
        let inst = self.op(a, "Instigator");
        if !self.is_live(inst) {
            return Ok(());
        }
        let now = self.time;
        let loc = self.vec_at(a, o.location);
        let slot = |vm: &mut Vm, n: u8| -> (f32, Vec3, f32) {
            let (t, s, l) = if n == 1 { ("noise1time", "noise1spot", "noise1loudness") } else { ("noise2time", "noise2spot", "noise2loudness") };
            (vm.fp(inst, t), vm.vp(inst, s), vm.fp(inst, l))
        };
        let store = |vm: &mut Vm, n: u8| {
            let (t, s, l) = if n == 1 { ("noise1time", "noise1spot", "noise1loudness") } else { ("noise2time", "noise2spot", "noise2loudness") };
            vm.set_fp(inst, t, now);
            vm.set_vp(inst, s, loc);
            vm.set_fp(inst, l, loudness);
        };
        let (t1, s1, l1) = slot(self, 1);
        let (t2, s2, l2) = slot(self, 2);
        let same = |t: f32, s: Vec3, l: f32| t > now - 0.2 && dist_sq(s, loc) < 2500.0 && l >= loudness * 0.9;
        if same(t1, s1, l1) || same(t2, s2, l2) {
            return Ok(());
        }
        if now - 0.18 <= t1 {
            if now - 0.18 <= t2 {
                if dist_sq(s1, loc) >= 2500.0 || loudness < l1 {
                    if l2 <= loudness {
                        store(self, 1);
                    }
                } else {
                    store(self, 1);
                }
            } else {
                store(self, 2);
            }
        } else {
            store(self, 1);
        }
        let inst_is_player = self.bp(inst, "bIsPlayer");
        let enemy = self.op(inst, "Enemy");
        let enemy_is_player = self.is_live(enemy) && self.bp(enemy, "bIsPlayer");
        let maker_class = self.class_of(a);
        for p in self.pawn_list() {
            if p == inst {
                continue;
            }
            if !inst_is_player && !enemy_is_player {
                // a monster's noise is only heard by its own kind: the listener's class and the maker's class descend
                // one from the other (either way)
                let kin = match (maker_class, self.class_of(p)) {
                    (Some(m), Some(pc)) => self.is_a(pc, m) || self.is_a(m, pc),
                    _ => true,
                };
                if !kin {
                    continue;
                }
            } else if self.is_instance_of(p, o.class_player_pawn) {
                continue;
            }
            if self.is_probing(p, "HearNoise") && self.can_hear(p, loc, loudness) {
                self.send_event(p, "HearNoise", vec![Value::Float(loudness), Value::Obj(a)])?;
            }
        }
        Ok(())
    }
}
