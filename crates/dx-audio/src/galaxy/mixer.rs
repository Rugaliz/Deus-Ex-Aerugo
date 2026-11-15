//! Galaxy's software mixer (docs/galaxy/mixer.md): 256 voice records, the chunk loop with its envelope and
//! sequencer ticks (10614920), the per-voice segment mix with the SSE voice mixer's arithmetic (10622730), the
//! envelope/pitch engine of music voices, master and voice fades (10617f50) and the bus converter (10622fe4).
use super::sample::{Sample, StreamSource as StreamSourceRef};
use super::song::{Instrument, Pcm};
use super::tables::{pan_table, pitch_table, WAVES};
use std::sync::Arc;

pub const VOICES: usize = 256;
/// Frames per mix chunk.
pub const CHUNK: usize = 512;

/// A voice record (`VOIC`, 0xa0 bytes in the original); field comments give the original offsets.
#[derive(Debug, Clone)]
pub struct Voice {
    /// +0x08 age stamp.
    pub age: u32,
    /// +0x0c (bit 7 = key off).
    pub b0c: u8,
    /// +0x0d instrument index (music voices), +0x0e.
    pub b0d: u8,
    pub b0e: u8,
    /// +0x0f.
    pub playing: bool,
    /// +0x10.
    pub enabled: bool,
    /// +0x11: bit 7 music bus, bit 3 primary, bit 1 end callback.
    pub b11: u8,
    /// +0x12 squared level `c*c*2` of +0x3f.
    pub level_sq: u16,
    /// +0x14 current pan (bit 15 surround), +0x16 target pan.
    pub pan: u16,
    pub pan_target: u16,
    /// +0x18, +0x1a volume factors.
    pub vol18: u16,
    pub vol1a: u16,
    /// +0x1c chorus send, +0x1e reverb send (x/128).
    pub chorus: u16,
    pub reverb: u16,
    /// +0x20 the instrument whose envelopes the envelope tick runs (none for game sounds).
    pub instrument: Option<Arc<Instrument>>,
    /// Volume envelope: value +0x24, slope +0x26, tick +0x28, depth/fade-out +0x2a, node +0x2c.
    pub venv: EnvState,
    /// Pitch envelope: value +0x34, slope +0x32, tick +0x30, depth +0x2e, node +0x2d.
    pub penv: EnvState,
    /// Pan envelope: value +0x36, slope +0x38, tick +0x3a, depth +0x3c, node +0x3e.
    pub panenv: EnvState,
    /// +0x3f level, +0x40 fade step, +0x41 target.
    pub level: u8,
    pub level_step: i8,
    pub level_target: u8,
    /// +0x42/+0x46: the sample data (positions index it).
    pub data: Option<Arc<Pcm>>,
    /// +0x46 position, +0x4a fraction.
    pub pos: i32,
    pub frac: u16,
    /// +0x4c sample flags: 0x04 16-bit, 0x08 loop, 0x10 ping-pong, 0x80 stream.
    pub sflags: u16,
    /// +0x4e start, +0x52 loop start, +0x56 loop end (swapped while moving backwards), +0x5a end.
    pub start: i32,
    pub loop_start: i32,
    pub loop_end: i32,
    pub end: i32,
    /// +0x5e base frequency in Hz (music: C5 speed).
    pub freq: u32,
    /// +0x62 step, 16.16.
    pub step: i32,
    /// +0x66 volume after tremolo, +0x68 volume base.
    pub vol66: u16,
    pub vol68: u16,
    /// +0x6a effective pitch, +0x6c pitch (portamento current).
    pub pitch: u16,
    pub pitch_base: u16,
    /// +0x6e vibrato depth, +0x70 speed, +0x72 phase, +0x74 waveform.
    pub vib_depth: i16,
    pub vib_speed: u16,
    pub vib_phase: u16,
    pub vib_wave: u8,
    /// +0x75 tremolo waveform, +0x76 phase, +0x78 speed, +0x7a depth.
    pub trem_wave: u8,
    pub trem_phase: u16,
    pub trem_speed: u16,
    pub trem_depth: i16,
    /// +0x7c portamento target, +0x7e speed.
    pub porta_target: u16,
    pub porta_speed: u16,
    /// +0x80 pitch offset (arpeggio).
    pub pitch_offset: i16,
    /// +0x82 vibrato, +0x83 tremolo, +0x84 tone portamento, +0x85 sample offset memories.
    pub vib_mem: u8,
    pub trem_mem: u8,
    pub porta_mem: u8,
    pub offset_mem: u8,
    /// +0x86 current effect command, +0x87 parameter.
    pub cmd: u8,
    pub param: u8,
    /// +0x42 the current sample slot (its instrument and index).
    pub slot: Option<(Arc<Instrument>, usize)>,
    /// +0x9c the linked voice (the second voice of a stereo sound).
    pub link: Option<usize>,
    /// +0x98 the voice has its mutex (started by the effect start, not stopped since).
    pub owned: bool,
    /// +0x88.. a streamed voice's ring state.
    pub stream: Option<StreamState>,
}

/// A streamed voice's ring (sounds.md section 9): the source it is filled from and the write and source positions.
#[derive(Debug, Clone)]
pub struct StreamState {
    pub sample: Arc<Sample>,
    /// This voice plays the right channel of the source.
    pub right: bool,
    /// +0x88 ring write position in bytes (0x10000 = not started), +0x8c source position (samples here).
    pub write: u32,
    pub srcpos: usize,
}

