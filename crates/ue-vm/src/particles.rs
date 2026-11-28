//! Render iterators: an actor with a `RenderIteratorClass` gets a `RenderInterface` object the renderer drives (Engine,
//! `ULevel`/`URender`); Deus Ex uses it for `ParticleGenerator` (smoke, steam, sparks, fire). The iterator keeps 64
//! particles; every frame the generator updates them (`UpdateParticles`, DeusEx.dll `UParticleIterator`) and the renderer
//! draws the generator's `proxy` actor once per live particle, moved to the particle and scaled and faded by it.

use crate::animation::MeshDraw;
use crate::{Value, Vm, VmError};

type Res<T> = Result<T, VmError>;

const PARTICLES: usize = 64;
/// Bytes of one `sParticle`: bActive, initVel, Velocity, Location, DrawScale, ScaleGlow, LifeSpan.
const PARTICLE_SIZE: usize = 52;

impl Vm {
    /// Makes the actor's render interface when it has none (the renderer does this the first time it draws the actor),
    /// then lets the iterator choose what to draw this frame (`Init`).
    pub fn prepare_render_interface(&mut self, a: u32, camera: u32) -> Res<()> {
        let class_ref = self.op(a, "RenderIteratorClass");
        if class_ref == 0 {
            return Ok(());
        }
        let mut it = self.op(a, "RenderInterface");
        if it == 0 || !self.is_live(it) {
            let path = self.reg.obj_path(ue_registry::ObjId(class_ref)).to_string();
            let Some(class) = self.reg.class_by_path(&path) else { return Ok(()) };
            it = self.new_object(class)?;
            self.set_prop(it, "Outer", 0, Value::Obj(a))?;
            self.set_prop(a, "RenderInterface", 0, Value::Obj(it))?;
        }
        // `ParticleIterator.Init` (the game's own class): the generator's proxy, and 64 items unless the generator is
        // frozen (0). The script does exactly this; run natively it costs a property copy instead of a script call per
        // generator per visibility pass (a quarter of the pass in Area 51). A subclass (a mod's) runs its own script.
        if self.class_of(it).is_some_and(|c| self.reg.class(c).path == "DeusEx.ParticleIterator") {
            if self.is_class(a, "ParticleGenerator") {
                let proxy = self.op(a, "proxy");
                self.set_prop(it, "proxy", 0, Value::Obj(proxy))?;
                let items = if self.bp(a, "bFrozen") { 0 } else { 64 };
                self.set_ip(it, "MaxItems", items);
            }
            return Ok(());
        }
        // `LaserIterator.Init` (the game's own class), likewise: the emitter's place, proxy and beam style, and as many
        // items as the active beams have segments, plus one for the last, unless the emitter is frozen or its beam
        // hidden
        if self.class_of(it).is_some_and(|c| self.reg.class(c).path == "DeusEx.LaserIterator") {
            if self.is_class(a, "LaserEmitter") {
                const BEAMS: usize = 8;
                const BEAM_SIZE: usize = 36;
                let (loc, rot, proxy, random) = (self.vp(a, "Location"), self.rotation(a), self.op(a, "proxy"), self.bp(a, "bRandomBeam"));
                self.set_ip(it, "MaxItems", 0);
                self.set_ip(it, "nextItem", 0);
                self.set_vp(it, "prevLoc", loc);
                self.set_vp(it, "prevRand", [0.0; 3]);
                self.set_vp(it, "savedLoc", loc);
                self.set_prop(it, "savedRot", 0, Value::Rotator(rot))?;
                self.set_prop(it, "proxy", 0, Value::Obj(proxy))?;
                self.set_bp(it, "bRandomBeam", random);
                if !self.bp(a, "bFrozen") && !self.bp(a, "bHiddenBeam") {
                    let mut items = 0i32;
                    if let Some((base, _)) = self.prop_at(it, "Beams") {
                        for i in 0..BEAMS {
                            let b = base + i * BEAM_SIZE;
                            if self.u32_at(it, b) & 1 != 0 {
                                items = items.wrapping_add(self.u32_at(it, b + 32) as i32);
                            }
                        }
                    }
                    if items > 0 {
                        items += 1;
                    }
                    self.set_ip(it, "MaxItems", items);
                }
            }
            return Ok(());
        }
        self.send_event(it, "Init", vec![Value::Obj(camera)])?;
        Ok(())
    }

