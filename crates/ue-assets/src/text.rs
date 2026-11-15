//! Small text-bearing objects: `ExtString`, `ConAudioList`, `ConEventRandomLabel` (spec: `docs/formats/text-objects.md`).

use crate::payload;
use ue_package::{Error, Package, Reader, Result};

fn finish(r: &Reader, len: usize, what: &str) -> Result<()> {
    if r.pos() != len {
        return Err(Error::Invalid(format!("{what} has {} trailing bytes", len as i64 - r.pos() as i64)));
    }
    Ok(())
}

/// An `ExtString`: one localisable string (the engine's UI text lives in these).
pub fn load_ext_string(pkg: &Package, export: usize) -> Result<String> {
    let p = payload(pkg, export)?;
    let r = &mut Reader::new(p.data, p.native_start);
    let s = r.string()?;
    finish(r, p.data.len(), "ExtString")?;
    Ok(s)
}

/// A `ConAudioList`: the sounds a conversation line can use, as raw object references.
pub fn load_con_audio_list(pkg: &Package, export: usize) -> Result<Vec<i32>> {
    let p = payload(pkg, export)?;
    let r = &mut Reader::new(p.data, p.native_start);
    let n = r.compact_index()?;
    if !(0..=1 << 16).contains(&n) {
        return Err(Error::Invalid(format!("audio list count {n}")));
    }
    let v = (0..n).map(|_| r.compact_index()).collect::<Result<Vec<_>>>()?;
    finish(r, p.data.len(), "ConAudioList")?;
    Ok(v)
}

/// A `ConEventRandomLabel`: the label names a random-jump event chooses from.
pub fn load_con_random_labels(pkg: &Package, export: usize) -> Result<Vec<String>> {
    let p = payload(pkg, export)?;
    let r = &mut Reader::new(p.data, p.native_start);
    let n = r.compact_index()?;
    if !(0..=1 << 16).contains(&n) {
        return Err(Error::Invalid(format!("label count {n}")));
    }
    let v = (0..n).map(|_| r.string()).collect::<Result<Vec<_>>>()?;
    finish(r, p.data.len(), "ConEventRandomLabel")?;
    Ok(v)
}
