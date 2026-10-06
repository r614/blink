//! Playback semantics checks (part of `harness audio`): the full HIRC decode, container loops
//! and transitions rendered offline through `dishonored_audio::engine`, random avoid-repeat,
//! interactive music transitions, the original music manager on the map's tweaks, footstep
//! surfaces from the collision import's physical materials, the contact table and the
//! AnimNotify_AkEvent sounds of the player AnimSets.

use crate::report::Report;
use anyhow::Result;
use dishonored_audio::anim_notify::NotifyKind;
use dishonored_audio::contact::ContactSystem;
use dishonored_audio::engine::{Engine, silence_profile};
use dishonored_audio::hirc::Node;
use dishonored_audio::library::SoundLibrary;
use dishonored_audio::music_manager::{MusicManager, MusicNpc, MusicState, MusicTweaks};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::Arc;

const SEC: &str = "audio";
const RATE: u32 = 48000;

fn hirc_exact(rep: &mut Report, root: &Path) {
    let mut ok = 0usize;
    let mut bad = 0usize;
    let mut kinds: BTreeMap<u8, usize> = BTreeMap::new();
    for d in dishonored_audio::library::pck_dirs(root) {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pck")) {
                continue;
            }
            let Ok(pck) = dishonored_audio::pck::Pck::open(&p) else { continue };
            for be in &pck.banks {
                let Ok(raw) = pck.read(be) else { continue };
                let Ok(b) = dishonored_audio::bank::Bank::parse(&raw) else { continue };
                for o in &b.hirc {
                    let (n, used) = dishonored_audio::hirc::decode_object(o);
                    if n.is_some() && used == o.body.len() {
                        ok += 1;
                        *kinds.entry(o.kind).or_default() += 1;
                    } else {
                        bad += 1;
                    }
                }
            }
        }
    }
    rep.check(SEC, "every HIRC object of every bank decodes to its exact length", bad == 0 && ok > 20000, format!("{ok} exact, {bad} inexact; by type {kinds:?}"));
}

fn engine(banks: &[&str], root: &Path) -> Result<Engine> {
    let mut lib = SoundLibrary::new(root)?;
    for b in banks {
        lib.load_bank(b)?;
    }
    let mut e = Engine::new(Arc::new(lib), RATE);
    e.log_posts = true;
    Ok(e)
}

/// Continuous containers: the Boyle street bed (random continuous, crossfade 3.5 s, infinite
/// loop) plays without gaps and crossfades into its next items.
fn loops(rep: &mut Report, root: &Path) -> Result<()> {
    let mut e = engine(&["Bank_Amb_LVL_Boyle_Estate"], root)?;
    e.post_event_name("Set_State_Amb_LVL_Boyle_Ext_Streets", 0);
    let pid = e.post_event_name("Play_Amb_LVL_Boyle_Estate", 0);
    let out = e.render_seconds(200.0);
    let (loud, gap) = silence_profile(&out, RATE, 1e-4);
    let xf = e.stats.transitions[1] + e.stats.transitions[2];
    rep.check(
        SEC,
        "continuous random container (Boyle street bed) loops without gaps through crossfades",
        loud >= 198.0 && gap < 0.2 && xf >= 2 && e.is_playing(pid),
        format!("{loud:.1}/200 s audible, longest gap {gap:.2} s, {xf} crossfaded transitions, still playing {}", e.is_playing(pid)),
    );
    // Stop with the event's fade: silent afterwards.
    e.stop_playing(pid, 500.0);
    let tail = e.render_seconds(2.0);
    let (_, gap) = silence_profile(&tail[RATE as usize * 2..], RATE, 1e-4);
    rep.check(SEC, "stopping a looping container fades it out", gap >= 0.9 && !e.is_playing(pid), format!("silent {gap:.2} s of the last second after a 0.5 s fade"));

    // Sound-level infinite loop (Loop = 0) on a placed emitter event, e.g. fountain water.
    let mut lib = SoundLibrary::new(root)?;
    lib.load_bank("Bank_Amb_LVL_Boyle_Estate")?;
    let mut looping = Vec::new();
    for b in &lib.banks {
        for (id, n) in &b.hier.nodes {
            if let Node::Sound { base, .. } = n {
                if base.prop(dishonored_audio::hirc::PROP_LOOP) == Some(0.0) {
                    looping.push(*id);
                }
            }
        }
    }
    rep.check(SEC, "Loop property (id 6) marks infinite loops on ambience sounds", !looping.is_empty(), format!("{} looping sounds in Bank_Amb_LVL_Boyle_Estate", looping.len()));
    Ok(())
}

