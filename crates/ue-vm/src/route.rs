//! The engine's route search over the map's navigation network, ported from Engine.dll (the `APawn` path functions
//! around 103d9f90..103dd900: `findPathToward`, `findPathTo`, `breadthPathFrom`, `clearPath(s)`, `HandleSpecial`, the
//! sorted candidate lists and their helpers, `jumpLanding`). The behaviour is described in
//! `docs/formats/../NATIVE_AUDIT.md`; in short:
//!
//! * Start candidates are the navigation points within 800 units of the pawn, end candidates those within 800 units of
//!   the goal, each kept in a list of the 32 nearest (squared distances, `FSortedPathList`). The pawn's `MoveTarget`
//!   is the start "anchor" when the pawn stands on it.
//! * The nearest start candidate the pawn can see and walk to becomes the anchor (when within `max(48, radius)`) or a
//!   start point (`bEndPoint`, weight = its distance). From an anchor the goal may be reached directly; otherwise the
//!   anchor's neighbours become the start points.
//! * The search runs backwards: from the end node that reaches the goal, over `upstreamPaths`, through an ordered
//!   list (`nextOrdered`/`prevOrdered`, weights in `visitedWeight`, with a moving insertion marker) until a start point
//!   is met; that node is where the pawn goes. Without such an end node (and not `bHunting`), the search runs from the
//!   start anchor instead and the route is reversed.
//! * The pawn is teleported around for the reachability tests and put back at the end, as in the engine.
//!
//! `RouteCache` is not filled: in Deus Ex `SetRouteCache` only logs a complaint.

use crate::collision::*;
use crate::physics::*;
use crate::{Value, Vm, VmError};

type Res<T> = Result<T, VmError>;

/// `FSortedPathList`: up to 32 navigation points sorted by an integer weight (ascending, newer first among equals).
#[derive(Clone)]
pub(crate) struct SortedPathList {
    pub path: [u32; 32],
    pub dist: [i32; 32],
    pub n: i32,
}

impl SortedPathList {
    pub fn new() -> Self {
        SortedPathList { path: [0; 32], dist: [0; 32], n: 0 }
    }

    /// `FSortedPathList::addPath`: a short bisection (for more than 8 entries) and a linear scan find the place.
    pub fn add(&mut self, node: u32, dist: i32) {
        let n = self.n;
        let mut i = 0;
        if n > 8 {
            let half = n / 2;
            if dist > self.dist[half as usize] {
                i = half;
                if n > 16 {
                    let q = n / 4 + i;
                    if dist > self.dist[q as usize] {
                        i = q;
                    }
                }
            } else if n > 16 {
                let q = n / 4;
                if dist > self.dist[q as usize] {
                    i = q;
                }
            }
        }
        while i < n && dist > self.dist[i as usize] {
            i += 1;
        }
        if i < 32 {
            let new_n = (n + 1).min(32);
            let mut j = new_n - 1;
            while j > i {
                self.path[j as usize] = self.path[j as usize - 1];
                self.dist[j as usize] = self.dist[j as usize - 1];
                j -= 1;
            }
            self.path[i as usize] = node;
            self.dist[i as usize] = dist;
            self.n = new_n;
        }
    }

    /// `FSortedPathList::removePath`.
    pub fn remove(&mut self, i: i32) {
        let mut k = i;
        while k < self.n - 1 {
            self.path[k as usize] = self.path[k as usize + 1];
            self.dist[k as usize] = self.dist[k as usize + 1];
            k += 1;
        }
        self.n -= 1;
    }
}

/// MSVC `_ftol`: truncation; out-of-range values give the "integer indefinite" `i32::MIN`.
/// MSVC `_ftol`: `fistp QWORD` with truncation, the low 32 bits returned. Outside the 64-bit range (and NaN, the
/// infinities) the "integer indefinite" 0x8000000000000000, whose low half is 0 (probes `vm.cast.float.int.*`).
pub(crate) fn ftol(x: f64) -> i32 {
    if x.is_nan() || x >= 9_223_372_036_854_775_808.0 || x < -9_223_372_036_854_775_808.0 {
        0
    } else {
        x.trunc() as i64 as i32
    }
}

fn dist_sq(a: Vec3, b: Vec3) -> f64 {
    let (x, y, z) = ((a[0] - b[0]) as f64, (a[1] - b[1]) as f64, (a[2] - b[2]) as f64);
    x * x + y * y + z * z
}

