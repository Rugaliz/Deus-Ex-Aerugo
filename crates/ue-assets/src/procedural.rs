//! Procedural textures (`FireTexture`, `WetTexture`, `WaveTexture`, `WaterTexture`, `IceTexture`): an ordinary texture
//! object plus, for fire, a list of sparks (spec: `docs/formats/procedural-texture-format.md`).

use crate::texture::Texture;
use ue_package::{Error, Package, Reader, Result};

/// One fire spark: eight bytes in the order the engine stores them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spark {
    /// `ESpark` type.
    pub kind: u8,
    pub heat: u8,
    pub x: u8,
    pub y: u8,
    /// Four type-specific parameters.
    pub params: [u8; 4],
}

/// One water drop (`WaterTexture.ADrop`): a source that disturbs the water every simulation step.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WaterDrop {
    /// `WDrop` type (0-19; the engine also uses 64 and 65 internally for reversed whirls).
    pub kind: u8,
    pub depth: u8,
    pub x: u8,
    pub y: u8,
    /// `ByteA`..`ByteD`: type-specific (speed, phase, frequency, size, ...).
    pub a: u8,
    pub b: u8,
    pub c: u8,
    pub d: u8,
}

/// The tagged properties that drive a water texture (`WetTexture`, `WaveTexture`).
#[derive(Debug, Clone, Default)]
pub struct WaterParams {
    /// The first `NumDrops` entries of `Drops`.
    pub drops: Vec<WaterDrop>,
    pub wave_amp: u8,
    /// `WetTexture.SourceTexture` (raw object reference, 0 if none).
    pub source_texture: i32,
    /// `WaveTexture` lighting: `BumpMapLight`, `BumpMapAngle`, `PhongRange`, `PhongSize`.
    pub bump_map_light: u8,
    pub bump_map_angle: u8,
    pub phong_range: u8,
    pub phong_size: u8,
    /// `Texture` animation timing: `MinFrameRate`, `MaxFrameRate` (0 = every frame), `PrimeCount`.
    pub min_frame_rate: f32,
    pub max_frame_rate: f32,
    pub prime_count: u8,
}

impl WaterParams {
    pub fn load(pkg: &Package, export: usize) -> Result<WaterParams> {
        let p = crate::payload(pkg, export)?;
        let num = p.int(pkg, "NumDrops").unwrap_or(0).clamp(0, 256) as u32;
        let mut drops = Vec::with_capacity(num as usize);
        for i in 0..num {
            let mut d = WaterDrop::default();
            if let Some(t) = p.element(pkg, "Drops", i) {
                let b = &p.data[t.data.clone()];
                if b.len() != 8 {
                    return Err(Error::Invalid(format!("water drop of {} bytes", b.len())));
                }
                d = WaterDrop { kind: b[0], depth: b[1], x: b[2], y: b[3], a: b[4], b: b[5], c: b[6], d: b[7] };
            }
            drops.push(d);
        }
        Ok(WaterParams {
            drops,
            wave_amp: p.byte(pkg, "WaveAmp").unwrap_or(0),
            source_texture: p.object(pkg, "SourceTexture").unwrap_or(0),
            bump_map_light: p.byte(pkg, "BumpMapLight").unwrap_or(0),
            bump_map_angle: p.byte(pkg, "BumpMapAngle").unwrap_or(0),
            phong_range: p.byte(pkg, "PhongRange").unwrap_or(0),
            phong_size: p.byte(pkg, "PhongSize").unwrap_or(0),
            min_frame_rate: p.float(pkg, "MinFrameRate").unwrap_or(0.0),
            max_frame_rate: p.float(pkg, "MaxFrameRate").unwrap_or(0.0),
            prime_count: p.byte(pkg, "PrimeCount").unwrap_or(0),
        })
    }
}

/// The tagged properties that drive a `FireTexture` (the sparks themselves come with `ProceduralTexture`).
#[derive(Debug, Clone, Default)]
pub struct FireParams {
    pub render_heat: u8,
    pub rising: bool,
    pub sparks_limit: i32,
    pub frequency: u8,
    pub num_sparks: i32,
    pub min_frame_rate: f32,
    pub max_frame_rate: f32,
    pub prime_count: u8,
}

