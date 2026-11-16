//! Every level transition of the game, as the maps declare them (`MapExit.DestMap`, `Teleporter.URL`): the
//! destination loads, the player arrives alive, no script error is logged, and where the destination leads back to a
//! map of the same mission, the level comes back as it was left (`DDeusExGameEngine::Browse`'s kept levels).
//! Writes `out/audit/transitions.tsv`. Long (every map, every exit):
//! `cargo test --release -p dx-play --test transitions -- --ignored --nocapture`. Set `DEUSEX_DIR`.

use dx_play::Session;
use std::collections::BTreeMap;
use std::path::PathBuf;
use ue_vm::travel::TravelRequest;
use ue_vm::Value;

const DT: f32 = 1.0 / 30.0;

fn run(s: &mut Session, frames: usize) {
    for _ in 0..frames {
        s.step_bound(DT);
    }
}

fn of_class(s: &mut Session, class: &str) -> Vec<u32> {
    let Some(c) = s.vm.reg.class_by_name(class) else { return Vec::new() };
    s.vm.live_actors().into_iter().filter(|&a| s.vm.class_of(a).map(|k| s.vm.is_a(k, c)).unwrap_or(false)).collect()
}

fn text(s: &mut Session, a: u32, field: &str) -> String {
    match s.vm.get_prop(a, field, 0) {
        Ok(Value::Str(t)) => t,
        _ => String::new(),
    }
}

/// The map name of a travel URL (`Map#Portal?options`).
fn url_map(url: &str) -> String {
    let m = url.split(['#', '?']).next().unwrap_or("").trim();
    // `04_NYC_UNATCOIsland.dx`, `14_OceanLab_UC.dx #UC`: the extension and spaces are not part of the name
    let m = if m.len() > 3 && m[m.len() - 3..].eq_ignore_ascii_case(".dx") { &m[..m.len() - 3] } else { m };
    m.trim().to_string()
}

/// The exits of a map: (exit actor class, URL).
fn exits(s: &mut Session) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for a in of_class(s, "MapExit") {
        let u = text(s, a, "DestMap");
        if !u.trim().is_empty() {
            out.push(("MapExit".to_string(), u));
        }
    }
    for a in of_class(s, "Teleporter") {
        let u = text(s, a, "URL");
        // a teleporter without a map in its URL moves within the level
        if !url_map(&u).is_empty() {
            out.push(("Teleporter".to_string(), u));
        }
    }
    out.sort();
    out.dedup();
    out
}

/// New log lines that are script errors.
fn errors(s: &Session, from: usize) -> Vec<String> {
    s.vm.log[from.min(s.vm.log.len())..]
        .iter()
        .filter(|l| l.contains("rror") || l.contains("Accessed None") || l.contains("runaway") || l.contains("infinite"))
        .cloned()
        .collect()
}

fn alive(s: &mut Session) -> bool {
    let p = s.player;
    p != 0 && s.vm.get_prop(p, "Health", 0).ok().and_then(|v| v.as_i32()).unwrap_or(0) > 0
}

/// Pawns standing (alive) other than the player and critters, by class, for comparing a level left and entered again.
fn pawns(s: &mut Session) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    // critters (rats, flies, pigeons) come and go from their generators
    let animal = s.vm.reg.class_by_name("Animal");
    for a in of_class(s, "ScriptedPawn") {
        if animal.is_some_and(|c| s.vm.class_of(a).map(|k| s.vm.is_a(k, c)).unwrap_or(false)) {
            continue;
        }
        if s.vm.get_prop(a, "Health", 0).ok().and_then(|v| v.as_i32()).unwrap_or(0) > 0 {
            let c = s.vm.class_of(a).map(|c| s.vm.reg.class(c).path.clone()).unwrap_or_default();
            *m.entry(c).or_insert(0) += 1;
        }
    }
    m
}

fn travel(s: &mut Session, url: &str) {
    s.vm.travel_request = Some(TravelRequest { url: url.to_string(), items: true });
    run(s, 2);
}

