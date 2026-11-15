//! The whole renderer: world + actors into an offscreen target that is either read back as RGBA8 or shown in a window.
use crate::actors::ActorRenderer;
use crate::ui::{UiQuad, UiRenderer};
use crate::world::{Placement, World};
use std::collections::HashMap;
use crate::present::Presenter;
use crate::{Buffer, Context, Offscreen, Result};
use ash::vk;
use std::ffi::c_char;
use dx_soft::{ActorDraw, Camera, MapView};

/// The sky zone of a map: the world seen from its `SkyZoneInfo` shows through fake-backdrop surfaces. As in the original's
/// renderer the view keeps its direction relative to the sky actor's `Rotation` (only yaw is used by the shipped maps).
#[derive(Clone, Copy, Debug)]
pub struct Sky {
    pub pos: [f32; 3],
    pub rot: [i32; 3],
}

impl Sky {
    pub fn new(pos: [f32; 3], rot: [f32; 3]) -> Sky {
        Sky { pos, rot: rot.map(|r| r as i32) }
    }

    /// The camera inside the sky zone for a view of the world.
    pub fn camera(&self, cam: &Camera) -> Camera {
        let sign = std::env::var("DX_SKY_SIGN").ok().and_then(|v| v.parse::<i32>().ok()).unwrap_or(1);
        Camera { pos: self.pos, pitch: cam.pitch, yaw: cam.yaw.wrapping_add(sign * self.rot[1]), fov: cam.fov, roll: cam.roll }
    }
}

/// A child frame (`CreateChildFrame`: the sky zone seen through a backdrop, or a mirror), drawn from its own lists: its
/// level nodes, movers, sprites and its third list (blended surfaces, in the walk's order).
#[derive(Clone)]
pub struct ChildView {
    /// Its index among the frames of the walk (its fog atlas).
    pub frame: usize,
    /// The frame it was opened from: `None` the main view, else the index of another child view (deeper frames: a
    /// mirror seen in a mirror, the sky seen in a mirror).
    pub parent: Option<usize>,
    /// `None`: the sky zone; `Some(plane)`: a mirror, its normal toward the viewer (in the parent's world).
    pub mirror: Option<[f32; 4]>,
    /// For the sky: the sky zone's rotation of the parent's axes (columns: the images of x, y, z) and the sky camera's
    /// origin (the `SkyZoneInfo`); the frame sees `P` where its parent sees `rotᵀ (P - origin) + parent eye`.
    pub sky: Option<([[f32; 3]; 3], [f32; 3])>,
    /// The frame's sprites (decals included), ranked in its sprite list.
    pub actors: Vec<ActorDraw>,
    /// The frame's blended nodes in `DrawFrame`'s order.
    pub blended: Vec<u32>,
    /// Its first list: the mirrors that opened a child frame of its own, in `DrawFrame`'s order.
    pub mirrors: Vec<u32>,
    /// The movers' dynamic nodes the frame drew (their fragments are listed with the main view's).
    pub dynamic: Vec<u32>,
    /// The level nodes the frame drew (any list: their solid and masked faces are drawn with the solid world).
    pub solid: Vec<u32>,
    /// The frame's detail texture layers (D3D's detail pass of its solid surfaces), and by node those of its first
    /// list's surfaces and of its translucent and modulated third-list surfaces.
    pub details: Vec<crate::actors::DetailPoly>,
    pub details_blended: HashMap<u32, Vec<crate::actors::DetailPoly>>,
}

/// Splits a frame's actors as `DrawFrame` draws them on a Direct3D device: decals, the sprites whose style is not
/// translucent, then (after the third list) the translucent-style ones, each group in sprite-list order.
fn frame_phases(actors: &[ActorDraw], cam: &Camera) -> (Vec<ActorDraw>, Vec<ActorDraw>, Vec<ActorDraw>) {
    let (decals, mut sprites): (Vec<ActorDraw>, Vec<ActorDraw>) = actors.iter().cloned().partition(|a| a.decal.is_some());
    let dist = |d: &ActorDraw| (0..3).map(|k| (d.location[k] - cam.pos[k]).powi(2)).sum::<f32>();
    sprites.sort_by(|a, b| a.rank.cmp(&b.rank).then(dist(b).total_cmp(&dist(a))));
    let (translucent, others): (Vec<ActorDraw>, Vec<ActorDraw>) = sprites.into_iter().partition(|a| a.style == 3);
    (decals, others, translucent)
}

/// A 3D view inside the window system's draw list/// A 3D view inside the window system's draw list (a `ViewportWindow`): drawn after the first `after` quads.
#[derive(Clone)]
pub struct SubView {
    pub after: usize,
    /// Order among the 3D commands after the same quad (see `UiActors`).
    pub seq: usize,
    /// Where the whole view lies on the screen (x, y, width, height in pixels) and the visible part of it.
    pub rect: [f32; 4],
    pub clip: [f32; 4],
    pub cam: Camera,
    pub actors: Vec<ActorDraw>,
    /// The coronas of the view's frame (its `DrawFrame` pass), as screen tiles in target pixels.
    pub coronas: Vec<UiQuad>,
    /// What the view's own visibility pass drew (`None`: no pass, the old way).
    pub lists: Option<ViewLists>,
}

/// What a view draws from its own visibility pass besides its sprites: its child frames (sky, mirrors), its first
/// and third lists in `DrawFrame`'s order, the movers' dynamic nodes its master frame drew, and its detail layers
/// (those of the solid surfaces, and by node those drawn with their surface).
#[derive(Clone, Default)]
pub struct ViewLists {
    pub children: Vec<ChildView>,
    pub blended: Vec<u32>,
    pub mirrors: Vec<u32>,
    pub dynamic: Vec<u32>,
    pub details: Vec<crate::actors::DetailPoly>,
    pub details_blended: HashMap<u32, Vec<crate::actors::DetailPoly>>,
}

/// Actors the window system draws by the main camera between its quads (`GC.DrawActor`), or a bare depth clear
/// (`GC.ClearZ`, empty `actors`).
pub struct UiActors {
    pub after: usize,
    pub seq: usize,
    pub clear_z: bool,
    pub actors: Vec<ActorDraw>,
}

/// Draws the world as seen by a sub-view into its rectangle (inside the open rendering scope).
#[allow(clippy::too_many_arguments)]
/// The movers' worlds, where each whole one stands, the dynamic BSP nodes of the listed ones (node -> brush export,
/// fragment) and those the main frame drew.
type Movers<'a> = (&'a HashMap<usize, World>, &'a [(usize, Placement)], &'a HashMap<u32, (usize, usize)>, &'a [u32]);

fn record_subview(cx: &Context, world: &World, movers: Movers, actors_gpu: &mut ActorRenderer, ui: &mut UiRenderer, sky: Option<Sky>, cb: vk::CommandBuffer, target: &Offscreen, v: &SubView) {
    let (tw, th) = (target.width as i32, target.height as i32);
    let x0 = (v.clip[0].floor() as i32).clamp(0, tw);
    let y0 = (v.clip[1].floor() as i32).clamp(0, th);
    let x1 = ((v.clip[0] + v.clip[2]).ceil() as i32).clamp(0, tw);
    let y1 = ((v.clip[1] + v.clip[3]).ceil() as i32).clamp(0, th);
    if x1 <= x0 || y1 <= y0 || v.rect[2] < 1.0 || v.rect[3] < 1.0 {
        return;
    }
    let scissor = vk::Rect2D { offset: vk::Offset2D { x: x0, y: y0 }, extent: vk::Extent2D { width: (x1 - x0) as u32, height: (y1 - y0) as u32 } };
    unsafe {
        cx.device.cmd_set_viewport(cb, 0, // flipped (negative height) like the main scene, so +Y is up in clip space
            &[vk::Viewport { x: v.rect[0], y: v.rect[1] + v.rect[3], width: v.rect[2], height: -v.rect[3], min_depth: 0.0, max_depth: 1.0 }]);
        cx.device.cmd_set_scissor(cb, 0, &[scissor]);
    }
    let clear = |cx: &Context| {
        let att = [vk::ClearAttachment::default().aspect_mask(vk::ImageAspectFlags::DEPTH).clear_value(vk::ClearValue { depth_stencil: vk::ClearDepthStencilValue { depth: 1.0, stencil: 0 } })];
        let rect = [vk::ClearRect::default().rect(scissor).layer_count(1)];
        unsafe { cx.device.cmd_clear_attachments(cb, &att, &rect) };
    };
    clear(cx);
    use_fog(world, movers, 0);
    let aspect = v.rect[2] / v.rect[3];
    // the view's frame is the window's rectangle (`FX` for the meshes' level of detail)
    actors_gpu.frame_x = v.rect[2];
    // its master frame drawn as the main view's: from its own pass's lists (movers from the fragments it drew)
    let (overlay, world_actors): (Vec<ActorDraw>, Vec<ActorDraw>) = v.actors.iter().cloned().partition(|a| a.overlay);
    let none = HashMap::new();
    let lists = match &v.lists {
        Some(l) => l.lists(),
        None => Lists { children: None, blended: None, mirrors: None, dynamic: &[], details: &[], details_blended: &none, whole_movers: true },
    };
    record_view(cx, world, movers, actors_gpu, sky, cb, target, &v.cam, world_actors, aspect, &lists, Some(scissor));
    use_fog(world, movers, 0);
    crate::set_stencil(cx, cb, crate::Stencil::Off);
    // `DrawFrame` ends with the frame's coronas (inside the view), then the viewport is the view's again
    if !v.coronas.is_empty() {
        let drawn = ui.drawn;
        ui.draw_clipped(cx, cb, target.width, target.height, &v.coronas, scissor);
        ui.drawn = drawn;
        unsafe {
            cx.device.cmd_set_viewport(cb, 0, &[vk::Viewport { x: v.rect[0], y: v.rect[1] + v.rect[3], width: v.rect[2], height: -v.rect[3], min_depth: 0.0, max_depth: 1.0 }]);
            cx.device.cmd_set_scissor(cb, 0, &[scissor]);
        }
    }
    if !overlay.is_empty() {
        clear(cx);
        actors_gpu.draw(cx, cb, &v.cam, aspect, &overlay);
    }
}