    /// The renderer's walk over an actor's render iterator (`URender::SetupDynamics`): makes the render interface
    /// when missing, then `Init(Camera)` (`URenderIterator::Init`: the script `Init`, then `Observer = Camera`) and
    /// `First` (`Index = 0`). Returns the iterator, or `None` when the actor has none.
    pub fn render_iterator_begin(&mut self, a: u32, camera: u32) -> Res<Option<u32>> {
        self.prepare_render_interface(a, camera)?;
        let it = self.op(a, "RenderInterface");
        if it == 0 || !self.is_live(it) {
            return Ok(None);
        }
        self.set_prop(it, "Observer", 0, Value::Obj(camera))?;
        self.set_ip(it, "Index", 0);
        Ok(Some(it))
    }

    /// How many of a particle iterator's particles are alive.
    pub fn live_particles(&mut self, it: u32) -> usize {
        let Some((base, _)) = self.prop_at(it, "Particles") else { return 0 };
        (0..PARTICLES).filter(|i| self.u32_at(it, base + i * PARTICLE_SIZE) & 1 != 0).count()
    }

    /// `URenderIterator::IsDone`: `Index >= MaxItems`.
    pub fn render_iterator_done(&mut self, it: u32) -> bool {
        self.ip(it, "MaxItems") <= self.ip(it, "Index")
    }

    /// `URenderIterator::Next`: `Index + 1`.
    pub fn render_iterator_next(&mut self, it: u32) {
        let i = self.ip(it, "Index");
        self.set_ip(it, "Index", i.wrapping_add(1));
    }

    /// `CurrentItem`. A `ParticleIterator` (DeusEx.dll 1001ab20) with a proxy and the current particle alive gives its
    /// proxy, given the particle's draw scale and glow and moved to it (`FarMoveActor`; a failed move deletes the
    /// particle); otherwise, as `URenderIterator::CurrentItem`, the iterator's outer (the generator itself).
    pub fn render_iterator_item(&mut self, it: u32) -> Res<u32> {
        let outer = self.op(it, "Outer");
        if self.prop_at(it, "Beams").is_some() {
            return self.laser_iterator_item(it, outer);
        }
        let Some((base, _)) = self.prop_at(it, "Particles") else { return Ok(outer) };
        let proxy = self.op(it, "proxy");
        let index = self.ip(it, "Index");
        if proxy != 0 && self.is_live(proxy) && (0..PARTICLES as i32).contains(&index) {
            let p = base + index as usize * PARTICLE_SIZE;
            if self.u32_at(it, p) & 1 != 0 {
                let (scale, glow) = (self.f32_at(it, p + 40), self.f32_at(it, p + 44));
                self.set_fp(proxy, "DrawScale", scale);
                self.set_fp(proxy, "ScaleGlow", glow);
                let loc = [self.f32_at(it, p + 28), self.f32_at(it, p + 32), self.f32_at(it, p + 36)];
                if self.far_move_actor(proxy, loc, false, false)? {
                    return Ok(proxy);
                }
                self.delete_particle(it, index as usize);
            }
        }
        Ok(outer)
    }

