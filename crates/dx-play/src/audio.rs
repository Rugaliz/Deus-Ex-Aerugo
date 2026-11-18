//! The audio subsystem (`UGalaxyAudioSubsystem`, docs/galaxy/sounds.md): the channel records `PlaySound` fills, the
//! per-frame `Update` that starts, updates and frees the effect voices of those channels (volume, pan, frequency,
//! priority, occlusion, Doppler, lip sync) and keeps actors' ambient sounds going, and the music block.
use dx_audio::galaxy::sample::{self, Sample};
use dx_audio::galaxy::song::Pcm;
use dx_audio::{Mixer, PhonemeDetector};
use dx_soft::Camera;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use ue_assets::audio::{load_music, load_sound};
use ue_assets::files::GameFiles;
use ue_vm::ai::SoundEvent;
use ue_vm::{Value, Vm};

/// One channel record (0x40 bytes in the original, sounds.md 2.2). Channel `i` plays on effect voice `i`.
#[derive(Debug, Clone, Default)]
pub struct Channel {
    /// The voice was started (record +0x00 non-zero).
    pub started: bool,
    pub actor: u32,
    /// The sound id; 0 = free.
    pub id: i32,
    pub sound: Option<Arc<Sample>>,
    pub location: [f32; 3],
    pub volume: f32,
    pub radius: f32,
    pub pitch: f32,
    pub priority: f32,
    pub play_time: f32,
    pub next_phoneme: f32,
    /// +0x3c, 0..0.5.
    pub occlusion: f32,
}

pub struct Audio {
    files: GameFiles,
    /// `RegisterSound`: the Galaxy samples by object path (None: could not be loaded).
    sounds: HashMap<String, Option<Arc<Sample>>>,
    pub mixer: Arc<Mutex<Mixer>>,
    /// The channel records, `EffectsChannels` of them.
    pub channels: Vec<Channel>,
    /// +0x8fc: the id counter of `SLOT_None` sounds.
    none_counter: i32,
    /// The driver's phoneme detector (shared buffers, as the original's statics).
    phonemes: PhonemeDetector,
    /// When `update` last ran: play times advance by real time, as the driver's `Update`.
    last_update: Option<Instant>,
    /// Tests: a fixed time step for the play times instead of real time.
    pub fixed_dt: Option<f64>,
    pub music_title: Option<String>,
    pub sounds_played: u64,
    /// `UGalaxyAudioSubsystem`'s music state (`CurrentSong`, `CurrentCDTrack`, `CurrentSection`, `MusicFade`).
    pub music: MusicState,
    /// The ini settings (`[Galaxy.GalaxyAudioSubsystem]`).
    latency: i32,
    digital_music: bool,
    reverse_stereo: bool,
    use_surround: bool,
    ambient_factor: f32,
    doppler_speed: f32,
    use_reverb: bool,
    /// The reverb block applied last (`this+0x88e`, compared bitwise every `Update`).
    reverb_block: dx_audio::galaxy::reverb::ReverbParams,
}

/// What Galaxy plays: the song (by object path) with its module, the CD track (255: none), the section it was
/// started at and the fade.
#[derive(Debug, Clone)]
pub struct MusicState {
    pub song: Option<String>,
    data: Option<Arc<Vec<u8>>>,
    pub cd_track: u8,
    pub section: u8,
    pub fade: f32,
}

impl Default for MusicState {
    fn default() -> Self {
        MusicState { song: None, data: None, cd_track: 255, section: 255, fade: 1.0 }
    }
}

/// `ESoundSlot`: Ambient 4, Talk 5 (ids carry `slot * 2`).
fn is_ambient(id: i32) -> bool {
    id & 0xe == 8
}
fn is_talk(id: i32) -> bool {
    matches!(id & 0xf, 10 | 11)
}

/// `_ftol`: truncation, the low 32 bits.
fn ftol(x: f64) -> i32 {
    if x.is_nan() {
        return i32::MIN;
    }
    (x as i64) as i32
}

