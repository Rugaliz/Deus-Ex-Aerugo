//! Conversations on Liberty Island. Set `DEUSEX_DIR` to the game folder.

use dx_play::Session;
use std::path::PathBuf;
use ue_vm::Value;

const DT: f32 = 1.0 / 30.0;

fn new_game() -> Option<Session> {
    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    let mut s = Session::open(&g, "DX").unwrap();
    s.exec("StartNewGame 01_NYC_UNATCOIsland");
    for _ in 0..5 {
        s.step_bound(DT);
    }
    assert_eq!(s.map_name, "01_NYC_UNATCOIsland");
    Some(s)
}

fn text(v: Result<Value, ue_vm::VmError>) -> String {
    match v {
        Ok(Value::Str(x)) => x,
        Ok(Value::Name(n)) => format!("name#{n}"),
        other => format!("{other:?}"),
    }
}

#[test]
fn actors_get_their_conversations() {
    let Some(s) = new_game() else { return };
    let s = s;
    let mut bound = 0;
    let mut sample = Vec::new();
    for a in s.vm.live_actors() {
        let head = match s.vm.get_prop(a, "ConListItems", 0) {
            Ok(Value::Obj(h)) if h != 0 => h,
            _ => continue,
        };
        bound += 1;
        let mut n = 0;
        let mut li = head;
        let mut first = String::new();
        while li != 0 && n < 100 {
            if let Ok(Value::Obj(con)) = s.vm.get_prop(li, "con", 0) {
                if first.is_empty() {
                    first = text(s.vm.get_prop(con, "conOwnerName", 0));
                }
            }
            li = match s.vm.get_prop(li, "next", 0) { Ok(Value::Obj(x)) => x, _ => 0 };
            n += 1;
        }
        let class = s.vm.class_of(a).map(|c| s.vm.reg.class(c).path.clone()).unwrap_or_default();
        sample.push(format!("{class} bind {:?}: {n} conversations, owner {first}", text(s.vm.get_prop(a, "BindName", 0))));
    }
    for l in sample.iter().take(15) {
        eprintln!("{l}");
    }
    eprintln!("{bound} actors with conversations");
    assert!(bound >= 3, "only {bound} actors have conversations");
}

fn actor_of(s: &mut Session, class: &str) -> u32 {
    let c = s.vm.reg.class_by_name(class).unwrap();
    s.vm.live_actors().into_iter().find(|&a| s.vm.class_of(a).map(|k| s.vm.is_a(k, c)).unwrap_or(false)).unwrap()
}

fn stand_before(s: &mut Session, target: u32, dist: f32) {
    let t = s.vm.location(target);
    let (spot, yaw) = (0..16)
        .find_map(|k| {
            let a = k as f32 * std::f32::consts::TAU / 16.0;
            let p = [t[0] + dist * a.cos(), t[1] + dist * a.sin(), t[2]];
            s.vm.bsp.as_ref().unwrap().fast_line_check(p, t).then_some((p, (a + std::f32::consts::PI).to_degrees()))
        })
        .expect("no spot with a line of sight");
    s.teleport(spot, yaw);
}

fn frob_paul(s: &mut Session) -> u32 {
    let paul = actor_of(s, "PaulDenton");
    stand_before(s, paul, 60.0);
    for _ in 0..10 {
        s.step_bound(DT);
    }
    s.input.key(dx_input::key_code("F").unwrap(), true);
    s.step_bound(DT);
    s.input.key(dx_input::key_code("F").unwrap(), false);
    paul
}

fn current_event_text(s: &mut Session) -> String {
    let cp = match s.vm.get_prop(s.player, "conPlay", 0) { Ok(Value::Obj(c)) => c, _ => 0 };
    let ev = match s.vm.get_prop(cp, "currentEvent", 0) { Ok(Value::Obj(e)) => e, _ => 0 };
    let sp = match s.vm.get_prop(ev, "conSpeech", 0) { Ok(Value::Obj(e)) => e, _ => 0 };
    text(s.vm.get_prop(sp, "speech", 0))
}

/// Frobbing Paul starts the conversation of the mission start: the player and Paul enter the conversation state, the first
/// line is spoken (voice line played, text shown), and after its length the next line follows.
#[test]
fn frobbing_paul_plays_his_conversation() {
    let Some(mut s) = new_game() else { return };
    let paul = frob_paul(&mut s);
    for _ in 0..15 {
        s.step_bound(DT);
    }
    assert_eq!(s.vm.state_name(s.player), Some("Conversation"));
    assert_eq!(s.vm.state_name(paul), Some("Conversation"));
    let first = current_event_text(&mut s);
    assert!(!first.is_empty(), "no line is being spoken");
    let voiced = s.vm.ai.sounds.iter().any(|r| s.vm.reg.obj_path(ue_registry::ObjId(r.sound)).contains("ConAudio"));
    assert!(voiced, "the voice line was not played");
    for _ in 0..30 * 14 {
        s.step_bound(DT);
    }
    let later = current_event_text(&mut s);
    assert_ne!(first, later, "the conversation did not move on");
}

