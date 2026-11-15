//! Conversation data and `ConBindEvents`. The conversations of a mission are ordinary tagged objects of the package
//! `<ConversationPackage>Text` (`DeusExConText`): a `ConversationList` whose `ConItem` chain holds `Conversation`
//! objects, each with a chain of `ConEvent`s. They are instantiated lazily, a whole object graph at a time, the first time
//! an actor of the mission binds. Binding gives the actor a chain of `ConListItem`s (newest first) for the conversations
//! whose owner name is the actor's `BindName` (barks: `BarkBindName`).
use crate::{ref_instance, Value, Vm, VmError};
use std::collections::HashMap;

type Res<T> = Result<T, VmError>;

impl Vm {
    /// The object at `path` (`Package.Name`) of a data package, with everything it refers to inside the same package whose
    /// class is a `ConObject`, as VM instances. 0 when the object does not exist.
    pub fn load_con_object(&mut self, path: &str) -> u32 {
        if let Some(&r) = self.con_objects.get(path) {
            return r;
        }
        self.reg.ensure_package_for_path(path);
        let Some(con_object) = self.reg.class_by_name("ConObject") else { return 0 };
        // 1. instantiate the object and every ConObject reachable from it
        let mut order: Vec<String> = Vec::new();
        let mut queue = vec![path.to_string()];
        let mut new_refs: HashMap<u32, u32> = HashMap::new();
        while let Some(p) = queue.pop() {
            if self.con_objects.contains_key(&p) || order.contains(&p) {
                continue;
            }
            let Some((pi, ei)) = self.reg.export_by_path(&p) else { continue };
            let Some(class) = self.reg.class_of_export(pi, ei) else { continue };
            if !self.is_a(class, con_object) {
                continue;
            }
            let Ok((inst, _)) = self.instantiate_export(class, pi, ei) else { continue };
            let r = self.adopt(class, inst.data, inst.strings);
            new_refs.insert(self.reg.obj_id(&p).0, r);
            self.con_objects.insert(p.clone(), r);
            self.con_paths.insert(r, p.clone());
            if self.reg.class_by_name("ConEventRandomLabel").map(|c| self.is_a(class, c)).unwrap_or(false) {
                if let Ok(labels) = ue_assets::text::load_con_random_labels(&self.reg.packages[pi].pkg, ei) {
                    self.con_labels.insert(r, labels);
                }
            }
            for id in self.object_ref_values(r) {
                let target = self.reg.obj_path(ue_registry::ObjId(id)).to_string();
                if !target.is_empty() && !self.con_objects.contains_key(&target) {
                    queue.push(target);
                }
            }
            order.push(p);
        }
        // 2. link: references to the objects just made become instance references
        let mut all: HashMap<u32, u32> = HashMap::new();
        for (p, &r) in &self.con_objects {
            all.insert(self.reg.obj_id(p).0, r);
        }
        for p in &order {
            let r = self.con_objects[p];
            self.rewrite_refs(r, &all);
        }
        self.con_objects.get(path).copied().unwrap_or(0)
    }