impl Audio {
    pub fn new(game: &Path, mixer: Arc<Mutex<Mixer>>) -> Audio {
        // one channel record per effect voice of the mixer (the original's subsystem keeps at most 32; the port's
        // mixer has `EFFECTS_CHANNELS`, 128)
        let n = mixer.lock().unwrap().effects();
        Audio {
            files: GameFiles::new(game),
            sounds: HashMap::new(),
            mixer,
            channels: vec![Channel::default(); n],
            none_counter: 0,
            phonemes: PhonemeDetector::new(),
            last_update: None,
            fixed_dt: None,
            music_title: None,
            sounds_played: 0,
            music: MusicState::default(),
            latency: 40,
            digital_music: true,
            reverse_stereo: false,
            use_surround: false,
            ambient_factor: 0.7,
            doppler_speed: 6500.0,
            use_reverb: true,
            reverb_block: Default::default(),
        }
    }

    /// The audio settings of the ini (`Latency`, `UseDigitalMusic`, `ReverseStereo`, `UseSurround`, `AmbientFactor`,
    /// `DopplerSpeed`).
    pub fn read_settings(&mut self, vm: &mut Vm) {
        let get = |vm: &mut Vm, k: &str| vm.reg.config_value("System", "Galaxy.GalaxyAudioSubsystem", k);
        if let Some(v) = get(vm, "Latency").and_then(|v| v.trim().parse::<i32>().ok()) {
            self.latency = v;
        }
        let flag = |v: Option<String>| v.map(|v| v.trim().eq_ignore_ascii_case("true") || v.trim() == "1");
        if let Some(b) = flag(get(vm, "UseDigitalMusic")) {
            self.digital_music = b;
        }
        if let Some(b) = flag(get(vm, "ReverseStereo")) {
            self.reverse_stereo = b;
        }
        if let Some(b) = flag(get(vm, "UseSurround")) {
            self.use_surround = b;
        }
        if let Some(b) = flag(get(vm, "UseReverb")) {
            self.use_reverb = b;
        }
        // Init clamps AmbientFactor to 0..10 and DopplerSpeed to 1..100000
        if let Some(v) = get(vm, "AmbientFactor").and_then(|v| v.trim().parse::<f32>().ok()) {
            self.ambient_factor = v.clamp(0.0, 10.0);
        }
        if let Some(v) = get(vm, "DopplerSpeed").and_then(|v| v.trim().parse::<f32>().ok()) {
            self.doppler_speed = v.clamp(1.0, 100000.0);
        }
    }

    /// `RegisterSound` (10606af0): a sound's Galaxy sample by object path, loaded once; `None` when it is missing or
    /// cannot be loaded.
    pub fn sound(&mut self, path: &str) -> Option<Arc<Sample>> {
        if let Some(s) = self.sounds.get(path) {
            return s.clone();
        }
        let s = (|| {
            let (pkg_name, rest) = path.split_once('.')?;
            let pkg = self.files.package(pkg_name)?;
            let export = pkg.find(rest)?;
            let a = load_sound(&pkg.pkg, export).ok()?;
            sample::load(&a.data).ok().map(Arc::new)
        })();
        self.sounds.insert(path.to_string(), s.clone());
        s
    }

    /// The listener `L`: the viewport actor's `ViewTarget`, else the viewport actor.
    fn listener(vm: &mut Vm, player: u32) -> u32 {
        match vm.get_prop(player, "ViewTarget", 0) {
            Ok(Value::Obj(v)) if v != 0 && vm.is_live(v) => v,
            _ => player,
        }
    }

    /// `PlaySound` (10608100): picks a channel (the lowest stored priority not above the new sound's, a free one
    /// counting 0, ties to the highest index; a same-id channel wins), cuts what played there and records the request.
    /// The voice starts at the next `Update`. Returns whether the sound was accepted.
    #[allow(clippy::too_many_arguments)]
    pub fn play_sound(&mut self, vm: &mut Vm, player: u32, actor: u32, mut id: i32, sound: Arc<Sample>, location: [f32; 3], volume: f32, radius: f32, pitch: f32) -> bool {
        if id & 0xe == 0 {
            self.none_counter = self.none_counter.wrapping_sub(1);
            id = self.none_counter.wrapping_mul(16);
        }
        let l = Self::listener(vm, player);
        let ll = vm.location(l);
        let (dx, dy, dz) = (location[0] as f64 - ll[0] as f64, location[1] as f64 - ll[1] as f64, location[2] as f64 - ll[2] as f64);
        let d = ((dx * dx + dy * dy) + dz * dz).sqrt();
        let p = (1.0 - d / radius as f64) * volume as f64;
        let mut best: Option<usize> = None;
        let mut cur = p;
        for (j, c) in self.channels.iter().enumerate() {
            if (c.id ^ id) & !1 == 0 {
                if id & 1 != 0 {
                    return false;
                }
                best = Some(j);
                break;
            } else if (c.priority as f64) <= cur {
                best = Some(j);
                cur = c.priority as f64;
            }
        }
        let Some(j) = best else { return false };
        self.stop_sound(j);
        self.channels[j] = Channel { started: false, actor, id, sound: Some(sound), location, volume, radius, pitch, priority: p as f32, play_time: 0.0, next_phoneme: 0.0, occlusion: 0.0 };
        true
    }