/// One envelope's running state in a voice.
#[derive(Debug, Clone, Copy, Default)]
pub struct EnvState {
    pub value: i16,
    pub slope: i16,
    pub tick: u16,
    pub node: u8,
    pub depth: u16,
}

impl Voice {
    /// The library-init defaults (1060df30).
    pub fn new() -> Voice {
        Voice {
            age: 0,
            b0c: 0xbc,
            b0d: 0,
            b0e: 0,
            playing: false,
            enabled: false,
            b11: 8,
            level_sq: 0x7e02,
            pan: 0x4000,
            pan_target: 0x4000,
            vol18: 0,
            vol1a: 0x7fff,
            chorus: 0,
            reverb: 0x7f,
            instrument: None,
            venv: EnvState::default(),
            penv: EnvState::default(),
            panenv: EnvState::default(),
            level: 0x7f,
            level_step: 0,
            level_target: 0x7f,
            data: None,
            pos: 0,
            frac: 0,
            sflags: 0,
            start: 0,
            loop_start: 0,
            loop_end: 0,
            end: 0,
            freq: 0,
            step: 0,
            vol66: 0,
            vol68: 0,
            pitch: 0,
            pitch_base: 0,
            vib_depth: 0,
            vib_speed: 0,
            vib_phase: 0,
            vib_wave: 0,
            trem_wave: 0,
            trem_phase: 0,
            trem_speed: 0,
            trem_depth: 0,
            porta_target: 0,
            porta_speed: 0,
            pitch_offset: 0,
            vib_mem: 0,
            trem_mem: 0,
            porta_mem: 0,
            offset_mem: 0,
            cmd: 0,
            param: 0,
            slot: None,
            link: None,
            owned: false,
            stream: None,
        }
    }

    /// The record in the original's 0xa0-byte layout, for comparing with the Galaxy oracle's dumps. Pointers are 0;
    /// positions are relative to the sample start (the original's are absolute).
    pub fn record(&self) -> [u8; 0xa0] {
        let mut r = [0u8; 0xa0];
        let mut put = |o: usize, b: &[u8]| r[o..o + b.len()].copy_from_slice(b);
        put(0x08, &self.age.to_le_bytes());
        put(0x0c, &[self.b0c, self.b0d, self.b0e, self.playing as u8, self.enabled as u8, self.b11]);
        for (o, v) in [(0x12, self.level_sq), (0x14, self.pan), (0x16, self.pan_target), (0x18, self.vol18), (0x1a, self.vol1a), (0x1c, self.chorus), (0x1e, self.reverb)] {
            put(o, &v.to_le_bytes());
        }
        put(0x24, &self.venv.value.to_le_bytes());
        put(0x26, &self.venv.slope.to_le_bytes());
        put(0x28, &self.venv.tick.to_le_bytes());
        put(0x2a, &self.venv.depth.to_le_bytes());
        put(0x2c, &[self.venv.node, self.penv.node]);
        put(0x2e, &self.penv.depth.to_le_bytes());
        put(0x30, &self.penv.tick.to_le_bytes());
        put(0x32, &self.penv.slope.to_le_bytes());
        put(0x34, &self.penv.value.to_le_bytes());
        put(0x36, &self.panenv.value.to_le_bytes());
        put(0x38, &self.panenv.slope.to_le_bytes());
        put(0x3a, &self.panenv.tick.to_le_bytes());
        put(0x3c, &self.panenv.depth.to_le_bytes());
        put(0x3e, &[self.panenv.node, self.level, self.level_step as u8, self.level_target]);
        put(0x46, &(self.pos - self.start).to_le_bytes());
        put(0x4a, &self.frac.to_le_bytes());
        put(0x4c, &self.sflags.to_le_bytes());
        for (o, v) in [(0x52, self.loop_start - self.start), (0x56, self.loop_end - self.start), (0x5a, self.end - self.start)] {
            put(o, &v.to_le_bytes());
        }
        put(0x5e, &self.freq.to_le_bytes());
        put(0x62, &self.step.to_le_bytes());
        for (o, v) in [(0x66, self.vol66), (0x68, self.vol68), (0x6a, self.pitch), (0x6c, self.pitch_base)] {
            put(o, &v.to_le_bytes());
        }
        put(0x6e, &self.vib_depth.to_le_bytes());
        put(0x70, &self.vib_speed.to_le_bytes());
        put(0x72, &self.vib_phase.to_le_bytes());
        put(0x74, &[self.vib_wave, self.trem_wave]);
        put(0x76, &self.trem_phase.to_le_bytes());
        put(0x78, &self.trem_speed.to_le_bytes());
        put(0x7a, &self.trem_depth.to_le_bytes());
        put(0x7c, &self.porta_target.to_le_bytes());
        put(0x7e, &self.porta_speed.to_le_bytes());
        put(0x80, &self.pitch_offset.to_le_bytes());
        put(0x82, &[self.vib_mem, self.trem_mem, self.porta_mem, self.offset_mem, self.cmd, self.param]);
        r
    }

    pub fn key_off(&self) -> bool {
        self.b0c & 0x80 != 0
    }
}

impl Default for Voice {
    fn default() -> Self {
        Voice::new()
    }
}