/// Offsets of the `NavigationPoint` fields the search uses (the same in every subclass).
#[derive(Clone, Copy)]
pub(crate) struct NavOffsets {
    upstream: usize,
    paths: usize,
    pruned: usize,
    visited_weight: usize,
    best_path_weight: usize,
    next_nav: usize,
    next_ordered: usize,
    prev_ordered: usize,
    previous_path: usize,
    cost: usize,
    extra_cost: usize,
    flags: usize,
    player_only: u32,
    end_point: u32,
    special_cost: u32,
}

const UNVISITED: i32 = 10_000_000;

impl Vm {
    pub(crate) fn nav_offsets(&mut self) -> Option<NavOffsets> {
        let nav = self.reg.class_by_name("NavigationPoint")?;
        let f = |vm: &Vm, n: &str| vm.reg.find_prop(nav, n).map(|p| (p.offset as usize, p.bit_mask));
        let (flags, player_only) = f(self, "bPlayerOnly")?;
        Some(NavOffsets {
            upstream: f(self, "upstreamPaths")?.0,
            paths: f(self, "Paths")?.0,
            pruned: f(self, "PrunedPaths")?.0,
            visited_weight: f(self, "visitedWeight")?.0,
            best_path_weight: f(self, "bestPathWeight")?.0,
            next_nav: f(self, "nextNavigationPoint")?.0,
            next_ordered: f(self, "nextOrdered")?.0,
            prev_ordered: f(self, "prevOrdered")?.0,
            previous_path: f(self, "previousPath")?.0,
            cost: f(self, "cost")?.0,
            extra_cost: f(self, "ExtraCost")?.0,
            flags,
            player_only,
            end_point: f(self, "bEndPoint")?.1,
            special_cost: f(self, "bSpecialCost")?.1,
        })
    }

    fn ni(&self, n: u32, off: usize) -> i32 {
        self.u32_at(n, off) as i32
    }
    fn set_ni(&mut self, n: u32, off: usize, v: i32) {
        self.set_u32_at(n, off, v as u32);
    }
    fn nflag(&self, n: u32, no: &NavOffsets, mask: u32) -> bool {
        self.u32_at(n, no.flags) & mask != 0
    }
    fn set_nflag(&mut self, n: u32, no: &NavOffsets, mask: u32, v: bool) {
        let w = self.u32_at(n, no.flags);
        self.set_u32_at(n, no.flags, if v { w | mask } else { w & !mask });
    }

    fn is_a_named(&self, a: u32, class: &str) -> bool {
        match (self.class_of(a), self.reg.class_by_name(class)) {
            (Some(c), Some(k)) => self.is_a(c, k),
            _ => false,
        }
    }

    fn fast_line(&self, a: Vec3, b: Vec3) -> bool {
        self.bsp.as_ref().map(|m| m.fast_line_check(a, b)).unwrap_or(true)
    }

    fn eye_of(&mut self, a: u32) -> Vec3 {
        let l = self.location(a);
        [l[0], l[1], l[2] + self.fp(a, "BaseEyeHeight")]
    }

    /// `APawn::calcMoveFlags`: the `R_*` abilities of the pawn.
    pub(crate) fn calc_move_flags(&mut self, a: u32) -> i32 {
        let mut f = 0;
        for (i, prop) in ["bCanWalk", "bCanFly", "bCanSwim", "bCanJump", "bCanOpenDoors", "bCanDoSpecial", "bIsPlayer"].into_iter().enumerate() {
            if self.bp(a, prop) {
                f |= 1 << i;
            }
        }
        f
    }

    fn pawn_size(&mut self, a: u32) -> (i32, i32) {
        (ftol(self.fp(a, "CollisionRadius") as f64), ftol(self.fp(a, "CollisionHeight") as f64))
    }

    fn spec_fits(&self, spec: &crate::navigation::ReachSpecRt, r: i32, h: i32, flags: i32) -> bool {
        r <= spec.radius && h <= spec.height && (spec.flags & flags) == spec.flags
    }

    /// `ANavigationPoint` clear state for one search (`clearPath`): unvisited, out of the ordered list, not an end
    /// point, `cost` = `SpecialCost(pawn)` or `ExtraCost`.
    fn clear_path(&mut self, no: &NavOffsets, node: u32, pawn: u32) -> Res<()> {
        self.set_ni(node, no.visited_weight, UNVISITED);
        self.set_u32_at(node, no.next_ordered, 0);
        self.set_u32_at(node, no.prev_ordered, 0);
        self.set_nflag(node, no, no.end_point, false);
        let cost = if self.nflag(node, no, no.special_cost) {
            match self.send_event(node, "SpecialCost", vec![Value::Obj(pawn)])? {
                Some(v) => v.as_i32().unwrap_or(0),
                None => 0,
            }
        } else {
            self.ni(node, no.extra_cost)
        };
        self.set_ni(node, no.cost, cost);
        Ok(())
    }

