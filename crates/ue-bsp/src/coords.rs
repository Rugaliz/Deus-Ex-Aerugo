//! `FCoords` (Core.dll): an origin and three axes. `TransformPointBy` gives `((p - origin) . x, . y, . z)`; the
//! engine builds an actor's frame by chaining `*=` with translations, rotators and scales, and undoes one with `/=`.

use crate::Vec3;

/// The dot product as Core's `TransformVectorBy` (10119e40) runs it on the x87 stack: `(x*a.x + y*a.y) + z*a.z` in
/// extended precision (double here), rounded once when stored.
pub(crate) fn dot_stack(v: Vec3, a: Vec3) -> f32 {
    ((v[0] as f64 * a[0] as f64 + v[1] as f64 * a[1] as f64) + v[2] as f64 * a[2] as f64) as f32
}

/// `FVector::SafeNormal` (Core 10144be0): zero when the squared length is below 1e-8, else each component times
/// `1 / sqrt(len^2)`. The squared length, root and scale stay in extended precision (double here); only the
/// components are rounded to floats.
pub fn safe_normal(v: Vec3) -> Vec3 {
    let (x, y, z) = (v[0] as f64, v[1] as f64, v[2] as f64);
    let sq = x * x + y * y + z * z;
    if sq < 1e-8 {
        return [0.0; 3];
    }
    let s = 1.0 / sq.sqrt();
    [(s * x) as f32, (s * y) as f32, (s * z) as f32]
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coords {
    pub origin: Vec3,
    pub x: Vec3,
    pub y: Vec3,
    pub z: Vec3,
}

/// `FSheerSnap` (1030f240, inlined the same in Core's `FCoords::operator*=(FScale)`): the sheer rate of a scale,
/// snapped (double constants).
pub fn sheer_snap(s: f32) -> f32 {
    let d = s as f64;
    (if d < -0.65 {
        d + 0.15
    } else if d > 0.65 {
        d - 0.15
    } else if d < -0.55 {
        -0.5
    } else if d > 0.55 {
        0.5
    } else if d < -0.05 {
        d + 0.05
    } else if d > 0.05 {
        d - 0.05
    } else {
        0.0
    }) as f32
}

impl Coords {
    pub const UNIT: Coords = Coords { origin: [0.0; 3], x: [1.0, 0.0, 0.0], y: [0.0, 1.0, 0.0], z: [0.0, 0.0, 1.0] };

    /// `FVector::TransformVectorBy` (10102225).
    pub fn transform_vector(&self, v: Vec3) -> Vec3 {
        [dot_stack(v, self.x), dot_stack(v, self.y), dot_stack(v, self.z)]
    }

    /// `FVector::TransformPointBy` (10101bc2).
    pub fn transform_point(&self, p: Vec3) -> Vec3 {
        self.transform_vector([p[0] - self.origin[0], p[1] - self.origin[1], p[2] - self.origin[2]])
    }

    /// `FPlane::TransformPlaneByOrtho` (101015eb): the normal through the axes, `W - (Origin.TransformVectorBy(C) | N')`.
    pub fn transform_plane(&self, p: [f32; 4]) -> [f32; 4] {
        let n = [p[0], p[1], p[2]];
        let t = |a: Vec3| a[0] * n[0] + a[2] * n[2] + a[1] * n[1];
        let nn = [t(self.x), t(self.y), t(self.z)];
        let o = self.transform_vector(self.origin);
        [nn[0], nn[1], nn[2], p[3] - (o[0] * nn[0] + o[1] * nn[1] + o[2] * nn[2])]
    }

    /// `FCoords::operator*=(FCoords)` (101027f7): origin and axes taken through `t`.
    pub fn mul(&mut self, t: &Coords) {
        self.origin = t.transform_point(self.origin);
        self.x = t.transform_vector(self.x);
        self.y = t.transform_vector(self.y);
        self.z = t.transform_vector(self.z);
    }

    /// `FCoords::operator*=(FVector)`: `Origin -= v`.
    pub fn translate(&mut self, v: Vec3) {
        self.origin = [self.origin[0] - v[0], self.origin[1] - v[1], self.origin[2] - v[2]];
    }

    /// `FCoords::operator*=(FRotator)` (1011ac80): yaw, then pitch, then roll frames; `sin` is `GMath.SinTab`
    /// (cosine = sine of angle + 16384). `rot` is pitch, yaw, roll.
    pub fn mul_rotator(&mut self, rot: [i32; 3], sin: impl Fn(i32) -> f32) {
        let sc = |a: i32| (sin(a), sin(a.wrapping_add(0x4000)));
        let (sy, cy) = sc(rot[1]);
        self.mul(&Coords { origin: [0.0; 3], x: [cy, sy, 0.0], y: [-sy, cy, 0.0], z: [0.0, 0.0, 1.0] });
        let (sp, cp) = sc(rot[0]);
        self.mul(&Coords { origin: [0.0; 3], x: [cp, 0.0, sp], y: [0.0, 1.0, 0.0], z: [-sp, 0.0, cp] });
        let (sr, cr) = sc(rot[2]);
        self.mul(&Coords { origin: [0.0; 3], x: [1.0, 0.0, 0.0], y: [0.0, cr, -sr], z: [0.0, sr, cr] });
    }

    /// `FCoords::operator*=(FScale)` (1010256d): the snapped sheer as a unit frame with one off-diagonal entry
    /// (axis 1 X.y, 2 X.z, 3 Y.x, 4 Y.z, 5 Z.x, 6 Z.y), then every axis times the scale and the origin divided by it.
    pub fn mul_scale(&mut self, scale: Vec3, sheer_rate: f32, sheer_axis: u8) {
        let s = sheer_snap(sheer_rate);
        let mut t = Coords::UNIT;
        match sheer_axis {
            1 => t.x[1] = s,
            2 => t.x[2] = s,
            3 => t.y[0] = s,
            4 => t.y[2] = s,
            5 => t.z[0] = s,
            6 => t.z[1] = s,
            _ => {}
        }
        self.mul(&t);
        for a in [&mut self.x, &mut self.y, &mut self.z] {
            *a = [a[0] * scale[0], a[1] * scale[1], a[2] * scale[2]];
        }
        self.origin = [self.origin[0] / scale[0], self.origin[1] / scale[1], self.origin[2] / scale[2]];
    }

    /// `FCoords::operator/=(FRotator)` (Core 1011b690): roll, then pitch, then yaw undone, each the transposed frame
    /// of `mul_rotator`'s chained through `*=`.
    pub fn div_rotator(&mut self, rot: [i32; 3], sin: impl Fn(i32) -> f32) {
        let sc = |a: i32| (sin(a), sin(a.wrapping_add(0x4000)));
        let (sr, cr) = sc(rot[2]);
        self.mul(&Coords { origin: [0.0; 3], x: [1.0, 0.0, 0.0], y: [0.0, cr, sr], z: [0.0, -sr, cr] });
        let (sp, cp) = sc(rot[0]);
        self.mul(&Coords { origin: [0.0; 3], x: [cp, 0.0, -sp], y: [0.0, 1.0, 0.0], z: [sp, 0.0, cp] });
        let (sy, cy) = sc(rot[1]);
        self.mul(&Coords { origin: [0.0; 3], x: [cy, -sy, 0.0], y: [sy, cy, 0.0], z: [0.0, 0.0, 1.0] });
    }

    /// `FCoords::operator/=(FScale)` (Core 1011c860): every axis divided by the scale and the origin times it, then
    /// the snapped sheer negated as a unit frame (same entries as `mul_scale`) chained through `*=`.
    pub fn div_scale(&mut self, scale: Vec3, sheer_rate: f32, sheer_axis: u8) {
        for a in [&mut self.x, &mut self.y, &mut self.z] {
            *a = [a[0] / scale[0], a[1] / scale[1], a[2] / scale[2]];
        }
        self.origin = [scale[0] * self.origin[0], scale[1] * self.origin[1], self.origin[2] * scale[2]];
        let s = -sheer_snap(sheer_rate);
        let mut t = Coords::UNIT;
        match sheer_axis {
            1 => t.x[1] = s,
            2 => t.x[2] = s,
            3 => t.y[0] = s,
            4 => t.y[2] = s,
            5 => t.z[0] = s,
            6 => t.z[1] = s,
            _ => {}
        }
        self.mul(&t);
    }

    /// The transpose as `ABrush::BuildCoords` (Engine 1030202c) builds it inline: the axes' columns as rows, the
    /// origin `-(Origin.TransformVectorBy(self))`.
    pub fn transpose(&self) -> Coords {
        let o = self.transform_vector(self.origin);
        Coords {
            origin: [-o[0], -o[1], -o[2]],
            x: [self.x[0], self.y[0], self.z[0]],
            y: [self.x[1], self.y[1], self.z[1]],
            z: [self.x[2], self.y[2], self.z[2]],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sheer_snap_bands() {
        assert_eq!(sheer_snap(0.0), 0.0);
        assert_eq!(sheer_snap(0.04), 0.0);
        // 0.05f widened to a double is just above 0.05
        assert_eq!(sheer_snap(0.05), (0.05f32 as f64 - 0.05) as f32);
        assert_eq!(sheer_snap(0.3), (0.3f32 as f64 - 0.05) as f32);
        assert_eq!(sheer_snap(0.6), 0.5);
        assert_eq!(sheer_snap(-0.6), -0.5);
        assert_eq!(sheer_snap(1.0), 0.85);
        assert_eq!(sheer_snap(-1.0), -0.85);
    }

    #[test]
    fn a_translated_yawed_frame_maps_points_and_planes() {
        // exact table values for a quarter turn: sin = 1, cos = 0
        let sin = |a: i32| match (a >> 2) & 0x3fff { 0 => 0.0, 0x1000 => 1.0, 0x2000 => 0.0, 0x3000 => -1.0, _ => f32::NAN };
        let mut c = Coords::UNIT;
        c.translate([-100.0, 0.0, 0.0]);
        c.mul_rotator([0, 0x4000, 0], sin);
        // `C *= T` maps p to C(T^-1(p)): the inverse quarter turn takes x to y, then the origin shift subtracts 100 x
        assert_eq!(c.transform_point([1.0, 0.0, 0.0]), [-100.0, 1.0, 0.0]);
        assert_eq!(c.transform_vector([1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]);
        // the plane x = 0 goes to y = 0, and its points land on it
        let p = c.transform_plane([1.0, 0.0, 0.0, 0.0]);
        assert_eq!(p, [0.0, 1.0, 0.0, 0.0]);
        let q = c.transform_point([0.0, 5.0, 7.0]);
        assert_eq!(q[0] * p[0] + q[1] * p[1] + q[2] * p[2], p[3]);
    }

    fn sin(a: i32) -> f32 {
        ((a & 0xffff) as f32 * std::f32::consts::TAU / 65536.0).sin()
    }

    fn dot(a: Vec3, b: Vec3) -> f32 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    #[test]
    fn transform_vector_sums_x_and_y_first_on_the_stack() {
        // (1 + 1e-8) - 1 keeps the small term in extended precision; summed z first in floats it would vanish
        let c = Coords { origin: [0.0; 3], x: [1.0, 1.0, 1.0], y: [0.0; 3], z: [0.0; 3] };
        assert_eq!(c.transform_vector([1.0, 1e-8, -1.0])[0], 1e-8);
    }

    #[test]
    fn dividing_by_a_rotator_undoes_it() {
        let mut c = Coords::UNIT;
        c.mul_rotator([3000, 12000, -7000], sin);
        c.div_rotator([3000, 12000, -7000], sin);
        for (a, b) in [(c.x, [1.0, 0.0, 0.0]), (c.y, [0.0, 1.0, 0.0]), (c.z, [0.0, 0.0, 1.0])] {
            for k in 0..3 {
                assert!((a[k] - b[k]).abs() < 1e-5, "{a:?}");
            }
        }
    }

    #[test]
    fn build_coords_vector_frame_keeps_dot_products() {
        // the vector frame is the point frame's inverse transpose: (P u) . (V w) = u . w, with sheer and mirroring
        let (ms, ps, rot) = ([1.5, -0.75, 2.0], [1.25, 1.0, 0.5], [4000, -20000, 9000]);
        let mut point = Coords::UNIT;
        point.mul_scale(ps, 0.3, 4);
        point.mul_rotator(rot, sin);
        point.mul_scale(ms, -0.7, 2);
        let mut inv = Coords::UNIT;
        inv.div_scale(ms, -0.7, 2);
        inv.div_rotator(rot, sin);
        inv.div_scale(ps, 0.3, 4);
        let vector = inv.transpose();
        for (u, w) in [([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]), ([0.3, -2.0, 1.0], [1.0, 0.5, -0.25]), ([0.0, 0.0, 1.0], [0.0, 0.0, 1.0])] {
            let d = dot(point.transform_vector(u), vector.transform_vector(w));
            assert!((d - dot(u, w)).abs() < 1e-4, "{d} vs {}", dot(u, w));
        }
    }
}
