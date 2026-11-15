//! Native iterator functions (`AllActors`, `TraceActors`, `RadiusActors`, ...). Those that walk the level's actor list
//! (`AllActors`, `ChildActors`, `BasedActors`, `RadiusActors`, `VisibleActors`) find one actor per step from where the
//! last one was found, as the engine does (`ActorWalk`); the others compute their list of items first (the engine's
//! too: touching array, hash radius list, multi-line check). The interpreter runs the loop body once per item with
//! the out parameters set from the item.

use crate::collision::Vec3;
use crate::{inst_ref, Value, Vm, VmError};
use ue_registry::ClassId;

type Res<T> = Result<T, VmError>;

fn vec_arg(a: &[Value], i: usize) -> Option<Vec3> {
    match a.get(i) {
        Some(Value::Vector(v)) => Some(*v),
        _ => None,
    }
}

fn float_arg(a: &[Value], i: usize) -> f32 {
    a.get(i).and_then(Value::as_f32).unwrap_or(0.0)
}

/// An iterator over the level's actor list (`AActor::execAllActors` 103e43f0, `execChildActors` 103e4600,
/// `execBasedActors` 103e4800, `execRadiusActors` 103e4fa0, `execVisibleActors` 103e5260): the engine keeps an index
/// into `Level->Actors` and reads the list's length again at every step, testing each actor as it stands then; actors
/// spawned during the loop are visited, destroyed ones (empty slots) skipped. No class means `Actor`.
#[derive(Debug, Clone)]
pub(crate) struct ActorWalk {
    cursor: usize,
    base: ClassId,
    this: u32,
    kind: WalkKind,
}

#[derive(Debug, Clone)]
enum WalkKind {
    /// `Tag` (at the offset given) must match when given.
    All(Option<(u32, usize)>),
    /// `this` is the actor or on its owner chain.
    Child,
    /// `Base == this`.
    Based,
    /// Within `radius + CollisionRadius` of the point.
    Radius(f32, Vec3),
    /// Not hidden, within `radius` (0: any distance) and a `FastLineCheck` to the point is clear.
    Visible(f32, Vec3),
    /// `ZoneActors` (Engine 103e5960): `Region.Zone` is the iterating zone, or is the level's `LevelInfo` (actors in
    /// the default zone are always included).
    Zone,
    /// `AllObjects` (Deus Ex, Core 1013f1f0): every object of the class in the object table's order (the port: live
    /// instances in creation order; package objects such as textures and classes are not walked).
    Objects,
    /// `CycleActors` (1030400c): from slot `idx` on, wrapping around, at most one round of `n` slots (`visited` so far);
    /// each actor of the class comes with the slot after it. One at a time: the scripts break out early.
    Cycle { idx: i32, visited: i32, n: i32 },
}

impl Vm {
    /// The walking iterator for native `name`, or `None` for the others.
    pub(crate) fn actor_walk(&mut self, name: &str, this: u32, args: &[Value]) -> Res<Option<ActorWalk>> {
        let o = self.offsets();
        let kind = match name {
            "AllActors" => WalkKind::All(match (args.get(2), self.reg.class_by_name("Actor").and_then(|c| self.reg.find_prop(c, "Tag"))) {
                (Some(Value::Name(n)), Some(p)) if *n != 0 => Some((*n, p.offset as usize)),
                _ => None,
            }),
            "ChildActors" => WalkKind::Child,
            "ZoneActors" => WalkKind::Zone,
            "BasedActors" => WalkKind::Based,
            "RadiusActors" => WalkKind::Radius(float_arg(args, 2), vec_arg(args, 3).unwrap_or_else(|| self.vec_at(this, o.location))),
            "VisibleActors" => WalkKind::Visible(float_arg(args, 2), vec_arg(args, 3).unwrap_or_else(|| self.vec_at(this, o.location))),
            "CycleActors" => {
                let n = self.actor_slots.len() as i32;
                let idx = match args.get(2) {
                    Some(Value::Int(i)) if *i >= 0 && *i < n => *i,
                    _ => 0,
                };
                let actor = self.reg.class_by_name("Actor").ok_or_else(|| VmError::Runtime("no Actor class".into()))?;
                return Ok(Some(ActorWalk { cursor: 0, base: self.base_class_arg(args).unwrap_or(actor), this, kind: WalkKind::Cycle { idx, visited: 0, n } }));
            }
            "AllObjects" => {
                let object = self.reg.class_by_name("Object").ok_or_else(|| VmError::Runtime("no Object class".into()))?;
                return Ok(Some(ActorWalk { cursor: 0, base: self.base_class_arg(args).unwrap_or(object), this, kind: WalkKind::Objects }));
            }
            _ => return Ok(None),
        };
        let actor = self.reg.class_by_name("Actor").ok_or_else(|| VmError::Runtime("no Actor class".into()))?;
        Ok(Some(ActorWalk { cursor: 0, base: self.base_class_arg(args).unwrap_or(actor), this, kind }))
    }