    /// The navigation points in `Level.NavigationPointList` order.
    fn nav_list(&mut self, no: &NavOffsets) -> Vec<u32> {
        let mut out = Vec::new();
        if self.level == 0 {
            return out;
        }
        let mut n = self.op(self.level, "NavigationPointList");
        while n != 0 && out.len() < 65536 {
            out.push(n);
            n = self.u32_at(n, no.next_nav);
        }
        out
    }

    /// `APawn::clearPaths`.
    pub fn clear_paths(&mut self, pawn: u32) -> Res<()> {
        let Some(no) = self.nav_offsets() else { return Ok(()) };
        for n in self.nav_list(&no) {
            self.clear_path(&no, n, pawn)?;
        }
        Ok(())
    }

    /// Start/end candidate setup shared by both searches (Engine.dll 103da210).
    #[allow(clippy::too_many_arguments)]
    fn route_init(&mut self, no: &NavOffsets, pawn: u32, goal: Vec3, s: &mut SortedPathList, e: &mut SortedPathList, clear: bool, start_anchored: &mut bool, end_anchored: &mut bool) -> Res<()> {
        let o = self.offsets();
        let loc = self.vec_at(pawn, o.location);
        let mt = self.op(pawn, "MoveTarget");
        if mt != 0 && self.is_a_named(mt, "NavigationPoint") {
            let ml = self.vec_at(mt, o.location);
            if ((ml[2] - loc[2]) as f64).abs() < (self.fp(mt, "CollisionHeight") + self.fp(pawn, "CollisionHeight")) as f64 {
                let (dy, dx) = ((ml[1] - loc[1]) as f64, (ml[0] - loc[0]) as f64);
                let r = self.fp(pawn, "CollisionRadius") as f64;
                let mut r2 = r * r;
                if self.bp(pawn, "bIsPlayer") && self.is_a_named(mt, "InventorySpot") {
                    r2 += r2;
                }
                if dy * dy + dx * dx < r2 {
                    *start_anchored = true;
                    s.path[0] = mt;
                    s.dist[0] = 0;
                    s.n = 1;
                }
            }
        }
        for n in self.nav_list(no) {
            if clear {
                self.clear_path(no, n, pawn)?;
            }
            let nl = self.vec_at(n, o.location);
            if !*start_anchored {
                let d = ftol(dist_sq(loc, nl));
                if d < 640_000 {
                    s.add(n, d);
                }
            }
            if !*end_anchored {
                let d = ftol(dist_sq(goal, nl));
                if d < 640_000 {
                    e.add(n, d);
                }
            }
        }
        Ok(())
    }

    /// `FSortedPathList::findStartAnchor` (103da610): drops candidates until one is visible from the eye and reachable;
    /// it becomes the anchor when the pawn stands on it, else a start point weighted by its distance.
    fn find_start_anchor(&mut self, no: &NavOffsets, list: &mut SortedPathList, pawn: u32, anchored: &mut bool) -> Res<bool> {
        let o = self.offsets();
        let eye = self.eye_of(pawn);
        loop {
            if list.n < 1 {
                return Ok(false);
            }
            let n0 = list.path[0];
            let nl = self.vec_at(n0, o.location);
            if self.fast_line(nl, eye) && self.point_reachable_kv(pawn, nl, true)? {
                list.dist[0] = ftol((list.dist[0] as f64).sqrt());
                let thr = ftol(self.fp(pawn, "CollisionRadius") as f64).max(48);
                if list.dist[0] < thr {
                    let pl = self.vec_at(pawn, o.location);
                    if ((nl[2] - pl[2]) as f64).abs() < self.fp(pawn, "CollisionHeight") as f64 {
                        *anchored = true;
                        return Ok(true);
                    }
                }
                self.set_nflag(n0, no, no.end_point, true);
                self.set_ni(n0, no.best_path_weight, list.dist[0]);
                return Ok(true);
            }
            list.remove(0);
        }
    }

    /// 103da880: can the pawn, put on the anchor, reach `dest` directly (within 800 units, in sight)? Leaves the pawn
    /// on the anchor when it can; otherwise keeps only the anchor in the list.
    fn anchor_reaches(&mut self, list: &mut SortedPathList, pawn: u32, dest: Vec3) -> Res<bool> {
        let o = self.offsets();
        let old = self.vec_at(pawn, o.location);
        let al = self.vec_at(list.path[0], o.location);
        if dist_sq(dest, al) < 640_000.0 && self.fast_line(al, dest) && self.far_move_actor(pawn, al, true, false)? {
            if self.point_reachable_kv(pawn, dest, false)? {
                return Ok(true);
            }
            self.far_move_actor(pawn, old, true, true)?;
        }
        list.n = 1;
        Ok(false)
    }