/// One stereo f32 bus.
pub type Bus = Vec<[f32; 2]>;

/// Frames per bus buffer (0x1040 bytes in the original).
pub const BUS_FRAMES: usize = 520;
/// The six buses, consecutive in one block as the original allocates them: music dry, effect dry, music reverb
/// send, effect reverb send, music chorus send, effect chorus send. A voice whose segment runs backwards (the
/// degenerate case of mixer.md 5.2) writes across neighbouring buses; writes outside the block are dropped (heap
/// memory in the original).
pub struct Buses {
    pub block: Vec<[f32; 2]>,
}

pub const MUSIC_DRY: usize = 0;
pub const FX_DRY: usize = 1;
pub const MUSIC_REV: usize = 2;
pub const FX_REV: usize = 3;
pub const MUSIC_CHO: usize = 4;
pub const FX_CHO: usize = 5;

impl Buses {
    fn new() -> Buses {
        Buses { block: vec![[0.0f32; 2]; 6 * BUS_FRAMES] }
    }
    pub fn bus(&self, i: usize) -> &[[f32; 2]] {
        &self.block[i * BUS_FRAMES..(i + 1) * BUS_FRAMES]
    }
    pub fn music_dry(&self) -> &[[f32; 2]] {
        self.bus(MUSIC_DRY)
    }
    pub fn fx_dry(&self) -> &[[f32; 2]] {
        self.bus(FX_DRY)
    }
}

/// The mixer's state: voices, masters, tick counters.
pub struct MixState {
    pub voices: Vec<Voice>,
    /// `E` effect voices (`EffectsChannels`), then `M` music voices.
    pub effects: usize,
    pub music: usize,
    /// `0x10697d18` mix rate.
    pub rate: u32,
    /// Music master `0x10697cf0` (i8; the IT loader also writes the song's GV here), target, step.
    pub music_master: u8,
    pub music_target: u8,
    pub music_step: i8,
    /// Effects master `0x1069817c`, target, step.
    pub fx_master: u8,
    pub fx_target: u8,
    pub fx_step: i8,
    /// Envelope and sequencer tick counters and periods (16.16 frames).
    pub env_ctr: i32,
    pub seq_ctr: i32,
    pub env_period: i32,
    pub seq_period: i32,
    /// `0x10698188` running tempo, `0x10698342` running speed (used for the non-IT tick period).
    pub tempo: u16,
    pub speed: u16,
    /// `0x10697ce8` module mode (bit 0 Amiga periods, bit 1 IT timing).
    pub mode: u8,
    /// `0x10697ce0` voice-age clock (+1 per sequencer tick).
    pub age_clock: u32,
    /// `0x10698194` sample clock.
    pub sample_clock: u32,
    /// Hook 2: the effect reverb (reverb.md), run on the effect dry and reverb send buses after each chunk.
    pub reverb: Option<super::reverb::Reverb>,
    buses: Buses,
}

/// `(a / b) << 16 + ((a % b) << 16) / b` in unsigned 32-bit: a 16.16 quotient.
pub fn fixed_div(a: u32, b: u32) -> u32 {
    ((a / b) << 16).wrapping_add(((a % b) << 16) / b)
}

impl MixState {
    /// The state after library init and an output open at `rate` (1060df30, 10613350).
    pub fn new(rate: u32, effects: usize) -> MixState {
        let mut voices = vec![Voice::new(); VOICES];
        for v in voices.iter_mut().take(effects) {
            v.enabled = true;
        }
        let mut s = MixState {
            voices,
            effects,
            music: 0,
            rate,
            music_master: 127,
            music_target: 127,
            music_step: 0,
            fx_master: 127,
            fx_target: 127,
            fx_step: 0,
            env_ctr: 0x160ccc,
            seq_ctr: 0xe5b000,
            env_period: 0,
            seq_period: 0,
            tempo: 120,
            speed: 48,
            mode: 0,
            age_clock: 0,
            sample_clock: 0,
            reverb: None,
            buses: Buses::new(),
        };
        s.set_periods();
        s
    }

    /// Recomputes the tick periods (output open; song start in 10615710).
    pub fn set_periods(&mut self) {
        if self.mode & 2 == 0 {
            self.seq_period = fixed_div(60 * self.rate, self.tempo as u32 * self.speed as u32) as i32;
            self.env_period = ((self.rate << 14) / 250) as i32;
        } else {
            let p = fixed_div(5 * self.rate, 2 * self.tempo as u32) as i32;
            self.env_period = p;
            self.seq_period = p;
        }
    }

    /// Pitch value to the 16.16 step (10615660), negated while the voice moves backwards.
    pub fn pitch_step(&self, v: &Voice, pitch: u32) -> i32 {
        let pitch = pitch as i32;
        let f: u32 = if self.mode & 1 == 0 {
            let q = pitch / 0xc00;
            let r = pitch % 0xc00;
            let prod = pitch_table()[(r >> 2) as usize] as i64 * v.freq as i64;
            (prod >> (10 + q)) as u32
        } else if pitch != 0 {
            (57269824 / pitch) as u32
        } else {
            0
        };
        let step = fixed_div(f, self.rate) as i32;
        if (v.loop_start as u32) > (v.loop_end as u32) {
            step.wrapping_neg()
        } else {
            step
        }
    }

