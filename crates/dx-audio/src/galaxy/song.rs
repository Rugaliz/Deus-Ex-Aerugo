//! Galaxy's tracker song: the Impulse Tracker loader (Galaxy.dll 10628990), the sample reader (1062fb10) and the
//! in-memory song it builds (docs/galaxy/loader.md). The loader converts IT data into Galaxy's own ProTracker-style
//! event stream, with all of the original's changes (effect memory resolved at load, dropped effects, PCM fades).

/// Load failures, with Galaxy's status codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadError {
    /// 0x11: no data.
    NoData,
    /// 9, 0x0A, 0x0B: an allocation failed (never happens here; kept for the codes).
    Alloc(u32),
    /// 0x10: a sample's PCM is shorter than its header says.
    ShortSample,
    /// The data is not an Impulse Tracker module (the other loaders are not ported: no game song uses them).
    UnsupportedFormat,
}

/// Envelope block (0x2E bytes in the original): flags 1 enabled, 2 sustain, 4 loop.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Envelope {
    pub flags: u8,
    /// Byte +2: the IT loader stores the node count (masked to 4 bits).
    pub count: u8,
    pub sustain: u8,
    pub loop_start: u8,
    pub loop_end: u8,
    /// (tick, value) for 10 nodes.
    pub nodes: [(u16, u16); 10],
}

/// Converted PCM: `length + 31` frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pcm {
    S8(Vec<i8>),
    S16(Vec<i16>),
}

impl Pcm {
    pub fn len(&self) -> usize {
        match self {
            Pcm::S8(v) => v.len(),
            Pcm::S16(v) => v.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Frame `i` widened to i32 (8-bit frames are not scaled).
    pub fn get(&self, i: usize) -> i32 {
        match self {
            Pcm::S8(v) => v.get(i).map(|&x| x as i32).unwrap_or(0),
            Pcm::S16(v) => v.get(i).map(|&x| x as i32).unwrap_or(0),
        }
    }
    fn set(&mut self, i: usize, x: i32) {
        match self {
            Pcm::S8(v) => {
                if let Some(s) = v.get_mut(i) {
                    *s = x as i8;
                }
            }
            Pcm::S16(v) => {
                if let Some(s) = v.get_mut(i) {
                    *s = x as i16;
                }
            }
        }
    }
}

/// Sample slot (0x48 bytes in the original).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Slot {
    /// Tagged `SAMP` (the reader ran with a non-zero length).
    pub tagged: bool,
    pub name: Vec<u8>,
    /// +0x28 default pan 0..0x7FFF (used only with flag 0x20).
    pub pan: u16,
    /// +0x2A default volume 0..0x7FFF.
    pub volume: u16,
    /// +0x2C: 0x01 delta, 0x02 unsigned, 0x04 16-bit, 0x08 loop, 0x10 ping-pong, 0x20 default pan, 0x40 stereo, 0x80.
    pub flags: u16,
    /// +0x2E: 0x80 converted.
    pub flags2: u16,
    pub length: u32,
    pub loop_start: u32,
    pub loop_end: u32,
    /// +0x3C C5 speed in Hz.
    pub c5speed: u32,
    /// +0x40 alternative modulation block (never set for IT).
    pub alt_modulation: u32,
    /// +0x44 the PCM, `None` when the length is 0.
    pub pcm: Option<std::sync::Arc<Pcm>>,
}

/// Instrument record (`INST`, 0x14A bytes plus slots).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instrument {
    pub index: u8,
    pub name: [u8; 26],
    /// +0x2A note -> slot map, indexed by note - 1.
    pub map: [u8; 120],
    /// +0xAA modulation block (XM auto-vibrato; zero for IT).
    pub modulation: [u8; 14],
    /// Volume (+0xB8), pitch (+0xE8) and pan (+0x118) envelopes.
    pub volume_env: Envelope,
    pub pitch_env: Envelope,
    pub pan_env: Envelope,
    /// +0xE6.
    pub fadeout: u16,
    pub slots: Vec<Slot>,
    /// The record's bytes from +0xAA to +0x14A (the voice's modulation block M as the player reads it: envelopes
    /// at +0x0E/+0x3E/+0x6E, fadeouts at +0x3C/+0x6C/+0x9C, the slot count at +0x9E), built by `seal`.
    pub block: [u8; 0xA0],
}