    /// `ConBindEvents`: gives `actor` the conversations of the current mission that belong to it.
    pub fn con_bind_events(&mut self, actor: u32) -> Result<(), VmError> {
        let Some(info_class) = self.reg.class_by_name("DeusExLevelInfo") else { return Ok(()) };
        let info = self.live_actors().into_iter().find(|&a| self.class_of(a).map(|c| self.is_a(c, info_class)).unwrap_or(false));
        let Some(info) = info else { return Ok(()) };
        let mission = self.get_prop(info, "missionNumber", 0)?.as_i32().unwrap_or(-1);
        if mission < 0 {
            return Ok(());
        }
        // the class default is "DeusExConversations", whose text package is DeusExConText
        let package = match self.get_prop(info, "ConversationPackage", 0)? {
            Value::Str(s) if !s.eq_ignore_ascii_case("DeusExConversations") => s,
            _ => "DeusExCon".to_string(),
        };
        let list = self.load_con_object(&format!("{package}Text.ConList_Mission{mission:02}"));
        if list == 0 {
            return Ok(());
        }
        // `DConversationList::BindConversations`
        self.purge_bound_conversations(actor)?;
        let text = |v: Value| match v {
            Value::Str(s) => s,
            _ => String::new(),
        };
        let bind = text(self.get_prop(actor, "BindName", 0)?);
        let bark_bind = text(self.get_prop(actor, "BarkBindName", 0)?);
        let item_class = self.reg.class_by_name("ConListItem").ok_or_else(|| VmError::Runtime("no ConListItem class".into()))?;
        let mut item = match self.get_prop(list, "conversations", 0)? {
            Value::Obj(i) => i,
            _ => 0,
        };
        let mut guard = 0;
        while item != 0 && guard < 100_000 {
            guard += 1;
            let con = match self.get_prop(item, "conObject", 0)? {
                Value::Obj(c) => c,
                _ => 0,
            };
            let next = match self.get_prop(item, "next", 0)? {
                Value::Obj(n) => n,
                _ => 0,
            };
            if con != 0 {
                let owner = text(self.get_prop(con, "conOwnerName", 0)?);
                let name = match self.get_prop(con, "conName", 0)? {
                    Value::Name(n) => self.reg.names.str(n).to_string(),
                    _ => String::new(),
                };
                let is_bark = name.contains("_Bark");
                // barks of the bark name; otherwise the actor's own conversations, without the barks when it has a
                // bark name (names compare case-insensitively, empty ones too)
                let attach = (is_bark && bark_bind.eq_ignore_ascii_case(&owner))
                    || (bind.eq_ignore_ascii_case(&owner) && (bark_bind.is_empty() || !is_bark));
                if attach {
                    let count = self.get_prop(con, "ownerRefCount", 0)?.as_i32().unwrap_or(0);
                    self.set_prop(con, "ownerRefCount", 0, Value::Int(count + 1))?;
                    let li = self.new_object(item_class)?;
                    self.set_prop(li, "con", 0, Value::Obj(con))?;
                    let head = self.get_prop(actor, "ConListItems", 0)?;
                    self.set_prop(li, "next", 0, head)?;
                    self.set_prop(actor, "ConListItems", 0, Value::Obj(li))?;
                }
            }
            item = next;
        }
        Ok(())
    }

    /// Drops the actor's conversation chain (`PurgeBoundConversations`).
    fn purge_bound_conversations(&mut self, actor: u32) -> Result<(), VmError> {
        let mut li = match self.get_prop(actor, "ConListItems", 0)? {
            Value::Obj(i) => i,
            _ => 0,
        };
        self.set_prop(actor, "ConListItems", 0, Value::Obj(0))?;
        while li != 0 && ref_instance(li).is_some() {
            if let Value::Obj(con) = self.get_prop(li, "con", 0)? {
                if con != 0 {
                    let count = self.get_prop(con, "ownerRefCount", 0)?.as_i32().unwrap_or(0);
                    self.set_prop(con, "ownerRefCount", 0, Value::Int(count - 1))?;
                }
            }
            li = match self.get_prop(li, "next", 0)? {
                Value::Obj(n) => n,
                _ => 0,
            };
        }
        Ok(())
    }

    // ---- the natives of the ConSys classes