/// World (with the sky pass) and actors into `target`, inside an open command buffer.
#[allow(clippy::too_many_arguments)]
fn record_scene(cx: &Context, world: &World, movers: Movers, actors_gpu: &mut ActorRenderer, ui: (&mut UiRenderer, &[UiQuad], &[SubView], &[UiActors]), sky: Option<Sky>, cb: vk::CommandBuffer, target: &Offscreen, cam: &Camera, actors: &[ActorDraw], rect: Option<[u32; 4]>, coronas: &[UiQuad], children: Option<&[ChildView]>, rt: RtPass) {
    target.begin(cx, cb, [24.0 / 255.0, 28.0 / 255.0, 40.0 / 255.0, 1.0]);
    // the scene frame (`XRootWindow::PreRender`): the whole target, a render viewport, or nothing at all
    let r = rect.unwrap_or([0, 0, target.width, target.height]);
    let r = [r[0].min(target.width), r[1].min(target.height), r[2].min(target.width.saturating_sub(r[0])), r[3].min(target.height.saturating_sub(r[1]))];
    if r[2] > 0 && r[3] > 0 {
        if rect.is_some() {
            unsafe {
                cx.device.cmd_set_viewport(cb, 0, &[vk::Viewport { x: r[0] as f32, y: (r[1] + r[3]) as f32, width: r[2] as f32, height: -(r[3] as f32), min_depth: 0.0, max_depth: 1.0 }]);
                cx.device.cmd_set_scissor(cb, 0, &[vk::Rect2D { offset: vk::Offset2D { x: r[0] as i32, y: r[1] as i32 }, extent: vk::Extent2D { width: r[2], height: r[3] } }]);
            }
        }
        actors_gpu.frame_x = r[2] as f32;
        record_world(cx, world, movers, actors_gpu, sky, cb, target, cam, actors, r[2] as f32 / r[3] as f32, (&mut *ui.0, coronas, rect), children, rt);
        if rect.is_some() {
            unsafe {
                cx.device.cmd_set_viewport(cb, 0, &[vk::Viewport { x: 0.0, y: target.height as f32, width: target.width as f32, height: -(target.height as f32), min_depth: 0.0, max_depth: 1.0 }]);
                cx.device.cmd_set_scissor(cb, 0, &[vk::Rect2D { offset: vk::Offset2D::default(), extent: vk::Extent2D { width: target.width, height: target.height } }]);
            }
        }
    }
    record_ui(cx, world, movers, actors_gpu, ui, sky, cb, target, cam);
}

/// A view's world as `DrawFrame` draws its master frame: sky, world, child frames, mirrors, movers, detail layers,
/// actors and blended surfaces from the view's lists (`lists.children` `None`: no walk, the old way). `area` limits the
/// depth clears (a viewport window's rectangle).
#[allow(clippy::too_many_arguments)]
fn record_view(cx: &Context, world: &World, movers: Movers, actors_gpu: &mut ActorRenderer, sky: Option<Sky>, cb: vk::CommandBuffer, target: &Offscreen, cam: &Camera, world_actors: Vec<ActorDraw>, aspect: f32, lists: &Lists, area: Option<vk::Rect2D>) {
    match lists.children {
        // `DrawFrame` draws its child frames first, each from its own lists, recursively: the sky zone before the main
        // view (then its depth is forgotten), each mirror inside its own stencil area after the solid world
        Some(ch) => {
            use_fog(world, movers, 0);
            let fc = FrameCtx { cx, world, movers, cb, cam, aspect, children: ch };
            let id = identity();
            if std::env::var_os("DX_NO_SKY").is_none() {
                for k in (0..ch.len()).filter(|&k| ch[k].parent.is_none() && ch[k].mirror.is_none()) {
                    if let Some(a) = child_map(&ch[k], world, cam.pos) {
                        draw_frame(&fc, actors_gpu, k, &a, 0);
                        target.clear_depth_in(cx, cb, area);
                    }
                }
                use_fog(world, movers, 0);
            }
            match world.main_solid.as_deref().filter(|_| std::env::var_os("DX_MAIN_ALL").is_none()) {
                Some(nodes) => world.draw_main_solid(cx, cb, cam, aspect, nodes),
                None => world.draw_pass(cx, cb, cam, aspect, None, crate::world::Pass::Solid),
            }
            if let Some(nodes) = world.solid_mirror_order.as_deref().filter(|n| !n.is_empty()) {
                world.draw_nodes_opaque(cx, cb, cam, aspect, nodes);
            }
            // the movers' listed fragments are part of the frame's own lists: `DrawFrame` draws them over whatever its
            // child frames drew, so they are in depth before the mirrors mark their areas (a door in front of a
            // mirror floor hides the reflection behind it)
            draw_movers_solid(cx, movers, cb, crate::world::view_proj(cam, aspect, 1.0, 65536.0), cam.pos, lists.dynamic);
            let mut reflected = Vec::new();
            for k in (0..ch.len()).filter(|&k| ch[k].parent.is_none() && ch[k].mirror.is_some() && std::env::var_os("DX_NO_MIRRORS").is_none()) {
                if let Some(p) = draw_child(&fc, actors_gpu, k, &id, cam.pos, 0) {
                    reflected.push(p);
                }
            }
            crate::set_stencil(cx, cb, crate::Stencil::Off);
            use_fog(world, movers, 0);
            world.draw_unreflected_mirrors(cx, cb, cam, aspect, &reflected);
            // the first list, after the child frames: translucent mirrors over their reflection, and every mirror's
            // detail layers (opaque ones are drawn invisible: depth only, done with their child frame)
            if let Some(order) = lists.mirrors {
                draw_list2(cx, world, movers, actors_gpu, lists.details_blended, cb, crate::world::view_proj(cam, aspect, 1.0, 65536.0), cam.pos, order);
            }
        }
        None => {
            if let Some(sky) = sky.filter(|_| std::env::var_os("DX_NO_SKY").is_none()) {
                // sky pass: the world as seen from the sky zone, same direction, then forget its depth
                world.draw_from(cx, cb, &sky.camera(cam), aspect);
                target.clear_depth_in(cx, cb, area);
            }
            world.draw_pass(cx, cb, cam, aspect, None, crate::world::Pass::Solid);
            // mirrors: mark where each mirror plane's faces are visible, then draw the reflected world and actors there
            let mirrors = world.mirrors_facing(cam);
            let reflected: Vec<[f32; 4]> = mirrors.iter().map(|m| m.1).collect();
            for (k, (h, p)) in mirrors.into_iter().enumerate() {
                let r = k as u32 + 1;
                crate::set_stencil(cx, cb, crate::Stencil::Mark(r));
                world.draw_mirror_mask(cx, cb, cam, aspect, p);
                crate::set_stencil(cx, cb, crate::Stencil::Inside(r));
                let opaque = world.is_opaque_mirror(p);
                if opaque {
                    world.reset_mirror_depth(cx, cb, cam, aspect, p);
                }
                world.draw_mirror(cx, cb, cam, aspect, h, p);
                actors_gpu.draw_with(cx, cb, cam, aspect, &world_actors, Some(crate::world::reflect_matrix([p[0], p[1], p[2]], p[3])));
                if opaque {
                    world.draw_mirror_depth(cx, cb, cam, aspect, p);
                }
            }
            crate::set_stencil(cx, cb, crate::Stencil::Off);
            world.draw_unreflected_mirrors(cx, cb, cam, aspect, &reflected);
        }
    }
    // a mover drawn through the lists: its solid fragments with the solid world (the walk branch drew them before
    // its mirrors); the others whole
    if lists.children.is_none() {
        draw_movers_solid(cx, movers, cb, crate::world::view_proj(cam, aspect, 1.0, 65536.0), cam.pos, lists.dynamic);
    }
    // the device's detail texture pass of the frame's solid surfaces (each right after its surface in the engine;
    // nothing drawn in between can be covered by it, it writes no depth)
    actors_gpu.draw_details(cx, cb, &crate::world::view_proj(cam, aspect, 1.0, 65536.0), lists.details);
    for (export, placement) in movers.1 {
        if let Some(w) = movers.0.get(export).filter(|w| lists.whole_movers || !w.listed()) {
            w.draw_placed(cx, cb, cam, aspect, Some(placement));
        }
    }
    // `DrawFrame` on a Direct3D device (not span-based): the decals with their surfaces; after the solid list the
    // sprites whose actor style is not translucent, then the non-occluding surfaces (back to front), then the
    // translucent-style sprites; each list of sprites in the frame's sprite-list order (`ActorDraw::rank`). Actors
    // without a rank (no visibility result) keep the old distance order.
    if world_actors.iter().any(|a| a.rank != u32::MAX) {
        let (decals, mut sprites): (Vec<ActorDraw>, Vec<ActorDraw>) = world_actors.into_iter().partition(|a| a.decal.is_some());
        actors_gpu.draw(cx, cb, cam, aspect, &decals);
        let dist = |d: &ActorDraw| (0..3).map(|k| (d.location[k] - cam.pos[k]).powi(2)).sum::<f32>();
        sprites.sort_by(|a, b| a.rank.cmp(&b.rank).then(dist(b).total_cmp(&dist(a))));
        let (translucent, others): (Vec<&ActorDraw>, Vec<&ActorDraw>) = sprites.iter().partition(|a| a.style == 3);
        actors_gpu.draw_in_order(cx, cb, cam, aspect, &others);
        draw_blended(cx, world, movers, actors_gpu, cb, cam, aspect, lists);
        actors_gpu.draw_in_order(cx, cb, cam, aspect, &translucent);
    } else {
        actors_gpu.draw(cx, cb, cam, aspect, &world_actors);
        draw_blended(cx, world, movers, actors_gpu, cb, cam, aspect, lists);
    }
}