    /// Mixes one chunk of at most 512 frames into the buses (10614920); `seq_tick` is the sequencer tick (the
    /// module player 10615710, run before the fades). Returns the frames mixed.
    pub fn mix_chunk(&mut self, n: usize, seq_tick: &mut dyn FnMut(&mut MixState)) -> usize {
        let n = n.min(CHUNK);
        for i in 0..6 {
            self.buses.block[i * BUS_FRAMES..i * BUS_FRAMES + n].fill([0.0; 2]);
        }
        let mut done = 0usize;
        while done < n {
            let mut seg = ((n - done) as i32) << 16;
            if self.env_ctr < seg {
                seg = self.env_ctr & !0xffff;
            }
            if self.seq_ctr < seg {
                seg = self.seq_ctr & !0xffff;
            }
            self.env_ctr -= seg;
            if self.env_ctr < 0x10000 {
                for i in 0..(self.effects + self.music).min(VOICES) {
                    self.envelope_tick(i);
                }
                self.env_ctr += self.env_period;
            }
            self.seq_ctr -= seg;
            if self.seq_ctr < 0x10000 {
                self.age_clock = self.age_clock.wrapping_add(1);
                seq_tick(self);
                self.stream_notifications();
                self.fades();
                self.seq_ctr += self.seq_period;
            }
            let frames = (seg >> 16) as usize;
            for i in 0..(self.effects + self.music).min(VOICES) {
                self.mix_voice(i, frames, done);
            }
            done += frames;
            self.sample_clock = self.sample_clock.wrapping_add(frames as u32);
        }
        done
    }

    /// The effect hooks after a chunk of `n` frames (mixer.md 4.1 step 2): the effect reverb adds its output to the
    /// effect dry bus. (No music reverb or chorus: Deus Ex's modules carry none.)
    pub fn run_hooks(&mut self, n: usize) {
        if let Some(r) = self.reverb.as_mut() {
            let (lo, hi) = self.buses.block.split_at_mut(FX_REV * BUS_FRAMES);
            r.process(&mut lo[FX_DRY * BUS_FRAMES..FX_DRY * BUS_FRAMES + n], &hi[..n], n);
        }
    }

    /// Installs or removes the effect reverb (106113d0): master gain 0 removes it; otherwise a fresh state (the old
    /// tail is cut). A block the setup rejects (cutoff above the rate) keeps what played before.
    pub fn set_reverb(&mut self, p: &super::reverb::ReverbParams) {
        if p.master == 0.0 {
            self.reverb = None;
        } else if let Some(r) = super::reverb::Reverb::new(p, self.rate) {
            self.reverb = Some(r);
        }
    }

    /// The buses of the last chunk.
    pub fn buses(&self) -> &Buses {
        &self.buses
    }

    /// Master and per-voice fades, then each voice's squared level (10617f50).
    fn fades(&mut self) {
        fn fade_i8(c: &mut u8, g: u8, s: &mut i8) {
            let (ci, gi) = (*c as i8 as i32, g as i8 as i32);
            fade(ci, gi, s).map(|v| *c = v as u8);
        }
        fn fade(c: i32, g: i32, s: &mut i8) -> Option<i32> {
            let d = g - c;
            if g < c && (*s as i32) < d {
                *s = d as i8;
            }
            if g > c && (*s as i32) > d {
                *s = d as i8;
            }
            if g != c {
                Some(c + *s as i32)
            } else {
                None
            }
        }
        fade_i8(&mut self.music_master, self.music_target, &mut self.music_step);
        fade_i8(&mut self.fx_master, self.fx_target, &mut self.fx_step);
        for v in self.voices.iter_mut().take((self.effects + self.music).min(VOICES)) {
            if let Some(c) = fade(v.level as i32, v.level_target as i32, &mut v.level_step) {
                v.level = c as u8;
            }
            let c = v.level as u32;
            v.level_sq = (c * c * 2) as u16;
        }
    }