impl Instrument {
    /// Lays the fields out as the original record's bytes from +0xAA (the player reads nodes at a fixed stride, even
    /// past the ten stored ones).
    fn seal(mut self) -> Instrument {
        let mut b = [0u8; 0xA0];
        b[..14].copy_from_slice(&self.modulation);
        for (base, env) in [(0x0E, &self.volume_env), (0x3E, &self.pitch_env), (0x6E, &self.pan_env)] {
            b[base] = env.flags;
            b[base + 2] = env.count;
            b[base + 3] = env.sustain;
            b[base + 4] = env.loop_start;
            b[base + 5] = env.loop_end;
            for (k, &(t, v)) in env.nodes.iter().enumerate() {
                b[base + 6 + 4 * k..base + 8 + 4 * k].copy_from_slice(&t.to_le_bytes());
                b[base + 8 + 4 * k..base + 10 + 4 * k].copy_from_slice(&v.to_le_bytes());
            }
        }
        b[0x3C..0x3E].copy_from_slice(&self.fadeout.to_le_bytes());
        b[0x9E..0xA0].copy_from_slice(&(self.slots.len() as u16).to_le_bytes());
        self.block = b;
        self
    }

    fn new(index: u8, name: &[u8]) -> Instrument {
        let mut n = [0u8; 26];
        n.copy_from_slice(&name[..26]);
        Instrument {
            index,
            name: n,
            map: [0; 120],
            modulation: [0; 14],
            volume_env: Envelope::default(),
            pitch_env: Envelope::default(),
            pan_env: Envelope::default(),
            fadeout: 0,
            slots: Vec::new(),
            block: [0; 0xA0],
        }
    }
}

/// The loaded song: Galaxy's song globals.
#[derive(Debug, Clone)]
pub struct Song {
    /// `106981e0`.
    pub name: [u8; 26],
    /// `10697cf0` (GV 128 stays 0x80).
    pub global_volume: u8,
    /// `10697cea`.
    pub initial_speed: u16,
    /// `1069817e`.
    pub initial_tempo: u16,
    /// `1069834a`: index of the last order (0xFF for an empty list).
    pub last_order: u8,
    /// `10698220`.
    pub orders: [u8; 256],
    /// `106886c0`: 0..0x7F or 0xA0.
    pub channel_pan: [u8; 32],
    /// `106882a0`.
    pub instruments: Vec<Option<std::sync::Arc<Instrument>>>,
    /// `10697d40`: each pattern's single track event stream.
    pub patterns: Vec<Option<Vec<u8>>>,
    /// `10698348`: highest committed channel + 1.
    pub channels: u8,
    /// `10697ce8`: 2 linear slides, 3 Amiga slides.
    pub mode: u8,
    /// `106886a6`, `10698180`.
    pub porta_up_limit: u16,
    pub porta_down_limit: u16,
}

/// Galaxy's memory stream (`10618aa0`.. `10618bd0`) over the music bytes plus the 0x400 bytes the subsystem appends.
struct Stream<'a> {
    data: &'a [u8],
    size: usize,
    pos: usize,
}

impl Stream<'_> {
    /// SEEK_SET, clamped to the size.
    fn seek_set(&mut self, off: u32) {
        self.pos = (off as usize).min(self.size);
    }
    /// SEEK_CUR with 32-bit wrap; past the end clamps to the size.
    fn seek_cur(&mut self, off: i64) {
        let p = (self.pos as i64 + off).rem_euclid(1 << 32) as usize;
        self.pos = if p > self.size { self.size } else { p };
    }
    fn tell(&self) -> usize {
        self.pos
    }
    fn byte(&self, i: usize) -> u8 {
        // the appended 0x400 bytes are undefined in the original; zeros here
        self.data.get(i).copied().unwrap_or(0)
    }
    /// Reads `count` items of `size` bytes into `dst` (as far as available); returns the items read.
    fn read(&mut self, dst: &mut [u8], size: usize, count: usize) -> usize {
        let avail = self.size - self.pos;
        let count = if size * count > avail { avail / size } else { count };
        for i in 0..size * count {
            dst[i] = self.byte(self.pos + i);
        }
        self.pos += size * count;
        count
    }
    fn read_vec(&mut self, n: usize) -> Vec<u8> {
        let mut v = vec![0u8; n];
        self.read(&mut v, 1, n);
        v
    }
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// `(x << 9)` truncated to 16 bits, then capped at 0x7FFF.
fn level9(x: u8) -> u16 {
    ((x as u32) << 9).min(0x7FFF) as u16
}