    /// `StopSound` (10607e50): stops the channel's voice and frees the record.
    pub fn stop_sound(&mut self, i: usize) {
        if self.channels[i].started {
            self.mixer.lock().unwrap().stop_voice(i);
        }
        self.channels[i] = Channel::default();
    }

    /// `StopSoundId` (10607ef0): the first channel with the id (ignoring `bNoOverride`).
    pub fn stop_sound_id(&mut self, id: i32) {
        if let Some(j) = self.channels.iter().position(|c| (c.id ^ id) & !1 == 0) {
            self.stop_sound(j);
        }
    }

    /// `NoteDestroy` (106083a0): an ambient channel of the actor stops, any other forgets the actor (the sound plays
    /// on where it was).
    pub fn note_destroy(&mut self, actor: u32) {
        for j in 0..self.channels.len() {
            if self.channels[j].actor == actor && actor != 0 {
                if is_ambient(self.channels[j].id) {
                    self.stop_sound(j);
                } else {
                    self.channels[j].actor = 0;
                }
            }
        }
    }

    /// `SetViewport`'s stop-all (every `LoadMap` calls it): all game sounds are cut.
    pub fn stop_all_sounds(&mut self) {
        for j in 0..self.channels.len() {
            self.stop_sound(j);
        }
    }

    /// The occlusion factor an actor's ambient sound plays with (tests): `max(1 - 2 * occlusion, 0.33)`.
    pub fn ambient_occlusion(&self, actor: u32) -> Option<f32> {
        self.channels.iter().find(|c| c.actor == actor && is_ambient(c.id)).map(|c| (1.0 - (c.occlusion as f64 + c.occlusion as f64)).max(0.33) as f32)
    }

    /// Whether an actor's ambient sound holds a channel (tests).
    pub fn ambient_playing(&self, actor: u32) -> bool {
        self.channels.iter().any(|c| c.actor == actor && is_ambient(c.id))
    }

    /// Call once per drawn frame (`UGalaxyAudioSubsystem::Update` 10609510 with the camera's frame): the requests
    /// the scripts made reach `PlaySound`/`StopSoundId` first, then the ambient passes, the per-channel pass and
    /// the music.
    pub fn update(&mut self, vm: &mut Vm, cam: &Camera, player: u32) {
        // actors destroyed since the last frame (the engine calls NoteDestroy at once)
        for j in 0..self.channels.len() {
            let a = self.channels[j].actor;
            if a != 0 && !vm.is_live(a) {
                self.note_destroy(a);
            }
        }
        // what the engine handed to the subsystem (sounds.rs: after CheckHearSound and ClientHearSound)
        for ev in vm.take_sound_events() {
            match ev {
                SoundEvent::Play(req) => {
                    let Some(path) = Self::object_path(vm, req.sound) else { continue };
                    let Some(sound) = self.sound(&path) else { continue };
                    let location = req.location.unwrap_or_else(|| vm.location(player));
                    let actor = if req.source != 0 && vm.is_live(req.source) { req.source } else { 0 };
                    if self.play_sound(vm, player, actor, req.id, sound, location, req.volume, req.radius, req.pitch) {
                        self.sounds_played += 1;
                    }
                }
                SoundEvent::Stop(id) => self.stop_sound_id(id),
            }
        }
        // real time since the last update, limited to 0..1 s
        let now = Instant::now();
        let dt = self.last_update.map(|t| now.duration_since(t).as_secs_f64().clamp(0.0, 1.0)).unwrap_or(0.0);
        self.last_update = Some(now);
        let dt = self.fixed_dt.unwrap_or(dt);
        let coords = ue_bsp::scene::view_coords(cam.pos, [cam.pitch, cam.yaw, cam.roll]);
        let ambient_ok = self.ambient_start(vm, player);
        self.ambient_refresh(vm, player, ambient_ok);
        self.channel_pass(vm, player, &coords, dt);
        self.update_music(vm, player, dt as f32);
        self.update_reverb(vm, player);
    }

