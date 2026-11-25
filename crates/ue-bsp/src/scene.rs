//! A scene node's camera (`FSceneNode`): how the engine places a view and maps points to the screen.
//!
//! - `URender::CreateMasterFrame` (Render.dll 10b15690) makes a view's master frame: the viewport's size, recursion 0,
//!   `Mirror` 1, a span buffer for the screen, then `FSceneNode::ComputeRenderCoords` (Engine.dll 10372580) and the
//!   camera's zone. In a game view `ComputeRenderCoords` takes `GMath.ViewCoords` (view x = world y, view y = -world z,
//!   view z = world x), undoes the view rotation with Core's `FCoords /= FRotator` (the engine's sine table) and moves
//!   the origin to the view location; then `ComputeRenderSize` (Engine.dll 10372980). The editor's orthogonal views
//!   (`RendMap` 13-15) have fixed axes and are not ported. `URender::FinishMasterFrame` (10b15930) only gives the scene
//!   memory back.
//! - `URender::Project` (10b20750) and `Deproject` (10b20910): screen position of a world point and back.

use crate::coords::{dot_stack, Coords};
use crate::Vec3;

/// `GMath.ViewCoords` (Core `FGlobalMath`, +0x5c): the view's axes for a zero rotation.
pub const VIEW_COORDS: Coords = Coords { origin: [0.0; 3], x: [0.0, 1.0, 0.0], y: [0.0, 0.0, -1.0], z: [1.0, 0.0, 0.0] };

/// `ComputeRenderCoords`' game path: `GMath.ViewCoords / Rotation`, origin at `location` (`FCoords /= FVector`).
/// The axes are the view's right (`x`), down (`y`) and forward (`z`) directions in the world.
pub fn view_coords(location: Vec3, rotation: [i32; 3]) -> Coords {
    let mut c = VIEW_COORDS;
    c.div_rotator(rotation, ue_assets::light::sin_tab);
    c.origin = [c.origin[0] + location[0], c.origin[1] + location[1], c.origin[2] + location[2]];
    c
}

static WIDE_VIEW: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The widescreen rule for every view of the process (`AerugoSettings.WideView`): off (the default, the original) the
/// field of view spans the whole screen width; on, it is meant for a 4:3 screen and wider screens see more at the
/// sides with the same vertical view ("Hor+").
pub fn set_wide_view(on: bool) {
    WIDE_VIEW.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn wide_view() -> bool {
    WIDE_VIEW.load(std::sync::atomic::Ordering::Relaxed)
}

/// How much wider than `fov` the view's half-width tangent is on a screen of `aspect` (1 without the wide view, or on
/// 4:3 and narrower screens).
pub fn widening(aspect: f32, wide: bool) -> f32 {
    if wide {
        (aspect / (4.0 / 3.0)).max(1.0)
    } else {
        1.0
    }
}

/// Half-width tangent of the view of a horizontal field of view `fov` (degrees) on a screen of `aspect`, under the
/// process's widescreen rule (`set_wide_view`).
pub fn view_tan(fov: f32, aspect: f32) -> f32 {
    (fov.to_radians() * 0.5).tan() * widening(aspect, wide_view())
}

/// The horizontal field of view (degrees) `fov` covers on a screen of `aspect` under the widescreen rule.
pub fn hor_plus_fov(fov: f32, aspect: f32) -> f32 {
    widened_fov(fov, aspect, wide_view())
}

pub fn widened_fov(fov: f32, aspect: f32, wide: bool) -> f32 {
    2.0 * ((fov.to_radians() * 0.5).tan() * widening(aspect, wide)).atan().to_degrees()
}

/// What `FSceneNode::ComputeRenderSize` derives from the screen size and the field of view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderSize {
    pub fx: f32,
    pub fy: f32,
    /// Half size, the projection centre (`FX2`, `FY2`).
    pub fx2: f32,
    pub fy2: f32,
    /// `(FX + 1.0001) / 2` (`FX15`, `FY15`).
    pub fx15: f32,
    pub fy15: f32,
    /// `Proj.Z`: screen units per unit of depth at distance 1.
    pub proj_z: f32,
}