    /// Does a move along the spec from `from` to `to` hit a door the pawn may not use (`bCanOpenDoors`, player-only
    /// movers)?
    fn spec_blocked_by_door(&mut self, pawn: u32, from: u32, to: u32) -> bool {
        let o = self.offsets();
        let (fl, tl) = (self.vec_at(from, o.location), self.vec_at(to, o.location));
        let hit = self.single_line_check(pawn, tl, fl, TRACE_MOVERS | TRACE_LEVEL, [0.0; 3], 0);
        if hit.actor == 0 || !self.is_a_named(hit.actor, "Mover") {
            return false;
        }
        !self.bp(pawn, "bCanOpenDoors") || (!self.bp(pawn, "bIsPlayer") && self.bp(hit.actor, "bPlayerOnly"))
    }

    /// The reach spec indices of a node's `Paths` (or `PrunedPaths`), up to the first -1.
    fn node_specs(&self, node: u32, off: usize) -> Vec<i32> {
        (0..16).map(|i| self.u32_at(node, off + 4 * i) as i32).take_while(|&x| x != -1).collect()
    }

    /// `expandAnchor` (103daaa0): the anchor's cost becomes 1000000 and the ends of its usable paths (then pruned paths)
    /// become start points weighted by the path length.
    fn expand_anchor(&mut self, no: &NavOffsets, list: &SortedPathList, pawn: u32) -> Res<()> {
        let anchor = list.path[0];
        self.set_ni(anchor, no.cost, 1_000_000);
        let flags = self.calc_move_flags(pawn);
        let (r, h) = self.pawn_size(pawn);
        let mut pruned = false;
        let mut i = 0;
        loop {
            if i > 15 {
                return Ok(());
            }
            let idx = self.ni(anchor, if pruned { no.pruned } else { no.paths } + 4 * i);
            if idx == -1 {
                if pruned {
                    return Ok(());
                }
                pruned = true;
                i = 0;
                continue;
            }
            if let Some(spec) = self.reach_specs.get(idx as usize).cloned() {
                if self.spec_fits(&spec, r, h, flags) && !self.spec_blocked_by_door(pawn, spec.start, spec.end) {
                    self.set_nflag(spec.end, no, no.end_point, true);
                    self.set_ni(spec.end, no.best_path_weight, spec.distance);
                }
            }
            i += 1;
        }
    }

    /// 103dad20: is there a usable direct path (`Paths`, then `PrunedPaths`) from `from` to `to`?
    fn can_move_to(&mut self, no: &NavOffsets, pawn: u32, from: u32, to: u32) -> bool {
        for off in [no.paths, no.pruned] {
            for idx in self.node_specs(from, off) {
                let Some(spec) = self.reach_specs.get(idx as usize).cloned() else { continue };
                if spec.end != to {
                    continue;
                }
                let flags = self.calc_move_flags(pawn);
                let (r, h) = self.pawn_size(pawn);
                if self.spec_fits(&spec, r, h, flags) && !self.spec_blocked_by_door(pawn, from, to) {
                    return true;
                }
            }
        }
        false
    }