    /// The values a finished walk leaves in the loop's out variables, when they are not just `None` (`CycleActors`: no
    /// actor, the index where the search stopped).
    pub(crate) fn walk_end(w: &ActorWalk) -> Option<Vec<Value>> {
        match w.kind {
            WalkKind::Cycle { idx, .. } => Some(vec![Value::Obj(0), Value::Int(idx)]),
            _ => None,
        }
    }

    /// The next actor of `w` from its cursor on (the list's current length), or `None` when the list is used up.
    pub(crate) fn walk_next(&mut self, w: &mut ActorWalk) -> Option<Vec<Value>> {
        if let WalkKind::Cycle { idx, visited, n } = &mut w.kind {
            while *visited < *n {
                if *idx >= *n {
                    *idx = 0;
                }
                let at = *idx as usize;
                *idx += 1;
                *visited += 1;
                let Some(&slot) = self.actor_slots.get(at) else { continue };
                let ob = &self.objects[slot];
                if ob.alive && self.is_a(ob.class, w.base) {
                    return Some(vec![Value::Obj(inst_ref(slot)), Value::Int(*idx)]);
                }
            }
            return None;
        }
        if matches!(w.kind, WalkKind::Objects) {
            while w.cursor < self.objects.len() {
                let i = w.cursor;
                w.cursor += 1;
                if self.objects[i].alive && self.is_a(self.objects[i].class, w.base) {
                    return Some(vec![Value::Obj(inst_ref(i))]);
                }
            }
            return None;
        }
        let o = self.offsets();
        while w.cursor < self.actor_slots.len() {
            let slot = self.actor_slots[w.cursor];
            w.cursor += 1;
            let (alive, class) = (self.objects[slot].alive, self.objects[slot].class);
            if !alive || !self.is_a(class, w.base) {
                continue;
            }
            let a = inst_ref(slot);
            let hit = match &w.kind {
                WalkKind::All(tag) => tag.is_none_or(|(t, off)| self.u32_at(a, off) == t),
                WalkKind::Child => {
                    let (mut c, mut guard) = (a, 0);
                    loop {
                        if c == w.this {
                            break true;
                        }
                        c = self.u32_at(c, o.owner);
                        guard += 1;
                        if c == 0 || guard >= 64 {
                            break false;
                        }
                    }
                }
                WalkKind::Based => self.u32_at(a, o.base) == w.this,
                WalkKind::Zone => {
                    let zone = self.u32_at(a, o.zone);
                    zone == self.u32_at(a, o.level) || zone == w.this
                }
                WalkKind::Radius(radius, loc) => {
                    let l = self.vec_at(a, o.location);
                    let d = [l[0] - loc[0], l[1] - loc[1], l[2] - loc[2]];
                    let reach = radius + self.f32_at(a, o.collision_radius);
                    d[0] * d[0] + d[1] * d[1] + d[2] * d[2] < reach * reach
                }
                WalkKind::Objects | WalkKind::Cycle { .. } => unreachable!("walked above"),
                WalkKind::Visible(radius, loc) => {
                    if self.flag(a, o.b_hidden.0, o.b_hidden.1) {
                        continue;
                    }
                    let l = self.vec_at(a, o.location);
                    let d = [l[0] - loc[0], l[1] - loc[1], l[2] - loc[2]];
                    (*radius == 0.0 || d[0] * d[0] + d[1] * d[1] + d[2] * d[2] < radius * radius) && self.bsp.as_ref().map(|b| b.fast_line_check(l, *loc)).unwrap_or(true)
                }
            };
            if hit {
                return Some(vec![Value::Obj(a)]);
            }
        }
        None
    }