/// Random step container with avoid-repeat: consecutive posts never repeat the last pick.
fn avoid_repeat(rep: &mut Report, root: &Path) -> Result<()> {
    let mut e = engine(&["Bank_Footsteps"], root)?;
    let ev = "FS_P_Stone_R";
    let mut picks = Vec::new();
    for _ in 0..40 {
        let pid = e.post_event_name(ev, 0);
        let s: Vec<u32> = e.playing_sounds().iter().filter(|x| x.0 == pid).map(|x| x.1).collect();
        let _ = e.render_seconds(0.02);
        let s2: Vec<u32> = e.playing_sounds().iter().filter(|x| x.0 == pid).map(|x| x.1).collect();
        if let Some(x) = s.first().or(s2.first()) {
            picks.push(*x);
        }
        let _ = e.render_seconds(0.3);
    }
    let repeats = picks.windows(2).filter(|w| w[0] == w[1]).count();
    let distinct: HashSet<u32> = picks.iter().copied().collect();
    rep.check(SEC, "random container avoid-repeat (FS_P_Stone_R, 40 posts)", picks.len() >= 35 && repeats == 0 && distinct.len() >= 3, format!("{} picks, {} distinct, {repeats} immediate repeats", picks.len(), distinct.len()));
    Ok(())
}

/// Interactive music: Boyle exploration, then a fight state, then back. The switch follows
/// the state through its transition rules and the segments come from the new playlist.
fn music(rep: &mut Report, root: &Path) -> Result<()> {
    let mut e = engine(&["Bank_Music_Boyle"], root)?;
    e.set_state(156687913, 862528611);
    e.post_event_name("Music_State_Boyle_Explor", 0);
    e.post_event_name("Play_Music_Boyle", 0);
    let a = e.render_seconds(60.0);
    let seg_explor: Vec<u32> = e.music_history().iter().map(|h| h.1).collect();
    e.post_event_name("Music_State_Boyle_Fight_2", 0);
    let t_fight = e.clock;
    let b = e.render_seconds(60.0);
    let seg_fight: Vec<u32> = e.music_history().iter().filter(|h| h.0 >= t_fight).map(|h| h.1).collect();
    e.post_event_name("Music_State_Boyle_Explor", 0);
    let c = e.render_seconds(40.0);
    let (la, _) = silence_profile(&a, RATE, 1e-4);
    let (lb, gb) = silence_profile(&b, RATE, 1e-4);
    let (lc, _) = silence_profile(&c, RATE, 1e-4);
    let overlap = seg_fight.iter().filter(|s| seg_explor.contains(s)).count();
    rep.check(
        SEC,
        "music switch follows Music_State_Boyle_* through its transition rules",
        e.stats.music_transitions >= 2 && !seg_explor.is_empty() && !seg_fight.is_empty() && overlap == 0 && lb > 50.0,
        format!(
            "exploration segments {seg_explor:?} ({la:.0} s audible / 60), fight segments {seg_fight:?} ({lb:.0} s audible, longest gap {gb:.2} s), back to exploration ({lc:.0} s audible / 40); {} transitions",
            e.stats.music_transitions
        ),
    );
    Ok(())
}