/// What a view draws from its walk (see `ViewLists`), borrowed.
struct Lists<'a> {
    children: Option<&'a [ChildView]>,
    blended: Option<&'a [u32]>,
    mirrors: Option<&'a [u32]>,
    dynamic: &'a [u32],
    details: &'a [crate::actors::DetailPoly],
    details_blended: &'a HashMap<u32, Vec<crate::actors::DetailPoly>>,
    /// Draw every mover whole (a view without a pass of its own this frame).
    whole_movers: bool,
}

impl ViewLists {
    fn lists(&self) -> Lists<'_> {
        Lists { children: Some(&self.children), blended: Some(&self.blended), mirrors: Some(&self.mirrors), dynamic: &self.dynamic, details: &self.details, details_blended: &self.details_blended, whole_movers: false }
    }
}

/// The world part of the scene: the main view (`record_view` with its lists), the coronas, the depth clear and the
/// overlays.
#[allow(clippy::too_many_arguments)]
fn record_world(cx: &Context, world: &World, movers: Movers, actors_gpu: &mut ActorRenderer, sky: Option<Sky>, cb: vk::CommandBuffer, target: &Offscreen, cam: &Camera, actors: &[ActorDraw], aspect: f32, coronas: (&mut UiRenderer, &[UiQuad], Option<[u32; 4]>), children: Option<&[ChildView]>, rt: RtPass) {
    let (overlay, world_actors): (Vec<ActorDraw>, Vec<ActorDraw>) = actors.iter().cloned().partition(|a| a.overlay);
    let details = std::mem::take(&mut actors_gpu.details);
    let details_blended = std::mem::take(&mut actors_gpu.details_blended);
    let lists = Lists { children, blended: world.blended_order.as_deref(), mirrors: world.mirror_order.as_deref(), dynamic: movers.3, details: &details, details_blended: &details_blended, whole_movers: false };
    record_view(cx, world, movers, actors_gpu, sky, cb, target, cam, world_actors, aspect, &lists, None);
    actors_gpu.details = details;
    actors_gpu.details_blended = details_blended;
    if let Some((post, tlas)) = rt {
        record_rt_post(cx, world, cb, target, cam, aspect, coronas.2, children, post, tlas);
    }
    // `DrawFrame` ends with the coronas: screen tiles over the world (drawn full-target, then the scene frame's
    // viewport is restored for the overlays)
    if !coronas.1.is_empty() {
        coronas.0.draw(cx, cb, target.width, target.height, coronas.1);
        let r = coronas.2.unwrap_or([0, 0, target.width, target.height]);
        unsafe {
            cx.device.cmd_set_viewport(cb, 0, &[vk::Viewport { x: r[0] as f32, y: (r[1] + r[3]) as f32, width: r[2] as f32, height: -(r[3] as f32), min_depth: 0.0, max_depth: 1.0 }]);
            cx.device.cmd_set_scissor(cb, 0, &[vk::Rect2D { offset: vk::Offset2D { x: r[0] as i32, y: r[1] as i32 }, extent: vk::Extent2D { width: r[2], height: r[3] } }]);
        }
    }
    // `URender::DrawWorld` ends with a depth clear before the overlays (the first-person weapon never sinks into walls);
    // the window system draws on that depth too, so its `DrawActor`s show through walls
    target.clear_depth(cx, cb);
    if !overlay.is_empty() {
        actors_gpu.draw(cx, cb, cam, aspect, &overlay);
    }
}

/// What drawing the child frames needs.
struct FrameCtx<'a> {
    cx: &'a Context,
    world: &'a World,
    movers: Movers<'a>,
    cb: vk::CommandBuffer,
    cam: &'a Camera,
    aspect: f32,
    children: &'a [ChildView],
}

/// Binds the fog atlas of walk frame `frame` for the level and every mover world.
fn use_fog(world: &World, movers: Movers, frame: usize) {
    world.use_fog(frame);
    for w in movers.0.values() {
        w.use_fog(frame);
    }
}

fn identity() -> [f32; 16] {
    let mut m = [0.0; 16];
    for k in 0..4 {
        m[k * 5] = 1.0;
    }
    m
}

/// The affine map through which child `c` sees the world in its parent's terms (the parent's eye `parent_eye`): a
/// mirror's reflection (its plane as the world holds it), or the sky's `rotᵀ (P - origin) + parent eye`.
fn child_map(c: &ChildView, world: &World, parent_eye: [f32; 3]) -> Option<[f32; 16]> {
    match (c.mirror, c.sky) {
        (Some(p), _) => world.mirror_plane_near(p).map(|q| crate::world::reflect_matrix([q[0], q[1], q[2]], q[3])),
        (None, Some((rot, origin))) => {
            let mut m = [0.0f32; 16];
            for col in 0..3 {
                for row in 0..3 {
                    m[col * 4 + row] = rot[row][col];
                }
            }
            for row in 0..3 {
                m[12 + row] = parent_eye[row] - (0..3).map(|col| rot[row][col] * origin[col]).sum::<f32>();
            }
            m[15] = 1.0;
            Some(m)
        }
        _ => None,
    }
}