    pub(crate) fn con_native(&mut self, name: &str, this: u32, a: &mut [Value]) -> Res<Value> {
        let int = |a: &[Value], i: usize| a.get(i).and_then(Value::as_i32).unwrap_or(0);
        let obj = |a: &[Value], i: usize| match a.get(i) {
            Some(Value::Obj(o)) => *o,
            _ => 0,
        };
        Ok(match name {
            // both made in the package `DeusEx` (ConSys 10010600, 100106f0: `CreatePackage(NULL, "DeusEx")`)
            "CreateFlagRef" => {
                let class = self.reg.class_by_name("ConFlagRef").ok_or_else(|| VmError::Runtime("no ConFlagRef class".into()))?;
                let outer = self.reg.obj_id("DeusEx").0;
                let r = self.new_script_object(class, outer, 0)?;
                if let Some(Value::Name(n)) = a.first() {
                    self.set_prop(r, "flagName", 0, Value::Name(*n))?;
                }
                self.set_prop(r, "value", 0, Value::Bool(matches!(a.get(1), Some(Value::Bool(true)))))?;
                Value::Obj(r)
            }
            "CreateConCamera" => {
                let class = self.reg.class_by_name("ConCamera").ok_or_else(|| VmError::Runtime("no ConCamera class".into()))?;
                let outer = self.reg.obj_id("DeusEx").0;
                Value::Obj(self.new_script_object(class, outer, 0)?)
            }
            // the native gets a pointer to the caller's `ConActorsBound[10]`: cleared, then filled with the cast
            // (the array's ten elements come first, `invokeActor` after them)
            "BindEvents" => {
                let bound = self.con_bind_all(this, obj(a, 10))?;
                let mut elems: Vec<Value> = bound.into_iter().map(Value::Obj).collect();
                elems.resize(10, Value::Obj(0));
                self.native_array_out = Some((0, elems));
                Value::Void
            }
            "BindActorEvents" => {
                let actor = obj(a, 0);
                for ev in self.con_events(this)? {
                    self.con_bind_actor(ev, actor, actor, this)?;
                }
                Value::Void
            }
            // `DConversation::ClearBindEvents` walks the event list and does nothing else: the bound actors stay
            "ClearBindEvents" => Value::Void,
            "GetSpeechAudio" => Value::Obj(self.con_speech_audio(this, int(a, 0))),
            "GetSpeechLength" => {
                let s = self.con_speech_audio(this, int(a, 0));
                Value::Float(if s == 0 { 0.0 } else { self.sound_length(s) })
            }
            "GetSoundLength" => Value::Float(self.sound_length(obj(a, 0))),
            "GetLabelCount" => Value::Int(self.con_labels.get(&this).map(|l| l.len()).unwrap_or(0) as i32),
            "GetLabel" => {
                let i = int(a, 0);
                Value::Str(self.con_labels.get(&this).and_then(|l| usize::try_from(i).ok().and_then(|i| l.get(i))).cloned().unwrap_or_default())
            }
            "GetRandomLabel" => Value::Str(self.con_random_label(this)?),
            other => return Err(VmError::Unsupported(format!("conversation native {other}"))),
        })
    }

    fn con_events(&mut self, con: u32) -> Res<Vec<u32>> {
        let mut out = Vec::new();
        let mut ev = match self.get_prop(con, "eventList", 0)? {
            Value::Obj(e) => e,
            _ => 0,
        };
        while ev != 0 && out.len() < 100_000 {
            out.push(ev);
            ev = match self.get_prop(ev, "nextEvent", 0)? {
                Value::Obj(e) => e,
                _ => 0,
            };
        }
        Ok(out)
    }

    fn text_prop(&mut self, obj: u32, name: &'static str) -> String {
        match self.get_prop(obj, name, 0) {
            Ok(Value::Str(s)) => s,
            _ => String::new(),
        }
    }

    /// `Conversation::BindEvents`: every pawn, then every decoration, then every player pawn (each in object order; a
    /// player is offered twice) that has a `BindName` is offered to every event. An actor sharing the invoking actor's
    /// name is skipped (only the invoker plays that part). Returns the actors that took a part, at most 10, first come
    /// first.
    fn con_bind_all(&mut self, con: u32, invoke: u32) -> Res<Vec<u32>> {
        let events = self.con_events(con)?;
        let invoke_name = if invoke != 0 { self.text_prop(invoke, "BindName") } else { String::new() };
        let mut bound: Vec<u32> = Vec::new();
        let actors = self.live_actors();
        for class_name in ["Pawn", "Decoration", "PlayerPawn"] {
            let Some(cls) = self.reg.class_by_name(class_name) else { continue };
            for &actor in &actors {
                if !self.class_of(actor).map(|c| self.is_a(c, cls)).unwrap_or(false) {
                    continue;
                }
                let name = self.text_prop(actor, "BindName");
                if name.is_empty() {
                    continue;
                }
                if invoke != actor && invoke != 0 && invoke_name.eq_ignore_ascii_case(&name) {
                    continue;
                }
                for &ev in &events {
                    if self.con_bind_actor(ev, invoke, actor, con)? && bound.len() < 10 && !bound.contains(&actor) {
                        bound.push(actor);
                    }
                }
            }
        }
        Ok(bound)
    }