/// `BindEvents` fills the conversation player's `ConActorsBound` with the cast, so destroying one of them mid-conversation
/// aborts it (`ConPlayBase.ActorDestroyed`).
#[test]
fn destroying_a_cast_member_ends_the_conversation() {
    let Some(mut s) = new_game() else { return };
    let paul = frob_paul(&mut s);
    for _ in 0..15 {
        s.step_bound(DT);
    }
    assert_eq!(s.vm.state_name(s.player), Some("Conversation"));
    let cp = match s.vm.get_prop(s.player, "conPlay", 0) { Ok(Value::Obj(c)) => c, _ => 0 };
    let bound: Vec<u32> = (0..10).filter_map(|i| match s.vm.get_prop(cp, "ConActorsBound", i) { Ok(Value::Obj(a)) if a != 0 => Some(a), _ => None }).collect();
    eprintln!("bound: {bound:?} (paul {paul}, player {})", s.player);
    assert!(bound.contains(&paul), "Paul is not in ConActorsBound");
    s.vm.destroy(paul).unwrap();
    assert_eq!(s.vm.get_prop(cp, "bEndingConversation", 0).ok(), Some(Value::Bool(true)), "ActorDestroyed did not terminate");
    // the conversation window closes first
    for _ in 0..60 {
        s.step_bound(DT);
    }
    assert_ne!(s.vm.state_name(s.player), Some("Conversation"), "the conversation went on without Paul");
}

/// Runs Paul's conversation to its end: the weapon choice is clicked with the mouse, everyone is released and the player
/// walks again. Slow (about a minute).
#[test]
#[ignore]
fn paul_conversation_ends() {
    let Some(mut s) = new_game() else { return };
    let paul = frob_paul(&mut s);
    let left = dx_input::key_code("LeftMouse").unwrap();
    let mut clicked = false;
    for sec in 0..120 {
        for _ in 0..30 {
            s.step_bound(DT);
        }
        // the choice window: the second option (the sniper rifle is the first)
        if !clicked && sec >= 50 && s.ui_wants_mouse() {
            s.ui_mouse_abs(150.0, 510.0, 800.0, 600.0);
            s.step_bound(DT);
            s.ui_key(left, true);
            s.step_bound(DT);
            s.ui_key(left, false);
            clicked = true;
            eprintln!("{sec}s: clicked a choice");
        }
        if s.vm.state_name(s.player) != Some("Conversation") && s.vm.state_name(paul) != Some("Conversation") && sec > 5 {
            eprintln!("conversation over after {sec}s; player {:?} paul {:?}", s.vm.state_name(s.player), s.vm.state_name(paul));
            assert!(clicked, "it ended without a choice");
            // the briefing's goal went into the player's goal list, and the weapon choice gave a weapon
            let goal = match s.vm.get_prop(s.player, "FirstGoal", 0) { Ok(Value::Obj(g)) => g, _ => 0 };
            eprintln!("first goal {goal}: {}", text(s.vm.get_prop(goal, "text", 0)));
            assert!(goal != 0, "no goal was added");
            return;
        }
    }
    panic!("the conversation never ended (clicked: {clicked})");
}

fn shot(s: &mut Session, r: &mut Option<dx_vk::renderer::Renderer>, name: &str) {
    let img = s.render_vk(r, 800, 600).expect("vulkan render");
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../out/screenshots");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), dx_soft::png(img.width, img.height, &img.rgba)).unwrap();
}

/// Pictures of the first seconds of Paul's conversation (`../out/screenshots/s10_con_*.png`); needs a Vulkan device.
#[test]
#[ignore]
fn shots_of_a_conversation() {
    let Some(mut s) = new_game() else { return };
    let paul = actor_of(&mut s, "PaulDenton");
    stand_before(&mut s, paul, 60.0);
    for _ in 0..10 {
        s.step_bound(DT);
    }
    s.input.key(dx_input::key_code("F").unwrap(), true);
    s.step_bound(DT);
    s.input.key(dx_input::key_code("F").unwrap(), false);
    let mut r = None;
    for i in 0..8 {
        for _ in 0..90 {
            s.step_bound(DT);
        }
        shot(&mut s, &mut r, &format!("s10_con_{i}.png"));
    }
    r.take().unwrap().destroy();
}