/// Child frame `k` seen through `pre` (its map composed with its parents'), inside stencil level `level` (0: no
/// stencil): its level nodes and movers' fragments, its own children (each in its area), then its sprites and third
/// list, as `DrawFrame`.
fn draw_frame(fc: &FrameCtx, actors_gpu: &mut ActorRenderer, k: usize, pre: &[f32; 16], level: u32) {
    let (cx, cb, world) = (fc.cx, fc.cb, fc.world);
    let c = &fc.children[k];
    let (vp, eye) = crate::world::frame_view(fc.cam, fc.aspect, pre);
    let stencil = if level == 0 { crate::Stencil::Off } else { crate::Stencil::Inside(level) };
    let clip = c.mirror.map(crate::world::mirror_clip).unwrap_or([0.0; 4]);
    let kids: Vec<usize> = (0..fc.children.len()).filter(|&j| fc.children[j].parent == Some(k)).collect();
    // opaque mirrors this frame opened a child for are drawn invisible over their reflection
    let skip: Vec<[f32; 4]> = kids.iter().filter_map(|&j| fc.children[j].mirror.and_then(|p| world.mirror_plane_near(p))).filter(|&q| world.is_opaque_mirror(q)).collect();
    crate::set_stencil(cx, cb, stencil);
    // the frame's own fog maps (made with its camera)
    use_fog(world, fc.movers, c.frame);
    world.draw_solid_nodes_vp(cx, cb, vp, eye, clip, &c.solid, &skip);
    draw_movers_solid(cx, fc.movers, cb, vp, eye, &c.dynamic);
    actors_gpu.draw_details(cx, cb, &vp, &c.details);
    for j in kids {
        draw_child(fc, actors_gpu, j, pre, eye, level);
    }
    crate::set_stencil(cx, cb, stencil);
    use_fog(world, fc.movers, c.frame);
    draw_list2(cx, world, fc.movers, actors_gpu, &c.details_blended, cb, vp, eye, &c.mirrors);
    let (decals, others, translucent) = frame_phases(&c.actors, fc.cam);
    // meshes only beyond the frame's mirror plane: `DrawLodMesh`'s polygons are clipped at the frame's `NearClip`
    // (Render 10b0bab0, after the side planes); sprites are not
    let mesh_clip = c.mirror.unwrap_or([0.0; 4]);
    actors_gpu.clip = mesh_clip;
    actors_gpu.draw_with(cx, cb, fc.cam, fc.aspect, &decals, Some(*pre));
    actors_gpu.draw_with(cx, cb, fc.cam, fc.aspect, &others, Some(*pre));
    actors_gpu.clip = [0.0; 4];
    draw_list2(cx, world, fc.movers, actors_gpu, &c.details_blended, cb, vp, eye, &c.blended);
    actors_gpu.clip = mesh_clip;
    actors_gpu.draw_with(cx, cb, fc.cam, fc.aspect, &translucent, Some(*pre));
    actors_gpu.clip = [0.0; 4];
}

/// Child frame `j` of a frame seen through `parent_pre` (eye `parent_eye`) at stencil level `level`: its opening faces
/// (the mirror's, or the backdrops for the sky) raise the stencil to `level + 1` where they show; under an opaque
/// opening the depth is forgotten (`ClearZ`), the child drawn, the opening's depth written back (drawn invisible), and
/// the area given back. Returns the mirror plane drawn, if any.
fn draw_child(fc: &FrameCtx, actors_gpu: &mut ActorRenderer, j: usize, parent_pre: &[f32; 16], parent_eye: [f32; 3], level: u32) -> Option<[f32; 4]> {
    use crate::world::{FaceUse, Opening};
    let (cx, cb, world) = (fc.cx, fc.cb, fc.world);
    let c = &fc.children[j];
    if level >= 254 {
        return None;
    }
    let a = child_map(c, world, parent_eye)?;
    let plane = c.mirror.and_then(|p| world.mirror_plane_near(p));
    let (opening, opaque) = match plane {
        Some(q) => (Opening::Mirror(q), world.is_opaque_mirror(q)),
        None => (Opening::Backdrop, true),
    };
    let (vp, eye) = crate::world::frame_view(fc.cam, fc.aspect, parent_pre);
    crate::set_stencil(cx, cb, crate::Stencil::Incr(level));
    world.opening_faces(cx, cb, vp, eye, opening, FaceUse::Mask);
    crate::set_stencil(cx, cb, crate::Stencil::Inside(level + 1));
    if opaque {
        world.opening_faces(cx, cb, vp, eye, opening, FaceUse::ResetDepth);
    }
    let pre = crate::actors::mat_mul(parent_pre, &a);
    draw_frame(fc, actors_gpu, j, &pre, level + 1);
    crate::set_stencil(cx, cb, crate::Stencil::Inside(level + 1));
    if opaque {
        world.opening_faces(cx, cb, vp, eye, opening, FaceUse::Depth);
    }
    crate::set_stencil(cx, cb, crate::Stencil::Decr(level + 1));
    world.opening_faces(cx, cb, vp, eye, opening, FaceUse::Unmask);
    crate::set_stencil(cx, cb, if level == 0 { crate::Stencil::Off } else { crate::Stencil::Inside(level) });
    plane
}

/// `DrawFrame`'s third list: the level's blended nodes and the listed movers' fragments in the walk's order.
#[allow(clippy::too_many_arguments)]
fn draw_blended(cx: &Context, world: &World, movers: Movers, actors_gpu: &mut ActorRenderer, cb: vk::CommandBuffer, cam: &Camera, aspect: f32, lists: &Lists) {
    let Some(order) = lists.blended else {
        world.draw_pass(cx, cb, cam, aspect, None, crate::world::Pass::Blended);
        return;
    };
    draw_list2(cx, world, movers, actors_gpu, lists.details_blended, cb, crate::world::view_proj(cam, aspect, 1.0, 65536.0), cam.pos, order);
}

/// A frame's first or third list `order` through `vp` from `eye`: level nodes in runs, mover fragments at their places, and
/// right after a surface its detail layers (`details` by node; `DrawComplexSurface` draws them with the surface).
#[allow(clippy::too_many_arguments)]
fn draw_list2(cx: &Context, world: &World, movers: Movers, actors_gpu: &mut ActorRenderer, details: &HashMap<u32, Vec<crate::actors::DetailPoly>>, cb: vk::CommandBuffer, vp: [f32; 16], eye: [f32; 3], order: &[u32]) {
    let mut run: Vec<u32> = Vec::new();
    for &node in order {
        match movers.2.get(&node) {
            Some(&(export, i)) => {
                if !run.is_empty() {
                    world.draw_nodes_vp(cx, cb, vp, eye, &run);
                    run.clear();
                }
                if let Some(w) = movers.0.get(&export) {
                    w.draw_fragments_vp(cx, cb, vp, eye, &[i], false);
                }
            }
            None => run.push(node),
        }
        if let Some(d) = details.get(&node) {
            if !run.is_empty() {
                world.draw_nodes_vp(cx, cb, vp, eye, &run);
                run.clear();
            }
            actors_gpu.draw_details(cx, cb, &vp, d);
        }
    }
    if !run.is_empty() {
        world.draw_nodes_vp(cx, cb, vp, eye, &run);
    }
}

/// The solid fragments of the listed movers among a frame's drawn dynamic nodes `nodes`, through `vp` from `eye`.
fn draw_movers_solid(cx: &Context, movers: Movers, cb: vk::CommandBuffer, vp: [f32; 16], eye: [f32; 3], nodes: &[u32]) {
    let mut per: HashMap<usize, Vec<usize>> = HashMap::new();
    for n in nodes {
        if let Some(&(export, i)) = movers.2.get(n) {
            per.entry(export).or_default().push(i);
        }
    }
    for (export, idxs) in per {
        if let Some(w) = movers.0.get(&export) {
            w.draw_fragments_vp(cx, cb, vp, eye, &idxs, true);
        }
    }
}