/// The original music manager on the Boyle tweaks, driven by a scripted encounter.
fn manager(rep: &mut Report, content: &dishonored_assets::Content) {
    let game = content.package("DishonoredGame").ok();
    let ext = content.package("L_Boyle_Ext_P").ok();
    let tw = match (&game, &ext) {
        (Some(g), Some(x)) => MusicTweaks::for_packages(Some(g), &[x]),
        _ => None,
    };
    let Some(tw) = tw else {
        rep.check(SEC, "music tweaks of L_Boyle_Ext_P", false, "Twk_Boyle_MusicEvents not found");
        return;
    };
    rep.check(
        SEC,
        "music tweaks of L_Boyle_Ext_P (DisTweaks_MusicEvents)",
        tw.start_exploration.as_deref() == Some("Music_State_Boyle_Explor") && tw.suspense_range == 800.0,
        format!("{}: suspense {} uu, chase {} s / {:?} uu, intensity {:?}", tw.name, tw.suspense_range, tw.chase_time, tw.chase_range, tw.intensity),
    );
    let mut m = MusicManager::new(tw);
    let g = |x: f32, combat: bool| MusicNpc { pos: [x, 0.0, 0.0], alive: true, hostile: true, combat, music_type: 4 };
    let mut posts = Vec::new();
    let mut run = |m: &mut MusicManager, secs: f32, npcs: &[MusicNpc]| {
        for _ in 0..(secs * 10.0) as usize {
            posts.extend(m.tick(0.1, [0.0; 3], npcs));
        }
    };
    run(&mut m, 2.0, &[g(5000.0, false)]);
    run(&mut m, 2.0, &[g(600.0, false)]);
    run(&mut m, 2.0, &[g(600.0, true)]);
    run(&mut m, 2.0, &[g(600.0, true), g(700.0, true), g(650.0, true)]);
    run(&mut m, 5.0, &[g(600.0, false)]);
    run(&mut m, 5.0, &[g(3000.0, false)]);
    run(&mut m, 2.0, &[g(6000.0, false)]);
    let want = [
        "Music_State_Boyle_Explor",
        "Music_State_Boyle_Suspens",
        "Music_State_Boyle_Fight_1",
        "Music_State_Boyle_Fight_2",
        "Music_State_Boyle_Escape_High",
        "Music_State_Boyle_Escape_Low",
        "Music_State_Boyle_Explor",
    ];
    rep.check(SEC, "music manager: exploration -> suspense -> combat levels -> chase near/far -> exploration", posts == want, format!("{posts:?}; final state {:?}", m.state));
    let _ = MusicState::Exploration;
}