    fn base_class_arg(&self, a: &[Value]) -> Option<ClassId> {
        match a.first() {
            Some(Value::Obj(r)) => self.class_of_ref(*r),
            _ => None,
        }
    }

    /// Items of the native iterator `name`: one vector of out-parameter values per visited item. `args` holds the
    /// evaluated in-parameters (out-parameters are `Void`) in declaration order; `this` is the calling object.
    /// Items of the native iterator `name` and the values its out-parameters get when the loop runs out (or finds
    /// nothing): the engine clears the out actor before every search, so a `foreach` that ends normally leaves it None
    /// (`Void` = left as it is). Out-parameters arrive with their current values.
    pub fn native_iterator(&mut self, name: &str, this: u32, args: &[Value]) -> Res<(Vec<Vec<Value>>, Vec<Value>)> {
        if name == "CycleActors" {
            return Ok(self.cycle_actors(args));
        }
        let rows = self.native_iterator_rows(name, this, args)?;
        Ok((rows, vec![Value::Obj(0)]))
    }

    /// `AActor::execCycleActors` (1030400c): from slot `index` of the level's actor list (0 when out of range) on,
    /// wrapping around, at most one full round of slots in all; each actor of the class found comes with the slot after
    /// it as the new index. When the round is used up the index is where the search stopped.
    fn cycle_actors(&mut self, args: &[Value]) -> (Vec<Vec<Value>>, Vec<Value>) {
        let actor = self.reg.class_by_name("Actor");
        let base = self.base_class_arg(args).or(actor);
        let n = self.actor_slots.len() as i32;
        let mut idx = match args.get(2) {
            Some(Value::Int(i)) => *i,
            _ => 0,
        };
        if idx >= n || idx < 0 {
            idx = 0;
        }
        let mut visited = 0;
        let mut rows = Vec::new();
        loop {
            let mut found = 0;
            while visited < n {
                if idx >= n {
                    idx = 0;
                }
                let slot = self.actor_slots[idx as usize];
                idx += 1;
                visited += 1;
                let ob = &self.objects[slot];
                if ob.alive && base.map(|b| self.is_a(ob.class, b)).unwrap_or(true) {
                    found = inst_ref(slot);
                    break;
                }
            }
            if found == 0 {
                break;
            }
            rows.push(vec![Value::Obj(found), Value::Int(idx)]);
        }
        (rows, vec![Value::Obj(0), Value::Int(idx)])
    }

