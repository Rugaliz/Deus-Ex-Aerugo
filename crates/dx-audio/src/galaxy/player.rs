//! Galaxy's tracker player (docs/galaxy/player.md): the order sequencer (10615710), the tracker event handler for
//! modes 2/3 (10616eb0) with every effect, the delayed note trigger (10617d40) and the music control the subsystem
//! uses (load reset 1060e850, start 106122a0, stop 106183a0, position 1060d0e0).
use super::mixer::{EnvState, MixState, Voice};
use super::song::Song;
use super::tables::{AMIGA_PERIODS, ARPEGGIO, FINETUNE};
use std::sync::Arc;

/// The player's song globals.
pub struct Player {
    pub song: Arc<Song>,
    /// `10698349`.
    pub playing: bool,
    /// `10698200` (DX never clears it).
    pub loop_flag: bool,
    /// `10698201` current order (0xFF before the first).
    pub order: u8,
    /// `10698208` current pattern (its number).
    current: Option<usize>,
    /// Track record runtime: read cursor (None = not started) and tick countdown.
    cursor: Option<usize>,
    countdown: u32,
    /// `10697ce6` tick inside the row.
    tick: u16,
    /// `10698141` last row read, `106981cc` last row of the pattern.
    last_row_read: u8,
    last_row: u8,
    /// `10698140` row + 1 to start the next pattern at.
    skip_rows: u8,
    /// `10697ca8` pattern end pending.
    end_pending: bool,
    /// `1068828c` pattern delay, `1068dc81` loop row, `10697d1c` loop counter.
    pattern_delay: u8,
    loop_row: u8,
    loop_counter: u8,
    /// `1068828d` first music voice.
    pub first_voice: usize,
}

impl Player {
    /// A freshly loaded song (`1060e850`'s reset of the music voices and globals, then the loader's values).
    pub fn new(song: Song, mix: &mut MixState) -> Player {
        let first = mix.effects;
        for v in mix.voices.iter_mut().skip(first) {
            let enabled = v.enabled;
            *v = Voice::new();
            v.enabled = enabled;
            v.b0c = 0xbc;
            v.b11 = 0;
            v.level_sq = 20000;
            v.pan = 0x4000;
            v.pan_target = 0x4000;
            v.vol18 = 0;
            v.vol1a = 0x7fff;
            v.chorus = 0;
            v.reverb = 0x28;
            v.level = 100;
            v.level_target = 100;
            v.playing = false;
        }
        mix.tempo = 0x78;
        mix.speed = 0x30;
        mix.mode = song.mode;
        mix.music = song.channels as usize;
        mix.music_master = song.global_volume;
        Player {
            song: Arc::new(song),
            playing: false,
            loop_flag: true,
            order: 0xFF,
            current: None,
            cursor: None,
            countdown: 0,
            tick: 0,
            last_row_read: 0xFF,
            last_row: 0,
            skip_rows: 1,
            end_pending: false,
            pattern_delay: 0,
            loop_row: 0,
            loop_counter: 0,
            first_voice: first,
        }
    }

    fn music_voices(&self) -> std::ops::Range<usize> {
        self.first_voice..(self.first_voice + self.song.channels as usize).min(256)
    }

    /// Stop (`106183a0`): music voices disabled, not playing.
    pub fn stop(&mut self, mix: &mut MixState) {
        for i in self.music_voices() {
            mix.voices[i].enabled = false;
        }
        self.playing = false;
    }

    /// Start (`106122a0`).
    pub fn start(&mut self, mix: &mut MixState) {
        for i in self.music_voices() {
            mix.voices[i].enabled = true;
        }
        self.playing = true;
    }

    /// Position control op 2 (`1060d0e0`): the jump happens at the next row boundary.
    pub fn set_order(&mut self, order: u8) -> bool {
        if order > self.song.last_order {
            return false;
        }
        self.order = order.wrapping_sub(1);
        self.end_pending = true;
        true
    }

    /// What `UGalaxyAudioSubsystem::Update` does to play a song from a section (player.md 5.4 step 3): stop, cut
    /// the 32 voices from the first music voice, start, jump, pattern end pending.
    pub fn play_section(&mut self, mix: &mut MixState, section: u8) {
        self.stop(mix);
        if section == 0xFF {
            return;
        }
        for i in self.first_voice..(self.first_voice + 32).min(256) {
            mix.voices[i].playing = false;
        }
        self.start(mix);
        self.set_order(section);
        self.end_pending = true;
    }