    /// The modulation tick of one voice (player.md section 10): envelopes, key-off fade, portamento, vibrato and the
    /// step, tremolo.
    fn envelope_tick(&mut self, i: usize) {
        let mode = self.mode;
        let v = &mut self.voices[i];
        if !v.playing {
            return;
        }
        let Some(ins) = v.instrument.clone() else { return };
        let m = &ins.block;
        let key_off = v.key_off();
        for (which, base) in [(0usize, 0x0Eusize), (1, 0x3E), (2, 0x6E)] {
            let st = match which {
                0 => &mut v.venv,
                1 => &mut v.penv,
                _ => &mut v.panenv,
            };
            step_envelope(m, base, st, key_off, which == 0);
        }
        if key_off {
            for (which, base) in [(0usize, 0x0Eusize), (1, 0x3E), (2, 0x6E)] {
                let enabled = m[base] & 1 != 0;
                let fade = u16::from_le_bytes([m[base + 0x2E], m[base + 0x2F]]);
                let st = match which {
                    0 => &mut v.venv,
                    1 => &mut v.penv,
                    _ => &mut v.panenv,
                };
                if !enabled {
                    if which == 0 {
                        v.playing = false;
                    }
                    st.depth = 0;
                    continue;
                }
                if mode & 2 == 0 && st.node <= m[base + 3] {
                    st.node = m[base + 3];
                    st.tick = env_node(m, base, st.node).0;
                }
                if which == 2 {
                    if fade <= st.depth {
                        st.depth -= fade;
                    } else {
                        st.depth = 0;
                    }
                } else if st.depth < fade {
                    if which == 0 {
                        v.playing = false;
                    }
                    st.depth = 0;
                } else {
                    st.depth -= fade;
                }
            }
        }
        // portamento (10.3)
        let mut sp = v.porta_speed as i32;
        let (cur, tgt) = (v.pitch_base as i32, v.porta_target as i32);
        let mut p = cur;
        if p < tgt {
            sp = sp.min(tgt - p);
            p += sp;
        }
        if p > tgt {
            p -= sp.min(p - tgt);
        }
        v.pitch_base = p as u16;
        // final pitch (10.4)
        let ph = v.vib_speed.wrapping_add(v.vib_phase);
        let w = wave(v.vib_wave, ph);
        let pitch = (((w * v.vib_depth as i32) >> 8) + ((v.penv.depth as i32 * v.penv.value as i32) >> 15) + v.pitch_offset as i32 + v.pitch_base as i32).clamp(0, 0x7800);
        v.vib_phase = ph;
        v.pitch = pitch as u16;
        // tremolo (10.5)
        let qh = v.trem_phase.wrapping_add(v.trem_speed);
        let w = wave(v.trem_wave, qh);
        let vol = (((w * v.trem_depth as i32) >> 8) + v.vol68 as i32).clamp(0, 0x7fff);
        v.trem_phase = qh;
        v.vol66 = vol as u16;
        if pitch != 0 {
            let step = self.pitch_step(&self.voices[i], pitch as u32);
            self.voices[i].step = step;
        }
    }

    /// The 16.16 step of a frequency at the mix rate.
    pub fn freq_step(&self, freq: u32) -> i32 {
        fixed_div(freq, self.rate) as i32
    }

    /// Stops a voice and its linked voice (10618410): only voices that still have their mutex change.
    pub fn stop_voice(&mut self, v: usize) {
        let mut cur = Some(v);
        let mut n = 0;
        while let Some(i) = cur {
            n += 1;
            if n > 2 || i >= VOICES {
                break;
            }
            let next = self.voices[i].link;
            let x = &mut self.voices[i];
            if x.owned {
                if x.stream.is_some() {
                    x.stream = None;
                    x.data = None;
                    x.start = 0;
                }
                x.age = 0;
                x.playing = false;
                x.b11 = 8;
                x.owned = false;
                x.link = None;
            }
            cur = next;
        }
    }

    /// Starts a sound on effect voice `c` (0-based; a stereo sound also takes `c + 1`) as the voice start 10612310
    /// does (sounds.md 6): `freq` in Hz, `vol` 0..0x7fff, `pan` 0..0x7fff or 0xc000. Returns whether a voice exists.
    pub fn start_effect(&mut self, c: usize, s: &Arc<Sample>, freq: u32, vol: u16, pan: u16) -> bool {
        if c >= VOICES {
            return false;
        }
        let n = if s.flags & super::sample::F_STEREO != 0 { 2 } else { 1 };
        let streamed = s.streamed();
        let age = self.age_clock;
        let step = self.freq_step(freq);
        let linear = self.mode & 1 == 0;
        for k in 0..n {
            let vi = c + k;
            if vi >= VOICES {
                break;
            }
            self.stop_voice(vi);
            let has_data = if streamed { s.stream.is_some() } else { s.data.is_some() };
            let v = &mut self.voices[vi];
            v.link = None;
            if !has_data {
                continue;
            }
            v.owned = true;
            v.age = if streamed && k != 0 { 0xffff_fffe } else { age };
            v.b0c = 0x3c;
            v.b0d = 0;
            v.b0e = 0;
            v.playing = true;
            v.enabled = true;
            v.b11 = if k == 0 { 8 } else { 0 };
            v.vol18 = 0x7fff;
            v.vol1a = 0x7fff;
            v.venv = EnvState { value: 0x7fff, slope: v.venv.slope, tick: 0, node: 0, depth: 0x7fff };
            v.penv = EnvState { value: 0, slope: v.penv.slope, tick: 0, node: 0, depth: 0 };
            v.panenv = EnvState { value: 0, slope: v.panenv.slope, tick: 0, node: 0, depth: 0 };
            v.instrument = None;
            v.slot = None;
            v.frac = 0;
            if streamed {
                // a ring of 0x10000 bytes (zeroed) plus 31 bytes; the voice loops over its first 0x7fff samples
                let bytes = s.stream.as_ref().unwrap().bytes();
                v.data = Some(Arc::new(if bytes { Pcm::S8(vec![0; 0x1001f]) } else { Pcm::S16(vec![0; 0x8010]) }));
                v.sflags = (s.flags & 0xff00) | ((s.flags | 0x88) & 0xff);
                v.start = 0;
                v.pos = 0;
                v.loop_start = 0;
                v.loop_end = 0x7fff;
                v.end = 0x7fff;
                v.stream = Some(StreamState { sample: s.clone(), right: k == 1, write: 0x10000, srcpos: 0 });
            } else {
                v.data = s.data.clone();
                v.sflags = s.flags;
                v.start = 0;
                v.pos = 0;
                v.loop_start = s.loop_start as i32;
                v.loop_end = s.loop_end as i32;
                v.end = s.length as i32;
                v.stream = None;
            }
            v.freq = freq;
            v.vol66 = vol;
            v.vol68 = vol;
            v.pan = pan;
            v.pan_target = pan;
            v.step = step;
            let p = if linear {
                0x3c00
            } else if freq != 0 {
                ((6848u32 * 8363) / freq) as u16
            } else {
                0
            };
            v.pitch = p;
            v.pitch_base = p;
            v.vib_phase = 0;
            v.trem_phase = 0;
        }
        if n == 2 && c + 1 < VOICES && self.voices[c + 1].owned {
            self.voices[c].link = Some(c + 1);
        }
        self.voices[c].owned
    }