#[test]
#[ignore]
fn tick_speed() {
    let Some(mut s) = new_game() else { return };
    let t = std::time::Instant::now();
    for _ in 0..300 {
        s.step_bound(DT);
    }
    eprintln!("300 idle frames: {:?}", t.elapsed());
    if s.vm.profile_on {
        eprintln!("{}", s.vm.profile_report(40));
    }
    let _ = frob_paul(&mut s);
    let t = std::time::Instant::now();
    for _ in 0..300 {
        s.step_bound(DT);
    }
    eprintln!("300 conversation frames: {:?}", t.elapsed());
}

/// The weapon choice at the end of Paul's briefing (`../out/screenshots/s10_choice.png`); needs a Vulkan device.
#[test]
#[ignore]
fn shot_of_the_choice() {
    let Some(mut s) = new_game() else { return };
    let _ = frob_paul(&mut s);
    for _ in 0..30 * 52 {
        s.step_bound(DT);
    }
    let mut r = None;
    shot(&mut s, &mut r, "s10_choice.png");
    r.take().unwrap().destroy();
}

#[test]
#[ignore]
fn survey_shadow() {
    let Some(mut s) = new_game() else { return };
    for _ in 0..30 { s.step_bound(DT); }
    let c = s.vm.reg.class_by_name("Shadow").unwrap();
    let a = s.vm.live_actors().into_iter().find(|&a| s.vm.class_of(a) == Some(c)).unwrap();
    let tex = match s.vm.get_prop(a, "Texture", 0) { Ok(Value::Obj(o)) => s.vm.reg.obj_path(ue_registry::ObjId(o)).to_string(), _ => "-".into() };
    eprintln!("shadow Style {:?} Texture {tex} Scale {:?} Glow {:?} Unlit {:?} DrawType {:?} bHidden {:?}", s.vm.get_prop(a, "Style", 0), s.vm.get_prop(a, "DrawScale", 0), s.vm.get_prop(a, "ScaleGlow", 0), s.vm.get_prop(a, "bUnlit", 0), s.vm.get_prop(a, "DrawType", 0), s.vm.get_prop(a, "bHidden", 0));
}

#[test]
#[ignore]
fn survey_shadow_texture() {
    let Some(s) = new_game() else { return };
    let p = "DeusExItems.Skins.FlatFXTex40";
    let mips = s.view.texture_mips_by_path(p).unwrap();
    let (w, h, px) = &mips[0];
    let at = |x: u32, y: u32| { let i = ((y * w + x) * 4) as usize; [px[i], px[i + 1], px[i + 2], px[i + 3]] };
    eprintln!("{p} {w}x{h} style {} corner {:?} edge {:?} centre {:?}", s.view.texture_style_by_path(p), at(0, 0), at(w / 2, 2), at(w / 2, h / 2));
}

#[test]
#[ignore]
fn survey_mission_start() {
    let Some(mut s) = new_game() else { return };
    s.issues.clear();
    s.vm.log.clear();
    for sec in 0..40 {
        for _ in 0..30 { s.step_bound(DT); }
        let o = |s: &mut Session, f: &str| match s.vm.get_prop(s.player, f, 0) { Ok(Value::Obj(o)) => o, _ => 0 };
        let (dl, cp) = (o(&mut s, "dataLinkPlay"), o(&mut s, "conPlay"));
        if sec % 5 == 0 || dl != 0 {
            eprintln!("{sec}s dataLinkPlay {dl} conPlay {cp} goals {} state {:?}", o(&mut s, "FirstGoal"), s.vm.state_name(s.player));
        }
    }
    let mut kinds = std::collections::BTreeMap::new();
    for i in &s.issues { *kinds.entry(format!("{} {}: {}", i.class, i.event, i.error)).or_insert(0) += 1; }
    for (k, n) in kinds { eprintln!("issue x{n} {k}"); }
    let mut logs = std::collections::BTreeMap::new();
    for l in &s.vm.log { *logs.entry(l.clone()).or_insert(0) += 1; }
    for (k, n) in logs { eprintln!("log x{n} {k}"); }
}

fn list_datalinks(s: &mut Session) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut li = match s.vm.get_prop(s.player, "ConListItems", 0) { Ok(Value::Obj(h)) => h, _ => 0 };
    while li != 0 {
        if let Ok(Value::Obj(con)) = s.vm.get_prop(li, "con", 0) {
            if matches!(s.vm.get_prop(con, "bDataLinkCon", 0), Ok(Value::Bool(true))) {
                let name = match s.vm.get_prop(con, "conName", 0) { Ok(Value::Name(n)) => s.vm.reg.names.str(n).to_string(), _ => String::new() };
                let flags = matches!(s.vm.get_prop(con, "flagRefList", 0), Ok(Value::Obj(f)) if f != 0);
                out.push((name, flags));
            }
        }
        li = match s.vm.get_prop(li, "next", 0) { Ok(Value::Obj(x)) => x, _ => 0 };
    }
    out
}