    /// The zone reverb block of `Update` (reverb.md 3.2): from the view actor's zone when `UseReverb` and the zone
    /// is a `bReverbZone`, else all zero (no reverb); applied only when it differs bitwise from the last one.
    fn update_reverb(&mut self, vm: &mut Vm, player: u32) {
        if player == 0 || !vm.is_live(player) {
            return;
        }
        let view = Self::listener(vm, player);
        let zone = vm.zone_of_actor(view);
        let mut block = dx_audio::galaxy::reverb::ReverbParams::default();
        if self.use_reverb && zone != 0 && vm.is_live(zone) && vm.bp(zone, "bReverbZone") {
            let byte = |vm: &mut Vm, k: &str, i: usize| match vm.get_prop(zone, k, i as u32) {
                Ok(Value::Byte(b)) => b,
                _ => 0,
            };
            let cutoff = vm.get_prop(zone, "CutoffHz", 0).ok().and_then(|v| v.as_i32()).unwrap_or(0);
            let master = byte(vm, "MasterGain", 0);
            let delay: [u8; 6] = std::array::from_fn(|i| byte(vm, "Delay", i));
            let gain: [u8; 6] = std::array::from_fn(|i| byte(vm, "Gain", i));
            block = dx_audio::galaxy::reverb::ReverbParams::from_zone(master, cutoff, delay, gain);
        }
        if !block.same_bits(&self.reverb_block) {
            self.reverb_block = block;
            self.mixer.lock().unwrap().set_reverb(&block);
        }
    }

    /// The ambient start pass (sounds.md 5.3): every actor with an `AmbientSound` within `(SoundRadius + 1) * 25`
    /// of `L` that holds no channel asks for one. Returns `ambient_ok` (the player controls the view and the game
    /// is not paused).
    fn ambient_start(&mut self, vm: &mut Vm, player: u32) -> bool {
        if player == 0 || !vm.is_live(player) {
            return false;
        }
        let flags = vm.get_prop(player, "ShowFlags", 0).ok().and_then(|v| v.as_i32()).unwrap_or(0);
        let paused = matches!(vm.get_prop(vm.level, "Pauser", 0), Ok(Value::Str(ref s)) if !s.is_empty());
        let ok = flags & 0x4800 != 0 && !paused;
        if !ok {
            return false;
        }
        let l = Self::listener(vm, player);
        let ll = vm.location(l);
        for a in vm.live_actors() {
            let snd = vm.op(a, "AmbientSound");
            if snd == 0 {
                continue;
            }
            let r = (vm.u8p(a, "SoundRadius") as f64 + 1.0) * 25.0;
            let p = vm.location(a);
            let (dx, dy, dz) = (p[0] as f64 - ll[0] as f64, p[1] as f64 - ll[1] as f64, p[2] as f64 - ll[2] as f64);
            let s = (dx * dx + dz * dz) + dy * dy;
            if !(r * r >= s) {
                continue;
            }
            let id = vm.actor_index(a).unwrap_or(0) as i32 * 16 + 8;
            if self.channels.iter().any(|c| c.id == id) {
                continue;
            }
            let Some(path) = Self::object_path(vm, snd) else { continue };
            let Some(sound) = self.sound(&path) else { continue };
            let volume = (vm.u8p(a, "SoundVolume") as f64 * self.ambient_factor as f64 * (1.0 / 255.0)) as f32;
            let pitch = (vm.u8p(a, "SoundPitch") as f64 * 0.015625) as f32;
            self.play_sound(vm, player, a, id, sound, p, volume, r as f32, pitch);
        }
        true
    }