/// The window system's quads, with the 3D views of viewport windows and its `DrawActor`s between them, then the flash.
#[allow(clippy::too_many_arguments)]
fn record_ui(cx: &Context, world: &World, movers: Movers, actors_gpu: &mut ActorRenderer, ui: (&mut UiRenderer, &[UiQuad], &[SubView], &[UiActors]), sky: Option<Sky>, cb: vk::CommandBuffer, target: &Offscreen, cam: &Camera) {
    let aspect = target.width as f32 / target.height as f32;
    ui.0.drawn = 0;
    let mut start = 0;
    enum Cmd<'a> {
        View(&'a SubView),
        Actors(&'a UiActors),
    }
    let mut cmds: Vec<(usize, usize, Cmd)> = ui.2.iter().map(|v| (v.after, v.seq, Cmd::View(v))).chain(ui.3.iter().map(|a| (a.after, a.seq, Cmd::Actors(a)))).collect();
    cmds.sort_by_key(|c| (c.0, c.1));
    for (after, _, cmd) in cmds {
        let end = after.clamp(start, ui.1.len());
        ui.0.draw(cx, cb, target.width, target.height, &ui.1[start..end]);
        start = end;
        match cmd {
            Cmd::View(v) => record_subview(cx, world, movers, actors_gpu, &mut *ui.0, sky, cb, target, v),
            Cmd::Actors(a) => {
                if a.clear_z {
                    target.clear_depth(cx, cb);
                }
                if !a.actors.is_empty() {
                    actors_gpu.frame_x = target.width as f32;
                    actors_gpu.draw(cx, cb, cam, aspect, &a.actors);
                }
            }
        }
    }
    ui.0.draw(cx, cb, target.width, target.height, &ui.1[start..]);
    // the screen flash covers the world and the window system (Deus Ex draws its windows in PostRenderFlash, before
    // the engine's EndFlash)
    ui.0.draw_flash(cx, cb, target.width, target.height);
}

pub struct Renderer {
    pub cx: Context,
    pub world: World,
    pub actors: ActorRenderer,
    pub ui: UiRenderer,
    /// The window system's draw list for this frame (set before `render`/`render_present`); coordinates in target pixels.
    pub ui_quads: Vec<UiQuad>,
    /// The 3D views of viewport windows for this frame (set together with `ui_quads`).
    pub ui_views: Vec<SubView>,
    /// The window system's `DrawActor`s for this frame (set together with `ui_quads`).
    pub ui_actors: Vec<UiActors>,
    /// World lightmap rectangles redone this frame (atlas x, y, w, h, RGBA8); set before `render`/`render_present`.
    pub lightmap_updates: Vec<(u32, u32, u32, u32, Vec<u8>)>,
    /// The main view's child frames from the walk (`None`: the sky and the nearest mirrors drawn the old way).
    pub child_views: Option<Vec<ChildView>>,
    /// The movers drawn through the lists this frame: per brush export its fragments (`None`: every mover whole).
    pub mover_fragments: Option<HashMap<usize, Vec<crate::world::Fragment>>>,
    /// The listed movers' dynamic BSP nodes: brush export and index into its fragments (for the third list's order).
    pub mover_nodes: HashMap<u32, (usize, usize)>,
    /// The dynamic nodes the main frame drew (the others are only seen in child frames).
    pub mover_main: Vec<u32>,
    /// The layout the last frame left the render target's colour image in (`None` before the first frame).
    target_layout: Option<vk::ImageLayout>,
    /// Lightmap rectangles for the movers' worlds (brush export, rectangle).
    pub mover_lightmap_updates: Vec<(usize, (u32, u32, u32, u32, Vec<u8>))>,
    /// Fog atlas rectangles for the movers' worlds (brush export, frame index of the walk, rectangle).
    pub mover_fog_updates: Vec<(usize, usize, (u32, u32, u32, u32, Vec<u8>))>,
    /// Fog atlas rectangles of the child frames (frame index of the walk, rectangle).
    pub child_fog_updates: Vec<(usize, (u32, u32, u32, u32, Vec<u8>))>,
    /// Fog atlas rectangles for this frame (`dx_light::fog_atlas_rect`).
    pub fog_updates: Vec<(u32, u32, u32, u32, Vec<u8>)>,
    target: Option<(Offscreen, Option<Buffer>)>,
    /// Location of the map's `SkyZoneInfo`: surfaces flagged as fake backdrop show the world seen from here with the player's view direction.
    pub sky: Option<Sky>,
    presenter: Option<Presenter>,
    /// Brush models of movers by export index in the map package (build with `add_mover`).
    mover_worlds: HashMap<usize, World>,
    /// Movers to draw this frame: brush export and where it stands. Set before `render`/`render_present`.
    pub movers: Vec<(usize, Placement)>,
    /// The viewport clock in seconds (real time, runs while the game is paused): procedural textures step by it.
    pub time: f64,
    /// The resolution the game renders at in the window (the game's resolution setting); `None` = the window's size.
    /// The picture is scaled to the window keeping its aspect ratio.
    pub render_size: Option<(u32, u32)>,
    /// The main view's scene frame in target pixels (x, y, width, height; `None` = the whole target, an empty one =
    /// no world at all), set before `render`/`render_present`.
    pub main_rect: Option<[u32; 4]>,
    /// Coronas for this frame: screen tiles drawn over the world before the overlays (target pixels).
    pub coronas: Vec<UiQuad>,
    /// Two timestamps around each frame's scene (GPU time) and whether a frame's are waiting to be read.
    timer: vk::QueryPool,
    timer_pending: bool,
    /// The GPU time of the last frame whose timestamps were read, in milliseconds.
    pub gpu_ms: Option<f64>,
    /// CPU time of this renderer's own phases in the last frame (milliseconds), for measurements.
    pub cpu_ms: Vec<(&'static str, f64)>,
    /// The ray tracing (with the ray-traced picture options): this frame's acceleration structures and the screen passes.
    rt: Option<(crate::rt::RtFrame, crate::rt_post::RtPost)>,
}

impl Renderer {
    pub fn new(view: &mut MapView, lighting: Option<&dx_light::MapLighting>, validation: bool) -> Result<Renderer> {
        Renderer::with_extensions(view, lighting, validation, &[])
    }

    /// `instance_extensions`: what the window system needs (from SDL); then call `attach_window` with a surface made from
    /// `renderer.cx.instance`.
    pub fn with_extensions(view: &mut MapView, lighting: Option<&dx_light::MapLighting>, validation: bool, instance_extensions: &[*const c_char]) -> Result<Renderer> {
        Renderer::with_quality(view, lighting, validation, instance_extensions, crate::Quality::ORIGINAL)
    }

    /// `with_extensions` with picture options other than the original's (`Quality`).
    pub fn with_quality(view: &mut MapView, lighting: Option<&dx_light::MapLighting>, validation: bool, instance_extensions: &[*const c_char], quality: crate::Quality) -> Result<Renderer> {
        let cx = Context::new(validation, instance_extensions)?;
        cx.set_quality(quality);
        let world = World::new(&cx, view, lighting)?;
        let actors = ActorRenderer::new(&cx)?;
        let ui = UiRenderer::new(&cx)?;
        let sky = view.find_actor("SkyZoneInfo").map(|(pos, rot)| Sky::new(pos, rot));
        let timer = unsafe { cx.device.create_query_pool(&vk::QueryPoolCreateInfo::default().query_type(vk::QueryType::TIMESTAMP).query_count(2), None)? };
        let rt = if cx.quality().ray_traced() { Renderer::make_rt(&cx)? } else { None };
        Ok(Renderer { cx, world, actors, ui, ui_quads: Vec::new(), ui_views: Vec::new(), ui_actors: Vec::new(), lightmap_updates: Vec::new(), mover_lightmap_updates: Vec::new(), mover_fog_updates: Vec::new(), child_fog_updates: Vec::new(), mover_fragments: None, mover_nodes: HashMap::new(), mover_main: Vec::new(), child_views: None, fog_updates: Vec::new(), target: None, sky, presenter: None, mover_worlds: HashMap::new(), movers: Vec::new(), time: 0.0, render_size: None, main_rect: None, coronas: Vec::new(), target_layout: None, timer, timer_pending: false, gpu_ms: None, cpu_ms: Vec::new(), rt })
    }

    /// Replaces the map being drawn (level travel): the world, the movers and the sky position. Meshes and textures
    /// loaded for actors stay cached.
    pub fn replace_world(&mut self, view: &mut MapView, lighting: Option<&dx_light::MapLighting>) -> Result<()> {
        unsafe {
            let _ = self.cx.device.device_wait_idle();
        }
        let world = World::new(&self.cx, view, lighting)?;
        std::mem::replace(&mut self.world, world).destroy(&self.cx);
        for (_, w) in std::mem::take(&mut self.mover_worlds) {
            w.destroy(&self.cx);
        }
        self.movers.clear();
        self.sky = view.find_actor("SkyZoneInfo").map(|(pos, rot)| Sky::new(pos, rot));
        Ok(())
    }

    fn make_rt(cx: &Context) -> Result<Option<(crate::rt::RtFrame, crate::rt_post::RtPost)>> {
        let Some(frame) = crate::rt::RtFrame::new(cx) else { return Ok(None) };
        match crate::rt_post::RtPost::new(cx)? {
            Some(post) => Ok(Some((frame, post))),
            None => {
                frame.destroy(cx);
                Ok(None)
            }
        }
    }

    /// The picture options in effect.
    pub fn quality(&self) -> crate::Quality {
        self.cx.quality()
    }

    /// Changes the picture options (limited to what the device can do). When they change, the samplers, pipelines and
    /// the scene target are made again: the actor and window-system renderers here (their textures load again as they
    /// are drawn), the world and the movers by the caller giving the map again (`replace_world`). Returns whether the
    /// caller has to.
    pub fn set_quality(&mut self, q: crate::Quality) -> Result<bool> {
        let before = self.cx.quality();
        let now = self.cx.set_quality(q);
        if now == before {
            return Ok(false);
        }
        unsafe {
            let _ = self.cx.device.device_wait_idle();
        }
        let flash = self.ui.flash;
        std::mem::replace(&mut self.actors, ActorRenderer::new(&self.cx)?).destroy(&self.cx);
        std::mem::replace(&mut self.ui, UiRenderer::new(&self.cx)?).destroy(&self.cx);
        self.ui.flash = flash;
        if let Some((t, b)) = self.target.take() {
            t.destroy(&self.cx);
            if let Some(b) = b {
                self.cx.destroy_buffer(b);
            }
        }
        self.target_layout = None;
        if let Some((f, p)) = self.rt.take() {
            f.destroy(&self.cx);
            p.destroy(&self.cx);
        }
        if now.ray_traced() {
            self.rt = Renderer::make_rt(&self.cx)?;
        }
        Ok(true)
    }

    /// Shows frame `index` of an animated surface texture on the world and the movers.
    pub fn set_anim_frame(&mut self, key: &(String, usize), index: usize) {
        self.world.set_anim_frame(key, index);
        for w in self.mover_worlds.values_mut() {
            w.set_anim_frame(key, index);
        }
    }

    /// Draws plain textures (a view whose `RendMap` is not the lit one) or lightmapped ones, on the level and the movers.
    pub fn set_plain_textures(&mut self, on: bool) {
        self.world.plain_textures = on;
        for w in self.mover_worlds.values_mut() {
            w.plain_textures = on;
        }
    }

    pub fn has_mover(&self, brush_export: usize) -> bool {
        self.mover_worlds.contains_key(&brush_export)
    }

    /// Uploads the brush model of a mover with lightmaps made by the caller (`dx_light::bake_brush`).
    pub fn add_mover(&mut self, view: &MapView, brush_export: usize, model: &ue_assets::model::Model, lighting: Option<&dx_light::MapLighting>, maps: Option<dx_light::Lightmaps>) -> Result<()> {
        let w = World::build(&self.cx, view, model, lighting, maps)?;
        self.mover_worlds.insert(brush_export, w);
        Ok(())
    }

    /// Makes the renderer present into a window (`surface` belongs to `self.cx.instance`; `size` in pixels), waiting for
    /// the vertical blank or not (`vsync`).
    pub fn attach_window(&mut self, surface: vk::SurfaceKHR, size: (u32, u32), vsync: crate::present::VSync) -> Result<()> {
        self.presenter = Some(Presenter::new(&self.cx, surface, size, vsync)?);
        Ok(())
    }

    /// Changes the window's vertical sync mode.
    pub fn set_vsync(&mut self, mode: crate::present::VSync) -> Result<()> {
        match self.presenter.as_mut() {
            Some(p) => p.set_vsync(&self.cx, mode),
            None => Ok(()),
        }
    }

    pub fn presenter(&mut self) -> Option<&mut Presenter> {
        self.presenter.as_mut()
    }

    /// Steps the procedural textures of everything about to be drawn and stages their pixels. The GPU must be done with
    /// the previous frame.
    fn update_procedurals(&mut self, view: &mut MapView) {
        let t = self.time;
        self.world.update_procedurals(view, t);
        let rects = std::mem::take(&mut self.lightmap_updates);
        self.world.update_lightmap(&rects);
        let rects = std::mem::take(&mut self.fog_updates);
        self.world.update_fog(&rects);
        for w in self.mover_worlds.values_mut() {
            w.update_procedurals(view, t);
        }
        for (export, w) in self.mover_worlds.iter_mut() {
            let frags = self.mover_fragments.as_ref().map(|m| m.get(export).cloned().unwrap_or_default());
            if w.set_fragments(&self.cx, frags).is_err() {
                let _ = w.set_fragments(&self.cx, None);
            }
        }
        for (export, rect) in std::mem::take(&mut self.mover_lightmap_updates) {
            if let Some(w) = self.mover_worlds.get_mut(&export) {
                w.update_lightmap(std::slice::from_ref(&rect));
            }
        }
        for (export, fi, rect) in std::mem::take(&mut self.mover_fog_updates) {
            if let Some(w) = self.mover_worlds.get_mut(&export) {
                let _ = w.update_frame_fog(&self.cx, fi, std::slice::from_ref(&rect));
            }
        }
        for (fi, rect) in std::mem::take(&mut self.child_fog_updates) {
            let _ = self.world.update_frame_fog(&self.cx, fi, std::slice::from_ref(&rect));
        }
        self.actors.update_procedurals(view, t);
        self.ui.update_procedurals(view, t);
    }

    fn record_procedural_uploads(cx: &Context, world: &mut World, movers: &mut HashMap<usize, World>, actors: &mut ActorRenderer, ui: &mut UiRenderer, cb: vk::CommandBuffer) {
        world.record_procedural_uploads(cx, cb);
        for w in movers.values_mut() {
            w.record_procedural_uploads(cx, cb);
        }
        actors.record_procedural_uploads(cx, cb);
        ui.record_procedural_uploads(cx, cb);
    }

    fn ensure_target(&mut self, width: u32, height: u32, readback: bool) -> Result<()> {
        let stale = self.target.as_ref().map(|(t, b)| (t.width, t.height) != (width, height) || (readback && b.is_none())).unwrap_or(true);
        if stale {
            if let Some((t, b)) = self.target.take() {
                unsafe { self.cx.device.device_wait_idle()? };
                t.destroy(&self.cx);
                if let Some(b) = b {
                    self.cx.destroy_buffer(b);
                }
            }
            let t = Offscreen::new(&self.cx, width, height)?;
            self.target_layout = None;
            let b = if readback { Some(t.readback_buffer(&self.cx)?) } else { None };
            self.target = Some((t, b));
        }
        Ok(())
    }

    /// Loads what this frame draws: the meshes, sprites and detail textures of the main view, its child frames, the
    /// viewport windows' views (and their child frames) and the window system's actors, and the window system's
    /// textures.
    fn prepare_frame(&mut self, view: &mut MapView, actors: &[ActorDraw]) -> Result<()> {
        self.actors.begin_frame();
        self.actors.prepare(&self.cx, view, actors)?;
        let sub_children = self.ui_views.iter().filter_map(|v| v.lists.as_ref()).flat_map(|l| l.children.iter());
        for c in self.child_views.iter().flatten().chain(sub_children) {
            self.actors.prepare(&self.cx, view, &c.actors)?;
            self.actors.prepare_detail_list(&self.cx, view, &c.details)?;
            for v in c.details_blended.values() {
                self.actors.prepare_detail_list(&self.cx, view, v)?;
            }
        }
        self.actors.prepare_details(&self.cx, view)?;
        self.ui.prepare(&self.cx, view, &self.ui_quads)?;
        self.ui.prepare(&self.cx, view, &self.coronas)?;
        for v in &self.ui_views {
            self.ui.prepare(&self.cx, view, &v.coronas)?;
            self.actors.prepare(&self.cx, view, &v.actors)?;
            if let Some(l) = &v.lists {
                self.actors.prepare_detail_list(&self.cx, view, &l.details)?;
                for d in l.details_blended.values() {
                    self.actors.prepare_detail_list(&self.cx, view, d)?;
                }
            }
        }
        for u in &self.ui_actors {
            self.actors.prepare(&self.cx, view, &u.actors)?;
        }
        Ok(())
    }

    /// Reads the GPU time of the last timed frame if its timestamps are ready (`gpu_ms`).
    fn read_timer(&mut self) {
        if !self.timer_pending {
            return;
        }
        let mut t = [0u64; 2];
        let ok = unsafe { self.cx.device.get_query_pool_results(self.timer, 0, &mut t, vk::QueryResultFlags::TYPE_64) };
        if ok.is_ok() {
            self.gpu_ms = Some(t[1].wrapping_sub(t[0]) as f64 * self.cx.timestamp_period as f64 / 1e6);
            self.timer_pending = false;
        }
    }

    fn timer_start(cx: &Context, timer: vk::QueryPool, cb: vk::CommandBuffer) {
        unsafe {
            cx.device.cmd_reset_query_pool(cb, timer, 0, 2);
            cx.device.cmd_write_timestamp(cb, vk::PipelineStageFlags::TOP_OF_PIPE, timer, 0);
        }
    }

    fn timer_end(cx: &Context, timer: vk::QueryPool, cb: vk::CommandBuffer) {
        unsafe { cx.device.cmd_write_timestamp(cb, vk::PipelineStageFlags::BOTTOM_OF_PIPE, timer, 1) };
    }

    /// Draws one frame offscreen the way `render_present` does (no read back) and waits for it: for measurements
    /// (`gpu_ms`, `cpu_ms`).
    pub fn render_offscreen(&mut self, view: &mut MapView, cam: &Camera, actors: &[ActorDraw], width: u32, height: u32) -> Result<()> {
        self.cpu_ms.clear();
        let t = std::time::Instant::now();
        self.ensure_target(width, height, false)?;
        self.prepare_frame(view, actors)?;
        self.cpu_ms.push(("load meshes/textures", t.elapsed().as_secs_f64() * 1000.0));
        let t = std::time::Instant::now();
        self.update_procedurals(view);
        self.cpu_ms.push(("procedurals/uploads", t.elapsed().as_secs_f64() * 1000.0));
        let (target, rb) = self.target.take().unwrap();
        let (cx, world, actors_gpu, sky, timer) = (&self.cx, &mut self.world, &mut self.actors, self.sky, self.timer);
        let (ui, quads, views, ui_actors) = (&mut self.ui, &self.ui_quads, &self.ui_views, &self.ui_actors);
        let movers = &mut self.mover_worlds;
        let placed = &self.movers;
        let rt = &mut self.rt;
        let mut rt_result = Ok(false);
        let mut record_ms = 0.0;
        let t = std::time::Instant::now();
        let r = cx.one_shot(|cb| {
            let t = std::time::Instant::now();
            Renderer::timer_start(cx, timer, cb);
            Renderer::record_procedural_uploads(cx, world, movers, actors_gpu, ui, cb);
            rt_result = record_rt(cx, rt, world, movers, placed, actors_gpu, actors, cam.pos, cb);
            let post = rt_post(rt, &rt_result);
            record_scene(cx, world, (movers, placed, &self.mover_nodes, &self.mover_main), actors_gpu, (ui, quads, views, ui_actors), sky, cb, &target, cam, actors, self.main_rect, &self.coronas, self.child_views.as_deref(), post);
            target.end_for_sampling(cx, cb);
            Renderer::timer_end(cx, timer, cb);
            record_ms = t.elapsed().as_secs_f64() * 1000.0;
        });
        let total = t.elapsed().as_secs_f64() * 1000.0;
        self.cpu_ms.push(("record commands", record_ms));
        self.cpu_ms.push(("submit and wait", total - record_ms));
        self.target = Some((target, rb));
        self.target_layout = Some(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
        r?;
        rt_result.map(|_| ())?;
        self.timer_pending = true;
        self.read_timer();
        Ok(())
    }

    /// The last frame drawn (before the window's gamma), `width * height * 4` RGBA8 bytes, top row first; `None` before
    /// the first frame. Waits for the GPU.
    pub fn read_last_frame(&mut self) -> Result<Option<(u32, u32, Vec<u8>)>> {
        let Some(layout) = self.target_layout else { return Ok(None) };
        let Some((target, rb)) = self.target.take() else { return Ok(None) };
        unsafe { self.cx.device.device_wait_idle()? };
        let own = match &rb {
            Some(_) => None,
            None => Some(target.readback_buffer(&self.cx)?),
        };
        let buf = rb.as_ref().or(own.as_ref()).unwrap();
        let r = self.cx.one_shot(|cb| target.copy_back(&self.cx, cb, buf, layout));
        let pixels = r.and_then(|_| target.read_pixels(&self.cx, buf));
        let size = (target.width, target.height);
        if let Some(b) = own {
            self.cx.destroy_buffer(b);
        }
        self.target = Some((target, rb));
        Ok(Some((size.0, size.1, pixels?)))
    }

    /// Draws one frame and returns `width * height * 4` RGBA8 bytes (top row first).
    pub fn render(&mut self, view: &mut MapView, cam: &Camera, actors: &[ActorDraw], width: u32, height: u32) -> Result<Vec<u8>> {
        self.ensure_target(width, height, true)?;
        self.prepare_frame(view, actors)?;
        self.update_procedurals(view);
        let (target, readback) = self.target.take().unwrap();
        let rb = readback.as_ref().unwrap();
        let (cx, world, actors_gpu, sky) = (&self.cx, &mut self.world, &mut self.actors, self.sky);
        let (ui, quads, views, ui_actors) = (&mut self.ui, &self.ui_quads, &self.ui_views, &self.ui_actors);
        let movers = &mut self.mover_worlds;
        let placed = &self.movers;
        let rt = &mut self.rt;
        let mut rt_result = Ok(false);
        let r = cx.one_shot(|cb| {
            Renderer::record_procedural_uploads(cx, world, movers, actors_gpu, ui, cb);
            rt_result = record_rt(cx, rt, world, movers, placed, actors_gpu, actors, cam.pos, cb);
            let post = rt_post(rt, &rt_result);
            record_scene(cx, world, (movers, placed, &self.mover_nodes, &self.mover_main), actors_gpu, (ui, quads, views, ui_actors), sky, cb, &target, cam, actors, self.main_rect, &self.coronas, self.child_views.as_deref(), post);
            target.end_and_copy(cx, cb, rb);
        });
        let pixels = r.and_then(|_| rt_result.map(|_| ())).and_then(|_| target.read_pixels(&self.cx, rb));
        self.target = Some((target, readback));
        self.target_layout = Some(vk::ImageLayout::TRANSFER_SRC_OPTIMAL);
        pixels
    }

    /// Draws one frame into the attached window. `brightness` is the engine's Brightness setting (0.6 as shipped; see `brightness_exponent`).
    /// `window_size` is the current drawable size in pixels (the swapchain is rebuilt when it changed). Returns false if nothing was drawn
    /// (minimised window or swapchain rebuilt).
    pub fn render_present(&mut self, view: &mut MapView, cam: &Camera, actors: &[ActorDraw], brightness: f32, crosshair: bool, window_size: (u32, u32)) -> Result<bool> {
        let mut presenter = self.presenter.take().expect("attach_window first");
        let result = (|| {
            if window_size != (presenter.extent.width, presenter.extent.height) {
                presenter.resize(&self.cx, window_size)?;
            }
            let Some(frame) = presenter.begin_frame(&self.cx, window_size)? else { return Ok(false) };
            // the previous frame is done (its fence waited for): its GPU time
            self.read_timer();
            let (w, h) = self.render_size.unwrap_or((presenter.extent.width, presenter.extent.height));
            self.ensure_target(w, h, false)?;
            self.prepare_frame(view, actors)?;
            self.update_procedurals(view);
            Renderer::timer_start(&self.cx, self.timer, frame.cb);
            Renderer::record_procedural_uploads(&self.cx, &mut self.world, &mut self.mover_worlds, &mut self.actors, &mut self.ui, frame.cb);
            let rt_result = record_rt(&self.cx, &mut self.rt, &self.world, &self.mover_worlds, &self.movers, &self.actors, actors, cam.pos, frame.cb);
            let post = rt_post(&mut self.rt, &rt_result);
            rt_result?;
            let (target, rb) = self.target.take().unwrap();
            record_scene(&self.cx, &self.world, (&self.mover_worlds, &self.movers, &self.mover_nodes, &self.mover_main), &mut self.actors, (&mut self.ui, &self.ui_quads, &self.ui_views, &self.ui_actors), self.sky, frame.cb, &target, cam, actors, self.main_rect, &self.coronas, self.child_views.as_deref(), post);
            target.end_for_sampling(&self.cx, frame.cb);
            Renderer::timer_end(&self.cx, self.timer, frame.cb);
            self.timer_pending = true;
            self.target_layout = Some(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
            let gamma_exp = crate::brightness_exponent(brightness);
            let r = presenter.end_frame(&self.cx, frame, &target, gamma_exp, crosshair);
            self.target = Some((target, rb));
            r?;
            Ok(true)
        })();
        self.presenter = Some(presenter);
        result
    }

    pub fn destroy(mut self) {
        unsafe {
            let _ = self.cx.device.device_wait_idle();
        }
        if let Some(p) = self.presenter.take() {
            p.destroy(&self.cx);
        }
        unsafe { self.cx.device.destroy_query_pool(self.timer, None) };
        if let Some((t, b)) = self.target.take() {
            t.destroy(&self.cx);
            if let Some(b) = b {
                self.cx.destroy_buffer(b);
            }
        }
        for (_, w) in std::mem::take(&mut self.mover_worlds) {
            w.destroy(&self.cx);
        }
        if let Some((f, p)) = self.rt.take() {
            f.destroy(&self.cx);
            p.destroy(&self.cx);
        }
        self.ui.destroy(&self.cx);
        self.actors.destroy(&self.cx);
        self.world.destroy(&self.cx);
    }
}

/// The screen passes' share of the ray tracing for `record_scene`: the passes and this frame's structure.
type RtPass<'a> = Option<(&'a mut crate::rt_post::RtPost, vk::AccelerationStructureKHR)>;

/// The screen passes when this frame's structures were recorded.
fn rt_post<'a>(rt: &'a mut Option<(crate::rt::RtFrame, crate::rt_post::RtPost)>, recorded: &Result<bool>) -> RtPass<'a> {
    match (rt.as_mut(), recorded) {
        (Some((frame, post)), Ok(true)) => frame.tlas().map(|t| (post, t)),
        _ => None,
    }
}

/// How bright light `(pos, colour, radius)` makes a point at `at` facing it, as `lodmesh::vertex_light` lights a mesh
/// (its luminance, before the actor's scale).
fn light_share(at: [f32; 3], light: &([f32; 3], [f32; 3], f32)) -> f32 {
    let (p, c, r) = light;
    let d = ((p[0] - at[0]).powi(2) + (p[1] - at[1]).powi(2) + (p[2] - at[2]).powi(2)).sqrt();
    let k = 2.5 * (1.0 - d / r.max(1.0));
    if k <= 0.0 {
        return 0.0;
    }
    k * (0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2])
}