/// Loads an Impulse Tracker module the way Galaxy's IT loader does.
pub fn load(data: &[u8]) -> Result<Song, LoadError> {
    if data.is_empty() {
        return Err(LoadError::NoData);
    }
    if data.len() < 4 || &data[0..4] != b"IMPM" {
        return Err(LoadError::UnsupportedFormat);
    }
    let mut s = Stream { data, size: data.len() + 0x400, pos: 0 };
    let base = s.tell() as u32;
    let mut hdr = [0u8; 0xC0];
    s.read(&mut hdr, 1, 0xC0);

    let mut name = [0u8; 26];
    name.copy_from_slice(&hdr[4..30]);
    // GV is clamped as a signed byte: 0x80..0xFF stay
    let gv = hdr[0x30];
    // (GV is clamped to 0x7F when, read as a signed byte, it is above 127: never, so 0x80..0xFF stay)
    let global_volume = gv;
    let initial_speed = hdr[0x32] as u16;
    let initial_tempo = hdr[0x33] as u16;
    let ord_num = u16_at(&hdr, 0x20);
    let ins_num = u16_at(&hdr, 0x22) as usize;
    let smp_num = u16_at(&hdr, 0x24) as usize;
    let pat_num = u16_at(&hdr, 0x26) as usize;
    let flags = u16_at(&hdr, 0x2C);

    // orders
    let n = ord_num.min(0xFF) as usize;
    let mut orders = [0u8; 256];
    s.read(&mut orders, 1, n);
    s.seek_cur(ord_num as i64 - n as i64);
    let read_table = |s: &mut Stream, count: usize| -> Vec<u32> {
        let raw = s.read_vec(count * 4);
        (0..count).map(|i| u32_at(&raw, i * 4)).collect()
    };
    let ins_off = read_table(&mut s, ins_num);
    let smp_off = read_table(&mut s, smp_num);
    let mut pat_off = read_table(&mut s, pat_num);
    pat_off.resize(pat_off.len().max(256), 0);

    let mut channel_pan = [0u8; 32];
    for (c, pan) in channel_pan.iter_mut().enumerate() {
        let p = hdr[0x40 + c];
        *pan = if p <= 0x40 { ((p as u32 * 2) as u8).min(0x7F) } else { 0xA0 };
    }
    let k = orders.iter().position(|&o| o == 0xFF).unwrap_or(0x100);
    let mut last = n as u8;
    if k < last as usize {
        last = k as u8;
    }
    let last_order = last.wrapping_sub(1);

    let mut instruments: Vec<Option<std::sync::Arc<Instrument>>> = vec![None; 256];
    if flags & 4 == 0 {
        // sample mode: one instrument per sample, every note plays slot 0
        for i in 0..smp_num {
            s.seek_set(base.wrapping_add(smp_off[i]));
            let sb = s.read_vec(0x50);
            let mut rec = Instrument::new(i as u8, &sb[0x14..0x2E]);
            if &sb[0..4] == b"IMPS" {
                let mut slot = Slot { name: sb[0x14..0x2E].to_vec(), ..Slot::default() };
                fill_slot(&mut slot, &sb);
                s.seek_set(base.wrapping_add(u32_at(&sb, 0x48)));
                read_sample(&mut slot, &mut s)?;
                rec.slots.push(slot);
            }
            instruments[i] = Some(std::sync::Arc::new(rec.seal()));
        }
    } else {
        for j in 0..ins_num {
            s.seek_set(base.wrapping_add(ins_off[j]));
            let ib = s.read_vec(0x226);
            let mut rec = Instrument::new(j as u8, &ib[0x20..0x3A]);
            if &ib[0..4] == b"IMPI" {
                let mut counter = 0u8;
                let mut table = [0u8; 256];
                for nn in 0..120 {
                    let smp = ib[0x41 + 2 * nn];
                    if smp != 0 {
                        if table[smp as usize - 1] == 0 {
                            counter = counter.wrapping_add(1);
                            table[smp as usize - 1] = counter;
                        }
                        rec.map[nn] = table[smp as usize - 1].wrapping_sub(1);
                    }
                }
                // the volume envelope's sustain node is IT's SLB, the pitch and pan envelopes' is SLE
                rec.volume_env = envelope(&ib[0x130..], ib[0x130], 4, |y| level9(y));
                // the pitch envelope's sustain bit comes from the pan envelope's flags
                rec.pitch_env = envelope(&ib[0x1D4..], ib[0x182], 5, |y| ((y as i8 as i32) << 7) as u16);
                rec.pan_env = envelope(&ib[0x182..], ib[0x182], 5, |y| {
                    let v = (y as i8 as i32) << 10;
                    if v > 0x7FFF { 0x7FFF } else { v as u16 }
                });
                let fo = (u16_at(&ib, 0x14) as u32) << 5;
                rec.fadeout = (fo as u16).min(0x7FFF);
                for t in 0..counter {
                    let si = table.iter().position(|&x| x == t + 1).unwrap();
                    s.seek_set(base.wrapping_add(*smp_off.get(si).unwrap_or(&0)));
                    let sb = s.read_vec(0x50);
                    let mut slot = Slot { name: sb[0x14..0x2A].to_vec(), ..Slot::default() };
                    if u32_at(&sb, 0x30) != 0 {
                        fill_slot(&mut slot, &sb);
                        s.seek_set(base.wrapping_add(u32_at(&sb, 0x48)));
                        read_sample(&mut slot, &mut s)?;
                    }
                    rec.slots.push(slot);
                }
            }
            instruments[j] = Some(std::sync::Arc::new(rec.seal()));
        }
    }

    // patterns, in order-list order of first use; decoder state persists over the whole song
    let mut st = Unpack::default();
    let mut channels_max = 0u8;
    let mut patterns: Vec<Option<Vec<u8>>> = vec![None; 256];
    let mut pbuf = vec![0u8; 0xE008];
    for o in 0..=last_order as usize {
        let p = orders[o] as usize;
        if p >= 0xFE || patterns[p].is_some() || pat_off[p] == 0 {
            continue;
        }
        s.seek_set(base.wrapping_add(pat_off[p]));
        s.read(&mut pbuf[..8], 1, 8);
        let len = u16_at(&pbuf, 0) as usize;
        let rows = u16_at(&pbuf, 2);
        let mut tmp = vec![0u8; len];
        let got = s.read(&mut tmp, 1, len);
        // the scratch buffer keeps whatever an earlier pattern left past the bytes read
        let end = (8 + got).min(pbuf.len());
        pbuf[8..end].copy_from_slice(&tmp[..end - 8]);
        patterns[p] = Some(st.unpack(&pbuf[8..], rows, &mut channels_max));
    }
    Ok(Song {
        name,
        global_volume,
        initial_speed,
        initial_tempo,
        last_order,
        orders,
        channel_pan,
        instruments,
        patterns,
        channels: channels_max.wrapping_add(1),
        mode: (((flags & 8) >> 3) as u8 ^ 1) | 2,
        porta_up_limit: 0x1C5,
        porta_down_limit: 0xFF00,
    })
}