    /// The ambient refresh/stop pass (sounds.md 5.4).
    fn ambient_refresh(&mut self, vm: &mut Vm, player: u32, ambient_ok: bool) {
        let l = if player != 0 && vm.is_live(player) { Self::listener(vm, player) } else { 0 };
        let ll = if l != 0 { vm.location(l) } else { [0.0; 3] };
        let realtime = vm.get_prop(player, "ShowFlags", 0).ok().and_then(|v| v.as_i32()).unwrap_or(0) >> 11 & 1;
        for j in 0..self.channels.len() {
            let c = &self.channels[j];
            if c.id == 0 || !is_ambient(c.id) {
                continue;
            }
            let a = c.actor;
            if a == 0 || !vm.is_live(a) {
                self.stop_sound(j);
                continue;
            }
            let r = (vm.u8p(a, "SoundRadius") as f64 + 1.0) * 25.0;
            let p = vm.location(a);
            let (dx, dy, dz) = (p[0] as f64 - ll[0] as f64, p[1] as f64 - ll[1] as f64, p[2] as f64 - ll[2] as f64);
            let s = (dz * dz + dy * dy) + dx * dx;
            let snd = vm.op(a, "AmbientSound");
            let same = Self::object_path(vm, snd).and_then(|p| self.sounds.get(&p).cloned().flatten()).is_some_and(|s| self.channels[j].sound.as_ref().is_some_and(|c| Arc::ptr_eq(c, &s)));
            if !(r * r >= s) || !same || !ambient_ok {
                self.stop_sound(j);
                continue;
            }
            let base = vm.u8p(a, "SoundVolume") as f64 * self.ambient_factor as f64 * (1.0 / 255.0);
            let mut v = (base + base) as f32;
            if vm.u8p(a, "LightType") != 0 {
                v = (vm.u8p(a, "LightBrightness") as f64 * v as f64 * (1.0 / 255.0)) as f32;
                // GlobalLighting: the steady value without real time, else the type's function, clamped to 0..1
                v = if realtime == 0 { v.clamp(0.0, 1.0) } else { vm.light_type_scale(a, v) };
            }
            let c = &mut self.channels[j];
            c.volume = v;
            c.radius = r as f32;
            c.pitch = (vm.u8p(a, "SoundPitch") as f64 * 0.015625) as f32;
        }
    }