/// The most casters the screen passes take (the nearest objects first).
const MAX_CASTERS: usize = 256;
/// Up to how many lights of an object give it a shadow.
const LIGHTS_PER_CASTER: usize = 4;
/// The radius of a light's disc (the softness of the shadows' edges).
const LIGHT_SIZE: f32 = 12.0;

/// This frame's ray tracing (before the scene's rendering starts): the meshes drawn as light blockers, the movers where
/// they stand and the world into the structures; the casters for the screen passes. Returns whether there is
/// something to trace against.
#[allow(clippy::too_many_arguments)]
fn record_rt(cx: &Context, rt: &mut Option<(crate::rt::RtFrame, crate::rt_post::RtPost)>, world: &World, movers: &HashMap<usize, World>, placed: &[(usize, Placement)], actors_gpu: &ActorRenderer, actors: &[ActorDraw], eye: [f32; 3], cb: vk::CommandBuffer) -> Result<bool> {
    let Some((frame, post)) = rt.as_mut() else { return Ok(false) };
    // each mesh its own geometry; its bounds and its lights make its casters
    let (mut tris, mut parts) = (Vec::new(), Vec::new());
    let mut casters: Vec<(f32, crate::rt_post::Caster)> = Vec::new();
    for d in actors {
        let first = tris.len();
        actors_gpu.occluder_triangles(d, &mut tris);
        if tris.len() == first {
            continue;
        }
        parts.push((first as u32, (tris.len() - first) as u32));
        let Some(l) = d.lighting.as_ref() else { continue };
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for v in &tris[first..] {
            for k in 0..3 {
                lo[k] = lo[k].min(v[k]);
                hi[k] = hi[k].max(v[k]);
            }
        }
        let centre: [f32; 3] = std::array::from_fn(|k| (lo[k] + hi[k]) * 0.5);
        let radius = tris[first..].iter().map(|v| (0..3).map(|k| (v[k] - centre[k]).powi(2)).sum::<f32>()).fold(0.0f32, f32::max).sqrt();
        if radius < 1.0 {
            continue;
        }
        let mut shares: Vec<(f32, usize)> = l.lights.iter().enumerate().map(|(i, light)| (light_share(centre, light) * l.scale, i)).filter(|s| s.0 > 0.0).collect();
        let total: f32 = shares.iter().map(|s| s.0).sum::<f32>() + 0.299 * l.ambient[0] + 0.587 * l.ambient[1] + 0.114 * l.ambient[2];
        shares.sort_by(|a, b| b.0.total_cmp(&a.0));
        let dist = ((centre[0] - eye[0]).powi(2) + (centre[1] - eye[1]).powi(2) + (centre[2] - eye[2]).powi(2)).sqrt();
        for &(share, i) in shares.iter().take(LIGHTS_PER_CASTER) {
            let dark = share / total.max(1e-4);
            let (p, _, lr) = l.lights[i];
            let lc = ((p[0] - centre[0]).powi(2) + (p[1] - centre[1]).powi(2) + (p[2] - centre[2]).powi(2)).sqrt();
            if dark < 0.05 || lc < radius * 1.1 + LIGHT_SIZE {
                continue;
            }
            casters.push((
                dist,
                crate::rt_post::Caster { light: [p[0], p[1], p[2], LIGHT_SIZE], centre: [centre[0], centre[1], centre[2], radius], params: [dark.min(1.0), (lr - lc).max(radius * 2.0), 0.0, 0.0] },
            ));
        }
    }
    casters.sort_by(|a, b| a.0.total_cmp(&b.0));
    casters.truncate(MAX_CASTERS);
    let casters: Vec<crate::rt_post::Caster> = casters.into_iter().map(|c| c.1).collect();
    post.set_casters(cx, &casters)?;
    let instances: Vec<(u64, [[f32; 4]; 3])> =
        placed.iter().filter_map(|(b, place)| movers.get(b).and_then(|w| w.rt_blas.as_ref()).map(|a| (a.address, crate::rt::instance_transform(place)))).collect();
    if std::env::var_os("DX_RT_LOG").is_some() {
        eprintln!("RT: {} casters, {} mesh triangles in {} meshes, {} movers, world {}", casters.len(), tris.len() / 3, parts.len(), instances.len(), world.rt_blas.is_some());
    }
    frame.record(cx, cb, world.rt_blas.as_ref().map(|a| a.address), &instances, &tris, &parts)?;
    Ok(frame.tlas().is_some())
}