    /// The row tick (`10615710`); the age clock is counted by the mixer.
    pub fn row_tick(&mut self, mix: &mut MixState) {
        if !self.playing {
            return;
        }
        loop {
            if let Some(p) = self.current {
                self.handle_track(mix, p);
                if self.cursor.is_some() {
                    return;
                }
            }
            self.current = None;
            while self.current.is_none() {
                self.order = self.order.wrapping_add(1);
                self.end_pending = false;
                self.last_row_read = 0xFF;
                if self.order == 0 {
                    mix.tempo = self.song.initial_tempo;
                    mix.speed = self.song.initial_speed;
                    mix.set_periods();
                    for c in 0..self.song.channels as usize {
                        let v = &mut mix.voices[self.first_voice + c];
                        let pan = (*self.song.channel_pan.get(c).unwrap_or(&0) as u16) << 8;
                        v.pan = pan;
                        v.pan_target = pan;
                    }
                }
                let o = self.song.orders[self.order as usize];
                if self.order > self.song.last_order || o == 0xFF {
                    self.playing = false;
                    self.order = 0xFF;
                    if self.loop_flag {
                        self.playing = true;
                    }
                } else if o < 0xFE && self.song.patterns[o as usize].is_some() {
                    self.current = Some(o as usize);
                }
            }
        }
    }

    /// The tracker event handler for one row tick (`10616eb0`).
    fn handle_track(&mut self, mix: &mut MixState, p: usize) {
        let song = self.song.clone();
        let data = song.patterns[p].as_ref().unwrap();
        let mut local;
        let boundary;
        match self.cursor {
            None => {
                self.last_row = data[0];
                self.cursor = Some(1);
                self.countdown = 0;
                boundary = true;
            }
            Some(_) => boundary = self.countdown == 0,
        }
        if boundary {
            self.tick = 0;
            local = mix.speed as u32;
            if self.pattern_delay != 0 {
                self.pattern_delay -= 1;
            } else if self.end_pending || self.last_row_read == self.last_row {
                self.cursor = None;
            } else {
                self.read_row(mix, data);
            }
        } else {
            local = self.countdown;
        }
        let t = self.tick;
        for i in self.music_voices() {
            if mix.voices[i].cmd != 0 || mix.voices[i].param != 0 {
                self.effect(mix, i, t);
            }
        }
        if t == 0 {
            local = mix.speed as u32;
        }
        self.tick = t.wrapping_add(1);
        self.countdown = local.wrapping_sub(1);
    }

    /// Reads one row of events (8.2).
    fn read_row(&mut self, mix: &mut MixState, data: &[u8]) {
        for i in self.music_voices() {
            let v = &mut mix.voices[i];
            v.vib_depth = 0;
            v.vib_speed = 0;
            v.trem_depth = 0;
            v.trem_speed = 0;
            v.porta_speed = 0;
            v.pitch_offset = 0;
            v.cmd = 0;
            v.param = 0;
        }
        let mut c = self.cursor.unwrap();
        let byte = |c: &mut usize| -> u8 {
            let b = data.get(*c).copied().unwrap_or(0);
            *c += 1;
            b
        };
        while self.skip_rows > 1 {
            loop {
                let h = byte(&mut c);
                if h == 0 {
                    break;
                }
                c += (if h & 0x80 != 0 { 2 } else { 0 }) + (if h & 0x40 != 0 { 2 } else { 0 }) + (if h & 0x20 != 0 { 1 } else { 0 });
            }
            self.skip_rows -= 1;
            self.last_row_read = self.last_row_read.wrapping_add(1);
        }
        loop {
            let h = byte(&mut c);
            if h == 0 {
                break;
            }
            let vi = self.first_voice + (h & 0x1F) as usize;
            if h & 0x80 != 0 {
                let param = byte(&mut c);
                let cmd = byte(&mut c);
                mix.voices[vi].param = param;
                mix.voices[vi].cmd = cmd;
            }
            if h & 0x40 != 0 {
                let ins = byte(&mut c);
                let note = byte(&mut c);
                self.note_event(mix, vi, ins, note);
            }
            if h & 0x20 != 0 {
                let vol = byte(&mut c);
                let v = &mut mix.voices[vi];
                v.vol66 = (vol as u16) << 8;
                v.vol68 = v.vol66;
            }
        }
        self.cursor = Some(c);
        self.last_row_read = self.last_row_read.wrapping_add(1);
    }