    /// `APawn::breadthPathFrom` (103dcd60): the backwards search from `start` over `upstreamPaths` until a start point
    /// (`bEndPoint`) is met. Gives up after 5 nodes with `single`, and after 1000.
    fn breadth_path_from(&mut self, no: &NavOffsets, pawn: u32, start: u32, best: &mut u32, single: bool, flags: i32) -> Res<bool> {
        let (r, h) = self.pawn_size(pawn);
        let is_player = self.bp(pawn, "bIsPlayer");
        let vw = |vm: &Vm, n: u32| vm.u32_at(n, no.visited_weight) as i32;
        let mut marker = start;
        let mut marker_steps = 0;
        let mut iterations = 0;
        let mut count: i32 = 1;
        let mut cur = start;
        loop {
            if cur == 0 {
                return Ok(false);
            }
            if self.nflag(cur, no, no.end_point) {
                self.set_u32_at(start, no.previous_path, 0);
                *best = cur;
                return Ok(true);
            }
            if !self.nflag(cur, no, no.player_only) || is_player || cur == start {
                for idx in self.node_specs(cur, no.upstream) {
                    let Some(spec) = self.reach_specs.get(idx as usize).cloned() else { continue };
                    if !self.spec_fits(&spec, r, h, flags) {
                        continue;
                    }
                    let next = spec.start;
                    if next == 0 {
                        continue;
                    }
                    let bonus = if self.nflag(next, no, no.end_point) { self.ni(next, no.best_path_weight) } else { 0 };
                    let w = bonus.wrapping_add(vw(self, cur)).wrapping_add(self.ni(next, no.cost)).wrapping_add(spec.distance);
                    if vw(self, next) <= w {
                        continue;
                    }
                    let prev = self.u32_at(next, no.prev_ordered);
                    if prev != 0 {
                        // already in the ordered list: take it out
                        let nx = self.u32_at(next, no.next_ordered);
                        self.set_u32_at(prev, no.next_ordered, nx);
                        if nx != 0 {
                            self.set_u32_at(nx, no.prev_ordered, prev);
                        }
                        if marker == next {
                            if vw(self, prev) > w {
                                marker = prev;
                            }
                        } else if vw(self, next) > vw(self, marker) && w < vw(self, marker) {
                            count -= 1;
                        }
                    } else if w <= vw(self, marker) {
                        count -= 1;
                    } else {
                        count += 1;
                    }
                    self.set_u32_at(next, no.previous_path, cur);
                    self.set_ni(next, no.visited_weight, w);
                    let mut pos = if vw(self, marker) < w { marker } else { cur };
                    let mut steps = 0;
                    loop {
                        let nx = self.u32_at(pos, no.next_ordered);
                        if nx == 0 || vw(self, nx) >= w {
                            break;
                        }
                        steps += 1;
                        if steps > 500 {
                            self.log.push("Path list corrupted (breadthPathFrom)".into());
                            return Ok(false);
                        }
                        pos = nx;
                    }
                    let after = self.u32_at(pos, no.next_ordered);
                    if after != next {
                        if after != 0 {
                            self.set_u32_at(after, no.prev_ordered, next);
                        }
                        self.set_u32_at(next, no.next_ordered, after);
                        self.set_u32_at(pos, no.next_ordered, next);
                        self.set_u32_at(next, no.prev_ordered, pos);
                    }
                }
                count += 1;
                let target = ftol(count as f64 * 0.5);
                while marker_steps < target {
                    marker_steps += 1;
                    let nx = self.u32_at(marker, no.next_ordered);
                    if nx != 0 {
                        marker = nx;
                    }
                }
            }
            iterations += 1;
            if single && iterations > 4 {
                return Ok(false);
            }
            if iterations > 1000 {
                self.log.push("Path search over 1000 nodes (breadthPathFrom)".into());
                return Ok(false);
            }
            cur = self.u32_at(cur, no.next_ordered);
        }
    }

    /// 103db0e0: after a search from an unanchored pawn, a nearer start candidate that is on the way (or in another
    /// direction but cheaper), at a similar height, in sight and reachable, replaces `best`.
    fn find_shortcut(&mut self, s: &SortedPathList, pawn: u32, best: &mut u32) -> Res<()> {
        let Some(no) = self.nav_offsets() else { return Ok(()) };
        let o = self.offsets();
        let pl = self.vec_at(pawn, o.location);
        let base = (self.u32_at(s.path[0], no.visited_weight) as i32).wrapping_add(s.dist[0]);
        let mut local = SortedPathList::new();
        for i in 1..s.n.max(1) as usize {
            let node = s.path[i];
            let w = (self.u32_at(node, no.visited_weight) as i32).wrapping_add(ftol((s.dist[i] as f64).sqrt()));
            if w >= base {
                continue;
            }
            let nl = self.vec_at(node, o.location);
            if ((nl[2] - pl[2]) as f64).abs() >= 120.0 {
                continue;
            }
            let bl = self.vec_at(*best, o.location);
            let d = (nl[0] - pl[0]) as f64 * (bl[0] - pl[0]) as f64 + (nl[1] - pl[1]) as f64 * (bl[1] - pl[1]) as f64 + (nl[2] - pl[2]) as f64 * (bl[2] - pl[2]) as f64;
            if d >= 0.0 {
                let t = ftol(base as f64 * 0.85).max(base.wrapping_sub(150));
                if w >= t {
                    continue;
                }
            }
            local.add(node, w);
        }
        let eye = self.eye_of(pawn);
        for j in 0..local.n as usize {
            let node = local.path[j];
            let nl = self.vec_at(node, o.location);
            let visible = self.fast_line(nl, eye);
            let reach = self.point_reachable_kv(pawn, nl, true)?;
            if visible && reach {
                *best = node;
                return Ok(());
            }
        }
        Ok(())
    }

    /// `ReverseRouteFor`: reverses the `previousPath` chain starting at `node`.
    fn reverse_route_for(&mut self, no: &NavOffsets, node: u32) {
        if node == 0 {
            return;
        }
        let mut cur = node;
        let mut prev = 0;
        let mut guard = 0;
        while self.u32_at(cur, no.previous_path) != 0 && guard < 100_000 {
            guard += 1;
            let tmp = self.u32_at(cur, no.previous_path);
            self.set_u32_at(cur, no.previous_path, prev);
            prev = cur;
            cur = tmp;
        }
        self.set_u32_at(cur, no.previous_path, prev);
    }