/// `ComputeRenderSize` for a `width` x `height` frame with horizontal field of view `fov` (degrees).
pub fn render_size(width: i32, height: i32, fov: f32) -> RenderSize {
    let (fx, fy) = (width as f32, height as f32);
    let half = 0.5f64;
    let t = (fov as f64 * 0.008726646259971648).tan();
    RenderSize {
        fx,
        fy,
        fx2: (fx as f64 * half) as f32,
        fy2: (fy as f64 * half) as f32,
        fx15: ((fx as f64 + 1.0001) * half) as f32,
        fy15: ((fy as f64 + 1.0001) * half) as f32,
        proj_z: ((fx as f64 * half) / t) as f32,
    }
}

/// A master frame's camera: its coordinates and screen numbers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneView {
    pub coords: Coords,
    pub size: RenderSize,
}

impl SceneView {
    /// `CreateMasterFrame`'s camera for a view at `location` with `rotation` (pitch, yaw, roll), field of view `fov`
    /// (degrees, as `ComputeRenderSize` reads it) and `width` x `height` pixels.
    pub fn master(location: Vec3, rotation: [i32; 3], fov: f32, width: i32, height: i32) -> SceneView {
        SceneView { coords: view_coords(location, rotation), size: render_size(width, height, fov) }
    }

    /// A world point in view space as `TransformVectorBy` stores it (the difference to the origin rounded first).
    pub fn view_space(&self, p: Vec3) -> Vec3 {
        let o = self.coords.origin;
        let d = [p[0] - o[0], p[1] - o[1], p[2] - o[2]];
        [dot_stack(d, self.coords.x), dot_stack(d, self.coords.y), dot_stack(d, self.coords.z)]
    }

    /// `URender::Project` (10b20750) in a perspective view: the screen position of `p` and the scale at its depth
    /// (`Proj.Z / Z`), and whether it lies in front of the camera (`Z > 1`). A depth within 0.01 of zero is moved by
    /// 0.02 first; that depth, the scale and the sums stay in extended precision, the results are stored as floats.
    pub fn project(&self, p: Vec3) -> (bool, f32, f32, f32) {
        let v = self.view_space(p);
        let mut z = v[2] as f64;
        if z.abs() < 0.01 {
            z += 0.02;
        }
        let scale = self.size.proj_z as f64 / z;
        let x = (v[0] as f64 * scale + self.size.fx2 as f64) as f32;
        let y = (v[1] as f64 * scale + self.size.fy2 as f64) as f32;
        (z > 1.0, x, y, scale as f32)
    }