/// One envelope block from an IT envelope header (Flg, Num, LpB, LpE, SLB, SLE, nodes of {y, tick u16}).
fn envelope(e: &[u8], sustain_flags: u8, sustain_at: usize, value: impl Fn(u8) -> u16) -> Envelope {
    let flg = e[0];
    let mut env = Envelope {
        flags: (flg & 1) | ((flg & 2) << 1) | ((sustain_flags & 4) >> 1),
        count: e[1] & 0xF,
        sustain: e[sustain_at] & 0xF,
        loop_start: e[2] & 0xF,
        loop_end: e[3] & 0xF,
        nodes: [(0, 0); 10],
    };
    for k in 0..10 {
        let y = e[6 + 3 * k];
        let tick = u16_at(e, 7 + 3 * k);
        env.nodes[k] = (tick, value(y));
    }
    env
}

/// Fills a sample slot from an IT sample header (`IMPS`, 0x50 bytes).
fn fill_slot(slot: &mut Slot, sb: &[u8]) {
    slot.length = u32_at(sb, 0x30);
    slot.loop_start = u32_at(sb, 0x34);
    slot.loop_end = u32_at(sb, 0x38);
    slot.volume = level9(sb[0x13]);
    let flg = sb[0x12] as u16;
    let cvt = sb[0x2E] as u16;
    let dfp = sb[0x2F] as u16;
    slot.flags = 0x80 | ((flg & 2) << 1) | ((flg & 0x10) >> 1) | ((flg & 0x40) >> 2) | ((dfp & 0x80) >> 2) | (((cvt & 1) ^ 1) << 1);
    slot.pan = level9(sb[0x2F] & 0x7F);
    slot.flags2 = 0;
    slot.c5speed = u32_at(sb, 0x3C);
}