    fn native_iterator_rows(&mut self, name: &str, this: u32, args: &[Value]) -> Res<Vec<Vec<Value>>> {
        let o = self.offsets();
        let base = self.base_class_arg(args);
        let need_base = || base.ok_or_else(|| VmError::Runtime(format!("{name} without a class")));
        Ok(match name {
            "TouchingActors" => {
                let base = need_base()?;
                (0..4)
                    .map(|k| self.u32_at(this, o.touching + k * 4))
                    .filter(|&a| a != 0 && self.is_live(a) && self.is_instance_of(a, base))
                    .map(|a| vec![Value::Obj(a)])
                    .collect()
            }
            // `AActor::execVisibleCollidingActors` (103e55b0): the hash's `ActorRadiusCheck` list (1000 units when no
            // radius is given), in its order; actors of the class, not hidden when `bIgnoreHidden`, that a
            // `FastLineCheck` from the location reaches
            "VisibleCollidingActors" => {
                // no class: every actor
                let base_class = base;
                let radius = match float_arg(args, 2) {
                    r if r == 0.0 => 1000.0,
                    r => r,
                };
                let loc = vec_arg(args, 3).unwrap_or_else(|| self.vec_at(this, o.location));
                let ignore_hidden = matches!(args.get(4), Some(Value::Bool(true)));
                let mut out = Vec::new();
                for a in self.hash_radius_check(loc, radius) {
                    if base_class.is_some_and(|b| !self.is_instance_of(a, b)) {
                        continue;
                    }
                    if ignore_hidden && self.flag(a, o.b_hidden.0, o.b_hidden.1) {
                        continue;
                    }
                    let l = self.vec_at(a, o.location);
                    if self.bsp.as_ref().map(|b| b.fast_line_check(l, loc)).unwrap_or(true) {
                        out.push(vec![Value::Obj(a)]);
                    }
                }
                out
            }
            // ReachablePathnodes(BaseClass, out NavPoint, FromPoint, out distance, bUsePrunedPaths)
            "ReachablePathnodes" => {
                let from = match args.get(2) { Some(Value::Obj(r)) => *r, _ => 0 };
                let pruned = matches!(args.get(4), Some(Value::Bool(true)));
                self.get_pathnode_list(this, 32, from, pruned, true)?.into_iter().map(|(n, d)| vec![Value::Obj(n), Value::Float(d)]).collect()
            }
            "TraceActors" | "TraceVisibleActors" => {
                let base = need_base()?;
                let end = vec_arg(args, 4).ok_or_else(|| VmError::Runtime("TraceActors without an end point".into()))?;
                let start = vec_arg(args, 5).unwrap_or_else(|| self.vec_at(this, o.location));
                let extent = vec_arg(args, 6).unwrap_or([0.0; 3]);
                self.multi_line_check(start, end, extent, true, true, 0)
                    .into_iter()
                    .filter(|h| self.is_instance_of(h.actor, base))
                    .map(|h| vec![Value::Obj(h.actor), Value::Vector(h.location), Value::Vector(h.normal)])
                    .collect()
            }
            "TraceTexture" => {
                let base = need_base()?;
                let end = vec_arg(args, 7).ok_or_else(|| VmError::Runtime("TraceTexture without an end point".into()))?;
                let start = vec_arg(args, 8).unwrap_or_else(|| self.vec_at(this, o.location));
                let extent = vec_arg(args, 9).unwrap_or([0.0; 3]);
                let hits = self.multi_line_check(start, end, extent, true, true, 0);
                let mut out = Vec::new();
                for h in hits {
                    if !self.is_instance_of(h.actor, base) {
                        continue;
                    }
                    let (tex_name, tex_group, flags) = if h.actor == self.level { self.surface_info(h.item) } else { (0, 0, 0) };
                    out.push(vec![
                        Value::Obj(h.actor),
                        Value::Name(tex_name),
                        Value::Name(tex_group),
                        Value::Int(flags),
                        Value::Vector(h.location),
                        Value::Vector(h.normal),
                    ]);
                }
                out
            }
            other => return Err(VmError::Unsupported(format!("iterator {other}"))),
        })
    }

    /// Texture name, texture group name and surface flags of the BSP surface of node `node`.
    fn surface_info(&mut self, node: i32) -> (u32, u32, i32) {
        match self.surface_of_node(node) {
            Some((tex, flags)) => {
                let name = tex.0;
                let group = tex.1;
                (name, group, flags as i32)
            }
            None => (0, 0, 0),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Value, Vm};

    /// `AllActors` walks the level's actor list from where it stopped, reading its length again each step: an actor of
    /// the class spawned during the loop is visited after the others, one destroyed before its turn is not.
    #[test]
    fn all_actors_sees_actors_spawned_during_the_loop() {
        let Some(dir) = std::env::var_os("DEUSEX_DIR").map(std::path::PathBuf::from) else { return };
        let mut vm = Vm::new(ue_registry::Registry::load_game(&dir).expect("load game"));
        let class = vm.reg.class_by_name("Effects").unwrap();
        let first: Vec<u32> = (0..3).map(|_| vm.new_object(class).unwrap()).collect();
        let class_ref = Value::Obj(vm.reg.obj_id(&vm.reg.class(class).path.clone()).0);
        let mut w = vm.actor_walk("AllActors", 0, &[class_ref, Value::Void, Value::Name(0)]).unwrap().unwrap();
        let mut seen = vec![vm.walk_next(&mut w).unwrap()[0].clone()];
        // during the loop: a new one, and the third of the old ones destroyed before its turn
        let late = vm.new_object(class).unwrap();
        assert!(vm.destroy(first[2]).unwrap());
        while let Some(row) = vm.walk_next(&mut w) {
            seen.push(row[0].clone());
        }
        assert_eq!(seen, vec![Value::Obj(first[0]), Value::Obj(first[1]), Value::Obj(late)]);
    }
}
