//! Reading texts in the game: datacubes, books and newspapers through `DeusExTextParser`. Set `DEUSEX_DIR` to the game folder.

use dx_play::Session;
use std::path::PathBuf;
use ue_vm::Value;

const DT: f32 = 1.0 / 30.0;

fn run(s: &mut Session, frames: usize) {
    for _ in 0..frames {
        s.step_bound(DT);
    }
}

fn new_game() -> Option<Session> {
    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR")?);
    let mut s = Session::open(&g, "DX").unwrap();
    s.exec("StartNewGame 01_NYC_UNATCOIsland");
    run(&mut s, 5);
    assert_eq!(s.map_name, "01_NYC_UNATCOIsland");
    Some(s)
}

fn obj(s: &mut Session, o: u32, name: &str) -> u32 {
    match s.vm.get_prop(o, name, 0) {
        Ok(Value::Obj(r)) => r,
        _ => 0,
    }
}

fn string(s: &mut Session, o: u32, name: &str) -> String {
    match s.vm.get_prop(o, name, 0) {
        Ok(Value::Str(t)) => t,
        _ => String::new(),
    }
}

fn class_name(s: &Session, o: u32) -> String {
    s.vm.class_of(o).map(|c| s.vm.reg.class(c).path.rsplit('.').next().unwrap_or("").to_string()).unwrap_or_default()
}

/// The texts of every `TextWindow` below `w`, in tree order.
fn texts_below(s: &mut Session, w: u32, out: &mut Vec<String>) {
    let mut c = obj(s, w, "firstChild");
    while c != 0 {
        if class_name(s, c) == "TextWindow" {
            out.push(string(s, c, "text"));
        }
        texts_below(s, c, out);
        c = obj(s, c, "nextSibling");
    }
}