    /// The per-channel pass (sounds.md 5.5).
    fn channel_pass(&mut self, vm: &mut Vm, player: u32, coords: &ue_bsp::Coords, dt: f64) {
        let mixer = self.mixer.clone();
        let (sv, pv) = {
            let m = mixer.lock().unwrap();
            (m.sound_volume, m.speech_volume)
        };
        let l = if player != 0 && vm.is_live(player) { Self::listener(vm, player) } else { 0 };
        let pawn_class = vm.reg.class_by_name("Pawn");
        let is_pawn = |vm: &Vm, a: u32| match (vm.class_of(a), pawn_class) {
            (Some(c), Some(p)) => vm.is_a(c, p),
            _ => false,
        };
        for i in 0..self.channels.len() {
            if self.channels[i].id == 0 {
                continue;
            }
            if self.channels[i].started && !mixer.lock().unwrap().voice_playing(i) {
                self.stop_sound(i);
                continue;
            }
            let c = &mut self.channels[i];
            c.play_time = (c.play_time as f64 + dt) as f32;
            if c.actor != 0 && vm.is_live(c.actor) {
                c.location = vm.location(c.actor);
            }
            // f. the priority from L
            let ll = if l != 0 { vm.location(l) } else { c.location };
            let (dx, dy, dz) = (c.location[0] as f64 - ll[0] as f64, c.location[1] as f64 - ll[1] as f64, c.location[2] as f64 - ll[2] as f64);
            c.priority = ((1.0 - ((dx * dx + dy * dy) + dz * dz).sqrt() / c.radius as f64) * c.volume as f64) as f32;
            // g. listener space (the camera frame)
            let (o, x, y, z) = (coords.origin, coords.x, coords.y, coords.z);
            let d = [c.location[0] as f64 - o[0] as f64, c.location[1] as f64 - o[1] as f64, c.location[2] as f64 - o[2] as f64];
            let dot = |a: [f32; 3]| ((d[0] * a[0] as f64 + d[1] * a[1] as f64) + d[2] * a[2] as f64) as f32;
            let (lx, ly, lz) = (dot(x), dot(y), dot(z));
            // h. pan
            let az = if lz < 0.0 { -lz } else { lz };
            let angle = (lx as f64).atan2(az as f64) as f32;
            let rad10 = (c.radius as f64 * 0.1) as f32;
            let r2e = ((ly as f64 * ly as f64) + lx as f64 * lx as f64) + lz as f64 * lz as f64;
            let r2 = r2e as f32;
            let dist = r2e.sqrt();
            let v = if (rad10 as f64 * rad10 as f64) > r2 as f64 { (dist / rad10 as f64) * angle as f64 } else { angle as f64 };
            let mut pan = ftol(v * 28671.125f32 as f64 * 0.3183098861837907 + 16383.0);
            pan = pan.clamp(0, 0x7fff);
            // i. distance attenuation from the camera
            let mut att = 1.0 - dist / c.radius as f64;
            if !(att >= 0.0) {
                att = 0.0;
            } else if att >= 1.0 {
                att = 1.0;
            }
            let att = att as f32;
            let talk = is_talk(c.id);
            // k. occlusion: a BSP line from the viewport actor's eye
            let blocked = if c.actor != 0 && !talk && vm.is_live(c.actor) && player != 0 && vm.is_live(player) {
                let mut eye = vm.location(player);
                if is_pawn(vm, player) {
                    eye[2] = (vm.fp(player, "EyeHeight") as f64 + eye[2] as f64) as f32;
                }
                let at = vm.location(c.actor);
                vm.bsp.as_ref().map(|b| !b.fast_line_check(eye, at)).unwrap_or(false)
            } else {
                false
            };
            let c = &mut self.channels[i];
            if blocked {
                let x = c.occlusion as f64 + dt;
                c.occlusion = if x < 0.5 { x as f32 } else { 0.5 };
            } else {
                let x = c.occlusion as f64 - dt;
                c.occlusion = if x > 0.0 { x as f32 } else { 0.0 };
            }
            let mut occf = 1.0 - (c.occlusion as f64 + c.occlusion as f64);
            if !(occf > 0.33) {
                occf = 0.33;
            }
            // l. the category scale and the volume
            let k: f64 = {
                let (a, b) = if talk { (pv, sv) } else { (sv, pv) };
                if a < b {
                    a as f64 / b as f64
                } else if a > b {
                    1.0
                } else {
                    sv as f64 * 0.003921568859368563f32 as f64
                }
            };
            let mut vol = ftol(((occf * att as f64) * k) * c.volume as f64 * 32767.0);
            vol = vol.clamp(0, 0x7fff);
            // m. reverse stereo, surround
            if self.reverse_stereo {
                pan = 0x7fff - pan;
            }
            if (lz as f64) < 0.0 && self.use_surround {
                pan = 0xc000;
            }
            // n. the floor
            if vol < 0x80 {
                vol = 0x80;
            }
            // o. Doppler (ambient sounds of an actor)
            let mut dop = 1.0f32;
            if c.actor != 0 && is_ambient(c.id) && vm.is_live(c.actor) && l != 0 {
                let a = vm.location(c.actor);
                let ll = vm.location(l);
                let n = ue_bsp::safe_normal([(a[0] as f64 - ll[0] as f64) as f32, (a[1] as f64 - ll[1] as f64) as f32, (a[2] as f64 - ll[2] as f64) as f32]);
                let vel = match vm.get_prop(c.actor, "Velocity", 0) {
                    Ok(Value::Vector(v)) => v,
                    _ => [0.0; 3],
                };
                let t = (n[2] as f64 * vel[2] as f64 + n[1] as f64 * vel[1] as f64) + n[0] as f64 * vel[0] as f64;
                let mut d = 1.0 - t / self.doppler_speed as f64;
                if !(d >= 0.5) {
                    d = 0.5;
                } else if d >= 2.0 {
                    d = 2.0;
                }
                dop = d as f32;
            }
            let Some(s) = c.sound.clone() else { continue };
            let freq = ftol(s.rate as f64 * c.pitch as f64 * dop as f64) as u32;
            if std::env::var("GX_DEBUG").is_ok() {
                eprintln!("channel {i} id {} started {} vol {vol} pan {pan} freq {freq} att {att} k {k} occ {occf} volume {} radius {} flags {:x}", c.id, c.started, c.volume, c.radius, s.flags);
            }
            if !c.started {
                c.started = true;
                mixer.lock().unwrap().start_voice(i, &s, freq, vol as u16, pan as u16);
            } else {
                if c.actor != 0 && talk && vm.is_live(c.actor) && is_pawn(vm, c.actor) && c.play_time >= c.next_phoneme && vm.bp(c.actor, "bIsSpeaking") {
                    let pt = c.play_time;
                    let letter = self.phoneme(i, &s, pt);
                    let c = &mut self.channels[i];
                    if let Some(letter) = letter {
                        let _ = vm.set_prop(c.actor, "nextPhoneme", 0, Value::Str(letter.to_string()));
                    }
                    c.next_phoneme = (c.next_phoneme as f64 + 0.1) as f32;
                }
                let mut m = mixer.lock().unwrap();
                m.update_voice(i, freq, vol as u16, pan as u16);
                m.set_voice_pan_target(i, pan as u16);
            }
        }
    }

