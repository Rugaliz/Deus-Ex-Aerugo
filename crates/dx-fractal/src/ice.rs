//! `UIceTexture` (Fire.dll): a source picture seen through a "glass" texture whose pixels are horizontal
//! displacements. Either the glass slides over a still source (`MoveIce`, `BlitTexIce` 10505e60) or the source slides
//! under still glass (`BlitIceTex` 105061e0); the slide follows `MoveIcePosition` (10505b10). The blitters are
//! hand-written assembly: pixel indices are added and masked as bytes.

/// The state of an ice texture (its script variables, `IceTexture.uc`).
#[derive(Debug, Clone, Default)]
pub struct Ice {
    pub u_size: usize,
    pub v_size: usize,
    pub u_bits: u32,
    pub panning_style: u8,
    /// 0: frame-rate synchronised (`ConstantTimeTick` at the texture's rate, 1/120 s each); 1: real time.
    pub time_method: u8,
    pub horiz_pan_speed: u8,
    pub vert_pan_speed: u8,
    pub frequency: u8,
    pub amplitude: u8,
    pub move_ice: bool,
    pub master_count: f32,
    pub u_displace: f32,
    pub v_displace: f32,
    pub u_position: f32,
    pub v_position: f32,
    pub old_u: i32,
    pub old_v: i32,
    pub force_refresh: bool,
    /// `LocalSource`: the source did not fit, so `PostLoad` copied it (scaled up) into the texture itself and the
    /// blitters do nothing.
    pub local_source: bool,
    pub bitmap: Vec<u8>,
}

/// x87 `fistp` with the default rounding (to nearest, ties to even); out of range gives the integer indefinite.
fn fistp(f: f64) -> i32 {
    let r = f.round_ties_even();
    if r >= i32::MIN as f64 && r <= i32::MAX as f64 {
        r as i32
    } else {
        i32::MIN
    }
}

impl Ice {
    pub fn u_mask(&self) -> usize {
        self.u_size.wrapping_sub(1)
    }

    pub fn v_mask(&self) -> usize {
        self.v_size.wrapping_sub(1)
    }

    /// `MoveIcePosition(dt)`.
    pub fn move_position(&mut self, dt: f32) {
        let master = dt * 120.0 + self.master_count;
        self.master_count = master;
        let hs = (self.horiz_pan_speed.wrapping_add(0x80) as i8) as f32 * dt;
        self.u_displace -= hs + hs;
        let vs = (self.vert_pan_speed.wrapping_add(0x80) as i8) as f32 * dt;
        self.v_displace = vs + vs + self.v_displace;
        let freq = (self.frequency as i32 + 1) as f32 as f64;
        let amp = (self.amplitude as i32 + 1) as f32;
        let wave = |scale: f32, cos: bool| -> f32 {
            let a = freq * master as f64 * scale as f64;
            let s = if cos { a.cos() } else { a.sin() } as f32;
            fistp((amp * s) as f64) as f32
        };
        let half_wave = |cos: bool| -> f32 {
            let a = freq * master as f64 * 0.0012f32 as f64;
            let s = if cos { a.cos() } else { a.sin() } as f32;
            fistp((amp * s * 0.5) as f64) as f32
        };
        match self.panning_style {
            0 => {
                self.u_position = self.u_displace;
                self.v_position = self.v_displace;
            }
            1 => {
                self.u_position = wave(0.0012, false) + self.u_displace;
                self.v_position = wave(0.0012, true) + self.v_displace;
            }
            2 => {
                self.u_position = wave(0.0012, false) + self.u_displace;
                self.v_position = wave(0.0011, true) + self.v_displace;
            }
            3 => {
                self.v_position = self.v_displace;
                self.u_position = half_wave(false) + self.u_displace;
            }
            4 => {
                self.u_position = self.u_displace;
                self.v_position = half_wave(true) + self.v_displace;
            }
            _ => {}
        }
    }

    /// `RenderIce(dt)` with the glass and source pictures (each the texture's own size); nothing without both.
    /// Returns whether the picture was drawn again.
    pub fn render(&mut self, dt: f32, glass: Option<&[u8]>, source: Option<&[u8]>) -> bool {
        let (Some(glass), Some(source)) = (glass, source) else { return false };
        self.move_position(dt);
        let (u, v) = (fistp(self.u_position as f64), fistp(self.v_position as f64));
        if u == self.old_u && v == self.old_v && !self.force_refresh {
            return false;
        }
        self.old_u = u;
        self.old_v = v;
        if self.move_ice {
            self.blit_tex_ice(glass, source);
        } else {
            self.blit_ice_tex(glass, source);
        }
        self.force_refresh = false;
        true
    }