/// The raw text of `DeusExText.<tag>` without tags, comments and line breaks: the words a reader must show.
fn plain_words(raw: &str) -> Vec<String> {
    let body = &raw[raw.find("<P>").map(|i| i + 3).unwrap_or(0)..];
    let mut out = String::new();
    let mut in_tag = false;
    for c in body.chars() {
        match c {
            '<' => {
                in_tag = true;
                out.push(' ');
            }
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().map(str::to_string).collect()
}

#[test]
fn a_datacube_shows_its_text_and_stores_a_note() {
    let Some(mut s) = new_game() else { return };
    let cube_class = s.vm.reg.class_by_name("DataCube").unwrap();
    let p = s.player_location();
    let mut cubes: Vec<(f32, u32)> = s
        .vm
        .live_actors()
        .into_iter()
        .filter(|&a| s.vm.class_of(a).map(|c| s.vm.is_a(c, cube_class)).unwrap_or(false))
        .collect::<Vec<_>>()
        .into_iter()
        .map(|a| {
            let l = s.vm.location(a);
            (((l[0] - p[0]).powi(2) + (l[1] - p[1]).powi(2)).sqrt(), a)
        })
        .collect();
    cubes.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let cube = cubes.first().expect("Liberty Island has datacubes").1;
    let tag = match s.vm.get_prop(cube, "textTag", 0) {
        Ok(Value::Name(n)) => s.vm.reg.names.str(n).to_string(),
        other => panic!("textTag {other:?}"),
    };
    let player = s.player;
    // read at once: the cube closes its window on the next tick unless the player keeps looking at it
    s.vm.call_by_name(cube, "Frob", vec![Value::Obj(player), Value::Obj(0)]).unwrap();

    let root = obj(&mut s, player, "rootWindow");
    let hud = obj(&mut s, root, "hud");
    let info = obj(&mut s, hud, "info");
    assert_ne!(info, 0, "the HUD has an information window");
    let mut shown = Vec::new();
    texts_below(&mut s, info, &mut shown);
    let shown_text = shown.join(" ");
    eprintln!("{tag}: {shown:?}");

    // the words of the datacube appear in order (paragraphs split into text windows)
    let raw = s.vm.ext_string_text(&format!("DeusExText.{tag}")).expect("text exists");
    let want = plain_words(&raw);
    let got: Vec<String> = shown_text.split_whitespace().map(str::to_string).collect();
    assert!(!want.is_empty());
    assert_eq!(got, want, "shown text differs from the datacube's text");

    // and the text went into the DataVault as a note
    let mut note = obj(&mut s, player, "FirstNote");
    let mut found = false;
    while note != 0 {
        if matches!(s.vm.get_prop(note, "textTag", 0), Ok(Value::Name(n)) if s.vm.reg.names.str(n).eq_ignore_ascii_case(&tag)) {
            let t = string(&mut s, note, "text");
            assert_eq!(t.split_whitespace().collect::<Vec<_>>(), want.iter().map(String::as_str).collect::<Vec<_>>());
            found = true;
        }
        note = obj(&mut s, note, "next");
    }
    assert!(found, "a note with the datacube's tag was added");
}

/// Every text of `DeusExText` parses to its end; the words outside tags come out as text blocks, in order.
#[test]
fn every_text_parses_to_its_end() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let pkg = ue_package::Package::open(g.join("System/DeusExText.u")).unwrap();
    let (mut n, mut tags) = (0, [0usize; 31]);
    for ei in 0..pkg.exports.len() {
        if pkg.export_class_name(ei) != "ExtString" {
            continue;
        }
        let raw = ue_assets::text::load_ext_string(&pkg, ei).unwrap();
        let mut p = ue_vm::text_parser::TextParser::default();
        p.set_player_name("JC Denton");
        assert!(p.open(Some(&raw)));
        let (mut words, mut steps) = (Vec::new(), 0);
        while p.process() {
            steps += 1;
            assert!(steps < 100_000, "text {ei} does not end");
            tags[p.last_tag as usize] += 1;
            if matches!(p.last_tag, 0 | 9 | 10) {
                words.extend(p.last_text.split_whitespace().map(str::to_string));
            }
        }
        // the plain words, with the player-name tags replaced and the comment/note/goal blocks removed
        let mut cleaned = raw.replace("<PLAYERNAME>", " JC Denton ").replace("<PLAYERFIRSTNAME>", " JC ");
        for (open, close) in [("<COMMENT", "</COMMENT>"), ("<NOTE", "</NOTE>"), ("<GOAL", "</GOAL>")] {
            while let Some(a) = cleaned.find(open) {
                let b = cleaned[a..].find(close).map(|b| a + b + close.len()).unwrap_or(cleaned.len());
                cleaned.replace_range(a..b, " ");
            }
        }
        let want = if cleaned.contains("<P>") { plain_words(&cleaned) } else { Vec::new() };
        assert_eq!(words, want, "text {} ({})", ei, pkg.name(pkg.exports[ei].object_name));
        n += 1;
    }
    eprintln!("{n} texts; tag counts {tags:?}");
    assert!(n > 400);
}

fn find_window(s: &mut Session, w: u32, class: &str) -> u32 {
    let mut c = obj(s, w, "firstChild");
    while c != 0 {
        if class_name(s, c) == class {
            return c;
        }
        let f = find_window(s, c, class);
        if f != 0 {
            return f;
        }
        c = obj(s, c, "nextSibling");
    }
    0
}

/// A personal computer's email screen lists the account's emails (from `<EMAIL=...>` tags) and shows the first one.
#[test]
fn a_computer_lists_and_shows_emails() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let mut s = Session::open(&g, "01_NYC_UNATCOHQ").unwrap();
    run(&mut s, 5);
    let c = s.vm.reg.class_by_name("ComputerPersonal").unwrap();
    let comps: Vec<u32> = s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).map(|k| s.vm.is_a(k, c)).unwrap_or(false)).collect();
    let player = s.player;
    let mut checked = 0;
    for comp in comps {
        let user = match s.vm.call_by_name(comp, "GetUserName", vec![Value::Int(0)]) {
            Ok(Value::Str(u)) if !u.is_empty() => u,
            _ => continue,
        };
        let menu = format!("DeusExText.01_EmailMenu_{user}");
        let Some(raw_menu) = s.vm.ext_string_text(&menu) else { continue };
        s.vm.call_by_name(player, "InvokeComputerScreen", vec![Value::Obj(comp), Value::Float(0.0), Value::Float(0.0)]).unwrap();
        let term = obj(&mut s, comp, "termwindow");
        assert_ne!(term, 0);
        s.vm.call_by_name(term, "SetLoginInfo", vec![Value::Str(user.clone()), Value::Int(0)]).unwrap();
        let cls = s.vm.reg.class_by_name("ComputerScreenEmail").unwrap();
        let path = s.vm.reg.class(cls).path.clone();
        let cls = Value::Obj(s.vm.reg.obj_id(&path).0);
        s.vm.call_by_name(term, "ShowScreen", vec![cls]).unwrap();
        let root = obj(&mut s, player, "rootWindow");
        let screen = find_window(&mut s, root, "ComputerScreenEmail");
        assert_ne!(screen, 0, "email screen open");
        // (`emailIndex` is reset when the first email's body is read, so count the list rows)
        let list = obj(&mut s, screen, "lstEmail");
        let n = s.vm.call_by_name(list, "GetNumRows", vec![]).ok().and_then(|v| v.as_i32()).unwrap_or(-1);
        let want = raw_menu.matches("<EMAIL=").count().min(10) as i32;
        assert_eq!(n, want, "{user}: emails listed");
        // the first email: its header fields and body text
        let first = raw_menu.split("<EMAIL=").nth(1).unwrap();
        let fields: Vec<&str> = first[..first.find('>').unwrap()].split(',').map(str::trim).collect();
        let win_from = obj(&mut s, screen, "winEmailFrom");
        let win_subject = obj(&mut s, screen, "winEmailSubject");
        assert_eq!(string(&mut s, win_from, "text"), fields[2]);
        assert_eq!(string(&mut s, win_subject, "text"), fields[1]);
        let win_email = obj(&mut s, screen, "winEmail");
        let body = string(&mut s, win_email, "text");
        // the body wraps at the width of its scroll area (the clip window forces its child's width)
        let clip = obj(&mut s, win_email, "winParent");
        s.ui_quads(800, 600);
        let (gw, gc) = (s.vm.ui_geometry(win_email), s.vm.ui_geometry(clip));
        assert_eq!(gw[2], gc[2], "email body {gw:?} in clip {gc:?}");
        let raw_body = s.vm.ext_string_text(&format!("DeusExText.{}", fields[0])).expect("email text");
        let want_words = plain_words(&raw_body);
        assert_eq!(body.split_whitespace().collect::<Vec<_>>(), want_words.iter().map(String::as_str).collect::<Vec<_>>());
        eprintln!("{user}: {n} emails; first from {} '{}': {} words", fields[2], fields[1], want_words.len());
        s.vm.call_by_name(term, "CloseScreen", vec![Value::Str("EXIT".into())]).ok();
        checked += 1;
        break;
    }
    assert!(checked > 0, "no personal computer with email in UNATCO HQ");
}

