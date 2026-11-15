//! Galaxy's game-sound samples (docs/galaxy/sounds.md section 8): the `SAMP` header the WAV loader (10619940) and
//! the MP3 loader (10620b30) build from a sound's data. Mono PCM is played whole; stereo PCM and MP3 are played
//! through a voice's stream ring (section 9), from the source kept here.
use super::song::Pcm;
use std::sync::Arc;

/// Header flags.
pub const F_16BIT: u16 = 0x0004;
pub const F_LOOP: u16 = 0x0008;
pub const F_PINGPONG: u16 = 0x0010;
pub const F_MONO: u16 = 0x0020;
pub const F_STEREO: u16 = 0x0040;
pub const F_ADPCM: u16 = 0x2000;
pub const F_MP3: u16 = 0x4000;

/// A loaded sound (`SAMP` header and data).
#[derive(Debug, Clone)]
pub struct Sample {
    /// +0x2c.
    pub flags: u16,
    /// +0x28 default pan, +0x2a default volume.
    pub pan: u16,
    pub volume: u16,
    /// +0x30: frames for PCM, data bytes for MP3.
    pub length: u32,
    /// +0x34, +0x38 (frames).
    pub loop_start: u32,
    pub loop_end: u32,
    /// +0x3c.
    pub rate: u32,
    /// The data a whole-PCM voice plays (mono PCM, with the loader's 31 zero bytes after it).
    pub data: Option<Arc<Pcm>>,
    /// What a stream fill reads: one sample sequence per output voice (left, right; mono MP3 gives both the same),
    /// in 16-bit units (8-bit stereo PCM: the bytes as the stream copy turns them).
    pub stream: Option<Arc<StreamSource>>,
}

/// What a stream fill reads (sounds.md 9.3): PCM (stereo WAVs, one sample sequence per output voice; 8-bit data as
/// the stream copy turns it) or an MP3 file with its decoder state (one per sound, never reset: mp3.md 4.6).
#[derive(Debug)]
pub enum StreamSource {
    Pcm { left: Vec<i16>, right: Vec<i16>, bytes: bool },
    Mp3 { data: Vec<u8>, decoder: std::sync::Mutex<Box<super::mp3::Mp3Decoder>> },
}

impl StreamSource {
    /// The ring holds bytes (8-bit data) rather than 16-bit samples.
    pub fn bytes(&self) -> bool {
        matches!(self, StreamSource::Pcm { bytes: true, .. })
    }
}

impl Sample {
    /// Played through a stream ring (`flags & 0xe000` or stereo).
    pub fn streamed(&self) -> bool {
        self.flags & 0xe000 != 0 || self.flags & F_STEREO != 0
    }
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    b.get(o..o + 2).map(|x| u16::from_le_bytes([x[0], x[1]])).unwrap_or(0)
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    b.get(o..o + 4).map(|x| u32::from_le_bytes([x[0], x[1], x[2], x[3]])).unwrap_or(0)
}

/// The sample load dispatcher (10618e60) for the formats Deus Ex uses: RIFF/WAVE, else MP3.
pub fn load(data: &[u8]) -> Result<Sample, String> {
    if data.get(8..12) == Some(b"WAVE") {
        load_wav(data)
    } else {
        load_mp3(data)
    }
}

