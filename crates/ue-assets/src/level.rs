//! The `Level` object: actor list, world model, reach specs (spec: `docs/formats/level-format.md`).

use crate::payload;
use ue_package::{Error, Package, Reader, Result};

#[derive(Debug, Clone, PartialEq)]
pub struct Url {
    pub protocol: String,
    pub host: String,
    pub map: String,
    pub portal: String,
    pub options: Vec<String>,
    pub port: i32,
    pub valid: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReachSpec {
    pub distance: i32,
    /// Object references (compact indices) of the two navigation points.
    pub start: i32,
    pub end: i32,
    pub collision_radius: i32,
    pub collision_height: i32,
    pub reach_flags: i32,
    pub pruned: bool,
}

#[derive(Debug, Clone)]
pub struct Level {
    /// Raw object references (compact indices) of every actor; 0 marks an empty slot (deleted actor).
    pub actors: Vec<i32>,
    pub url: Url,
    /// Object reference of the world `Model`.
    pub model: i32,
    pub reach_specs: Vec<ReachSpec>,
    pub time_seconds: f32,
    /// Head of the engine's list of deleted actors (object reference; 0 in every shipped map).
    pub first_deleted: i32,
    /// The 16 `TextBlocks` references (text buffers, normally only slot 6 is used).
    pub text_blocks: [i32; 16],
    /// Key/value pairs carried between maps.
    pub travel_info: Vec<(String, String)>,
}

fn string_array(r: &mut Reader) -> Result<Vec<String>> {
    let n = r.compact_index()?;
    if !(0..=4096).contains(&n) {
        return Err(Error::Invalid(format!("string array count {n}")));
    }
    (0..n).map(|_| r.string()).collect()
}

impl Level {
    pub fn load(pkg: &Package, export: usize) -> Result<Level> {
        if pkg.version < 63 {
            return Err(Error::Invalid("levels older than package version 63 are not supported".into()));
        }
        let p = payload(pkg, export)?;
        let r = &mut Reader::new(p.data, p.native_start);
        // ULevelBase: actor list (count, capacity, then the references), then the URL
        let num = r.i32()?;
        let _max = r.i32()?;
        if num < 0 || num as usize > 1 << 22 {
            return Err(Error::Invalid(format!("actor count {num}")));
        }
        let actors = (0..num).map(|_| r.compact_index()).collect::<Result<Vec<_>>>()?;
        let url = Url {
            protocol: r.string()?,
            host: r.string()?,
            map: r.string()?,
            portal: r.string()?,
            options: string_array(r)?,
            port: r.i32()?,
            valid: r.i32()? != 0,
        };
        // ULevel
        let model = r.compact_index()?;
        let n = r.compact_index()?;
        if !(0..=1 << 22).contains(&n) {
            return Err(Error::Invalid(format!("reach spec count {n}")));
        }
        let reach_specs = (0..n)
            .map(|_| {
                Ok(ReachSpec {
                    distance: r.i32()?,
                    start: r.compact_index()?,
                    end: r.compact_index()?,
                    collision_radius: r.i32()?,
                    collision_height: r.i32()?,
                    reach_flags: r.i32()?,
                    pruned: r.u8()? != 0,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let time_seconds = r.f32()?;
        let first_deleted = r.compact_index()?;
        let mut text_blocks = [0; 16];
        for s in &mut text_blocks {
            *s = r.compact_index()?;
        }
        let n = r.compact_index()?;
        if !(0..=1 << 16).contains(&n) {
            return Err(Error::Invalid(format!("travel info count {n}")));
        }
        let travel_info = (0..n).map(|_| Ok((r.string()?, r.string()?))).collect::<Result<Vec<_>>>()?;
        if r.pos() != p.data.len() {
            return Err(Error::Invalid(format!("level has {} trailing bytes", p.data.len() as i64 - r.pos() as i64)));
        }
        Ok(Level { actors, url, model, reach_specs, time_seconds, first_deleted, text_blocks, travel_info })
    }
}