    /// `APawn::findPathToward(goal, bSinglePath, out best, bClearPaths)`.
    pub fn find_path_toward(&mut self, pawn: u32, goal: u32, single: bool, best: &mut u32, clear: bool) -> Res<bool> {
        *best = 0;
        if goal == 0 || self.level == 0 || self.op(self.level, "NavigationPointList") == 0 || self.reach_specs.is_empty() {
            return Ok(false);
        }
        let Some(no) = self.nav_offsets() else { return Ok(false) };
        let o = self.offsets();
        if self.u8_at(pawn, o.physics) != PHYS_FLYING && self.is_a_named(goal, "Pawn") && self.u8_at(goal, o.physics) == PHYS_FALLING {
            let vel = self.vec_at(goal, o.velocity);
            let landing = self.jump_landing(goal, vel, false)?;
            return self.find_path_to(pawn, landing, single, best, clear);
        }
        let start = self.vec_at(pawn, o.location);
        let goal_loc = self.vec_at(goal, o.location);
        let restore = |vm: &mut Vm| vm.far_move_actor(pawn, start, true, true).map(|_| ());
        let mut s = SortedPathList::new();
        let mut e = SortedPathList::new();
        let (mut start_anchored, mut end_anchored) = (false, false);
        let goal_nav = self.is_a_named(goal, "NavigationPoint");
        if goal_nav {
            e.path[0] = goal;
            e.dist[0] = 0;
            e.n = 1;
            end_anchored = true;
        }
        self.route_init(&no, pawn, goal_loc, &mut s, &mut e, clear, &mut start_anchored, &mut end_anchored)?;
        if s.n == 0 || e.n == 0 {
            return Ok(false);
        }
        let mut anchored_now = start_anchored;
        if !start_anchored {
            if !self.find_start_anchor(&no, &mut s, pawn, &mut start_anchored)? {
                restore(self)?;
                return Ok(false);
            }
            anchored_now = start_anchored;
        }
        if anchored_now {
            let direct = (goal_nav && self.can_move_to(&no, pawn, s.path[0], goal)) || self.anchor_reaches(&mut s, pawn, goal_loc)?;
            if direct {
                *best = if goal_nav { goal } else { s.path[0] };
                restore(self)?;
                return Ok(true);
            }
            self.expand_anchor(&no, &s, pawn)?;
        }
        if !end_anchored {
            let mut i = 0;
            while i < e.n {
                let n = e.path[i as usize];
                let nl = self.vec_at(n, o.location);
                if self.fast_line(goal_loc, nl) && self.far_move_actor(pawn, nl, true, false)? && self.actor_reachable_kv(pawn, goal, true)? {
                    end_anchored = true;
                    e.path[0] = n;
                    e.dist[0] = ftol((e.dist[i as usize] as f64).sqrt());
                    break;
                }
                i += 1;
            }
            if !end_anchored && self.bp(pawn, "bHunting") {
                end_anchored = true;
            }
        }
        if end_anchored {
            let mut b = 0;
            let flags = self.calc_move_flags(pawn);
            self.set_ni(e.path[0], no.visited_weight, e.dist[0]);
            if self.breadth_path_from(&no, pawn, e.path[0], &mut b, single, flags)? {
                *best = b;
                restore(self)?;
                if !start_anchored && !single {
                    self.find_shortcut(&s, pawn, best)?;
                }
                return Ok(true);
            }
            restore(self)?;
            return Ok(false);
        }
        // no end node reaches the goal: search from the start side and reverse the route
        self.route_init(&no, pawn, goal_loc, &mut s, &mut e, clear, &mut start_anchored, &mut end_anchored)?;
        self.expand_anchor(&no, &e, pawn)?;
        if !end_anchored && !self.find_start_anchor(&no, &mut e, pawn, &mut end_anchored)? {
            restore(self)?;
            return Ok(false);
        }
        let mut b = 0;
        let flags = self.calc_move_flags(pawn);
        self.set_ni(s.path[0], no.visited_weight, s.dist[0]);
        if !self.breadth_path_from(&no, pawn, s.path[0], &mut b, single, flags)? {
            restore(self)?;
            return Ok(false);
        }
        restore(self)?;
        let pl = self.vec_at(pawn, o.location);
        let bl = self.vec_at(b, o.location);
        if dist_sq(goal_loc, bl) > dist_sq(goal_loc, pl) {
            return Ok(false);
        }
        *best = b;
        let mut last = b;
        let mut x = b;
        let mut guard = 0;
        while x != 0 && guard < 100_000 {
            guard += 1;
            last = x;
            x = self.u32_at(x, no.previous_path);
        }
        self.reverse_route_for(&no, b);
        *best = last;
        let next = self.u32_at(last, no.previous_path);
        let at_anchor = start_anchored && s.path[0] == last;
        if next == 0 {
            return Ok(!at_anchor);
        }
        let nl = self.vec_at(next, o.location);
        if at_anchor || ((nl[2] - pl[2]) as f64).abs() < 120.0 {
            let eye = self.eye_of(pawn);
            let visible = self.fast_line(nl, eye);
            let reach = self.point_reachable_kv(pawn, nl, true)?;
            if visible && reach {
                *best = next;
            }
            restore(self)?;
        }
        Ok(true)
    }