/// The ray-traced screen passes over the main view just drawn (`rt_post`): the openings to other frames (mirrors, the
/// sky) marked in the stencil and left out (`rt_dist.frag`), the scene's rendering paused for the half-size trace and blur, then the
/// factor multiplied into the picture.
#[allow(clippy::too_many_arguments)]
fn record_rt_post(cx: &Context, world: &World, cb: vk::CommandBuffer, target: &Offscreen, cam: &Camera, aspect: f32, rect: Option<[u32; 4]>, children: Option<&[ChildView]>, post: &mut crate::rt_post::RtPost, tlas: vk::AccelerationStructureKHR) {
    const OPENING: u32 = 0x80;
    let q = cx.quality();
    // `DX_RT_DEBUG`: 1 the multiply alone (by one half), 4 the traced factor unblurred, 8 distance bands, 16 rays from
    // the eye (black: the world, grey: an object), 32 no rays (the passes' fixed cost)
    let debug = std::env::var("DX_RT_DEBUG").ok().and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
    let flags = q.rt_shadows as u32 | (q.rt_ao as u32) << 1 | debug << 4;
    let r = rect.unwrap_or([0, 0, target.width, target.height]);
    let push = crate::rt_post::Push::new(cam, aspect, r, target.width, target.height, post.caster_count, flags);
    if std::env::var_os("DX_RT_LOG").is_some() {
        eprintln!("RT post: rect {r:?}, target {}x{}, {} casters, flags {flags}", target.width, target.height, post.caster_count);
    }
    if let Some(ch) = children {
        use crate::world::{FaceUse, Opening};
        let vp = crate::world::view_proj(cam, aspect, 1.0, 65536.0);
        // a see-through mirror (a shiny floor) is a surface of its own over its reflection: its depth written, it is
        // shaded like the rest; opaque mirrors and the sky show other frames and are left out
        for c in ch.iter().filter(|c| c.parent.is_none()) {
            if let Some(q) = c.mirror.and_then(|p| world.mirror_plane_near(p)).filter(|&q| !world.is_opaque_mirror(q)) {
                world.opening_faces(cx, cb, vp, cam.pos, Opening::Mirror(q), FaceUse::Depth);
            }
        }
        crate::set_stencil(cx, cb, crate::Stencil::Mark(OPENING));
        for c in ch.iter().filter(|c| c.parent.is_none()) {
            let opening = match c.mirror {
                Some(p) => match world.mirror_plane_near(p).filter(|&q| world.is_opaque_mirror(q)) {
                    Some(q) => Opening::Mirror(q),
                    None => continue,
                },
                None => Opening::Backdrop,
            };
            world.opening_faces(cx, cb, vp, cam.pos, opening, FaceUse::MaskEqual);
        }
    }
    target.pause(cx, cb);
    let traced = post.trace(cx, cb, target, tlas, &push);
    target.resume(cx, cb);
    let area = vk::Rect2D { offset: vk::Offset2D { x: r[0] as i32, y: r[1] as i32 }, extent: vk::Extent2D { width: r[2], height: r[3] } };
    unsafe {
        cx.device.cmd_set_viewport(cb, 0, &[vk::Viewport { x: r[0] as f32, y: (r[1] + r[3]) as f32, width: r[2] as f32, height: -(r[3] as f32), min_depth: 0.0, max_depth: 1.0 }]);
        cx.device.cmd_set_scissor(cb, 0, &[area]);
    }
    match traced {
        Ok(()) => {
            post.apply(cx, cb, &push);
        }
        Err(e) => eprintln!("ray tracing: {e}"),
    }
}