/// Galaxy's `/ 32` on a signed product: truncates toward zero.
fn div32(x: i32) -> i32 {
    (x + ((x >> 31) & 31)) >> 5
}

/// The sample reader (`1062fb10`): reads and converts one slot's PCM, then writes the 31-frame tail and the
/// 32-frame fade-in.
fn read_sample(slot: &mut Slot, s: &mut Stream) -> Result<(), LoadError> {
    if slot.flags & 0x08 == 0 {
        slot.flags &= 0x00E7;
    }
    if slot.volume > 0x7FFF {
        slot.volume = 0x7FFF;
    }
    if slot.loop_end > slot.length {
        slot.loop_end = slot.length;
    }
    if slot.loop_end == slot.loop_start {
        slot.flags &= 0x00E7;
    }
    if slot.length == 0 {
        return Ok(());
    }
    let len = slot.length as usize;
    let wide = slot.flags & 0x04 != 0;
    let bpf = if wide { 2 } else { 1 };
    let mut raw = vec![0u8; len * bpf];
    let got = s.read(&mut raw, bpf, len);
    let mut pcm = if wide {
        Pcm::S16((0..len + 31).map(|i| if i < got { i16::from_le_bytes([raw[2 * i], raw[2 * i + 1]]) } else { 0 }).collect())
    } else {
        Pcm::S8((0..len + 31).map(|i| if i < got { raw[i] as i8 } else { 0 }).collect())
    };
    if got < len {
        slot.pcm = Some(std::sync::Arc::new(pcm));
        return Err(LoadError::ShortSample);
    }
    slot.tagged = true;
    if slot.flags & 0x43 != 0 {
        if slot.flags & 0x40 != 0 && slot.flags2 & 0x80 == 0 && len >= 2 {
            let src: Vec<i32> = (0..len).map(|i| pcm.get(i)).collect();
            for i in 0..len / 2 {
                pcm.set(i, src[2 * i]);
                pcm.set(len / 2 + i, src[2 * i + 1]);
            }
        }
        let mut acc: u16 = 0;
        for i in 0..len {
            if slot.flags & 0x01 != 0 {
                if !wide {
                    acc = acc.wrapping_add(pcm.get(i) as i8 as i16 as u16);
                    pcm.set(i, acc as u8 as i8 as i32);
                } else if slot.flags & 0x20 != 0 {
                    acc = acc.wrapping_add(pcm.get(i) as u16);
                    pcm.set(i, acc as i16 as i32);
                } else {
                    let w = pcm.get(i) as u16;
                    acc = acc.wrapping_add((w as u8) as i8 as i16 as u16);
                    let lo = acc as u8;
                    acc = acc.wrapping_add(((w >> 8) as u8) as i8 as i16 as u16);
                    let hi = acc as u8;
                    pcm.set(i, u16::from_le_bytes([lo, hi]) as i16 as i32);
                }
            }
            if slot.flags & 0x02 != 0 {
                if wide {
                    pcm.set(i, (pcm.get(i) as u16).wrapping_add(0x8000) as i16 as i32);
                } else {
                    pcm.set(i, (pcm.get(i) as u8).wrapping_add(0x80) as i8 as i32);
                }
            }
        }
        if slot.flags2 & 0x01 == 0 {
            slot.flags &= !0x0003;
            slot.flags2 |= 0x80;
        }
    }
    // the tail past the end (or past the loop end)
    let le = slot.loop_end as usize;
    let ls = slot.loop_start as usize;
    if slot.flags & 0x08 == 0 {
        let last = pcm.get(len - 1);
        for k in 0..31 {
            pcm.set(len + k, div32(last * (32 - k as i32)));
        }
    } else if slot.flags & 0x10 == 0 {
        for k in 0..31 {
            let v = pcm.get(ls + k);
            pcm.set(le + k, v);
        }
    } else {
        for k in 0..31 {
            let v = if le >= 1 + k { pcm.get(le - 1 - k) } else { 0 };
            pcm.set(le + k, v);
        }
    }
    for i in 0..32 {
        let v = pcm.get(i);
        pcm.set(i, div32(v * i as i32));
    }
    slot.pcm = Some(std::sync::Arc::new(pcm));
    Ok(())
}

