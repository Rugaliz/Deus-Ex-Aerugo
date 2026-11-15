//! `Sound` and `Music` objects (spec: `docs/formats/audio-format.md`): an encoded file plus a format name.

use crate::payload;
use ue_package::{Error, Package, Reader, Result};

/// The payload shared by `Sound` and `Music`: a format name and the bytes of a complete audio file.
#[derive(Debug, Clone)]
pub struct AudioFile {
    /// Format name as stored (for sounds `WAV`; for music the tracker type, for example `it`, `s3m`, `xm`, `mod`).
    pub format: String,
    pub data: Vec<u8>,
}

fn load(pkg: &Package, export: usize) -> Result<AudioFile> {
    if pkg.version < 62 {
        return Err(Error::Invalid("audio older than package version 62 is not supported".into()));
    }
    let p = payload(pkg, export)?;
    let r = &mut Reader::new(p.data, p.native_start);
    let format = pkg.name(r.compact_index()?).to_string();
    // lazy byte array: absolute end offset, compact length, bytes
    let end = r.i32()?;
    let len = r.compact_index()?;
    if len < 0 {
        return Err(Error::Invalid(format!("audio length {len}")));
    }
    let data = r.bytes(len as usize)?.to_vec();
    if end as usize != pkg.exports[export].serial_offset as usize + r.pos() {
        return Err(Error::Invalid("audio end marker mismatch".into()));
    }
    if r.pos() != p.data.len() {
        return Err(Error::Invalid(format!("audio has {} trailing bytes", p.data.len() - r.pos())));
    }
    Ok(AudioFile { format, data })
}

/// A `Sound` object.
pub fn load_sound(pkg: &Package, export: usize) -> Result<AudioFile> {
    load(pkg, export)
}

/// A `Music` object.
pub fn load_music(pkg: &Package, export: usize) -> Result<AudioFile> {
    load(pkg, export)
}

/// Header information of a RIFF/WAVE file.
#[derive(Debug, Clone, PartialEq)]
pub struct WavInfo {
    /// `wFormatTag` (1 = PCM, 2 = MS ADPCM, 0x11 = IMA ADPCM, ...).
    pub format_tag: u16,
    pub channels: u16,
    pub sample_rate: u32,
    pub bits_per_sample: u16,
    /// Bytes in the `data` chunk.
    pub data_len: usize,
}

/// Parses the chunk structure of a WAV file.
pub fn wav_info(data: &[u8]) -> Result<WavInfo> {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err(Error::Invalid("not a RIFF/WAVE file".into()));
    }
    let mut pos = 12;
    let (mut fmt, mut data_len) = (None, None);
    while pos + 8 <= data.len() {
        let id = &data[pos..pos + 4];
        let size = u32::from_le_bytes(data[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = pos + 8;
        if id == b"fmt " && body + 16 <= data.len() {
            let b = &data[body..];
            fmt = Some((
                u16::from_le_bytes([b[0], b[1]]),
                u16::from_le_bytes([b[2], b[3]]),
                u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
                u16::from_le_bytes([b[14], b[15]]),
            ));
        } else if id == b"data" {
            data_len = Some(size.min(data.len() - body));
        }
        pos = body + size + (size & 1);
    }
    let (format_tag, channels, sample_rate, bits_per_sample) = fmt.ok_or_else(|| Error::Invalid("WAV without fmt chunk".into()))?;
    Ok(WavInfo { format_tag, channels, sample_rate, bits_per_sample, data_len: data_len.ok_or_else(|| Error::Invalid("WAV without data chunk".into()))? })
}

/// Summary of an MP3 stream obtained by walking its frame headers.
#[derive(Debug, Clone, PartialEq)]
pub struct Mp3Info {
    /// 1 = MPEG-1, 2 = MPEG-2, 25 = MPEG-2.5.
    pub mpeg: u8,
    pub layer: u8,
    pub sample_rate: u32,
    pub channels: u8,
    pub frames: usize,
    /// Bytes skipped at the start (ID3v2 tag) and after the last frame (ID3v1 tag or padding).
    pub leading_bytes: usize,
    pub trailing_bytes: usize,
    pub duration_secs: f32,
}

const BITRATES_V1_L3: [u32; 16] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0];
const BITRATES_V2_L3: [u32; 16] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0];

/// Walks the frame headers of an MP3 (layer III) file; every frame must follow the previous one directly.
pub fn mp3_info(data: &[u8]) -> Result<Mp3Info> {
    let mut pos = 0;
    if data.len() >= 10 && &data[0..3] == b"ID3" {
        let size = ((data[6] as usize & 0x7F) << 21) | ((data[7] as usize & 0x7F) << 14) | ((data[8] as usize & 0x7F) << 7) | (data[9] as usize & 0x7F);
        pos = 10 + size;
    }
    let leading_bytes = pos;
    let (mut frames, mut info) = (0usize, None);
    while pos + 4 <= data.len() {
        let h = u32::from_be_bytes(data[pos..pos + 4].try_into().unwrap());
        if h >> 21 != 0x7FF {
            break;
        }
        let version = (h >> 19) & 3; // 3 = MPEG-1, 2 = MPEG-2, 0 = MPEG-2.5
        let layer = (h >> 17) & 3; // 1 = layer III
        let (bitrate_idx, rate_idx, padding) = ((h >> 12) & 0xF, (h >> 10) & 3, (h >> 9) & 1);
        if version == 1 || layer != 1 || rate_idx == 3 || bitrate_idx == 0 || bitrate_idx == 15 {
            break;
        }
        let base_rate = [44100u32, 48000, 32000][rate_idx as usize];
        let (mpeg, sample_rate, bitrate, spf) = match version {
            3 => (1u8, base_rate, BITRATES_V1_L3[bitrate_idx as usize], 1152u32),
            2 => (2, base_rate / 2, BITRATES_V2_L3[bitrate_idx as usize], 576),
            _ => (25, base_rate / 4, BITRATES_V2_L3[bitrate_idx as usize], 576),
        };
        let len = (spf / 8 * bitrate * 1000 / sample_rate + padding) as usize;
        let channels = if (h >> 6) & 3 == 3 { 1 } else { 2 };
        if pos + len > data.len() {
            break;
        }
        info.get_or_insert((mpeg, sample_rate, channels, spf));
        frames += 1;
        pos += len;
    }
    let (mpeg, sample_rate, channels, spf) = info.ok_or_else(|| Error::Invalid("no MP3 frame found".into()))?;
    Ok(Mp3Info {
        mpeg,
        layer: 3,
        sample_rate,
        channels,
        frames,
        leading_bytes,
        trailing_bytes: data.len() - pos,
        duration_secs: frames as f32 * spf as f32 / sample_rate as f32,
    })
}