    /// `ConEvent::BindActor(invoke, actor, con)` for the event's class; true when the actor took a part in the event.
    fn con_bind_actor(&mut self, ev: u32, invoke: u32, actor: u32, con: u32) -> Res<bool> {
        let Some(class) = self.class_of(ev) else { return Ok(false) };
        let is = |vm: &Vm, n: &str| vm.reg.class_by_name(n).map(|c| vm.is_a(class, c)).unwrap_or(false);
        let bind = self.text_prop(actor, "BindName");
        let bark = self.text_prop(actor, "BarkBindName");
        let first_person = matches!(self.get_prop(con, "bFirstPerson", 0), Ok(Value::Bool(true)));
        let same = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
        if is(self, "ConEventSpeech") {
            let (speaker, to) = (self.text_prop(ev, "speakerName"), self.text_prop(ev, "speakingToName"));
            let mut taken = false;
            if same(&bind, &speaker) || (invoke == actor && same(&bark, &speaker)) {
                self.set_prop(ev, "speaker", 0, Value::Obj(actor))?;
                taken = true;
            }
            if same(&bind, &to) || (invoke == actor && same(&bark, &to)) {
                self.set_prop(ev, "speakingTo", 0, Value::Obj(actor))?;
                taken = true;
            }
            return Ok(taken);
        }
        if is(self, "ConEventAnimation") || is(self, "ConEventTrade") {
            let owner = self.text_prop(ev, "eventOwnerName");
            if (same(&bark, &owner) && first_person) || same(&bind, &owner) {
                self.set_prop(ev, "eventOwner", 0, Value::Obj(actor))?;
                return Ok(true);
            }
            return Ok(false);
        }
        if is(self, "ConEventTransferObject") {
            let (from, to, object) = (self.text_prop(ev, "fromName"), self.text_prop(ev, "toName"), self.text_prop(ev, "objectName"));
            let mut taken = false;
            if (same(&bark, &from) && first_person) || same(&bind, &from) {
                self.set_prop(ev, "fromActor", 0, Value::Obj(actor))?;
                taken = true;
            }
            if (same(&bark, &to) && first_person) || same(&bind, &to) {
                self.set_prop(ev, "toActor", 0, Value::Obj(actor))?;
                taken = true;
            }
            let class_ref = self.deusex_class_ref(&object);
            self.set_prop(ev, "giveObject", 0, Value::Obj(class_ref))?;
            return Ok(taken);
        }
        if is(self, "ConEventCheckObject") {
            let object = self.text_prop(ev, "objectName");
            let class_ref = self.deusex_class_ref(&object);
            self.set_prop(ev, "checkObject", 0, Value::Obj(class_ref))?;
        }
        Ok(false)
    }

    /// The class `DeusEx.<name>` as a class reference value (0 when the name is empty or no such class exists).
    fn deusex_class_ref(&mut self, name: &str) -> u32 {
        if name.is_empty() {
            return 0;
        }
        let path = format!("DeusEx.{name}");
        if self.reg.class_by_path(&path).is_some() {
            self.reg.obj_id(&path).0
        } else {
            0
        }
    }

    /// The voice line `soundID` of a conversation: sound `ConAudio<audioPackageName>_<id>` of the package
    /// `<conversation package without "Text">Audio<audioPackageName>` (0 when there is none).
    fn con_speech_audio(&mut self, con: u32, id: i32) -> u32 {
        if id == -1 {
            return 0;
        }
        let audio = self.text_prop(con, "audioPackageName");
        let Some(path) = self.con_paths.get(&con) else { return 0 };
        let package = path.split('.').next().unwrap_or("");
        let base = package.strip_suffix("Text").unwrap_or(package);
        let sound = format!("{base}Audio{audio}.ConAudio{audio}_{id}");
        self.reg.ensure_package_for_path(&sound);
        if self.reg.export_by_path(&sound).is_some() {
            self.reg.obj_id(&sound).0
        } else {
            0
        }
    }

