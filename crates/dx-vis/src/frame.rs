//! `FSceneNode`: the view a visibility pass works in (camera coordinates, screen size, projection), set up as
//! `FSceneNode::ComputeRenderSize` (Engine.dll 10302d6f) does; child frames (sky zone, mirrors) as
//! `URender::CreateChildFrame` (Render.dll 10b15a40) makes them.

use crate::span::SpanBuffer;

pub type Vec3 = [f32; 3];

fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[derive(Debug, Clone)]
pub struct Frame {
    /// Screen size in pixels (`X`, `Y`).
    pub x: i32,
    pub y: i32,
    pub fx: f32,
    pub fy: f32,
    /// Half size and the projection centre (`FX2`, `FY2`, `FX15`, `FY15`).
    pub fx2: f32,
    pub fy2: f32,
    pub fx15: f32,
    pub fy15: f32,
    /// Projection distance (`Proj.Z`): screen units per unit of depth at distance 1.
    pub proj_z: f32,
    /// Side plane slopes (`PrjXM`, `PrjYM`, `PrjXP`, `PrjYP`): a view-space point is inside when
    /// `z * PrjXM + x >= 0`, `z * PrjYM + y >= 0`, `z * PrjXP - x >= 0`, `z * PrjYP - y >= 0`.
    pub prj_xm: f32,
    pub prj_ym: f32,
    pub prj_xp: f32,
    pub prj_yp: f32,
    /// Camera: origin and axes (x to the right, y down the screen, z forward).
    pub origin: Vec3,
    pub x_axis: Vec3,
    pub y_axis: Vec3,
    pub z_axis: Vec3,
    /// Extra clip plane in view space (`NearClip`): points with `n . v - w < 0` are cut away. Used only when `w` is
    /// not 0 (the main view has all zeros); mirrors set it to the mirror plane.
    pub near_clip: [f32; 4],
    /// Zone the camera is in (`ZoneNumber`): the zone whose span buffer starts as `span`.
    pub zone: u8,
    /// Surface that opened this frame (`iSurf`; -1 for the main view, 0 for sky and mirror frames).
    pub i_surf: i32,
    /// Depth of child frames (`Recursion`): 0 for the main view.
    pub recursion: i32,
    /// 1, or -1 in an odd number of mirrors (`Mirror`): surface windings flip.
    pub mirror: f32,
    /// What the frame may draw into (`Span`): the whole screen for the main view, the opening surfaces' visible
    /// spans for a child.
    pub span: SpanBuffer,
}

impl Frame {
    /// A frame for a camera at `origin` looking along `forward` with `right`/`up`, horizontal field of view `fov`
    /// (degrees), `width` x `height` pixels.
    pub fn new(origin: Vec3, forward: Vec3, right: Vec3, up: Vec3, fov: f32, width: i32, height: i32) -> Frame {
        let ue_bsp::scene::RenderSize { fx, fy, fx2, fy2, fx15, fy15, proj_z } = ue_bsp::scene::render_size(width, height, fov);
        let rz = 1.0 / proj_z;
        Frame {
            x: width,
            y: height,
            fx,
            fy,
            fx2,
            fy2,
            fx15,
            fy15,
            proj_z,
            prj_xm: fx2 * rz,
            prj_ym: fy2 * rz,
            prj_xp: (fx - fx2) * rz,
            prj_yp: (fy - fy2) * rz,
            origin,
            x_axis: right,
            y_axis: [-up[0], -up[1], -up[2]],
            z_axis: forward,
            near_clip: [0.0; 4],
            zone: 0,
            i_surf: -1,
            recursion: 0,
            mirror: 1.0,
            span: SpanBuffer::for_screen(width, height),
        }
    }

    /// `URender::CreateMasterFrame` (Render.dll 10b15690) for a game view at `location` with `rotation` (pitch, yaw, roll):
    /// the camera's coordinates from `ComputeRenderCoords` (`GMath.ViewCoords /= Rotation`, the engine's sine table) and
    /// `ComputeRenderSize`'s numbers for `fov` and `width` x `height` (`ue_bsp::scene`). The zone is the caller's.
    pub fn master(location: Vec3, rotation: [i32; 3], fov: f32, width: i32, height: i32) -> Frame {
        let c = ue_bsp::scene::view_coords(location, rotation);
        Frame::new(c.origin, c.z, c.x, [-c.y[0], -c.y[1], -c.y[2]], fov, width, height)
    }

