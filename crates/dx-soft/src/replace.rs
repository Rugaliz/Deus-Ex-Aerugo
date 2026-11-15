//! Replacement pictures from the mods folder (`ue_package::mods`): a PNG at any size stands in for a texture's
//! pixels. The texture keeps its own size for the game (scripts, UV mapping); only the picture changes.

/// The mod's replacement picture for the texture `Package.Group.Name`, as every mip level down to 1x1, RGBA8.
pub fn replacement_mips(path: &str) -> Option<Vec<(u32, u32, Vec<u8>)>> {
    let source = ue_package::mods::texture(path)?;
    let bytes = source.read().ok()?;
    match decode_png(&bytes) {
        Some((w, h, rgba)) => Some(mip_chain(w, h, rgba)),
        None => {
            eprintln!("mods: cannot decode {}", source.describe());
            None
        }
    }
}

/// A PNG as RGBA8 (any colour type and bit depth).
pub fn decode_png(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16 | png::Transformations::ALPHA);
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width, info.height);
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::GrayscaleAlpha => buf.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        _ => return None,
    };
    (rgba.len() == (w * h * 4) as usize).then_some((w, h, rgba))
}

/// Every mip level from `rgba` down to 1x1: each texel the average of up to four, colour weighted by alpha so that
/// transparent texels do not darken the edges of masked pictures.
pub fn mip_chain(w: u32, h: u32, rgba: Vec<u8>) -> Vec<(u32, u32, Vec<u8>)> {
    let mut out = vec![(w, h, rgba)];
    while let Some(&(pw, ph, ref prev)) = out.last() {
        if pw == 1 && ph == 1 {
            break;
        }
        let (nw, nh) = ((pw / 2).max(1), (ph / 2).max(1));
        let mut next = vec![0u8; (nw * nh * 4) as usize];
        for y in 0..nh {
            for x in 0..nw {
                let mut acc = [0u32; 4];
                let mut n = 0;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (sx, sy) = ((x * 2 + dx).min(pw - 1), (y * 2 + dy).min(ph - 1));
                    let i = ((sy * pw + sx) * 4) as usize;
                    let a = prev[i + 3] as u32;
                    for c in 0..3 {
                        acc[c] += prev[i + c] as u32 * a;
                    }
                    acc[3] += a;
                    n += 1;
                }
                let o = ((y * nw + x) * 4) as usize;
                for c in 0..3 {
                    next[o + c] = if acc[3] > 0 { (acc[c] / acc[3]) as u8 } else { 0 };
                }
                next[o + 3] = (acc[3] / n) as u8;
            }
        }
        out.push((nw, nh, next));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_round_trips_and_gets_every_mip_level() {
        let (w, h) = (8u32, 4u32);
        let rgba: Vec<u8> = (0..w * h).flat_map(|i| [i as u8 * 8, 100, 200, 255]).collect();
        let (dw, dh, back) = decode_png(&crate::png(w, h, &rgba)).unwrap();
        assert_eq!((dw, dh), (w, h));
        assert_eq!(back, rgba);
        let mips = mip_chain(w, h, back);
        assert_eq!(mips.iter().map(|m| (m.0, m.1)).collect::<Vec<_>>(), [(8, 4), (4, 2), (2, 1), (1, 1)]);
    }

    #[test]
    fn transparent_texels_do_not_darken_the_smaller_levels() {
        // one opaque white texel next to three transparent black ones
        let rgba = vec![255, 255, 255, 255, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let mips = mip_chain(2, 2, rgba);
        assert_eq!(mips[1].2, [255, 255, 255, 63]);
    }
}