    /// The lip-sync analysis of a channel's voice (sounds.md 5.5 s): a streamed sound is read in its ring from the
    /// play position, a whole one 0.1 s ahead of the play time.
    fn phoneme(&mut self, i: usize, s: &Arc<Sample>, play_time: f32) -> Option<char> {
        let vd = self.mixer.lock().unwrap().voice_data(i)?;
        let bits8 = vd.sflags & 4 == 0;
        let data: Arc<Pcm> = vd.data.clone();
        let at = move |k: usize| data.get(k);
        if s.flags & sample::F_MP3 != 0 {
            let len = (vd.loop_end - vd.start).max(0) as usize;
            let pos = (vd.pos - vd.start).max(0) as usize;
            Some(self.phonemes.detect_raw(s.rate, bits8, &at, pos, len))
        } else {
            let p = ftol(ftol(s.rate as f64 * play_time as f64) as u32 as f64 + s.rate as f64 * 0.1) as u32 as usize;
            Some(self.phonemes.detect_raw(s.rate, bits8, &at, p, s.length as usize))
        }
    }

    fn object_path(vm: &Vm, r: u32) -> Option<String> {
        (r != 0).then(|| vm.reg.obj_path(ue_registry::ObjId(r)).to_string())
    }

    /// The module of a music object (the whole file), `None` when it cannot be loaded.
    fn music_data(&mut self, path: &str) -> Option<Arc<Vec<u8>>> {
        let (pkg_name, rest) = path.split_once('.')?;
        let pkg = self.files.package(pkg_name)?;
        let export = pkg.find(rest)?;
        load_music(&pkg.pkg, export).ok().map(|a| Arc::new(a.data))
    }

    /// A level was loaded (travel or a saved game). `UGameEngine::LoadMap` purges the old level once the new one is
    /// in, before the player logs in: a song nothing in the new level refers to is destroyed, and Galaxy's
    /// `UnregisterMusic` (10606e60) stops it and forgets it as the current song. A song the new level uses stays and
    /// plays on (the login's `ClientSetMusic` then fades it).
    pub fn level_changed(&mut self, vm: &mut Vm) {
        // `LoadMap` calls `SetViewport`, whose first step stops every channel; it also applies the stored reverb
        // block again (a fresh state)
        self.stop_all_sounds();
        let b = self.reverb_block;
        self.mixer.lock().unwrap().set_reverb(&b);
        let Some(song) = self.music.song.clone() else { return };
        if !Self::level_refers_to(vm, &song) {
            self.mixer.lock().unwrap().unload_music();
            self.music.song = None;
            self.music.data = None;
        }
    }