    /// The per-frame update of a playing effect voice and its linked voice (1060d280).
    pub fn update_effect(&mut self, v: usize, freq: u32, vol: u16, pan: u16) {
        let step = self.freq_step(freq);
        let mut cur = Some(v);
        let mut n = 0;
        while let Some(i) = cur {
            n += 1;
            if n > 2 || i >= VOICES {
                break;
            }
            let x = &mut self.voices[i];
            x.freq = freq;
            x.vol66 = vol;
            x.vol68 = vol;
            x.pan = pan;
            x.pan_target = pan;
            x.step = step;
            cur = x.link;
        }
    }

    /// On each sequencer tick, the stream thread's fill for every enabled, playing, primary, streamed effect voice
    /// (mixer.md 4.4, sounds.md 9.3), run at once with the play position of the tick.
    fn stream_notifications(&mut self) {
        for i in 0..self.effects.min(VOICES) {
            let v = &self.voices[i];
            if v.enabled && v.playing && v.b11 & 8 != 0 && (v.sflags & 0xe000 != 0 || v.sflags & 0x40 != 0) && v.stream.is_some() {
                let p = (v.pos - v.start) as u32;
                self.stream_fill(i, p);
            }
        }
    }

    /// One fill of a voice's ring (sounds.md 9.3) at play position `p` (samples from the ring start).
    fn stream_fill(&mut self, vi: usize, p: u32) {
        let partner = self.voices[vi].link;
        let Some(st) = self.voices[vi].stream.clone() else { return };
        let src = st.sample.stream.clone().unwrap();
        let w2: u32 = if self.voices[vi].sflags & 4 != 0 { 2 } else { 1 };
        let pos = p * w2;
        let mut write = st.write;
        let mut srcpos = st.srcpos;
        if write == 0x10000 {
            write = (pos + 0x2000) & 0xffff;
        }
        let mut avail = pos as i64 - write as i64;
        if avail < 0 {
            avail += 0x10000;
        }
        let mut avail = avail.min(0x8000) as u32;
        let mut tries = 0;
        while avail != 0 {
            let chunk = avail.min(0x10000 - write);
            // decode or copy at most `chunk` bytes per channel: (consumed source units, left, right)
            let (consumed, left, right): (usize, Vec<i16>, Vec<i16>) = match &*src {
                StreamSourceRef::Pcm { left, right, .. } => {
                    let n = ((chunk / w2) as usize).min(left.len().saturating_sub(srcpos));
                    (n, left[srcpos..srcpos + n].to_vec(), right[srcpos..srcpos + n].to_vec())
                }
                StreamSourceRef::Mp3 { data, decoder } => {
                    let mut d = decoder.lock().unwrap();
                    let (c, l, r) = d.decode(&data[srcpos.min(data.len())..], chunk);
                    (c.min(data.len().saturating_sub(srcpos)), l, r)
                }
            };
            let n = left.len();
            if consumed != 0 || n != 0 {
                for (vv, right_ch) in [(Some(vi), st.right), (partner, true)] {
                    let Some(vv) = vv else { continue };
                    let samples = if right_ch { &right } else { &left };
                    if let Some(pcm) = self.voices[vv].data.as_mut() {
                        let pcm = Arc::make_mut(pcm);
                        let at = (write / w2) as usize;
                        for (k, &x) in samples.iter().enumerate() {
                            set_sample(pcm, at + k, x as i32);
                        }
                    }
                }
                let produced = n as u32 * w2;
                write = (write + produced) & 0xffff;
                avail -= produced;
                srcpos += consumed;
                tries = 0;
            } else {
                tries += 1;
                if tries >= 2 {
                    break;
                }
            }
        }
        if avail != 0 {
            if st.sample.flags & super::sample::F_LOOP != 0 {
                srcpos = st.sample.loop_start as usize;
            } else {
                for vv in [Some(vi), partner].into_iter().flatten() {
                    let x = &mut self.voices[vv];
                    if (pos as i32) < write as i32 {
                        x.sflags &= !0x88;
                        let e = (write / w2) as i32;
                        x.loop_end = e;
                        x.end = e;
                    }
                    if let Some(pcm) = x.data.as_mut() {
                        let pcm = Arc::make_mut(pcm);
                        for k in 0..avail / w2 {
                            set_sample(pcm, (((write / w2) + k) & (0xffff / w2)) as usize, 0);
                        }
                    }
                }
                write = (write + avail) & 0xffff;
            }
        }
        if let Some(s) = self.voices[vi].stream.as_mut() {
            s.write = write;
            s.srcpos = srcpos;
        }
    }