    /// Pitch value of note index `k` with C5 speed `c5` (8.4 step 7), 32-bit.
    fn note_pitch(mode: u8, k: i32, c5: u32) -> u32 {
        if !(0..120).contains(&k) {
            return 0;
        }
        if mode & 1 == 0 {
            ((120 - k) * 256) as u32
        } else if c5 != 0 {
            (AMIGA_PERIODS[k as usize] as u32).wrapping_mul(8363) / c5
        } else {
            0
        }
    }

    /// An instrument/note pair (8.3, 8.4).
    fn note_event(&mut self, mix: &mut MixState, vi: usize, ins: u8, note: u8) {
        let mode = mix.mode;
        if ins != 0 {
            let v = &mut mix.voices[vi];
            v.b0d = ins - 1;
            if let Some(m) = v.instrument.clone() {
                for (base, st) in [(0x0Eusize, &mut v.venv), (0x3E, &mut v.penv), (0x6E, &mut v.panenv)] {
                    if m.block[base] & 1 != 0 {
                        st.value = 0;
                        st.tick = 0;
                        st.depth = 0x7fff;
                        st.node = 0;
                    }
                }
            }
        }
        if note != 0 {
            if note & 0x80 != 0 {
                mix.voices[vi].b0c = note;
            } else {
                let (cmd, param) = (mix.voices[vi].cmd, mix.voices[vi].param);
                {
                    let v = &mut mix.voices[vi];
                    v.vol18 = 0x7fff;
                    v.b0c = note - 1;
                }
                let delayed = cmd == 3 || cmd == 5 || (cmd == 0x0E && param >> 4 == 0xD);
                if !delayed {
                    mix.voices[vi].playing = false;
                    if self.trigger(mix, vi, (note - 1) as i32, true) {
                        mix.voices[vi].playing = true;
                    }
                }
                let v = &mut mix.voices[vi];
                v.porta_target = Self::note_pitch(mode, (note - 1) as i32, v.freq) as u16;
            }
        }
        if ins != 0 {
            let v = &mut mix.voices[vi];
            if let Some((rec, s)) = &v.slot {
                let vol = rec.slots[*s].volume;
                v.vol66 = vol;
                v.vol68 = vol;
            }
        }
    }

    /// Steps 2..7 of a note trigger (8.4); `normal` also sets +0x11 and resets the envelopes (not for EDx).
    /// Returns whether the note starts.
    fn trigger(&mut self, mix: &mut MixState, vi: usize, k: i32, normal: bool) -> bool {
        let mode = mix.mode;
        let ins_index = mix.voices[vi].b0d as usize;
        let Some(rec) = self.song.instruments[ins_index].clone() else { return false };
        let s = rec.map.get(k as usize).copied().unwrap_or(0) as usize;
        if s >= rec.slots.len() {
            return false;
        }
        let v = &mut mix.voices[vi];
        v.slot = Some((rec.clone(), s));
        v.b0e = s as u8;
        let slot = &rec.slots[s];
        let Some(pcm) = slot.pcm.clone() else { return false };
        if normal {
            v.b11 = 0x80;
        }
        v.instrument = Some(rec.clone());
        v.data = Some(pcm);
        v.frac = 0;
        v.pos = 0;
        v.start = 0;
        v.loop_start = slot.loop_start as i32;
        v.loop_end = slot.loop_end as i32;
        v.end = slot.length as i32;
        v.freq = slot.c5speed;
        v.sflags = slot.flags;
        if slot.flags & 0x20 != 0 {
            v.pan = slot.pan;
            v.pan_target = slot.pan;
        }
        // auto modulation from the block (all zero for IT)
        let m = &rec.block;
        v.vib_wave = m[0];
        if m[0] & 4 == 0 {
            v.vib_phase = 0;
        }
        v.vib_speed = u16::from_le_bytes([m[5], m[6]]);
        v.vib_depth = i16::from_le_bytes([m[3], m[4]]);
        v.trem_wave = m[7];
        if m[7] & 4 == 0 {
            v.trem_phase = 0;
        }
        v.trem_speed = u16::from_le_bytes([m[0x0C], m[0x0D]]);
        v.trem_depth = i16::from_le_bytes([m[0x0A], m[0x0B]]);
        if normal {
            let reset = EnvState { value: 0, slope: 0, tick: 0, node: 0, depth: 0x7fff };
            v.venv = EnvState { slope: v.venv.slope, ..reset };
            v.penv = EnvState { slope: v.penv.slope, ..reset };
            v.panenv = EnvState { slope: v.panenv.slope, ..reset };
        }
        let pitch = Self::note_pitch(mode, k, v.freq);
        v.pitch = pitch as u16;
        v.pitch_base = pitch as u16;
        if pitch != 0 {
            let step = mix.pitch_step(&mix.voices[vi], pitch);
            mix.voices[vi].step = step;
        }
        true
    }