    /// `APawn::findPathTo(dest, bSinglePath, out best, bClearPaths)`.
    pub fn find_path_to(&mut self, pawn: u32, dest: Vec3, single: bool, best: &mut u32, clear: bool) -> Res<bool> {
        *best = 0;
        if self.level == 0 || self.op(self.level, "NavigationPointList") == 0 || self.reach_specs.is_empty() {
            return Ok(false);
        }
        let Some(no) = self.nav_offsets() else { return Ok(false) };
        let o = self.offsets();
        let start = self.vec_at(pawn, o.location);
        let restore = |vm: &mut Vm| vm.far_move_actor(pawn, start, true, true).map(|_| ());
        let mut s = SortedPathList::new();
        let mut e = SortedPathList::new();
        let (mut start_anchored, mut end_anchored) = (false, false);
        self.route_init(&no, pawn, dest, &mut s, &mut e, clear, &mut start_anchored, &mut end_anchored)?;
        if s.n == 0 || e.n == 0 {
            return Ok(false);
        }
        let mut anchored_now = start_anchored;
        if !start_anchored {
            if !self.find_start_anchor(&no, &mut s, pawn, &mut start_anchored)? {
                restore(self)?;
                return Ok(false);
            }
            anchored_now = start_anchored;
        }
        if anchored_now {
            if self.anchor_reaches(&mut s, pawn, dest)? {
                *best = s.path[0];
                restore(self)?;
                return Ok(true);
            }
            self.expand_anchor(&no, &s, pawn)?;
        }
        if !end_anchored {
            let mut i = 0;
            while i < e.n {
                let n = e.path[i as usize];
                let nl = self.vec_at(n, o.location);
                if self.fast_line(dest, nl) && self.far_move_actor(pawn, nl, true, false)? && self.point_reachable_kv(pawn, dest, true)? {
                    end_anchored = true;
                    e.path[0] = n;
                    e.dist[0] = ftol((e.dist[i as usize] as f64).sqrt());
                    break;
                }
                i += 1;
            }
            if !end_anchored {
                restore(self)?;
                return Ok(false);
            }
        }
        let mut b = 0;
        let flags = self.calc_move_flags(pawn);
        self.set_ni(e.path[0], no.visited_weight, e.dist[0]);
        if !self.breadth_path_from(&no, pawn, e.path[0], &mut b, single, flags)? {
            restore(self)?;
            return Ok(false);
        }
        *best = b;
        restore(self)?;
        if !start_anchored && !single {
            self.find_shortcut(&s, pawn, best)?;
        }
        Ok(true)
    }

    /// `APawn::HandleSpecial`: lets a navigation point with `SpecialHandling` (lifts, teleporters, doors, ...) redirect
    /// the pawn; with `bCanDoSpecial` the redirection becomes `SpecialGoal` and is searched for or checked in turn.
    fn handle_special(&mut self, pawn: u32, best: &mut u32) -> Res<()> {
        let orig = *best;
        let ev = |vm: &mut Vm, on: u32| -> Res<u32> {
            Ok(match vm.send_event(on, "SpecialHandling", vec![Value::Obj(pawn)])? {
                Some(Value::Obj(r)) => r,
                _ => 0,
            })
        };
        let r = ev(self, orig)?;
        *best = r;
        if r == 0 || r == orig {
            return Ok(());
        }
        if self.bp(pawn, "bCanDoSpecial") {
            self.set_u32_prop(pawn, "SpecialGoal", r);
            if !self.actor_reachable_kv(pawn, r, false)? {
                let mut t = 0;
                if self.find_path_toward(pawn, r, false, &mut t, true)? && t != orig {
                    self.set_u32_prop(pawn, "SpecialGoal", r);
                    *best = t;
                    return Ok(());
                }
            } else {
                if !self.is_probing(r, "SpecialHandling") {
                    return Ok(());
                }
                let r2 = ev(self, r)?;
                if r2 != 0 {
                    if r == r2 {
                        return Ok(());
                    }
                    if r != orig && self.actor_reachable_kv(pawn, r2, false)? {
                        *best = r2;
                        return Ok(());
                    }
                }
            }
        }
        *best = 0;
        Ok(())
    }