    /// One voice's part of a segment (mixer.md 5.1, 5.2).
    fn mix_voice(&mut self, i: usize, frames: usize, done: usize) {
        let music_master = self.music_master;
        let fx_master = self.fx_master;
        let v = &mut self.voices[i];
        if !v.enabled || !v.playing || v.step == 0 {
            return;
        }
        let pan0 = v.pan;
        let mut flags = 0u8;
        if v.sflags & 4 != 0 {
            flags |= 1;
        }
        if pan0 & 0x8000 != 0 {
            flags |= 4;
        }
        let music = v.b11 & 0x80 != 0;
        let m = (if music { music_master } else { fx_master }) as i8 as i32;
        let mm = m * m * 2;
        let d = ((v.pan_target & 0x7fff) as i32 - (pan0 & 0x7fff) as i32).clamp(-0x200, 0x200);
        v.pan = pan0.wrapping_add(d as u16);
        let p = (v.pan & 0x7fff) as i32;
        let mut e = ((0x4000 - (p - 0x4000).abs()) * v.panenv.value as i32) >> 15;
        e = (e * v.panenv.depth as i32) >> 15;
        let pp = (p + e).clamp(0, 0x7fff) as usize;
        let mut vol = v.level_sq as i32;
        for x in [mm, v.vol18 as i32, v.vol1a as i32, v.venv.value as i32, v.venv.depth as i32, v.vol66 as i32] {
            vol = (vol * x) >> 15;
        }
        let t = pan_table();
        let l = (t[0x7fff - pp] * vol) >> 15;
        let r = (t[pp] * vol) >> 15;
        let gains = [l, r, (v.reverb as i32 * l) >> 7, (v.reverb as i32 * r) >> 7, (v.chorus as i32 * l) >> 7, (v.chorus as i32 * r) >> 7];
        let bases = if music { [MUSIC_DRY, MUSIC_REV, MUSIC_CHO] } else { [FX_DRY, FX_REV, FX_CHO] }.map(|b| (b * BUS_FRAMES) as isize);
        let block = &mut self.buses.block;
        let mut remaining = frames as i32;
        let mut off = done as i32;
        let mut guard = 0;
        while remaining > 0 && v.playing {
            guard += 1;
            if guard > 1000 {
                // the degenerate branch (a non-positive frame count) cannot finish; the original spins on it too
                break;
            }
            let looping = v.sflags & 8 != 0 && !(v.sflags & 0x80 == 0 && v.key_off() && v.step >= 0);
            let bd = if looping { v.loop_end } else { v.end };
            let dist = (bd.wrapping_sub(v.pos)).clamp(-0x7fff, 0x7fff);
            let mut k = ((dist << 16) - v.frac as i32) / v.step + 1;
            k = k.min(remaining);
            if k <= 0 && std::env::var("GX_DEBUG").is_ok() {
                eprintln!("degenerate voice {i} k {k} pos {} frac {} step {} bd {} ls {} le {} end {} sflags {:x} rem {remaining} off {off}", v.pos, v.frac, v.step, bd, v.loop_start, v.loop_end, v.end, v.sflags);
            }
            if vol != 0 && k > 0 {
                if let Some(data) = &v.data {
                    let o = off as isize;
                    // the mixer counts with the low 16 bits, signed
                    let c = k as i16;
                    if c > 0 {
                        voice_mix(block, bases.map(|b| b + o), c as usize, data, v.pos, v.frac, v.step, gains, flags);
                    }
                }
            }
            let t = v.step.wrapping_mul(k).wrapping_add(v.frac as i32);
            v.frac = (t & 0xffff) as u16;
            v.pos = v.pos.wrapping_add(t >> 16);
            let over = if v.step < 0 { bd.wrapping_sub(v.pos) } else { v.pos.wrapping_sub(bd) };
            if over >= 0 {
                if looping {
                    if v.sflags & 0x10 != 0 {
                        std::mem::swap(&mut v.loop_start, &mut v.loop_end);
                        v.step = v.step.wrapping_neg();
                    }
                    v.pos = v.loop_start.wrapping_add(over);
                } else {
                    v.playing = false;
                }
            }
            remaining -= k;
            off += k;
        }
    }

    /// Converts the last chunk's music and effect dry buses (10622fe4) into 16-bit frames: `music` is overwritten,
    /// `fx` too (each bus has its own ring position in the original; the caller adds them with saturation).
    pub fn convert(&self, frames: usize, music: &mut [i16], fx: &mut [i16]) {
        for f in 0..frames {
            for c in 0..2 {
                music[f * 2 + c] = to_i16(self.buses.music_dry()[f][c]);
                fx[f * 2 + c] = to_i16(self.buses.fx_dry()[f][c]);
            }
        }
    }
}