/// The RIFF/WAVE loader (10619940).
pub fn load_wav(d: &[u8]) -> Result<Sample, String> {
    let riff = u32_at(d, 4);
    let mut left = ((riff.wrapping_add(1)) & !1).wrapping_sub(4) as i64;
    let mut pos = 12usize;
    let mut fmt: Option<(u16, u16, u32, u16, u16)> = None;
    let mut s = Sample { flags: 0, pan: 0x4000, volume: 0x7fff, length: 0, loop_start: 0, loop_end: 0, rate: 0, data: None, stream: None };
    let mut have_data = false;
    let mut smpl: Option<(u32, u32, u32)> = None;
    while left > 0 && pos + 8 <= d.len() {
        let id = &d[pos..pos + 4];
        let size = u32_at(d, pos + 4) as usize;
        let body = pos + 8;
        match id {
            b"fmt " => {
                fmt = Some((u16_at(d, body), u16_at(d, body + 2), u32_at(d, body + 4), u16_at(d, body + 12), u16_at(d, body + 14)));
            }
            b"data" => {
                let (tag, channels, rate, align, bits) = fmt.ok_or("data before fmt")?;
                match tag {
                    1 => {
                        let align = align.max(1) as usize;
                        let len = size / align;
                        s.flags = (if channels == 2 { F_STEREO } else { F_MONO }) | (if bits == 16 { F_16BIT } else { 0 });
                        s.pan = 0x4000;
                        s.volume = 0x7fff;
                        s.length = len as u32;
                        s.loop_start = 0;
                        s.loop_end = 0;
                        s.rate = rate;
                        let end = (body + len * align).min(d.len());
                        let mut raw = d[body..end].to_vec();
                        raw.resize(len * align, 0);
                        // 8 bits: the first `length` bytes become signed (all of a mono sound, half of a stereo one)
                        if bits == 8 {
                            for b in raw.iter_mut().take(len) {
                                *b = b.wrapping_sub(0x80);
                            }
                        }
                        let bytes_total = size;
                        if channels == 2 {
                            let (mut l, mut r) = (Vec::new(), Vec::new());
                            if bits == 16 {
                                for f in raw.chunks_exact(4) {
                                    l.push(i16::from_le_bytes([f[0], f[1]]));
                                    r.push(i16::from_le_bytes([f[2], f[3]]));
                                }
                            } else {
                                // the stream copy subtracts 0x80 from every byte again (quirk 11)
                                for f in raw.chunks_exact(2) {
                                    l.push(f[0].wrapping_sub(0x80) as i8 as i16);
                                    r.push(f[1].wrapping_sub(0x80) as i8 as i16);
                                }
                            }
                            s.stream = Some(Arc::new(StreamSource::Pcm { left: l, right: r, bytes: bits != 16 }));
                        } else if bits == 16 {
                            // the 31 zero bytes after the data: 15 samples and a half (the other half byte is heap)
                            let mut v: Vec<i16> = raw.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect();
                            v.resize(len + 16, 0);
                            let _ = bytes_total;
                            s.data = Some(Arc::new(Pcm::S16(v)));
                        } else {
                            let mut v: Vec<i8> = raw.iter().map(|&b| b as i8).collect();
                            v.resize(len + 31, 0);
                            s.data = Some(Arc::new(Pcm::S8(v)));
                        }
                        have_data = true;
                    }
                    0x55 => return load_mp3(d),
                    t => return Err(format!("WAV format tag {t} is not played (Deus Ex uses PCM and MP3)")),
                }
            }
            b"smpl" => {
                let count = u32_at(d, body + 28);
                if count != 0 {
                    // the first loop record: type +4, start +8, end +12
                    smpl = Some((u32_at(d, body + 36 + 8), u32_at(d, body + 36 + 12), u32_at(d, body + 36 + 4)));
                } else {
                    smpl = None;
                }
                if have_data {
                    apply_loop(&mut s, smpl.take());
                }
            }
            _ => {}
        }
        let padded = (size + 1) & !1;
        left -= 8 + padded as i64;
        pos = body + padded;
    }
    if !have_data {
        return Err("no data chunk".into());
    }
    // a smpl chunk before the data chunk is overwritten by the data branch
    let _ = smpl;
    Ok(s)
}

fn apply_loop(s: &mut Sample, l: Option<(u32, u32, u32)>) {
    if let Some((start, end, kind)) = l {
        s.loop_start = start;
        s.loop_end = end;
        s.flags |= F_LOOP;
        if kind & 1 != 0 {
            s.flags |= F_PINGPONG;
        }
    }
}