#[test]
#[ignore]
fn survey_datalinks() {
    let Some(mut s) = new_game() else { return };
    for (n, f) in list_datalinks(&mut s) { eprintln!("DL {n} flagRefs {f}"); }
}

#[test]
#[ignore]
fn shots_of_a_datalink() {
    let Some(mut s) = new_game() else { return };
    for _ in 0..30 { s.step_bound(DT); }
    s.issues.clear();
    let r = s.vm.call_by_name(s.player, "StartDataLinkTransmission", vec![Value::Str("DL_StartGame".into()), Value::Obj(0)]);
    eprintln!("start: {r:?}");
    let mut rd = None;
    for i in 0..6 {
        for _ in 0..90 { s.step_bound(DT); }
        let dl = match s.vm.get_prop(s.player, "dataLinkPlay", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
        eprintln!("t+{}s dataLinkPlay {dl}", i * 3 + 3);
        shot(&mut s, &mut rd, &format!("s11_dl_{i}.png"));
    }
    for i in s.issues.iter().filter(|i| !i.error.contains("InStasis")).take(10) { eprintln!("issue {} {}: {}", i.class, i.event, i.error); }
    rd.take().unwrap().destroy();
}

/// A DataLink transmission (the infolink at the top of the HUD): it starts through the player, its voice line is played, the
/// typewriter display runs, and the transmission ends by itself.
#[test]
fn a_datalink_plays_and_ends() {
    let Some(mut s) = new_game() else { return };
    for _ in 0..30 {
        s.step_bound(DT);
    }
    let before = s.vm.ai.sounds.len();
    let r = s.vm.call_by_name(s.player, "StartDataLinkTransmission", vec![Value::Str("DL_StartGame".into()), Value::Obj(0)]);
    assert!(matches!(r, Ok(Value::Bool(true))), "the transmission did not start: {r:?}");
    for _ in 0..90 {
        s.step_bound(DT);
    }
    let playing = |s: &mut Session| matches!(s.vm.get_prop(s.player, "dataLinkPlay", 0), Ok(Value::Obj(o)) if o != 0);
    assert!(playing(&mut s));
    let voiced = s.vm.ai.sounds[before..].iter().any(|r| s.vm.reg.obj_path(ue_registry::ObjId(r.sound)).contains("ConAudio"));
    assert!(voiced, "no voice line was played");
    // the typewriter text is on its way: some computer window holds display cells
    assert!(s.vm.ui_computer_cells() > 0, "nothing is typed on the infolink");
    for _ in 0..30 * 40 {
        s.step_bound(DT);
        if !playing(&mut s) {
            return;
        }
    }
    panic!("the transmission never ended");
}

#[test]
#[ignore]
fn survey_datalink_triggers() {
    let Some(mut s) = new_game() else { return };
    let me = s.player_location();
    eprintln!("player {me:?}");
    let c = s.vm.reg.class_by_name("DataLinkTrigger").unwrap();
    for a in s.vm.live_actors() {
        if s.vm.class_of(a) != Some(c) { continue; }
        let tag = match s.vm.get_prop(a, "datalinkTag", 0) { Ok(Value::Name(n)) => s.vm.reg.names.str(n).to_string(), o => format!("{o:?}") };
        let r = |s: &mut Session, f: &str| s.vm.get_prop(a, f, 0).ok();
        eprintln!("DLT {tag} at {:?} radius {:?} height {:?} collide {:?} tag {:?} event {:?} bTriggerOnce {:?}", s.vm.location(a), r(&mut s, "CollisionRadius"), r(&mut s, "CollisionHeight"), r(&mut s, "bCollideActors"), r(&mut s, "Tag"), r(&mut s, "Event"), r(&mut s, "bTriggerOnce"));
    }
}

/// Walking into the start-of-game trigger volume makes the first DataLink play, as in the original game.
#[test]
fn touching_a_datalink_trigger_starts_the_transmission() {
    let Some(mut s) = new_game() else { return };
    for _ in 0..30 {
        s.step_bound(DT);
    }
    let c = s.vm.reg.class_by_name("DataLinkTrigger").unwrap();
    let trigger = s.vm.live_actors().into_iter().find(|&a| s.vm.class_of(a) == Some(c) && matches!(s.vm.get_prop(a, "datalinkTag", 0), Ok(Value::Name(n)) if s.vm.reg.names.str(n) == "DL_StartGame")).unwrap();
    let t = s.vm.location(trigger);
    let playing = |s: &mut Session| matches!(s.vm.get_prop(s.player, "dataLinkPlay", 0), Ok(Value::Obj(o)) if o != 0);
    assert!(!playing(&mut s));
    // walk in from 450 units away
    s.teleport([t[0] - 450.0, t[1], t[2] + 30.0], 0.0);
    for _ in 0..4 {
        s.step_bound(DT);
    }
    s.input.key(dx_input::key_code("W").unwrap(), true);
    for _ in 0..150 {
        s.step_bound(DT);
        if playing(&mut s) {
            return;
        }
    }
    panic!("the DataLink did not start; player at {:?}, trigger at {t:?}", s.player_location());
}

#[test]
#[ignore]
fn survey_player_flags() {
    let Some(s) = new_game() else { return };
    for f in ["AugmentationSystem", "SkillSystem", "conPlay", "flagBase", "bCollideActors", "bBlockActors", "bBlockPlayers", "bCollideWorld", "bProjTarget", "CollisionRadius", "CollisionHeight"] {
        eprintln!("player {f}: {:?}", s.vm.get_prop(s.player, f, 0));
    }
}

/// After New Game the player has live augmentation and skill systems (a `None` carried over by level travel used to wipe the
/// augmentation system the new pawn made for itself, and every move then logged "Accessed None").
#[test]
fn the_new_game_player_has_its_subsystems() {
    let Some(mut s) = new_game() else { return };
    for f in ["AugmentationSystem", "SkillSystem", "flagBase"] {
        let live = matches!(s.vm.get_prop(s.player, f, 0), Ok(Value::Obj(o)) if s.vm.is_live(o));
        assert!(live, "{f} is not live");
    }
    s.vm.log.clear();
    for _ in 0..30 {
        s.step_bound(DT);
    }
    assert!(!s.vm.log.iter().any(|l| l.starts_with("Accessed None")), "{:?}", s.vm.log.iter().take(3).collect::<Vec<_>>());
}

#[test]
#[ignore]
fn survey_transient_props() {
    let Some(s) = new_game() else { return };
    let c = s.vm.reg.class_by_name("JCDentonMale").unwrap();
    for p in s.vm.reg.all_props(c) {
        if p.flags & 0x2000 != 0 {
            eprintln!("transient {} kind {:?} flags {:#x}", s.vm.reg.names.str(p.name), p.kind, p.flags);
        }
    }
}

/// Walking up to Paul with the movement keys and looking at him: what the game picks as the frob target on the way.
#[test]
#[ignore]
fn probe_frob_target_while_approaching_paul() {
    let Some(mut s) = new_game() else { return };
    let paul = actor_of(&mut s, "PaulDenton");
    stand_before(&mut s, paul, 300.0);
    for pitch_deg in [0.0f32, -10.0, -20.0] {
        let p = s.player_location();
        let t = s.vm.location(paul);
        let yaw = (t[1] - p[1]).atan2(t[0] - p[0]).to_degrees();
        s.teleport(p, yaw);
        let pitch = (pitch_deg / 360.0 * 65536.0) as i32;
        let r = match s.vm.get_prop(s.player, "ViewRotation", 0) { Ok(Value::Rotator(r)) => r, _ => [0; 3] };
        s.vm.set_prop(s.player, "ViewRotation", 0, Value::Rotator([pitch, r[1], 0])).unwrap();
        for _ in 0..10 {
            s.step_bound(DT);
        }
        let ft = match s.vm.get_prop(s.player, "FrobTarget", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
        let name = s.vm.class_of(ft).map(|c| s.vm.reg.class(c).path.clone()).unwrap_or_default();
        let d = { let a = s.player_location(); let b = s.vm.location(paul); ((a[0]-b[0]).powi(2)+(a[1]-b[1]).powi(2)).sqrt() };
        eprintln!("pitch {pitch_deg}: distance {d:.0}, frob target {name:?}, MaxFrobDistance {:?}, Paul radius {:?} height {:?}", s.vm.get_prop(s.player, "MaxFrobDistance", 0), s.vm.get_prop(paul, "CollisionRadius", 0), s.vm.get_prop(paul, "CollisionHeight", 0));
        // walk closer
        s.input.key(dx_input::key_code("W").unwrap(), true);
        for _ in 0..40 {
            s.step_bound(DT);
            let ft = match s.vm.get_prop(s.player, "FrobTarget", 0) { Ok(Value::Obj(o)) => o, _ => 0 };
            let a = s.player_location();
            let fname = s.vm.class_of(ft).map(|c| s.vm.reg.class(c).path.clone()).unwrap_or_default();
            eprintln!("   d {:.0} target {fname:?} vr {:?}", ((a[0]-t[0]).powi(2)+(a[1]-t[1]).powi(2)).sqrt(), s.vm.get_prop(s.player, "ViewRotation", 0));
            if ft == paul {
                eprintln!("   Paul becomes the target at distance {:.0}", ((a[0]-t[0]).powi(2)+(a[1]-t[1]).powi(2)).sqrt());
                break;
            }
        }
        s.input.key(dx_input::key_code("W").unwrap(), false);
        stand_before(&mut s, paul, 300.0);
    }
}

#[test]
#[ignore]
fn print_a_spot_before_paul() {
    let Some(mut s) = new_game() else { return };
    let paul = actor_of(&mut s, "PaulDenton");
    stand_before(&mut s, paul, 70.0);
    eprintln!("SPOT {:?} yaw {} paul {:?}", s.player_location(), s.yaw as f32 * 360.0 / 65536.0, s.vm.location(paul));
}

/// Paul's first line is heard: the voice (an MP3 of a `ConAudio` package) reaches the mixer and makes sound.
#[test]
fn pauls_voice_is_heard() {
    let Some(mut s) = new_game() else { return };
    let mixer = std::sync::Arc::new(std::sync::Mutex::new(dx_audio::Mixer::new(44100)));
    s.attach_audio(mixer.clone());
    mixer.lock().unwrap().stop_music();
    // full volumes, whatever the user's ini says (Galaxy squares the effects master)
    mixer.lock().unwrap().sound_volume = 255;
    mixer.lock().unwrap().speech_volume = 255;
    let paul = actor_of(&mut s, "PaulDenton");
    let started = mixer.lock().unwrap().started;
    s.vm.ai.sounds.clear();
    // walking up to Paul starts his radius conversation by itself once he has been seen (LastRendered); the use key
    // starts it otherwise
    stand_before(&mut s, paul, 60.0);
    for _ in 0..10 {
        s.step_bound(DT);
    }
    s.input.key(dx_input::key_code("F").unwrap(), true);
    s.step_bound(DT);
    s.input.key(dx_input::key_code("F").unwrap(), false);
    let mut voice_paths = Vec::new();
    let mut loud = 0.0f32;
    for _ in 0..60 {
        s.step_bound(DT);
        for r in &s.vm.ai.sounds {
            let p = s.vm.reg.obj_path(ue_registry::ObjId(r.sound)).to_string();
            if p.to_ascii_lowercase().contains("conaudio") && !voice_paths.contains(&p) {
                // the voice must be requested audibly (an omitted volume once arrived as 0)
                assert!(r.volume > 0.0, "voice {p} requested at volume {}", r.volume);
                voice_paths.push(p);
            }
        }
        let mut buf = vec![0.0f32; 2 * 1470];
        mixer.lock().unwrap().mix(&mut buf);
        loud = loud.max((buf.iter().map(|x| x * x).sum::<f32>() / buf.len() as f32).sqrt());
    }
    let after = mixer.lock().unwrap().started;
    eprintln!("voice lines {voice_paths:?}; mixer started {started} -> {after}; loudest rms {loud:.4}");
    assert!(!voice_paths.is_empty(), "no voice line was requested");
    assert!(after > started, "nothing reached the mixer");
    assert!(loud > 0.01, "the voice is silent");
}

/// A trooper's idle bark (as `ScriptedPawn.PlayIdleSound` asks for it) is spoken when the player is near.
#[test]
fn a_trooper_barks_when_the_player_is_near() {
    let Some(mut s) = new_game() else { return };
    let trooper = actor_of(&mut s, "UNATCOTroop");
    stand_before(&mut s, trooper, 100.0);
    s.step_bound(DT);
    let player = s.player;
    s.vm.ai.sounds.clear();
    // BM_Idle = 0
    let r = s.vm.call_by_name(player, "StartAIBarkConversation", vec![Value::Obj(trooper), Value::Byte(0)]).unwrap();
    for _ in 0..30 {
        s.step_bound(DT);
    }
    let voices: Vec<String> = s.vm.ai.sounds.iter().map(|r| s.vm.reg.obj_path(ue_registry::ObjId(r.sound)).to_string()).filter(|p| p.contains("ConAudioAIBarks")).collect();
    eprintln!("bark started {r:?}, voices {voices:?}");
    assert_eq!(r, Value::Bool(true));
    assert!(!voices.is_empty(), "no bark voice");
}

/// `GetSoundLength` estimates a voice line's length the engine's way (file size over the first frame's bit rate); for the
/// constant-bit-rate MP3s of the game that agrees with counting the frames.
#[test]
fn speech_length_is_the_engines_estimate() {
    let Some(mut s) = new_game() else { return };
    frob_paul(&mut s);
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..30 * 30 {
        s.step_bound(DT);
        for r in &s.vm.ai.sounds {
            if s.vm.reg.obj_path(ue_registry::ObjId(r.sound)).contains("ConAudio") {
                seen.insert(r.sound);
            }
        }
    }
    assert!(seen.len() >= 2, "too few voice lines played");
    for id in seen {
        let path = s.vm.reg.obj_path(ue_registry::ObjId(id)).to_string();
        let (pi, ei) = s.vm.reg.export_by_path(&path).unwrap();
        let file = ue_assets::audio::load_sound(&s.vm.reg.packages[pi].pkg, ei).unwrap();
        let m = ue_assets::audio::mp3_info(&file.data).unwrap();
        let est = s.vm.sound_length(id);
        eprintln!("{path}: engine {est:.3} s, frames {:.3} s", m.duration_secs);
        assert!((est - m.duration_secs).abs() < 0.1 + 0.05 * m.duration_secs, "{path}: {est} vs {}", m.duration_secs);
    }
}

/// Lip sync: while Paul's voice plays, the ported phoneme detector writes changing mouth letters into his
/// `nextPhoneme`, and `LipSynch` puts the matching mouth animations into blend channel 0.
#[test]
fn pauls_mouth_follows_his_voice() {
    let Some(mut s) = new_game() else { return };
    let mixer = std::sync::Arc::new(std::sync::Mutex::new(dx_audio::Mixer::new(44100)));
    s.attach_audio(mixer.clone());
    mixer.lock().unwrap().stop_music();
    s.audio.as_mut().unwrap().fixed_dt = Some(DT as f64);
    let paul = frob_paul(&mut s);
    let (mut letters, mut mouths) = (std::collections::BTreeMap::new(), std::collections::BTreeSet::new());
    let mut buf = vec![0.0f32; 2 * 1470];
    for _ in 0..240 {
        s.step_bound(DT);
        mixer.lock().unwrap().mix(&mut buf);
        if let Ok(Value::Str(p)) = s.vm.get_prop(paul, "nextPhoneme", 0) {
            *letters.entry(p).or_insert(0) += 1;
        }
        if let Ok(Value::Name(n)) = s.vm.get_prop(paul, "BlendAnimSequence", 0) {
            let n = s.vm.reg.names.str(n).to_string();
            if n.starts_with("Mouth") {
                mouths.insert(n);
            }
        }
    }
    eprintln!("letters {letters:?}; mouth animations {mouths:?}");
    let spoken = letters.keys().filter(|l| l.as_str() != "X" && !l.is_empty()).count();
    assert!(spoken >= 2, "too few mouth letters: {letters:?}");
    assert!(mouths.len() >= 3, "too few mouth animations: {mouths:?}");
}

/// Blend channels reach the renderer: while Paul talks, his mesh draw carries blend poses (blinking in channel 1,
/// lip sync in channel 0 when audio runs) that change over time.
#[test]
fn a_talking_pawn_draws_blend_poses() {
    let Some(mut s) = new_game() else { return };
    let paul = frob_paul(&mut s);
    let mut seen = std::collections::HashSet::new();
    for _ in 0..300 {
        s.step_bound(DT);
        if let Some(d) = s.vm.mesh_draw(paul) {
            if !d.blends.is_empty() {
                seen.insert(format!("{:?}", d.blends));
            }
        }
    }
    let ch = |i: u32| s.vm.get_prop(paul, "BlendAnimSequence", i as _).unwrap();
    eprintln!("{} different blend poses; channels {:?} {:?}", seen.len(), ch(0), ch(1));
    assert!(seen.len() >= 2, "no changing blend poses while talking");
}

/// Main-sequence tweens start from the pose last drawn (`ULodMesh::GetFrame`'s cache): Paul tweening into another
/// sequence is drawn with his previous frames first, then more and more of the new sequence's first frame.
#[test]
fn a_tween_starts_from_the_drawn_pose() {
    let Some(mut s) = new_game() else { return };
    let paul = actor_of(&mut s, "PaulDenton");
    for _ in 0..5 {
        s.step_bound(DT);
    }
    let before = s.vm.mesh_draw(paul).unwrap();
    let old: Vec<usize> = if before.blend > 0.0 { vec![before.frame_a, before.frame_b] } else { vec![before.frame_a] };
    let target = s.vm.reg.names.intern("Shoot");
    s.vm.native_tween_anim(paul, target, 0.5);
    let mut new_weights = Vec::new();
    for _ in 0..20 {
        s.step_bound(DT);
        let d = s.vm.mesh_draw(paul).unwrap();
        // whole pose as weighted frames: the base frames plus the extra entries
        let mut w: std::collections::BTreeMap<usize, f32> = Default::default();
        *w.entry(d.frame_a).or_default() += 1.0 - d.blend;
        *w.entry(d.frame_b).or_default() += d.blend;
        // extra entries are `weight * (frame - frame 0)`
        for &(f, x) in &d.blends {
            *w.entry(f).or_default() += x;
            *w.entry(0).or_default() -= x;
        }
        w.retain(|_, x| x.abs() > 1e-4);
        new_weights.push(w);
    }
    eprintln!("old frames {old:?}; poses {:?}", new_weights.iter().step_by(4).collect::<Vec<_>>());
    // early in the tween the old frames still carry most of the weight, at the end they are gone
    let old_w = |w: &std::collections::BTreeMap<usize, f32>| old.iter().map(|f| w.get(f).copied().unwrap_or(0.0)).sum::<f32>();
    assert!(old_w(&new_weights[1]) > 0.5, "tween did not start from the drawn pose: {:?}", new_weights[1]);
    assert!(old_w(new_weights.last().unwrap()) < 0.05, "tween did not finish: {:?}", new_weights.last());
    let mid = old_w(&new_weights[7]);
    assert!(mid > 0.05 && mid < old_w(&new_weights[1]), "no gradual change: {mid}");
}

/// Occlusion as the Galaxy driver: an actor's sound heard through a wall fades by the real time passed (occlusion
/// +dt per frame up to 0.5, level `max(1 - 2 * occlusion, 0.33)`), and comes back the same way in the open.
#[test]
fn sounds_behind_walls_are_occluded() {
    let Some(mut s) = new_game() else { return };
    let mixer = std::sync::Arc::new(std::sync::Mutex::new(dx_audio::Mixer::new(44100)));
    s.attach_audio(mixer.clone());
    mixer.lock().unwrap().stop_music();
    s.audio.as_mut().unwrap().fixed_dt = Some(DT as f64);
    let eye = s.vm.get_prop(s.player, "EyeHeight", 0).unwrap().as_f32().unwrap();
    let clear = |s: &Session, a: [f32; 3], p: [f32; 3]| s.vm.bsp.as_ref().unwrap().fast_line_check(a, [p[0], p[1], p[2] + eye]);
    // an NPC with a spot near it from which a wall hides it, and a spot in the open (the player must land there)
    let sp = s.vm.reg.class_by_name("ScriptedPawn").unwrap();
    let npcs: Vec<u32> = s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).map(|c| s.vm.is_a(c, sp)).unwrap_or(false)).collect();
    let lands = |s: &mut Session, p: [f32; 3]| {
        s.teleport(p, 0.0);
        let l = s.player_location();
        ((l[0] - p[0]).powi(2) + (l[1] - p[1]).powi(2)).sqrt() < 2.0
    };
    let mut found = None;
    'search: for &npc in &npcs {
        let t = s.vm.location(npc);
        let (mut hidden, mut open) = (None, None);
        for r in [200.0f32, 350.0, 500.0, 700.0] {
            for k in 0..24 {
                let a = k as f32 * std::f32::consts::TAU / 24.0;
                let p = [t[0] + r * a.cos(), t[1] + r * a.sin(), t[2]];
                let c = clear(&s, t, p);
                if (c && open.is_some()) || (!c && hidden.is_some()) || !lands(&mut s, p) {
                    continue;
                }
                let at = s.player_location();
                if clear(&s, t, at) { open = Some(at) } else { hidden = Some(at) }
                if let (Some(h), Some(o)) = (hidden, open) {
                    found = Some((npc, h, o));
                    break 'search;
                }
            }
        }
    }
    let (paul, hidden, open) = found.expect("no NPC with a hidden and an open spot");
    let sound = match s.vm.get_prop(paul, "HitSound1", 0) { Ok(Value::Obj(o)) if o != 0 => o, other => panic!("no sound: {other:?}") };
    s.vm.set_prop(paul, "AmbientSound", 0, Value::Obj(sound)).unwrap();
    s.vm.set_prop(paul, "SoundRadius", 0, Value::Byte(255)).unwrap();
    let t = s.vm.location(paul);
    let bsp_ok = |s: &Session, p: [f32; 3]| clear(s, t, p);
    let occ = |s: &mut Session| {
        s.audio.as_ref().unwrap().ambient_occlusion(paul).expect("Paul's ambient sound is not playing")
    };
    s.teleport(open, 0.0);
    for _ in 0..12 {
        s.step_bound(DT);
    }
    let start = occ(&mut s);
    s.teleport(hidden, 0.0);
    let mut seen = Vec::new();
    for _ in 0..20 {
        s.step_bound(DT);
        let here = s.player_location();
        if !bsp_ok(&s, here) {
            seen.push(occ(&mut s));
        }
    }
    let hidden_level = occ(&mut s);
    s.teleport(open, 0.0);
    for _ in 0..20 {
        s.step_bound(DT);
    }
    let back = occ(&mut s);
    eprintln!("open {start}, behind the wall {seen:?}, after 20 frames {hidden_level}, back in the open {back}");
    assert_eq!(start, 1.0);
    // one frame (1/30 s) blocked: 1 - 2/30
    assert!((seen[0] - (1.0 - 2.0 / 30.0)).abs() < 1e-3, "first blocked frame {}", seen[0]);
    assert!((hidden_level - 0.33).abs() < 1e-4, "the floor is 0.33: {hidden_level}");
    assert_eq!(back, 1.0);
}