/// The packed-pattern decoder's per-channel state, zeroed once per song.
struct Unpack {
    mask: [u8; 64],
    note: [u8; 64],
    ins: [u8; 64],
    vol: [u8; 64],
    cmd: [u8; 64],
    /// The effect memory: one byte per channel shared by all effects.
    mem: [u8; 64],
}

impl Default for Unpack {
    fn default() -> Self {
        Unpack { mask: [0; 64], note: [0; 64], ins: [0; 64], vol: [0; 64], cmd: [0; 64], mem: [0; 64] }
    }
}

impl Unpack {
    /// Expands one packed IT pattern into Galaxy's event stream.
    fn unpack(&mut self, src: &[u8], rows: u16, channels_max: &mut u8) -> Vec<u8> {
        let mut out = vec![rows.wrapping_sub(1) as u8];
        let mut i = 0usize;
        let next = |i: &mut usize| -> u8 {
            let b = src.get(*i).copied().unwrap_or(0);
            *i += 1;
            b
        };
        for _ in 0..rows {
            loop {
                let cv = next(&mut i);
                if cv == 0 {
                    out.push(0);
                    break;
                }
                let ch = ((cv - 1) & 0x3F) as usize;
                let mask = if cv & 0x80 != 0 { next(&mut i) } else { self.mask[ch] };
                self.mask[ch] = mask;
                let (mut note, mut ins, mut vol, mut cmd, mut param) = (0u8, 0u8, 0xFFu8, 0u8, 0u8);
                if mask & 0x01 != 0 {
                    let mut n = next(&mut i);
                    if n < 0x78 {
                        n += 1;
                    }
                    if n == 0xFE {
                        n = 0;
                        vol = 0;
                    } else if n == 0xFF {
                        n = 0x80;
                    }
                    note = n;
                    self.note[ch] = n;
                }
                if mask & 0x02 != 0 {
                    ins = next(&mut i);
                    self.ins[ch] = ins;
                }
                if mask & 0x04 != 0 {
                    let mut v = next(&mut i);
                    if v <= 0x40 {
                        v = (v * 2).min(0x7F);
                        vol &= v;
                    }
                    self.vol[ch] = v;
                }
                if mask & 0x08 != 0 {
                    cmd = next(&mut i);
                    param = next(&mut i);
                    self.cmd[ch] = cmd;
                    if !matches!(cmd, 1 | 4 | 5 | 6 | 11 | 12) {
                        self.mem[ch] = param;
                    }
                }
                if mask & 0x10 != 0 {
                    note = self.note[ch];
                }
                if mask & 0x20 != 0 {
                    ins = self.ins[ch];
                }
                if mask & 0x40 != 0 && self.vol[ch] < 0x80 {
                    vol &= self.vol[ch];
                }
                if mask & 0x80 != 0 {
                    cmd = self.cmd[ch];
                    param = self.mem[ch];
                }
                let (c, p) = self.translate(ch, cmd, param);
                let start = out.len();
                out.push(ch as u8);
                if c != 0 || p != 0 {
                    out.push(p);
                    out.push(c);
                    out[start] |= 0x80;
                }
                if note != 0 || ins != 0 {
                    out.push(ins);
                    out.push(note);
                    out[start] |= 0x40;
                }
                if vol != 0xFF {
                    out.push(vol);
                    out[start] |= 0x20;
                }
                if out[start] & 0xE0 != 0 && ch < 32 {
                    *channels_max = (*channels_max).max(ch as u8);
                } else {
                    out.truncate(start);
                }
            }
        }
        out
    }