/// The MP3 loader (10620b30): the header at the first byte-aligned sync gives the rate and the channel mode; the
/// whole file is kept and decoded by Galaxy's decoder (`super::mp3`) as the voice's ring is filled.
pub fn load_mp3(d: &[u8]) -> Result<Sample, String> {
    let limit = d.len().min(0x4000);
    let mut at = None;
    for i in 0..limit.saturating_sub(3) {
        let w = u32::from_be_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]);
        if w & 0xffe0_0000 == 0xffe0_0000 {
            at = Some(w);
            break;
        }
    }
    let h = at.ok_or("no MPEG sync in the first 0x4000 bytes")?;
    // version bits: 3 MPEG-1, 2 MPEG-2, 0 MPEG-2.5
    let v = match (h >> 19) & 3 {
        3 => 1,
        2 => 0,
        _ => 2,
    };
    let rate_index = ((h >> 10) & 3) as usize;
    const RATES: [u32; 11] = [22050, 24000, 16000, 0, 44100, 48000, 32000, 0, 11025, 12000, 8000];
    let rate = *RATES.get(rate_index + 4 * v).unwrap_or(&0);
    let mode = (h >> 6) & 3;
    Ok(Sample {
        flags: 0x4004 | if mode == 3 { F_MONO } else { F_STEREO },
        pan: 0x4000,
        volume: 0x7fff,
        length: d.len() as u32,
        loop_start: 0,
        loop_end: 0,
        rate,
        data: Some(Arc::new(Pcm::S8(Vec::new()))),
        stream: Some(Arc::new(StreamSource::Mp3 { data: d.to_vec(), decoder: std::sync::Mutex::new(Box::new(super::mp3::Mp3Decoder::new())) })),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(bits: u16, channels: u16, data: &[u8], smpl: Option<(u32, u32, u32)>) -> Vec<u8> {
        let mut w = Vec::new();
        w.extend(b"RIFF");
        w.extend(0u32.to_le_bytes());
        w.extend(b"WAVEfmt ");
        w.extend(16u32.to_le_bytes());
        w.extend(1u16.to_le_bytes());
        w.extend(channels.to_le_bytes());
        w.extend(22050u32.to_le_bytes());
        w.extend((22050 * channels as u32 * bits as u32 / 8).to_le_bytes());
        w.extend((channels * bits / 8).to_le_bytes());
        w.extend(bits.to_le_bytes());
        w.extend(b"data");
        w.extend((data.len() as u32).to_le_bytes());
        w.extend(data);
        if data.len() & 1 == 1 {
            w.push(0);
        }
        if let Some((a, b, k)) = smpl {
            w.extend(b"smpl");
            w.extend(60u32.to_le_bytes());
            let mut c = vec![0u8; 60];
            c[28..32].copy_from_slice(&1u32.to_le_bytes());
            c[40..44].copy_from_slice(&k.to_le_bytes());
            c[44..48].copy_from_slice(&a.to_le_bytes());
            c[48..52].copy_from_slice(&b.to_le_bytes());
            w.extend(c);
        }
        let n = (w.len() - 8) as u32;
        w[4..8].copy_from_slice(&n.to_le_bytes());
        w
    }

    #[test]
    fn wav_loader_signs_8_bit_data_pads_and_reads_loops() {
        let s = load(&wav(8, 1, &[0x80, 0x90, 0x70], Some((1, 3, 1)))).unwrap();
        assert_eq!(s.flags, F_MONO | F_LOOP | F_PINGPONG);
        assert_eq!((s.length, s.loop_start, s.loop_end, s.rate), (3, 1, 3, 22050));
        let Pcm::S8(v) = &**s.data.as_ref().unwrap() else { panic!() };
        assert_eq!(&v[..3], &[0, 0x10, -0x10]);
        assert_eq!(v.len(), 3 + 31);
        let s = load(&wav(16, 2, &[1, 0, 2, 0, 3, 0, 4, 0], None)).unwrap();
        assert!(s.streamed());
        assert_eq!(s.flags, F_STEREO | F_16BIT);
        let StreamSource::Pcm { left, right, .. } = &*s.stream.unwrap() else { panic!() };
        assert_eq!((left.clone(), right.clone()), (vec![1, 3], vec![2, 4]));
    }
}
