//! Decoding the sound formats found in the game: PCM WAV (effects, ambience) and MPEG-1 layer III (conversation voices).

/// Decoded audio: interleaved f32 samples in -1..1.
#[derive(Debug, Clone)]
pub struct PcmSound {
    pub rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
    /// Bits per sample of the stored data (8 or 16; MP3 decodes to 16). The lip-sync detector reads 8-bit data differently.
    pub bits: u16,
    /// Compressed data (MP3) that the original driver decodes while playing into a ring buffer instead of keeping it whole.
    pub streamed: bool,
}

impl PcmSound {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }

    pub fn seconds(&self) -> f32 {
        self.frames() as f32 / self.rate.max(1) as f32
    }

    /// `format` is the name stored with the `Sound` object (`wav`, `mp3`; case does not matter).
    pub fn decode(format: &str, data: &[u8]) -> Result<PcmSound, String> {
        match format.to_ascii_lowercase().as_str() {
            "wav" => decode_wav(data),
            "mp3" => decode_mp3(data),
            other => Err(format!("unsupported sound format {other}")),
        }
    }
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

/// RIFF/WAVE with PCM data of 8 or 16 bits.
pub fn decode_wav(d: &[u8]) -> Result<PcmSound, String> {
    if d.get(0..4) != Some(b"RIFF") || d.get(8..12) != Some(b"WAVE") {
        return Err("not a RIFF/WAVE file".into());
    }
    let (mut pos, mut fmt, mut data) = (12usize, None, None);
    while pos + 8 <= d.len() {
        let id = &d[pos..pos + 4];
        let size = u32_at(d, pos + 4).ok_or("truncated chunk")? as usize;
        let body = pos + 8;
        let end = (body + size).min(d.len());
        match id {
            b"fmt " => fmt = Some((u16_at(d, body).ok_or("short fmt")?, u16_at(d, body + 2).ok_or("short fmt")?, u32_at(d, body + 4).ok_or("short fmt")?, u16_at(d, body + 14).ok_or("short fmt")?)),
            b"data" => data = Some(&d[body..end]),
            _ => {}
        }
        pos = body + size + (size & 1);
    }
    let (tag, channels, rate, bits) = fmt.ok_or("no fmt chunk")?;
    let data = data.ok_or("no data chunk")?;
    if tag != 1 {
        return Err(format!("WAV format tag {tag} (only PCM is supported)"));
    }
    if channels == 0 || channels > 2 {
        return Err(format!("{channels} channels"));
    }
    let samples: Vec<f32> = match bits {
        16 => data.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect(),
        8 => data.iter().map(|&b| (b as f32 - 128.0) / 128.0).collect(),
        n => return Err(format!("{n}-bit WAV")),
    };
    Ok(PcmSound { rate, channels, samples, bits, streamed: false })
}

/// MPEG audio layer III through Galaxy's decoder (`galaxy::mp3`), the whole file in 0x8000-byte steps.
pub fn decode_mp3(d: &[u8]) -> Result<PcmSound, String> {
    let s = crate::galaxy::sample::load_mp3(d)?;
    let channels: u16 = if s.flags & crate::galaxy::sample::F_STEREO != 0 { 2 } else { 1 };
    let mut dec = crate::galaxy::mp3::Mp3Decoder::new();
    let mut pos = 0usize;
    let mut samples = Vec::new();
    loop {
        let (c, l, r) = dec.decode(&d[pos.min(d.len())..], 0x8000);
        if c == 0 && l.is_empty() {
            break;
        }
        pos += c;
        for (a, b) in l.iter().zip(&r) {
            samples.push(*a as f32 / 32768.0);
            if channels == 2 {
                samples.push(*b as f32 / 32768.0);
            }
        }
    }
    if samples.is_empty() {
        return Err("no audio in the MP3 data".into());
    }
    Ok(PcmSound { rate: s.rate, channels, samples, bits: 16, streamed: true })
}

/// A 16-bit PCM WAV file of interleaved samples in -1..1 (for rendering the mixer to a file).
pub fn encode_wav(rate: u32, channels: u16, samples: &[f32]) -> Vec<u8> {
    let data_len = samples.len() * 2;
    let mut w = Vec::with_capacity(44 + data_len);
    w.extend(b"RIFF");
    w.extend((36 + data_len as u32).to_le_bytes());
    w.extend(b"WAVEfmt ");
    w.extend(16u32.to_le_bytes());
    w.extend(1u16.to_le_bytes());
    w.extend(channels.to_le_bytes());
    w.extend(rate.to_le_bytes());
    w.extend((rate * channels as u32 * 2).to_le_bytes());
    w.extend((channels * 2).to_le_bytes());
    w.extend(16u16.to_le_bytes());
    w.extend(b"data");
    w.extend((data_len as u32).to_le_bytes());
    for &x in samples {
        w.extend(((x.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_round_trip() {
        let src: Vec<f32> = (0..200).map(|i| ((i as f32) * 0.1).sin() * 0.8).collect();
        let bytes = encode_wav(22050, 2, &src);
        let back = decode_wav(&bytes).unwrap();
        assert_eq!((back.rate, back.channels, back.frames()), (22050, 2, 100));
        assert!(src.iter().zip(&back.samples).all(|(a, b)| (a - b).abs() < 1e-3));
    }
}