/// Footstep surfaces: the collision import carries each triangle's contact type, and the
/// contact system turns (footfall type, surface) into the original FS_ events.
fn surfaces(rep: &mut Report, root: &Path, content: &dishonored_assets::Content) -> Result<()> {
    let game = content.package("DishonoredGame")?;
    let gc = content.package("DishonoredGameContent").ok();
    let cs = ContactSystem::load(&game, &gc.iter().map(|p| &**p).collect::<Vec<_>>());
    let Some(cs) = cs else {
        rep.check(SEC, "contact system table", false, "Dis_ContactSystem not found");
        return Ok(());
    };
    let probe = [
        ("DisContactType_Footfall_Run_Player", "DisContactType_Env_Wood"),
        ("DisContactType_Footfall_Sneak_Player", "DisContactType_Env_Stone"),
        ("DisContactType_Footfall_Sprint_Player", "DisContactType_Env_Metal"),
        ("DisContactType_Footfall_Run", "DisContactType_Env_Gravel"),
    ];
    let rows: Vec<String> = probe.iter().map(|(s, t)| format!("{s} x {t} -> {}", cs.sound(s, t).map(|x| x.event).unwrap_or_else(|| "-".into()))).collect();
    let ok = probe.iter().all(|(s, t)| cs.sound(s, t).is_some_and(|x| x.event.starts_with("FS_")));
    rep.check(SEC, "contact system: footfall x surface -> original FS_ events", ok, format!("{} intersections; {}", cs.intersections.len(), rows.join("; ")));

    let mc = dishonored_content::load_match(dishonored_content::DEFAULT_MAP)?;
    let w = &mc.collision;
    let mut hist: BTreeMap<String, usize> = BTreeMap::new();
    for i in 0..w.tris.len() {
        *hist.entry(w.surface_of(i as u32).unwrap_or("(none)").to_string()).or_default() += 1;
    }
    let with = w.tris.len() - hist.get("(none)").copied().unwrap_or(0);
    let mut lib = SoundLibrary::new(root)?;
    lib.load_bank("Bank_Footsteps")?;
    let mut events = HashSet::new();
    for s in hist.keys() {
        let surface = if s == "(none)" { "DisContactType_Environment" } else { s.as_str() };
        if let Some(snd) = cs.sound("DisContactType_Footfall_Run_Player", surface) {
            if lib.resolve(&snd.event).is_some() {
                events.insert(snd.event);
            }
        }
    }
    let top: Vec<String> = {
        let mut v: Vec<(&String, &usize)> = hist.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1));
        v.iter().take(8).map(|(k, n)| format!("{k} {n}")).collect()
    };
    rep.check(
        SEC,
        "collision triangles carry their physical material's contact type (Boyle Ext)",
        with * 2 > w.tris.len() && events.len() >= 4,
        format!("{with}/{} triangles with a surface, {} surfaces ({}), {} distinct resolvable run footstep events: {:?}", w.tris.len(), hist.len(), top.join(", "), events.len(), events),
    );
    Ok(())
}

/// AnimNotify_AkEvent in the player AnimSets (Startup): timing and resolution.
fn notifies(rep: &mut Report, root: &Path, content: &dishonored_assets::Content) -> Result<()> {
    let startup = content.package("Startup")?;
    let n = dishonored_audio::anim_notify::read_package(&startup);
    let mut lib = SoundLibrary::new(root)?;
    for b in dishonored_audio::cues::PLAYER_BANKS {
        let _ = lib.load_bank(b);
    }
    let mut total = 0;
    let mut resolved = 0;
    let mut footfalls = 0;
    for c in n.values() {
        for x in &c.notifies {
            match &x.kind {
                NotifyKind::AkEvent { event, .. } => {
                    total += 1;
                    resolved += lib.has_event(event) as usize;
                }
                NotifyKind::Footfall { .. } => footfalls += 1,
            }
        }
    }
    let swing = n.get("Sword_Ready_AttackRight_A_Small");
    let ev = swing.and_then(|c| c.window(0.0, 0.2).into_iter().find_map(|x| match &x.kind {
        NotifyKind::AkEvent { event, .. } => Some((x.time, event.clone())),
        _ => None,
    }));
    rep.check(
        SEC,
        "AnimNotify_AkEvent sounds of the player AnimSets resolve (sword swing at its notify time)",
        total > 50 && resolved * 10 >= total * 8 && ev.is_some(),
        format!("{} clips, {total} AkEvent notifies ({resolved} in the player banks), {footfalls} footfall notifies; Sword_Ready_AttackRight_A_Small: {ev:?}", n.len()),
    );
    Ok(())
}

pub fn run(rep: &mut Report, root: &Path) -> Result<()> {
    let t0 = std::time::Instant::now();
    hirc_exact(rep, root);
    loops(rep, root)?;
    avoid_repeat(rep, root)?;
    music(rep, root)?;
    let content = dishonored_assets::Content::locate()?;
    manager(rep, &content);
    surfaces(rep, root, &content)?;
    notifies(rep, root, &content)?;
    rep.check(SEC, "playback checks finished", true, format!("{:.1}s", t0.elapsed().as_secs_f32()));
    Ok(())
}