    /// The effect pass of one voice (8.5, 8.6).
    fn effect(&mut self, mix: &mut MixState, vi: usize, t: u16) {
        let (cmd, p) = (mix.voices[vi].cmd, mix.voices[vi].param);
        let (x, y) = (p >> 4, p & 0x0F);
        let mode = mix.mode;
        let up_limit = self.song.porta_up_limit;
        let down_limit = self.song.porta_down_limit;
        let set_volume = |v: &mut Voice, w: i32| {
            v.vol66 = w as u16;
            v.vol68 = w as u16;
        };
        let slide = |v: &mut Voice| {
            let d = if p <= 0x0F { p as i32 } else { -(x as i32) };
            let w = (v.vol66 as i32 - d * 0x200).clamp(0, 0x7fff);
            v.vol66 = w as u16;
            v.vol68 = w as u16;
        };
        let vibrato = |v: &mut Voice| {
            v.vib_depth = ((v.vib_mem & 0x0F) as i16) << 5;
            v.vib_speed = (((v.vib_mem >> 2) & 0x3C) as u16) << 8;
        };
        let nibbles = |mem: &mut u8| {
            if y != 0 {
                *mem = (*mem & 0xF0) | y;
            }
            if x != 0 {
                *mem = (*mem & 0x0F) | (x << 4);
            }
        };
        match cmd & 0x0F {
            0 => {
                if t != 0 {
                    let v = &mut mix.voices[vi];
                    let base = v.pitch_base as i32;
                    let f = match t % 3 {
                        0 => ARPEGGIO[0],
                        1 => ARPEGGIO[x as usize],
                        _ => ARPEGGIO[y as usize],
                    };
                    v.pitch_offset = (((base * f) >> 15) - base) as i16;
                }
            }
            1 | 2 => {
                if t != 0 {
                    let v = &mut mix.voices[vi];
                    v.porta_speed = (p as u16) << 4;
                    v.porta_target = if cmd & 0x0F == 1 { up_limit } else { down_limit };
                }
            }
            3 => {
                let v = &mut mix.voices[vi];
                if t == 0 {
                    if p != 0 {
                        v.porta_mem = p;
                    }
                } else {
                    v.porta_speed = (v.porta_mem as u16) << 4;
                }
            }
            4 => {
                let v = &mut mix.voices[vi];
                if t == 0 {
                    nibbles(&mut v.vib_mem);
                } else {
                    vibrato(v);
                }
            }
            5 => {
                if t != 0 {
                    let v = &mut mix.voices[vi];
                    slide(v);
                    v.porta_speed = (v.porta_mem as u16) << 4;
                }
            }
            6 => {
                if t != 0 {
                    let v = &mut mix.voices[vi];
                    slide(v);
                    vibrato(v);
                }
            }
            7 => {
                let v = &mut mix.voices[vi];
                if t == 0 {
                    nibbles(&mut v.trem_mem);
                } else {
                    v.trem_depth = ((v.trem_mem & 0x0F) as i16) << 11;
                    v.trem_speed = (((v.trem_mem >> 2) & 0x3C) as u16) << 8;
                }
            }
            8 => {
                if t == 0 {
                    let mut u = p as u32;
                    if u <= 0x80 {
                        if u == 0x80 {
                            u = 0x7F;
                        }
                    } else {
                        u -= 4;
                        if !(0x80..=0xC0).contains(&u) {
                            u = 0xC0;
                        } else {
                            u = ((u & 0x7F) * 2).min(0x7F) | 0x80;
                        }
                    }
                    let v = &mut mix.voices[vi];
                    v.pan = (u << 8) as u16;
                    v.pan_target = v.pan;
                }
            }
            9 => {
                if t == 0 {
                    let v = &mut mix.voices[vi];
                    if p != 0 {
                        v.offset_mem = p;
                    }
                    let mut pos = v.start.wrapping_add(v.offset_mem as i32 * 256);
                    if pos as u32 >= v.end as u32 {
                        pos = v.end.wrapping_sub(1);
                    }
                    v.pos = pos;
                    v.frac = 0;
                }
            }
            0x0A => {
                if t != 0 {
                    slide(&mut mix.voices[vi]);
                }
            }
            0x0B => {
                if t == 0 && self.pattern_delay == 0 && p <= self.song.last_order && (self.loop_flag || p > self.order) {
                    self.order = p.wrapping_sub(1);
                    self.end_pending = true;
                }
            }
            0x0C => {
                if t == 0 {
                    set_volume(&mut mix.voices[vi], ((p as i32) << 8).min(0x7fff));
                }
            }
            0x0D => {
                if t == 0 && self.pattern_delay == 0 {
                    self.skip_rows = x.wrapping_mul(10).wrapping_add(y).wrapping_add(1);
                    self.end_pending = true;
                }
            }
            0x0E => self.extended(mix, vi, t, x, y, up_limit, down_limit),
            _ => {
                if t == 0 {
                    if p == 0 {
                        self.playing = false;
                    } else if p <= 0x20 {
                        mix.speed = p as u16;
                    } else {
                        mix.tempo = p as u16;
                    }
                    mix.set_periods();
                }
            }
        }
        let _ = mode;
    }