    /// `ULaserIterator::CurrentItem` (DeusEx 1001a1a0): item `Index` is a point along the active beams, one per
    /// segment (`numSegments` per beam, the script's `AddBeam`), the proxy moved there and turned along the beam.
    /// A random beam (`bRandomBeam`) jitters each point by a random unit vector plus the last one and turns the proxy
    /// from the point before. Each item may become the saved point with probability `NextItem / MaxItems` while the
    /// saved point is still the emitter's location; the last item (`NextItem == MaxItems`) shows the proxy there.
    /// Without a proxy, `URenderIterator::CurrentItem` (the emitter).
    fn laser_iterator_item(&mut self, it: u32, outer: u32) -> Res<u32> {
        const BEAMS: usize = 8;
        const BEAM_SIZE: usize = 36;
        let proxy = self.op(it, "proxy");
        if proxy == 0 || !self.is_live(proxy) {
            return Ok(outer);
        }
        let Some((base, _)) = self.prop_at(it, "Beams") else { return Ok(outer) };
        let index = self.ip(it, "Index");
        // the beam holding the item: the first active one whose running segment count passes the index (beam 0
        // when none does)
        let (mut acc, mut beam) = (0i32, 0usize);
        for i in 0..BEAMS {
            let b = base + i * BEAM_SIZE;
            if self.u32_at(it, b) & 1 == 0 {
                continue;
            }
            acc = acc.wrapping_add(self.u32_at(it, b + 32) as i32);
            if acc > index {
                beam = i;
                break;
            }
        }
        let b = base + beam * BEAM_SIZE;
        let segments = self.u32_at(it, b + 32) as i32;
        let t = ((1.0 - (acc.wrapping_sub(index)) as f32 as f64 / segments as f32 as f64) as f32).clamp(0.0, 1.0);
        let loc = [self.f32_at(it, b + 4), self.f32_at(it, b + 8), self.f32_at(it, b + 12)];
        let mut rot = [self.u32_at(it, b + 16) as i32, self.u32_at(it, b + 20) as i32, self.u32_at(it, b + 24) as i32];
        let len = self.f32_at(it, b + 28);
        // `(GMath.UnitCoords / Rotation).XAxis`
        let x = crate::natives::rotate_vector([1.0, 0.0, 0.0], rot, false);
        let mut pos = [0.0f32; 3];
        for k in 0..3 {
            pos[k] = ((x[k] * len + loc[k]) - loc[k]) * t + loc[k];
        }
        if self.bp(it, "bRandomBeam") {
            let v = loop {
                let v = [self.frand() * 2.0 - 1.0, self.frand() * 2.0 - 1.0, self.frand() * 2.0 - 1.0];
                if v[0] * v[0] + v[1] * v[1] + v[2] * v[2] <= 1.0 {
                    break v;
                }
            };
            // `FVector::UnsafeNormal`
            let inv = 1.0 / (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            let n = [v[0] * inv, v[1] * inv, v[2] * inv];
            let (prev_loc, prev_rand) = (self.vp(it, "prevLoc"), self.vp(it, "prevRand"));
            let mut ahead = [0.0f32; 3];
            let mut back = [0.0f32; 3];
            for k in 0..3 {
                pos[k] = n[k] + prev_rand[k] + pos[k];
                ahead[k] = (pos[k] - prev_loc[k]) + pos[k];
                back[k] = prev_loc[k] - ahead[k];
            }
            rot = crate::ai_move::vec_to_rot(back);
            self.set_vp(it, "prevLoc", ahead);
            self.set_vp(it, "prevRand", n);
        }
        self.far_move_actor(proxy, pos, false, false)?;
        // `MoveActor` by nothing: only the rotation changes (the proxy does not collide)
        self.set_prop(proxy, "Rotation", 0, Value::Rotator(rot))?;
        let emitter_loc = self.vp(outer, "Location");
        let saved = self.vp(it, "savedLoc");
        let (next, max) = (self.ip(it, "NextItem"), self.ip(it, "MaxItems"));
        if saved == emitter_loc && self.frand() < next as f32 / max as f32 {
            self.set_vp(it, "savedLoc", pos);
            self.set_prop(it, "savedRot", 0, Value::Rotator(rot))?;
        }
        // what the draw uses (the walk is random): the proxy's place for this item, and for the last the saved one
        let items = self.laser_items.entry(it).or_default();
        if next == 0 {
            items.clear();
        }
        items.push((index.max(0) as usize, pos, rot));
        let next = next.wrapping_add(1);
        self.set_ip(it, "NextItem", next);
        if next == max {
            let saved = self.vp(it, "savedLoc");
            self.far_move_actor(proxy, saved, false, false)?;
            let r = match self.get_prop(it, "savedRot", 0)? {
                Value::Rotator(r) => r,
                _ => [0; 3],
            };
            self.set_prop(proxy, "Rotation", 0, Value::Rotator(r))?;
            if let Some(last) = self.laser_items.get_mut(&it).and_then(|v| v.last_mut()) {
                *last = (last.0, saved, r);
            }
        }
        Ok(proxy)
    }

    /// `UParticleIterator::UpdateParticles(dt)`.
    pub fn update_particles(&mut self, it: u32, dt: f32) {
        let Some((base, _)) = self.prop_at(it, "Particles") else { return };
        let gravity = self.bp(it, "bOwnerUsesGravity");
        let scales = self.bp(it, "bOwnerScales");
        let fades = self.bp(it, "bOwnerFades");
        let (zone_g, rise, life, scale) = (self.fp(it, "OwnerZoneGravity"), self.fp(it, "OwnerRiseRate"), self.fp(it, "OwnerLifeSpan"), self.fp(it, "OwnerDrawScale"));
        let mut none_active = true;
        for i in 0..PARTICLES {
            let p = base + i * PARTICLE_SIZE;
            if self.u32_at(it, p) & 1 == 0 {
                continue;
            }
            let mut life_left = self.f32_at(it, p + 48);
            if life_left > 0.0 {
                let old = life_left;
                life_left = old - dt;
                if life_left <= 0.0 {
                    life_left = -1.0;
                }
                self.set_u32_at(it, p + 48, life_left.to_bits());
            }
            if life_left < 0.0 {
                self.delete_particle(it, i);
                continue;
            }
            none_active = false;
            let (init_x, init_y, init_z) = (self.f32_at(it, p + 4), self.f32_at(it, p + 8), self.f32_at(it, p + 12));
            let vx = init_x + 2.0 - self.frand() * 5.0;
            let vy = init_y + 2.0 - self.frand() * 5.0;
            let vz = if !gravity {
                (life - life_left) * rise * (self.frand() * 0.2 + 0.9) + init_z
            } else {
                dt * zone_g * 0.2 + self.f32_at(it, p + 24)
            };
            self.set_u32_at(it, p + 16, vx.to_bits());
            self.set_u32_at(it, p + 20, vy.to_bits());
            self.set_u32_at(it, p + 24, vz.to_bits());
            let draw_scale = if !scales { scale } else { (((life - life_left) + 1.0) * scale).clamp(0.01, 3.0) };
            self.set_u32_at(it, p + 40, draw_scale.to_bits());
            let glow = if !fades { 1.0 } else { life_left / life };
            self.set_u32_at(it, p + 44, glow.to_bits());
            for (k, v) in [vx, vy, vz].into_iter().enumerate() {
                let l = self.f32_at(it, p + 28 + 4 * k) + dt * v;
                self.set_u32_at(it, p + 28 + 4 * k, l.to_bits());
            }
        }
        let proxy = self.op(it, "proxy");
        if proxy != 0 && self.is_live(proxy) {
            self.set_bp(proxy, "bHidden", none_active);
        }
    }

    fn delete_particle(&mut self, it: u32, i: usize) {
        let Some((base, _)) = self.prop_at(it, "Particles") else { return };
        let p = base + i * PARTICLE_SIZE;
        let w = self.u32_at(it, p) & !1;
        self.set_u32_at(it, p, w);
        self.set_ip(it, "nextFreeParticle", i as i32);
    }

    /// What the renderer draws for an actor's render interface: its proxy once per live particle, or for a laser once
    /// per item of its last walk.
    pub fn particle_draws(&mut self, a: u32) -> Vec<(usize, MeshDraw)> {
        let it = self.op(a, "RenderInterface");
        if it == 0 || !self.is_live(it) {
            return Vec::new();
        }
        let proxy = self.op(it, "proxy");
        if proxy == 0 || !self.is_live(proxy) {
            return Vec::new();
        }
        let Some(template) = self.mesh_draw(proxy) else { return Vec::new() };
        if self.prop_at(it, "Beams").is_some() {
            // a laser: the places its last walk gave the proxy
            return self
                .laser_items
                .get(&it)
                .map(|v| {
                    v.iter()
                        .map(|&(i, loc, rot)| {
                            let mut d = template.clone();
                            d.location = loc;
                            d.rotation = rot;
                            (i, d)
                        })
                        .collect()
                })
                .unwrap_or_default();
        }
        let Some((base, _)) = self.prop_at(it, "Particles") else { return Vec::new() };
        let max = (self.ip(it, "MaxItems").max(0) as usize).min(PARTICLES);
        let mut out = Vec::new();
        for i in 0..max {
            let p = base + i * PARTICLE_SIZE;
            if self.u32_at(it, p) & 1 == 0 {
                continue;
            }
            let mut d = template.clone();
            d.location = [self.f32_at(it, p + 28), self.f32_at(it, p + 32), self.f32_at(it, p + 36)];
            let s = self.f32_at(it, p + 40);
            d.scale = if s == 0.0 { 1.0 } else { s };
            d.glow = self.f32_at(it, p + 44);
            out.push((i, d));
        }
        out
    }
}