    /// Whether the loaded level refers to a music object: its `LevelInfo.Song`, or an import of that name and class
    /// `Music` in the map package (an actor's property).
    fn level_refers_to(vm: &mut Vm, path: &str) -> bool {
        let level = vm.level;
        let level_song = vm.op(level, "Song");
        if Self::object_path(vm, level_song).is_some_and(|p| p.eq_ignore_ascii_case(path)) {
            return true;
        }
        let Some(map) = vm.level_paths.get(&level).and_then(|p| p.split('.').next()).map(str::to_string) else { return false };
        let name = path.rsplit('.').next().unwrap_or(path);
        vm.reg.packages.iter().filter(|p| p.name.eq_ignore_ascii_case(&map)).any(|p| {
            p.pkg.imports.iter().any(|i| p.pkg.name(i.object_name).eq_ignore_ascii_case(name) && p.pkg.name(i.class_name).eq_ignore_ascii_case("Music"))
        })
    }

    /// The music part of `UGalaxyAudioSubsystem::Update` (10609510), from the viewing player's `Song`, `SongSection`,
    /// `CdTrack` and `Transition` (set by `ClientSetMusic`). No transition: the player's `SongSection` becomes the
    /// order the music is at (255 when none plays). A transition first ends the current song: at once (`Instant`,
    /// `Segue`, or when it was stopped with section 255) or after a fade of `MusicFade` from 1 by `dt` (`Fade`),
    /// `3 dt` (`FastFade`) or `0.2 dt` (`SlowFade`), down past a latency margin (-0.002, -0.006, -0.0004 x `Latency`).
    /// Then the new song starts at full volume at its section (section 255: stopped), the module loaded again only
    /// when the song changed; the transition is cleared.
    pub fn update_music(&mut self, vm: &mut Vm, player: u32, dt: f32) {
        if player == 0 || !vm.is_live(player) {
            return;
        }
        let transition = vm.u8p(player, "Transition");
        if transition == 0 {
            let order = self.mixer.lock().unwrap().music_order();
            let section = match order {
                Some(o) if (0..255).contains(&o) => o as u8,
                _ => 255,
            };
            let _ = vm.set_prop(player, "SongSection", 0, Value::Byte(section));
            return;
        }
        let song_ref = vm.op(player, "Song");
        let wanted = Self::object_path(vm, song_ref);
        let changed = self.music.song.as_deref().map(str::to_ascii_lowercase) != wanted.as_deref().map(str::to_ascii_lowercase);
        let latency = self.latency as f64;
        if self.music.song.is_some() || self.music.cd_track != 255 {
            let done = if self.music.section == 255 {
                true
            } else {
                match transition {
                    // MTRAN_Fade
                    3 => {
                        let left = self.music.fade - dt;
                        self.music.fade = left;
                        (left as f64) < latency * -0.002
                    }
                    // MTRAN_SlowFade
                    5 => {
                        let left = self.music.fade as f64 - dt as f64 * 0.2;
                        self.music.fade = left as f32;
                        left < latency * -0.0004
                    }
                    // MTRAN_FastFade
                    4 => {
                        let left = self.music.fade as f64 - dt as f64 * 3.0;
                        self.music.fade = left as f32;
                        left < latency * -0.006
                    }
                    _ => true,
                }
            };
            if done {
                // the old song is unregistered (stopped and unloaded) only when the song object changes
                if changed {
                    self.mixer.lock().unwrap().unload_music();
                    self.music.data = None;
                }
                self.music.song = None;
                self.music.cd_track = 255;
            } else {
                self.mixer.lock().unwrap().music_fade = self.music.fade;
            }
        }
        if self.music.song.is_none() && self.music.cd_track == 255 {
            self.music.fade = 1.0;
            self.mixer.lock().unwrap().music_fade = 1.0;
            self.music.song = wanted.clone();
            self.music.cd_track = vm.u8p(player, "CdTrack");
            self.music.section = vm.u8p(player, "SongSection");
            if let (Some(path), true) = (wanted, self.digital_music) {
                // a changed song is loaded (from memory); the same song stays loaded with all its state
                if changed || self.music.data.is_none() {
                    self.music.data = self.music_data(&path);
                    if let Some(d) = &self.music.data {
                        if let Ok(title) = self.mixer.lock().unwrap().load_music(d) {
                            self.music_title = Some(title);
                        }
                    }
                }
                // section 255 stops; any other: stop, cut the music voices, start, jump to the section's order
                self.mixer.lock().unwrap().start_music_section(self.music.section);
            }
            let _ = vm.set_prop(player, "Transition", 0, Value::Byte(0));
        }
    }

}