impl FireParams {
    pub fn load(pkg: &Package, export: usize) -> Result<FireParams> {
        let p = crate::payload(pkg, export)?;
        Ok(FireParams {
            render_heat: p.byte(pkg, "RenderHeat").unwrap_or(0),
            rising: p.bool(pkg, "bRising"),
            sparks_limit: p.int(pkg, "SparksLimit").unwrap_or(0),
            frequency: p.byte(pkg, "FX_Frequency").unwrap_or(0),
            num_sparks: p.int(pkg, "NumSparks").unwrap_or(0),
            min_frame_rate: p.float(pkg, "MinFrameRate").unwrap_or(0.0),
            max_frame_rate: p.float(pkg, "MaxFrameRate").unwrap_or(0.0),
            prime_count: p.byte(pkg, "PrimeCount").unwrap_or(0),
        })
    }
}

/// The tagged properties of an `IceTexture` (`IceTexture.uc`; absent ones are 0, as the class has no defaults).
#[derive(Debug, Clone, Default)]
pub struct IceParams {
    /// Raw object references (0: none).
    pub glass_texture: i32,
    pub source_texture: i32,
    pub panning_style: u8,
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
    pub min_frame_rate: f32,
    pub max_frame_rate: f32,
    pub prime_count: u8,
}

impl IceParams {
    pub fn load(pkg: &Package, export: usize) -> Result<IceParams> {
        let p = crate::payload(pkg, export)?;
        Ok(IceParams {
            glass_texture: p.object(pkg, "GlassTexture").unwrap_or(0),
            source_texture: p.object(pkg, "SourceTexture").unwrap_or(0),
            panning_style: p.byte(pkg, "PanningStyle").unwrap_or(0),
            time_method: p.byte(pkg, "TimeMethod").unwrap_or(0),
            horiz_pan_speed: p.byte(pkg, "HorizPanSpeed").unwrap_or(0),
            vert_pan_speed: p.byte(pkg, "VertPanSpeed").unwrap_or(0),
            frequency: p.byte(pkg, "Frequency").unwrap_or(0),
            amplitude: p.byte(pkg, "Amplitude").unwrap_or(0),
            move_ice: p.bool(pkg, "MoveIce"),
            master_count: p.float(pkg, "MasterCount").unwrap_or(0.0),
            u_displace: p.float(pkg, "UDisplace").unwrap_or(0.0),
            v_displace: p.float(pkg, "VDisplace").unwrap_or(0.0),
            u_position: p.float(pkg, "UPosition").unwrap_or(0.0),
            v_position: p.float(pkg, "VPosition").unwrap_or(0.0),
            min_frame_rate: p.float(pkg, "MinFrameRate").unwrap_or(0.0),
            max_frame_rate: p.float(pkg, "MaxFrameRate").unwrap_or(0.0),
            prime_count: p.byte(pkg, "PrimeCount").unwrap_or(0),
        })
    }
}

#[derive(Debug, Clone)]
pub struct ProceduralTexture {
    /// The base texture data (palette ref, size, mips; the mips of a parametric texture are only placeholders).
    pub texture: Texture,
    /// Fire sparks (empty for the other classes).
    pub sparks: Vec<Spark>,
}

impl ProceduralTexture {
    /// Loads a procedural texture of any of the classes above.
    pub fn load(pkg: &Package, export: usize) -> Result<ProceduralTexture> {
        let class = pkg.export_class_name(export);
        let (texture, end) = Texture::load_prefix(pkg, export)?;
        let data = pkg.export_bytes(export)?;
        let mut sparks = Vec::new();
        let mut r = Reader::new(data, end);
        if class == "FireTexture" {
            let n = r.compact_index()?;
            if !(0..=1 << 16).contains(&n) {
                return Err(Error::Invalid(format!("spark count {n}")));
            }
            for _ in 0..n {
                let b = r.bytes(8)?;
                sparks.push(Spark { kind: b[0], heat: b[1], x: b[2], y: b[3], params: [b[4], b[5], b[6], b[7]] });
            }
        }
        if r.pos() != data.len() {
            return Err(Error::Invalid(format!("{class} has {} trailing bytes", data.len() as i64 - r.pos() as i64)));
        }
        Ok(ProceduralTexture { texture, sparks })
    }
}