    #[allow(clippy::too_many_arguments)]
    fn extended(&mut self, mix: &mut MixState, vi: usize, t: u16, x: u8, y: u8, up: u16, down: u16) {
        match x {
            1 | 2 => {
                let v = &mut mix.voices[vi];
                if t == 0 {
                    v.porta_speed = (y as u16) << 4;
                    v.porta_target = if x == 1 { up } else { down };
                } else {
                    v.porta_speed = 0;
                }
            }
            4 => mix.voices[vi].vib_wave = y & 7,
            5 => mix.voices[vi].freq = ((FINETUNE[y as usize] * 8363) >> 15) as u32,
            6 => {
                if t == 0 && self.pattern_delay == 0 && !self.end_pending {
                    if y == 0 {
                        self.loop_row = self.last_row_read;
                    } else {
                        let was_zero = self.loop_counter == 0;
                        self.loop_counter = self.loop_counter.wrapping_sub(1);
                        if was_zero {
                            self.loop_counter = y;
                        }
                        if self.loop_counter != 0 {
                            self.skip_rows = self.loop_row.wrapping_add(1);
                            self.end_pending = true;
                            self.order = self.order.wrapping_sub(1);
                        }
                    }
                }
            }
            7 => mix.voices[vi].trem_wave = y & 7,
            8 => {
                let v = &mut mix.voices[vi];
                v.pan = y as u16 * 0x800 + 0x400;
                v.pan_target = v.pan;
            }
            9 => {
                if y != 0 && t % y as u16 == 0 {
                    let v = &mut mix.voices[vi];
                    v.pos = v.start;
                    v.frac = 0;
                }
            }
            0xA => {
                if t == 0 {
                    let v = &mut mix.voices[vi];
                    let w = (v.vol66 as i32 + y as i32 * 0x200).min(0x7fff);
                    v.vol66 = w as u16;
                    v.vol68 = w as u16;
                }
            }
            0xB => {
                if t == 0 {
                    let v = &mut mix.voices[vi];
                    let w = (v.vol66 as i32 - y as i32 * 0x200).max(0);
                    v.vol66 = w as u16;
                    v.vol68 = w as u16;
                }
            }
            0xC => {
                if t > y as u16 {
                    let v = &mut mix.voices[vi];
                    v.vol66 = 0;
                    v.vol68 = 0;
                }
            }
            0xD => self.note_delay(mix, vi, t),
            0xE => {
                if t == 0 {
                    self.pattern_delay = y;
                }
                mix.voices[vi].cmd = 0;
                mix.voices[vi].param = 0;
            }
            _ => {}
        }
    }

    /// The delayed note trigger (`10617d40`).
    fn note_delay(&mut self, mix: &mut MixState, vi: usize, t: u16) {
        let v = &mix.voices[vi];
        if t != (v.param & 0x0F) as u16 {
            return;
        }
        let k = (v.b0c & 0x7F) as i32;
        if self.trigger(mix, vi, k, false) {
            mix.voices[vi].playing = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_note_pitch_and_step() {
        // C-5 (k = 60) plays at the C5 speed
        assert_eq!(Player::note_pitch(2, 60, 8363), 15360);
        let mut mix = MixState::new(44100, 16);
        mix.mode = 2;
        let mut v = Voice::new();
        v.freq = 44100;
        assert_eq!(mix.pitch_step(&v, 15360), 0x10000);
        v.freq = 22050;
        assert_eq!(mix.pitch_step(&v, 15360 - 3072), 0x10000);
    }
}