    /// `BlitTexIce`: the glass moves. Row `y` of the source seen through glass row `(y + V) & VMask` shifted by `U`.
    pub fn blit_tex_ice(&mut self, glass: &[u8], source: &[u8]) {
        if self.local_source {
            return;
        }
        let (um, vm) = (self.u_mask(), self.v_mask());
        let u = (fistp(self.u_position as f64) as usize) & um;
        let v = (fistp(self.v_position as f64) as usize) & vm;
        for y in 0..self.v_size {
            let g = ((y + v) & vm) << self.u_bits;
            let row = y << self.u_bits;
            for x in 0..self.u_size {
                let d = glass[g + ((u + x) & um)].wrapping_add(x as u8) & um as u8;
                self.bitmap[row + x] = source[row + d as usize];
            }
        }
    }

    /// `BlitIceTex`: the source moves. Row `(y + V) & VMask` of the source, shifted by `U`, under glass row `y`.
    pub fn blit_ice_tex(&mut self, glass: &[u8], source: &[u8]) {
        if self.local_source {
            return;
        }
        let (um, vm) = (self.u_mask(), self.v_mask());
        let u = (fistp(self.u_position as f64) as usize) & um;
        let v = (fistp(self.v_position as f64) as usize) & vm;
        for y in 0..self.v_size {
            let s = ((y + v) & vm) << self.u_bits;
            let row = y << self.u_bits;
            for x in 0..self.u_size {
                let d = glass[row + x].wrapping_add((x + u) as u8) & um as u8;
                self.bitmap[row + x] = source[s + d as usize];
            }
        }
    }

    /// `PostLoad`'s copy of a source of another size: scaled up by repeating pixels (`du`, `dv` = the bit
    /// differences) into the texture itself. `None` when the source is larger (the engine then drops it).
    pub fn scale_source(u_size: usize, v_size: usize, src: &[u8], src_u_bits: u32, du: u32, dv: u32) -> Vec<u8> {
        let mut out = vec![0u8; u_size * v_size];
        for y in 0..v_size {
            for x in 0..u_size {
                out[u_size * y + x] = src[(x >> du) + ((y >> dv) << src_u_bits)];
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ice(u: usize, v: usize) -> Ice {
        Ice { u_size: u, v_size: v, u_bits: u.trailing_zeros(), bitmap: vec![0; u * v], ..Default::default() }
    }

    #[test]
    fn linear_slide_moves_by_twice_the_signed_speed() {
        let mut i = ice(8, 8);
        // the speed is the byte plus 0x80 read as signed: 0x8a -> 10
        i.horiz_pan_speed = 0x8a;
        i.vert_pan_speed = 0x80;
        i.move_position(0.5);
        // U -= 2 * 10 * 0.5
        assert_eq!(i.u_displace, -10.0);
        // 0x80 -> 0: V unchanged
        assert_eq!(i.v_displace, 0.0);
        assert_eq!((i.u_position, i.v_position), (-10.0, 0.0));
        assert_eq!(i.master_count, 60.0);
    }

    #[test]
    fn wavy_x_rounds_half_the_wave() {
        let mut i = ice(8, 8);
        i.panning_style = 3;
        i.frequency = 9;
        i.amplitude = 19;
        i.horiz_pan_speed = 0x80;
        i.vert_pan_speed = 0x80;
        i.move_position(1.0);
        // master 120; sin(10 * 120 * 0.0012) = sin(1.44); 20 * that * 0.5 = 9.9145 -> 10
        let s = ((10.0f64 * 120.0 * 0.0012f32 as f64).sin() as f32 * 20.0 * 0.5) as f64;
        assert_eq!(i.u_position, s.round() as f32 + i.u_displace);
        assert_eq!(i.u_position, 10.0);
    }

    #[test]
    fn glass_displaces_source_pixels_as_bytes() {
        let mut i = ice(8, 8);
        let source: Vec<u8> = (0..64).map(|p| p as u8).collect();
        // glass value 3 everywhere except row 2, column 5 (value 250: wraps as a byte)
        let mut glass = vec![3u8; 64];
        glass[2 * 8 + 5] = 250;
        i.u_position = 1.0;
        i.v_position = 1.0;
        i.blit_tex_ice(&glass, &source);
        // row 1 reads glass row 2 shifted by 1: column 4 meets the 250 -> (250 + 4) & 7 = 254 & 7 = 6
        assert_eq!(i.bitmap[8 + 4], source[8 + 6]);
        // elsewhere (3 + x) & 7
        assert_eq!(i.bitmap[8 + 7], source[8 + ((3 + 7) & 7)]);
        i.blit_ice_tex(&glass, &source);
        // row 0 under glass row 0, source row 1, column 6: (3 + 6 + 1) & 7 = 2
        assert_eq!(i.bitmap[6], source[8 + 2]);
    }

    #[test]
    fn unchanged_position_does_not_redraw() {
        let mut i = ice(8, 8);
        i.horiz_pan_speed = 0x80;
        i.vert_pan_speed = 0x80;
        i.force_refresh = true;
        let (g, s) = (vec![0u8; 64], vec![1u8; 64]);
        assert!(i.render(0.01, Some(&g), Some(&s)));
        assert!(!i.render(0.01, Some(&g), Some(&s)));
        assert!(!i.render(0.01, None, Some(&s)));
    }
}