    /// The `FindPathTo`/`FindPathToward` natives: search, clear `SpecialPause`/`bShootSpecial`, run `HandleSpecial` on a
    /// node that has it, forget a `SpecialGoal` that is the result.
    pub fn native_find_path_to(&mut self, pawn: u32, dest: Vec3, single: bool, clear: bool, toward: Option<u32>) -> Res<u32> {
        let mut b = 0;
        let found = match toward {
            Some(g) => {
                if g == 0 {
                    return Ok(0);
                }
                self.find_path_toward(pawn, g, single, &mut b, clear)?
            }
            None => self.find_path_to(pawn, dest, single, &mut b, clear)?,
        };
        let mut best = if found { b } else { 0 };
        self.set_fp(pawn, "SpecialPause", 0.0);
        self.set_bp(pawn, "bShootSpecial", false);
        if best != 0 && self.is_probing(best, "SpecialHandling") {
            self.handle_special(pawn, &mut best)?;
        }
        if best == self.op(pawn, "SpecialGoal") {
            self.set_u32_prop(pawn, "SpecialGoal", 0);
        }
        Ok(best)
    }

    /// `APawn::jumpLanding`: where an actor falling with `vel` comes down (simulated in 0.1 s steps with the zone's
    /// gravity, fluid friction and current; sliding along walls; done on ground steeper than 0.7, in water, after 35
    /// steps, outside the world or above 1581 units/s, the last three counting as landing where it started). The actor
    /// is put back unless `keep`.
    pub fn jump_landing(&mut self, a: u32, vel: Vec3, keep: bool) -> Res<Vec3> {
        let o = self.offsets();
        let ro = self.region_offsets();
        let start = self.vec_at(a, o.location);
        let dt = 0.1f32;
        let mut v = vel;
        let mut done = false;
        let mut steps = 0;
        loop {
            if done {
                let landing = self.vec_at(a, o.location);
                if !keep {
                    self.far_move_actor(a, start, true, true)?;
                }
                return Ok(landing);
            }
            let zone = self.u32_at(a, ro.region);
            let f = 1.0 - dt * self.fp(zone, "ZoneFluidFriction");
            let g = self.vp(zone, "ZoneGravity");
            let zv = self.vp(zone, "ZoneVelocity");
            v = [v[0] * f + dt * g[0], v[1] * f + dt * g[1], v[2] * f + dt * g[2]];
            let delta = [(v[0] + zv[0]) * dt, (v[1] + zv[1]) * dt, (v[2] + zv[2]) * dt];
            let rot = self.rotation(a);
            let mut hit = CheckResult::none();
            self.move_actor(a, delta, rot, &mut hit, true, true, false, false)?;
            let zone = self.u32_at(a, ro.region);
            if self.bp(zone, "bWaterZone") {
                done = true;
            } else if hit.time < 1.0 {
                if hit.normal[2] as f64 > 0.7 {
                    done = true;
                } else {
                    let old_normal = hit.normal;
                    let n = hit.normal;
                    let d = delta[0] * n[0] + delta[1] * n[1] + delta[2] * n[2];
                    let k = 1.0 - hit.time;
                    let mut adj = [(delta[0] - n[0] * d) * k, (delta[1] - n[1] * d) * k, (delta[2] - n[2] * d) * k];
                    if adj[0] * delta[0] + adj[1] * delta[1] + adj[2] * delta[2] >= 0.0 {
                        self.move_actor(a, adj, rot, &mut hit, true, true, false, false)?;
                        if hit.time < 1.0 {
                            if hit.normal[2] as f64 > 0.7 {
                                done = true;
                            }
                            let dir = safe_normal(delta);
                            crate::physics::two_wall_adjust(dir, &mut adj, hit.normal, old_normal, hit.time);
                            self.move_actor(a, adj, rot, &mut hit, true, true, false, false)?;
                            if hit.normal[2] as f64 > 0.7 {
                                done = true;
                            }
                        }
                    }
                }
            }
            steps += 1;
            if self.zone_number(a) == 0 || steps > 35 || v[0] * v[0] + v[1] * v[1] + v[2] * v[2] > 2_500_000.0 {
                self.far_move_actor(a, start, true, true)?;
                done = true;
            }
        }
    }
}