    /// `DConversation::GetSoundLength`: playing time in seconds of a `Sound` object, as the engine estimates it. A WAV
    /// file: the 32-bit word at byte 40 (the data size of a plain 44-byte header) over the byte rate at byte 28. An MP3
    /// file starting with a frame header: the whole file size in bits over the first frame's bit rate (constant bit rate
    /// assumed; free/bad bit rates give a negative length, as in the engine). 0 when it cannot be read.
    pub fn sound_length(&mut self, sound: u32) -> f32 {
        // kbit/s by bit-rate index; columns: MPEG-1 layer I, II, III, MPEG-2/2.5 layer I, layer II/III
        const BITRATES: [[i32; 5]; 16] = [
            [-1, -1, -1, -1, -1],
            [32, 32, 32, 32, 8],
            [64, 48, 40, 48, 16],
            [96, 56, 48, 56, 24],
            [128, 64, 56, 64, 32],
            [160, 80, 64, 80, 40],
            [192, 96, 80, 96, 48],
            [224, 112, 96, 112, 56],
            [256, 128, 112, 128, 64],
            [288, 160, 128, 144, 80],
            [320, 192, 160, 160, 96],
            [352, 224, 192, 176, 112],
            [384, 256, 224, 192, 128],
            [416, 320, 256, 224, 144],
            [448, 384, 320, 256, 160],
            [-1, -1, -1, -1, -1],
        ];
        if sound == 0 {
            return 0.0;
        }
        let path = self.reg.obj_path(ue_registry::ObjId(sound)).to_string();
        let Some((pi, ei)) = self.reg.export_by_path(&path) else { return 0.0 };
        let Ok(file) = ue_assets::audio::load_sound(&self.reg.packages[pi].pkg, ei) else { return 0.0 };
        let d = &file.data;
        let word = |o: usize| d.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).unwrap_or(0);
        if d.starts_with(b"RIFF") {
            return word(40) as f32 / word(28) as i32 as f32;
        }
        let h = word(0);
        if h & 0xff != 0xff || h & 0xe000 != 0xe000 {
            return 0.0; // the engine logs and returns garbage
        }
        let (version, layer) = (h & 0x1800, h & 0x600);
        let col = match (version, layer) {
            (0x1800, 0x600) => 0,
            (0x1800, 0x400) => 1,
            (0x1800, 0x200) => 2,
            (0x1800, _) | (0x800, _) => 0,
            (_, 0x600) => 3,
            (_, 0x400) | (_, 0x200) => 4,
            _ => 0,
        };
        let kbps = BITRATES[((h >> 20) & 0xf) as usize][col];
        (d.len() as i32 * 8) as f32 / (kbps * 1000) as f32
    }

    /// `ConEventRandomLabel::GetRandomLabel`: random, or the labels in turn when the event cycles.
    fn con_random_label(&mut self, ev: u32) -> Res<String> {
        let n = self.con_labels.get(&ev).map(|l| l.len()).unwrap_or(0) as i32;
        if n < 1 {
            return Ok(String::new());
        }
        let flag = |vm: &mut Vm, f: &'static str| matches!(vm.get_prop(ev, f, 0), Ok(Value::Bool(true)));
        let mut index = self.get_prop(ev, "cycleIndex", 0)?.as_i32().unwrap_or(0);
        if index == n {
            self.set_prop(ev, "bLabelsCycled", 0, Value::Bool(true))?;
        }
        let cycled = flag(self, "bLabelsCycled");
        let pick = if !flag(self, "bCycleEvents") || (flag(self, "bCycleRandom") && cycled) {
            ((self.frand() * n as f32) as i32).min(n - 1)
        } else if !flag(self, "bCycleOnce") || index != n - 1 {
            let pick = index % n;
            index += 1;
            self.set_prop(ev, "cycleIndex", 0, Value::Int(index))?;
            pick
        } else {
            n - 1
        };
        Ok(self.con_labels[&ev][pick as usize].clone())
    }
}