/// Writes one sample of a ring.
fn set_sample(p: &mut Pcm, i: usize, x: i32) {
    match p {
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

/// `cvtps2dq` (round to nearest even, out of range = i32::MIN) then saturation to i16.
pub fn to_i16(x: f32) -> i16 {
    let r = x.round_ties_even();
    let i = if r >= 2147483648.0 || r < -2147483648.0 || r.is_nan() { i32::MIN } else { r as i32 };
    i.clamp(-32768, 32767) as i16
}

/// Waveform `w` at phase `ph` (waveforms 3..7 read the tables that follow in the image; only 0..3 are kept).
fn wave(w: u8, ph: u16) -> i32 {
    WAVES[(w as usize).min(3)][(ph >> 10) as usize]
}

/// Node `k` of the envelope at `base` in a modulation block, read at the fixed stride (zeros past the record).
fn env_node(m: &[u8; 0xA0], base: usize, k: u8) -> (u16, i16) {
    let o = base + 6 + 4 * k as usize;
    if o + 4 > m.len() {
        return (0, 0);
    }
    (u16::from_le_bytes([m[o], m[o + 1]]), i16::from_le_bytes([m[o + 2], m[o + 3]]))
}

/// One envelope's step (player.md 10.1).
fn step_envelope(m: &[u8; 0xA0], base: usize, st: &mut EnvState, key_off: bool, volume: bool) {
    let flags = m[base];
    if flags & 1 == 0 {
        if volume {
            st.value = 0x7fff;
            st.depth = 0x7fff;
        } else {
            st.value = 0;
            st.depth = 0;
        }
        return;
    }
    let (count, sustain, loop_start, loop_end) = (m[base + 2], m[base + 3], m[base + 4], m[base + 5]);
    if st.tick != env_node(m, base, st.node).0 {
        st.value = st.value.wrapping_add(st.slope);
        st.tick = st.tick.wrapping_add(1);
        return;
    }
    if flags & 4 != 0 && st.node == loop_end {
        st.node = loop_start;
        let (t, v) = env_node(m, base, st.node);
        st.tick = t;
        st.value = v;
    }
    if flags & 2 != 0 && !key_off && st.node == sustain {
        st.value = env_node(m, base, sustain).1;
    } else if st.node < count {
        let (_, v0) = env_node(m, base, st.node);
        let (t1, v1) = env_node(m, base, st.node + 1);
        st.value = v0;
        let dt = t1 as i32 - st.tick as i32;
        let dv = v1 as i32 - v0 as i32;
        if dt == 0 {
            st.value = v1;
            st.slope = 0;
        } else {
            st.slope = (dv / dt) as i16;
        }
        st.node += 1;
    }
}

/// The SSE voice mixer (10622730): linear interpolation with the fraction's bit 0 dropped, f32 accumulation.
#[allow(clippy::too_many_arguments)]
fn voice_mix(block: &mut [[f32; 2]], at: [isize; 3], count: usize, data: &Pcm, pos: i32, frac: u16, step: i32, gains: [i32; 6], flags: u8) {
    let mut g = gains.map(|x| x as i16);
    if flags & 4 != 0 {
        g[1] = g[1].wrapping_neg();
        g[3] = g[3].wrapping_neg();
        g[5] = g[5].wrapping_neg();
    }
    let scale = 1.0f32 / (1u32 << 30) as f32;
    let gf = g.map(|x| x as f32 * scale);
    let sample = |i: i32| -> i32 {
        if (i < 0 || i as usize >= data.len()) && std::env::var("GX_DEBUG").is_ok() {
            eprintln!("read outside the sample: index {i} of {} (pos {pos} step {step} count {count})", data.len());
        }
        if i < 0 {
            return 0;
        }
        match data {
            Pcm::S8(v) => v.get(i as usize).map(|&x| (x as i32) << 8).unwrap_or(0),
            Pcm::S16(v) => v.get(i as usize).map(|&x| x as i32).unwrap_or(0),
        }
    };
    let stepm = (step as u32) & 0xffff_fffe;
    let step_frac = stepm & 0xfffe;
    let step_int = (stepm as i32) >> 16;
    let mut f = (frac & 0xfffe) as u32;
    let mut i = pos;
    let len = block.len() as isize;
    let mut add = |p: isize, a: f32, b: f32| {
        if p >= 0 && p < len {
            let f = &mut block[p as usize];
            f[0] += a;
            f[1] += b;
        }
    };
    for j in 0..count {
        let w = (f >> 1) as i32;
        let r = sample(i).wrapping_mul(0x7fff - w).wrapping_add(sample(i + 1).wrapping_mul(w));
        let x = r as f32;
        let jj = j as isize;
        add(at[0] + jj, x * gf[0], x * gf[1]);
        add(at[1] + jj, x * gf[2], x * gf[3]);
        add(at[2] + jj, x * gf[4], x * gf[5]);
        f += step_frac;
        let carry = (f >> 16) as i32;
        f &= 0xffff;
        i = i.wrapping_add(step_int).wrapping_add(carry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_voice_level_chain_matches_the_spec() {
        // 0x7e02 x master 127 -> 31755, then four 0x7fff steps -> 31751
        let mut v = 0x7e02i32;
        for x in [127 * 127 * 2, 0x7fff, 0x7fff, 0x7fff, 0x7fff] {
            v = (v * x) >> 15;
        }
        assert_eq!(v, 31751);
    }

    #[test]
    fn conversion_rounds_to_even_and_saturates() {
        assert_eq!(to_i16(2.5), 2);
        assert_eq!(to_i16(3.5), 4);
        assert_eq!(to_i16(-40000.0), -32768);
        assert_eq!(to_i16(1e12), -32768);
    }

    #[test]
    fn tick_periods() {
        let s = MixState::new(44100, 16);
        assert_eq!(s.env_period, 0x2c1999);
        assert_eq!(s.seq_period, (459 << 16) + 0x6000);
    }
}