fn shot(s: &mut Session, r: &mut Option<dx_vk::renderer::Renderer>, name: &str) {
    let img = s.render_vk(r, 800, 600).expect("vulkan render");
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../out/screenshots");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), dx_soft::png(img.width, img.height, &img.rgba)).unwrap();
}

/// Pictures of a datacube's text on the HUD and of an email on a computer; needs a Vulkan device.
#[test]
#[ignore]
fn shots_of_reading() {
    let Some(mut s) = new_game() else { return };
    let cube_class = s.vm.reg.class_by_name("DataCube").unwrap();
    let cube = s.vm.live_actors().into_iter().find(|&a| s.vm.class_of(a).map(|c| s.vm.is_a(c, cube_class)).unwrap_or(false)).unwrap();
    let player = s.player;
    s.vm.call_by_name(cube, "Frob", vec![Value::Obj(player), Value::Obj(0)]).unwrap();
    let mut r = None;
    s.ui_quads(800, 600);
    shot(&mut s, &mut r, "s12_datacube.png");
    r.take().unwrap().destroy();

    let g = PathBuf::from(std::env::var_os("DEUSEX_DIR").unwrap());
    let mut s = Session::open(&g, "01_NYC_UNATCOHQ").unwrap();
    run(&mut s, 5);
    let c = s.vm.reg.class_by_name("ComputerPersonal").unwrap();
    let comp = s
        .vm
        .live_actors()
        .into_iter()
        .filter(|&a| s.vm.class_of(a).map(|k| s.vm.is_a(k, c)).unwrap_or(false))
        .collect::<Vec<_>>()
        .into_iter()
        .find(|&a| matches!(s.vm.call_by_name(a, "GetUserName", vec![Value::Int(0)]), Ok(Value::Str(u)) if u.eq_ignore_ascii_case("jmanderley")))
        .unwrap();
    let player = s.player;
    s.vm.call_by_name(player, "InvokeComputerScreen", vec![Value::Obj(comp), Value::Float(0.0), Value::Float(0.0)]).unwrap();
    let term = obj(&mut s, comp, "termwindow");
    s.vm.call_by_name(term, "SetLoginInfo", vec![Value::Str("JMANDERLEY".into()), Value::Int(0)]).unwrap();
    let cls = s.vm.reg.class_by_name("ComputerScreenEmail").unwrap();
    let path = s.vm.reg.class(cls).path.clone();
    let cls = Value::Obj(s.vm.reg.obj_id(&path).0);
    s.vm.call_by_name(term, "ShowScreen", vec![cls]).unwrap();
    run(&mut s, 3);
    let mut r = None;
    s.ui_quads(800, 600);
    shot(&mut s, &mut r, "s12_email.png");
    r.take().unwrap().destroy();
}