    /// `URender::Deproject` (10b20910) in a perspective view: no point can be recovered from a screen position; the
    /// camera's location and false.
    pub fn deproject(&self, _x: i32, _y: i32) -> (bool, Vec3) {
        (false, self.coords.origin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3, eps: f32) -> bool {
        (0..3).all(|k| (a[k] - b[k]).abs() <= eps)
    }

    /// No rotation: the view looks along world x, right is world y, down is world -z.
    #[test]
    fn a_zero_rotation_gives_the_view_coords() {
        let c = view_coords([10.0, 20.0, 30.0], [0, 0, 0]);
        assert_eq!(c.origin, [10.0, 20.0, 30.0]);
        assert!(close(c.x, [0.0, 1.0, 0.0], 0.0) && close(c.y, [0.0, 0.0, -1.0], 0.0) && close(c.z, [1.0, 0.0, 0.0], 0.0));
    }

    /// Quarter turns (exact entries of the sine table): yaw 16384 looks along world y with world -x on the right;
    /// pitch 16384 looks straight up with world x as "down" on the screen; roll 16384 turns the screen's right into
    /// world down.
    #[test]
    fn quarter_turns() {
        let c = view_coords([0.0; 3], [0, 16384, 0]);
        assert!(close(c.z, [0.0, 1.0, 0.0], 1e-7) && close(c.x, [-1.0, 0.0, 0.0], 1e-7) && close(c.y, [0.0, 0.0, -1.0], 1e-7), "{c:?}");
        let c = view_coords([0.0; 3], [16384, 0, 0]);
        assert!(close(c.z, [0.0, 0.0, 1.0], 1e-7) && close(c.y, [1.0, 0.0, 0.0], 1e-7) && close(c.x, [0.0, 1.0, 0.0], 1e-7), "{c:?}");
        let c = view_coords([0.0; 3], [0, 0, 16384]);
        assert!(close(c.x, [0.0, 0.0, -1.0], 1e-7) && close(c.y, [0.0, -1.0, 0.0], 1e-7) && close(c.z, [1.0, 0.0, 0.0], 1e-7), "{c:?}");
    }

    /// Any rotation: orthonormal, right-handed as the engine's view (x cross y = z for right, down, forward, which
    /// is a left-handed world frame: determinant -1 in world terms), and the forward axis is the rotator's direction
    /// (`cos p cos y, cos p sin y, sin p` from the table).
    #[test]
    fn axes_are_orthonormal_and_forward_is_the_rotator_direction() {
        let sin = ue_assets::light::sin_tab;
        let cos = |a: i32| sin(a.wrapping_add(16384));
        for &(p, y, r) in &[(1000, 2000, 300), (-5000, 40000, -12000), (15000, 65000, 777), (60000, 123, 32768)] {
            let c = view_coords([0.0; 3], [p, y, r]);
            for a in [c.x, c.y, c.z] {
                assert!(((a[0] * a[0] + a[1] * a[1] + a[2] * a[2]) - 1.0).abs() < 1e-5);
            }
            let d = |a: Vec3, b: Vec3| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
            assert!(d(c.x, c.y).abs() < 1e-6 && d(c.y, c.z).abs() < 1e-6 && d(c.x, c.z).abs() < 1e-6);
            let fwd = [cos(p) * cos(y), cos(p) * sin(y), sin(p)];
            assert!(close(c.z, fwd, 2e-6), "{:?} {fwd:?}", c.z);
        }
    }

    /// `ComputeRenderSize` numbers for 800 x 600 at 90 degrees: half size 400/300, `Proj.Z` = 400 / tan 45.
    #[test]
    fn render_size_of_a_4_3_screen() {
        let s = render_size(800, 600, 90.0);
        assert_eq!((s.fx2, s.fy2), (400.0, 300.0));
        assert_eq!((s.fx15, s.fy15), (400.50005, 300.50005));
        assert!((s.proj_z - 400.0).abs() < 1e-3);
        // a 4:3 screen keeps the field of view; 16:9 widens it (tan 45 * 4/3 -> 106.26 degrees)
        assert!((widened_fov(90.0, 4.0 / 3.0, true) - 90.0).abs() < 1e-4);
        assert!((widened_fov(90.0, 16.0 / 9.0, true) - 106.260_2).abs() < 1e-3);
        assert!((widened_fov(90.0, 16.0 / 9.0, false) - 90.0).abs() < 1e-4);
    }

    /// `Project`: a point straight ahead lands on the centre with scale `Proj.Z / Z`; one 100 units right at depth 400
    /// lands `400 * 100 / 400` = 100 pixels right; behind the camera (or within one unit) it is not visible; a depth of
    /// 0.005 is moved to 0.025 before dividing.
    #[test]
    fn project_maps_view_space_to_the_screen() {
        let v = SceneView::master([0.0; 3], [0, 0, 0], 90.0, 800, 600);
        let (vis, x, y, s) = v.project([400.0, 0.0, 0.0]);
        assert!(vis);
        assert_eq!((x, y), (400.0, 300.0));
        assert!((s - 1.0).abs() < 1e-6);
        let (_, x, y, _) = v.project([400.0, 100.0, -50.0]);
        assert!((x - 500.0).abs() < 1e-3 && (y - 350.0).abs() < 1e-3);
        assert!(!v.project([-10.0, 0.0, 0.0]).0);
        assert!(!v.project([1.0, 0.0, 0.0]).0);
        let (vis, x, _, s) = v.project([0.005, 1.0, 0.0]);
        assert!(!vis);
        let want = v.size.proj_z as f64 / (0.005f32 as f64 + 0.02);
        assert_eq!(s, want as f32);
        assert_eq!(x, (1.0 * want + 400.0) as f32);
        assert_eq!(v.deproject(10, 20), (false, [0.0; 3]));
    }
}