    /// IT effect -> Galaxy's internal command and parameter (loader.md 4.11).
    fn translate(&mut self, ch: usize, cmd: u8, p: u8) -> (u8, u8) {
        let mem = |m: &mut [u8; 64], p: u8| -> u8 {
            let p = if p == 0 { m[ch] } else { p };
            m[ch] = p;
            p
        };
        match cmd {
            1 => (0x0F, mem(&mut self.mem, p).min(0x20)),
            2 => (0x0B, p),
            3 => (0x0D, p),
            4 => {
                let p = mem(&mut self.mem, p);
                if p & 0x0F == 0x0F && p & 0xF0 != 0 {
                    (0x0E, 0xA0 | (p >> 4))
                } else if p >= 0xF1 {
                    (0x0E, 0xB0 | (p & 0x0F))
                } else if p & 0x0F != 0 {
                    (0x0A, p & 0x0F)
                } else {
                    (0x0A, p)
                }
            }
            5 | 6 => {
                let p = mem(&mut self.mem, p);
                let (fine, coarse) = if cmd == 5 { (0x20, 0x02) } else { (0x10, 0x01) };
                if p >= 0xF0 {
                    (0x0E, fine | (p & 0xF))
                } else if p >= 0xE0 {
                    (0x0E, fine | (((p & 0xF) + 2) >> 2))
                } else {
                    (coarse, p)
                }
            }
            7 => (0x03, p),
            8 => (0x04, p),
            10 => (0x00, p),
            11 => (0x06, mem(&mut self.mem, p)),
            12 => (0x05, mem(&mut self.mem, p)),
            15 => (0x09, p),
            17 => (0x0E, 0x90 | (p & 0x0F)),
            18 => (0x07, p),
            19 => {
                let x = p & 0x0F;
                match p >> 4 {
                    0x0 => (0x0E, x),
                    0x1 => (0x0E, 0x30 | x),
                    0x3 => (0x0E, 0x40 | x),
                    0x4 => (0x0E, 0x70 | x),
                    0x8 => (0x0E, 0x80 | x),
                    0x9 => (0x08, 0xA4),
                    0xB => (0x0E, 0x60 | x),
                    0xC => (0x0E, 0xC0 | x),
                    0xD => (0x0E, 0xD0 | x),
                    0xE => (0x0E, 0xE0 | x),
                    0xF => (0x0E, 0xF0 | x),
                    _ => (0, 0),
                }
            }
            20 => {
                if p >= 0x20 {
                    (0x0F, p)
                } else {
                    (0x14, p)
                }
            }
            24 => (0x08, p),
            _ => (0, 0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn div32_truncates_toward_zero() {
        assert_eq!(div32(-33), -1);
        assert_eq!(div32(-31), 0);
        assert_eq!(div32(63), 1);
    }

    /// A minimal module: one sample-mode sample, one pattern with a note cut and a remembered effect.
    #[test]
    fn unpack_applies_galaxy_conversions() {
        let mut u = Unpack::default();
        let mut max = 0;
        // row 0: ch1 mask 0x0F note C-5(60) ins 1 vol 64 effect D 0x0F (fine down via low nibble... D0F -> slide down)
        // row 1: ch1 mask 0x01 note cut; row 2: ch2 mask 0x88 S91 then ch2 mask 0x80 repeat
        let src = [
            0x81, 0x0F, 60, 1, 64, 4, 0x0F, 0, // row 0
            0x81, 0x01, 0xFE, 0, // row 1
            0x82, 0x08, 19, 0x91, 0x82, 0x80, 0, // row 2
        ];
        let out = u.unpack(&src, 3, &mut max);
        assert_eq!(out[0], 2);
        // row 0: head 0xE0|0, param 0x0F cmd 0x0A, ins 1 note 61, vol 0x7F
        assert_eq!(&out[1..8], &[0xE0, 0x0F, 0x0A, 1, 61, 0x7F, 0]);
        // row 1: the cut becomes volume 0 with no note
        assert_eq!(&out[8..11], &[0x20, 0, 0]);
        // row 2: S91 -> 8A4; the repeat uses the memory (0x91) -> again 8A4
        assert_eq!(&out[11..], &[0x81, 0xA4, 0x08, 0x81, 0xA4, 0x08, 0]);
        assert_eq!(max, 1);
    }
}