#[test]
#[ignore]
fn every_transition() {
    let Some(game) = std::env::var_os("DEUSEX_DIR").map(PathBuf::from) else { return };
    let only = std::env::var("ONLY").ok();
    let mut maps: Vec<String> = std::fs::read_dir(game.join("Maps"))
        .unwrap()
        .flatten()
        .filter_map(|e| e.path().file_stem().and_then(|s| s.to_str()).map(|s| s.to_string()))
        .filter(|m| e_ext(&game, m))
        .collect();
    maps.sort();
    // 1. the graph
    let mut graph: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for m in &maps {
        if only.as_ref().is_some_and(|o| !m.to_lowercase().starts_with(&o.to_lowercase())) {
            continue;
        }
        let Ok(mut s) = Session::open(&game, m) else { continue };
        graph.insert(m.clone(), exits(&mut s));
    }
    let lower: BTreeMap<String, String> = maps.iter().map(|m| (m.to_lowercase(), m.clone())).collect();
    // 2. every edge, and the way back where there is one
    let mut rows = vec!["from\texit\turl\tarrived\talive\terrors\tback\tback_restored\tpawns_kept\tnote".to_string()];
    let (mut bad, mut edges) = (0, 0);
    for (from, ex) in &graph {
        for (kind, url) in ex {
            edges += 1;
            let dest = url_map(url);
            let Some(dest_name) = lower.get(&dest.to_lowercase()).cloned() else {
                rows.push(format!("{from}\t{kind}\t{url}\t-\t-\t-\t-\t-\t-\tno such map"));
                continue;
            };
            let mut s = Session::open(&game, from).unwrap();
            run(&mut s, 5);
            let before = pawns(&mut s);
            let log0 = s.vm.log.len();
            travel(&mut s, url);
            let arrived = s.map_name.eq_ignore_ascii_case(&dest_name);
            run(&mut s, 30);
            let ok_alive = alive(&mut s);
            let mut errs = errors(&s, log0);
            // the way back: an exit of the destination to the map left
            let back = graph.get(&dest_name).and_then(|e| e.iter().find(|(_, u)| url_map(u).eq_ignore_ascii_case(from)).cloned());
            let (mut back_ok, mut restored, mut kept, mut note) = (String::from("-"), String::from("-"), String::from("-"), String::new());
            if let (true, Some((_, back_url))) = (arrived, back) {
                let kept_level = s.current_levels.contains_key(&from.to_lowercase());
                let log1 = s.vm.log.len();
                travel(&mut s, &back_url);
                back_ok = s.map_name.eq_ignore_ascii_case(from).to_string();
                run(&mut s, 30);
                restored = kept_level.to_string();
                let after = pawns(&mut s);
                kept = (after == before).to_string();
                if after != before {
                    let diff: Vec<String> = before
                        .keys()
                        .chain(after.keys())
                        .collect::<std::collections::BTreeSet<_>>()
                        .into_iter()
                        .filter(|k| before.get(*k) != after.get(*k))
                        .map(|k| format!("{} {}->{}", k.rsplit('.').next().unwrap_or(k), before.get(k).unwrap_or(&0), after.get(k).unwrap_or(&0)))
                        .collect();
                    note = diff.join(", ");
                }
                errs.extend(errors(&s, log1));
            }
            if std::env::var_os("DEBUG").is_some() && !errs.is_empty() {
                for (i, l) in s.vm.log[log0.min(s.vm.log.len())..].iter().enumerate().take(12) { println!("  {from} -> {url}: [{i}] {l}"); }
            }
            let failed = !arrived || !ok_alive || !errs.is_empty() || back_ok == "false";
            if failed {
                bad += 1;
            }
            if !errs.is_empty() {
                note = format!("{note} {}", errs.iter().take(2).cloned().collect::<Vec<_>>().join(" | "));
            }
            rows.push(format!("{from}\t{kind}\t{url}\t{arrived}\t{ok_alive}\t{}\t{back_ok}\t{restored}\t{kept}\t{}", errs.len(), note.trim()));
        }
    }
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../audit");
    let _ = std::fs::create_dir_all(&dir);
    std::fs::write(dir.join("transitions.tsv"), rows.join("\n") + "\n").unwrap();
    println!("{edges} transitions, {bad} with a problem; table in {}", dir.join("transitions.tsv").display());
}

/// A map file (`.dx`) of the game.
fn e_ext(game: &std::path::Path, m: &str) -> bool {
    game.join("Maps").join(format!("{m}.dx")).exists()
}