/// A static array argument is passed whole (Core copies the complete value): `GC.DrawBorders(..., texBorders)` gets
/// all nine textures, so the datacube's information window and the "Note Received" box draw their nine background
/// and nine border pieces (user report: text over the world with only a corner of metal; WORKLOG 3.187).
#[test]
fn the_datacube_window_draws_its_whole_background_and_border() {
    let Some(g) = std::env::var_os("DEUSEX_DIR").map(std::path::PathBuf::from) else { return };
    let mut s = dx_play::Session::open(&g, "00_Training").unwrap();
    for _ in 0..30 {
        s.step_bound(1.0 / 30.0);
    }
    let all: Vec<u32> = s.vm.live_actors().into_iter().collect();
    let cube = all.into_iter().find(|&a| s.vm.class_of(a).map(|c| s.vm.reg.names.str(s.vm.reg.class(c).name) == "DataCube").unwrap_or(false)).unwrap();
    let p = s.player;
    s.vm.send_event(cube, "Frob", vec![ue_vm::Value::Obj(p), ue_vm::Value::Obj(0)]).unwrap();
    for _ in 0..20 {
        s.step_bound(1.0 / 30.0);
    }
    let q = s.ui_quads(1000, 625);
    let count = |n: &str| q.iter().filter(|x| x.texture.contains(n)).count();
    // two windows (the information window and the note received box), nine pieces each
    assert!(count("HUDWindowBackground") >= 18, "background pieces {}", count("HUDWindowBackground"));
    assert!(count("HUDWindowBorder") >= 18, "border pieces {}", count("HUDWindowBorder"));
    for piece in ["_TL", "_TR", "_BL", "_BR", "_Left", "_Right", "_Top", "_Bottom", "_Center"] {
        assert!(q.iter().any(|x| x.texture.contains("HUDWindowBackground") && x.texture.ends_with(piece)), "no background {piece}");
    }
}