    /// `CreateChildFrame`'s new frame: the parent's screen and projection, the given camera, one recursion deeper,
    /// the side slopes narrowed to the opening surface's pixel box `bounds` (min x, min y, max x, max y), an empty
    /// span buffer the caller merges into.
    pub fn child(parent: &Frame, zone: u8, i_surf: i32, mirror: f32, near_clip: [f32; 4], coords: [Vec3; 4], bounds: Option<[f32; 4]>) -> Frame {
        let mut f = parent.clone();
        f.zone = zone;
        f.i_surf = i_surf;
        f.recursion = parent.recursion + 1;
        f.mirror = mirror;
        f.near_clip = near_clip;
        f.origin = coords[0];
        f.x_axis = coords[1];
        f.y_axis = coords[2];
        f.z_axis = coords[3];
        // ComputeRenderSize again gives the parent's values (same screen and field of view)
        let rz = 1.0 / f.proj_z;
        f.prj_xm = f.fx2 * rz;
        f.prj_ym = f.fy2 * rz;
        f.prj_xp = (f.fx - f.fx2) * rz;
        f.prj_yp = (f.fy - f.fy2) * rz;
        if let Some(b) = bounds {
            f.set_bounds(b, false);
        }
        f.span = SpanBuffer::alloc(0, 0);
        f
    }

    /// The side slopes from a pixel box; `widen` keeps the larger of the old and new slope (a second opening surface
    /// for an existing child frame).
    pub fn set_bounds(&mut self, b: [f32; 4], widen: bool) {
        let rz = 1.0 / self.proj_z;
        let v = [(b[0] - self.fx2) * -rz, (b[1] - self.fy2) * -rz, (b[2] - self.fx2) * rz, (b[3] - self.fy2) * rz];
        let slots = [&mut self.prj_xm, &mut self.prj_ym, &mut self.prj_xp, &mut self.prj_yp];
        for (slot, x) in slots.into_iter().zip(v) {
            *slot = if widen && x <= *slot { *slot } else { x };
        }
    }

    /// `ViewPlanes` (`FSceneNode::ComputeRenderSize`, Engine.dll 10372980): the four planes through the camera along the
    /// screen's edges (from `FX2`, `FY2`, `Proj.Z`, not the side slopes), normals toward the inside; a point is inside
    /// plane `p` when `n . P - w >= 0`. Order: bottom, top, right, left (signs -1, +1 of y, then of x).
    pub fn view_planes(&self) -> [[f32; 4]; 4] {
        let mut out = [[0.0f32; 4]; 4];
        let rz = 1.0 / self.proj_z;
        for (k, sgn) in [-1.0f32, 1.0].into_iter().enumerate() {
            for (slot, v) in [(k, [0.0f32, sgn / self.fy2, rz]), (k + 2, [sgn / self.fx2, 0.0, rz])] {
                let len = ((v[2] * v[2] + v[1] * v[1]) as f64 + (v[0] * v[0]) as f64).sqrt();
                let inv = 1.0 / len;
                let v = [(inv * v[0] as f64) as f32, (inv * v[1] as f64) as f32, (inv * v[2] as f64) as f32];
                let n: Vec3 = std::array::from_fn(|i| v[2] * self.z_axis[i] + v[1] * self.y_axis[i] + v[0] * self.x_axis[i]);
                let o = self.origin;
                out[slot] = [n[0], n[1], n[2], o[1] * n[1] + o[2] * n[2] + n[0] * o[0]];
            }
        }
        out
    }

    /// World point to view space.
    pub fn to_view(&self, p: Vec3) -> Vec3 {
        let d = [p[0] - self.origin[0], p[1] - self.origin[1], p[2] - self.origin[2]];
        [dot(d, self.x_axis), dot(d, self.y_axis), dot(d, self.z_axis)]
    }
}
