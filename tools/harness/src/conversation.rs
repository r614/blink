//! `harness conversation`: the original Boyle conversations played headless.
//!
//! 1. Runner: every conversation of the key one-shots (doorman, Samuel, front-door guard,
//!    Lord Brisby, Miss White, the three sisters) is started from the dialog data read out of
//!    the install and stepped to completion, once per choice policy (always option 1, always
//!    option 2). Every line must carry subtitle text, and every line whose VO event exists in
//!    its `Bk_<GUID>.INT` bank must decode to non-silent audio; Kismet outputs and story flags
//!    are recorded and a few known ones are required.
//! 2. Mission: `missions/boyle_conversations.ron` is played on the authoritative match with a
//!    scripted agent (walk, F to talk, number keys for choices): the doorman's invitation
//!    check, Lydia's choices, Miss White's use hook, Brisby's approach and use hooks. The
//!    replicated mission view must carry the subtitles and the choice, the mission rules
//!    must see the outputs/flags, and two runs must be identical.
//!
//! Retail text never goes into the report; lines are counted, not quoted.

use crate::Ctx;
use crate::report::Report;
use anyhow::{Context, Result};
use dishonored_bots::NavGraph;
use dishonored_bots::nav::Follower;
use dishonored_content::MapContents;
use dishonored_conversation::model::{ConversationDb, NodeKind, Target};
use dishonored_conversation::runner::{ConvEvent, Runner, SimpleEnv, StartSpec};
use dishonored_mission::{MissionDef, TaskState};
use dishonored_multiplayer::game::Game;
use dishonored_sim::input::{LeftHand, PlayerInput, buttons};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

const S: &str = "Conversations";

const PACKAGES: &[&str] = &["L_BoyleStreet_Ext_Script", "L_Boyle_Ext_Script", "L_Boyle_Int_Script"];
const ONESHOTS: &[&str] = &[
    "L_BoyleStreet_Ext_Script:DisDialogOneShot_4",
    "L_BoyleStreet_Ext_Script:DisDialogOneShot_7",
    "L_Boyle_Ext_Script:DisDialogOneShot_5",
    "L_Boyle_Int_Script:DisDialogOneShot_6",
    "L_Boyle_Int_Script:DisDialogOneShot_9",
    "L_Boyle_Int_Script:DisDialogOneShot_10",
    "L_Boyle_Int_Script:DisDialogOneShot_11",
    "L_Boyle_Int_Script:DisDialogOneShot_12",
];

#[derive(Default, Debug)]
struct Played {
    lines: Vec<(Option<(String, String)>, bool)>,
    choices: usize,
    outputs: BTreeSet<String>,
    flags: BTreeSet<String>,
    soirees: usize,
    ended: bool,
    seconds: f32,
}

fn play(db: &Arc<ConversationDb>, target: Target, policy: usize, flags: &[&str]) -> Played {
    let mut env = SimpleEnv::default();
    for f in flags {
        env.flags.insert(f.to_string(), true);
    }
    let mut r = Runner::new(db.clone(), 7 + policy as u64);
    let mut p = Played::default();
    if r.start(StartSpec { target, initiator: Some(1), bindings: vec![], force: true }, &mut env).is_none() {
        return p;
    }
    let dt = 1.0 / 60.0;
    for _ in 0..(600 * 60) {
        for e in r.take_events() {
            match e {
                ConvEvent::Line { text, vo, .. } => p.lines.push((vo.map(|v| (v.event, v.bank)), !text.trim().is_empty())),
                ConvEvent::Choice { .. } => p.choices += 1,
                ConvEvent::Output { name, .. } => {
                    p.outputs.insert(name);
                }
                ConvEvent::Flag { name, value } => {
                    p.flags.insert(format!("{name}={value}"));
                }
                ConvEvent::Soiree { .. } => p.soirees += 1,
                ConvEvent::Ended { .. } => p.ended = true,
                _ => {}
            }
        }
        if r.active().next().is_none() {
            break;
        }
        let open = r.active().find_map(|i| i.choice.as_ref().map(|c| c.options.len()));
        if let Some(n) = open {
            r.choose(1, policy.min(n - 1), &mut env);
            continue;
        }
        r.update(dt, &mut env);
        p.seconds += dt;
    }
    p
}

pub fn run(ctx: &Ctx, rep: &mut Report) -> Result<()> {
    let content = dishonored_assets::Content::locate()?;
    let t = std::time::Instant::now();
    let pkgs: Vec<String> = PACKAGES.iter().map(|s| s.to_string()).collect();
    let db = Arc::new(dishonored_content::load_conversations(&content, &pkgs)?);
    let (lines, vo, len) = db.line_stats();
    rep.check(
        S,
        "dialog graphs load from the install",
        !db.conversations.is_empty(),
        format!("{} packages: {} one-shots, {} trees, {} conversations, {} nodes, {lines} lines ({vo} with a VO event, {len} with a decodable clip length) in {:.1?}", db.packages.len(), db.oneshots.len(), db.trees.len(), db.conversations.len(), db.nodes.len(), t.elapsed()),
    );
    let mut voice = dishonored_audio::vo::VoResolver::open(&content.root)?;
    let mut decoded: BTreeMap<(String, String), Option<(f32, f32)>> = BTreeMap::new();
    let mut rows = Vec::new();
    let (mut n_conv, mut n_lines, mut n_text, mut n_vo_ok, mut n_vo_missing, mut n_vo_bad, mut n_choices, mut n_outputs) = (0, 0, 0, 0, 0, 0, 0, 0);
    let mut missing_list = Vec::new();
    let mut all_outputs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut all_flags: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut not_ended = Vec::new();
    for os in ONESHOTS {
        let o = db.find_oneshot(os).map_err(anyhow::Error::msg)?;
        let Some(tree) = db.oneshots[o as usize].tree else { continue };
        for &c in &db.trees[tree as usize].conversations {
            let conv = &db.conversations[c as usize];
            let has_choice = db.nodes.iter().any(|n| n.conversation == Some(c) && matches!(n.kind, NodeKind::Choice { .. }));
            for policy in if has_choice { vec![0, 1] } else { vec![0] } {
                let p = play(&db, Target::Conversation { oneshot: Some(o), conversation: c }, policy, &[]);
                n_conv += 1;
                if !p.ended {
                    not_ended.push(db.conversation_name(c));
                }
                let mut vo_ok = 0;
                for (v, has_text) in &p.lines {
                    n_lines += 1;
                    n_text += *has_text as usize;
                    let Some(v) = v else {
                        n_vo_missing += 1;
                        continue;
                    };
                    let r = decoded.entry(v.clone()).or_insert_with(|| {
                        let clip = voice.resolve(&v.0, &v.1)?;
                        let pcm = voice.lib.decode(clip.leaf.media).ok()?;
                        Some((pcm.duration(), pcm.peak()))
                    });
                    match r {
                        Some((d, peak)) if *d > 0.05 && *peak > 1e-3 => vo_ok += 1,
                        Some(_) => n_vo_bad += 1,
                        None => {
                            // Event absent from its bank (cut line): timed from its text.
                            n_vo_missing += 1;
                            missing_list.push(format!("{} ({} in {})", db.conversation_name(c), v.0, v.1));
                        }
                    }
                }
                n_vo_ok += vo_ok;
                n_choices += p.choices;
                n_outputs += p.outputs.len();
                all_outputs.entry(conv.label.clone()).or_default().extend(p.outputs.iter().cloned());
                all_flags.entry(conv.label.clone()).or_default().extend(p.flags.iter().cloned());
                rows.push(vec![
                    db.conversation_name(c),
                    format!("{policy}"),
                    format!("{}", p.lines.len()),
                    format!("{vo_ok}"),
                    format!("{}", p.choices),
                    format!("{}", p.soirees),
                    p.outputs.iter().cloned().collect::<Vec<_>>().join(", "),
                    p.flags.iter().cloned().collect::<Vec<_>>().join(", "),
                    format!("{:.1}", p.seconds),
                ]);
            }
        }
    }
    missing_list.sort();
    missing_list.dedup();
    rep.check(S, "every conversation of the key one-shots plays to its end", not_ended.is_empty(), format!("{n_conv} runs ({} one-shots){}", ONESHOTS.len(), if not_ended.is_empty() { String::new() } else { format!("; did not end: {not_ended:?}") }));
    rep.check(S, "every spoken line has subtitle text", n_text == n_lines && n_lines > 0, format!("{n_text}/{n_lines} lines"));
    rep.check(
        S,
        "every line whose VO event exists decodes (non-silent original VO)",
        n_vo_bad == 0 && n_vo_ok > 0,
        format!("{n_vo_ok} lines decoded, {n_vo_bad} bad, {n_vo_missing} without a clip in the install (timed from text): {}", missing_list.join("; ")),
    );
    rep.check(S, "player choices offered and taken", n_choices > 0, format!("{n_choices} choices across both policies"));
    let need = |label: &str, out: &str| all_outputs.get(label).is_some_and(|s| s.contains(out));
    let expect = [
        ("InvitationPlease", "InvitationPlease_DONE"),
        ("Samuel_Intro", "Add Party Objective"),
        ("Lord_Brisby_Quest_Start", "Join_Brisby_in_Private"),
        ("Hunting Waverly", "Waverly_Tattles"),
        ("Hits_On_Player", "Quest For the Drink"),
        ("Hunting Lydia", "Lydia_moves_to_garden"),
    ];
    let missing: Vec<String> = expect.iter().filter(|(l, o)| !need(l, o)).map(|(l, o)| format!("{l}->{o}")).collect();
    rep.check(S, "Kismet outputs fire (DisConv_KismetActivateRemoteEvent)", missing.is_empty(), format!("{n_outputs} outputs; required {} present{}", expect.len() - missing.len(), if missing.is_empty() { String::new() } else { format!(", missing {missing:?}") }));
    let white_flag = all_flags.get("Hits_On_Player").is_some_and(|f| f.contains("WhiteQuestforDrink=true"));
    rep.check(S, "story flags set by conversations (DisConv_SetStoryFlag)", white_flag, format!("Hits_On_Player sets {:?}", all_flags.get("Hits_On_Player")));
    // Condition polarity: the doorman's use hook only reaches "windo guard used" once the
    // invitation was handed over (CheckStoryFlag output 1 = set).
    let o4 = db.find_oneshot("L_BoyleStreet_Ext_Script:DisDialogOneShot_4").map_err(anyhow::Error::msg)?;
    let (h, out) = db.used_hook(o4).context("doorman use hook")?;
    let before = play(&db, Target::Hook { oneshot: o4, hook: h, output: out }, 0, &[]);
    let after = play(&db, Target::Hook { oneshot: o4, hook: h, output: out }, 0, &["GaveInvitationtoGuard"]);
    rep.check(S, "story-flag checks branch on the shared flags", before.lines.is_empty() && after.lines.len() == 1, format!("use hook: {} lines without GaveInvitationtoGuard, {} with it", before.lines.len(), after.lines.len()));
    rep.table("Original conversations played (lines/VO counted; text stays in the install)", &["conversation", "choice policy", "lines", "VO decoded", "choices", "soirees", "outputs", "flags", "seconds"], rows);

    // ---- Mission integration -------------------------------------------------------
    let path = ctx.opt("--mission").unwrap_or_else(|| "missions/boyle_conversations.ron".into());
    let def = MissionDef::load(std::path::Path::new(&path))?;
    let maps = MapContents::load(&dishonored_content::mission_maps(&def))?;
    let a = mission_run(&maps, &def)?;
    let b = mission_run(&maps, &def)?;
    let has = |l: &str| a.log.iter().any(|x| x.contains(l));
    rep.check(S, "mission: doorman refuses without an invitation (InvitationPlease -> CantLetYouIn)", has("ended DisDialogOneShot_4/InvitationPlease") && has("ended DisDialogOneShot_4/CantLetYouIn"), a.first("CantLetYouIn"));
    rep.check(S, "mission: Lydia's choices taken with the number keys", a.choices_taken >= 1 && has("ended DisDialogOneShot_10/Hunting Lydia"), format!("{} choices, view showed a choice: {}", a.choices_taken, a.view_choice));
    rep.check(S, "mission: Miss White's use hook output and flag reach the rules", a.flags.contains("WhiteQuestforDrink") && a.tasks.contains("guests.white"), a.first("Quest For the Drink"));
    rep.check(S, "mission: Brisby's approach hook, then his use hook once the flag is set", has("ended DisDialogOneShot_6/Lord_Brisby_Quest_Start") && has("ended DisDialogOneShot_6/Brisby_Boyle_Invite"), a.first("Brisby_Boyle_Invite"));
    rep.check(S, "mission: doorman lets the player in with the invitation (Come In), mission completes", a.outcome.as_ref().is_some_and(|o| o.0) && has("ended DisDialogOneShot_4/Come In"), format!("{:?} after {:.0}s", a.outcome, a.seconds));
    rep.check(S, "mission view replicates subtitles (speaker, text, VO event) and the choice", a.view_lines > 0 && a.view_vo > 0 && a.view_choice, format!("{} distinct lines in the view, {} with a VO event", a.view_lines, a.view_vo));
    rep.check(S, "determinism: identical mission runs", a.hash == b.hash && a.log == b.log, format!("{:016x} vs {:016x}, {} vs {} conversation events", a.hash, b.hash, a.log.len(), b.log.len()));
    rep.table("Mission conversation timeline", &["t", "event"], a.log.iter().map(|l| l.splitn(2, ' ').map(str::to_string).collect()).collect());
    Ok(())
}

struct MissionRun {
    log: Vec<String>,
    flags: BTreeSet<String>,
    tasks: BTreeSet<String>,
    outcome: Option<(bool, String)>,
    choices_taken: usize,
    view_choice: bool,
    view_lines: usize,
    view_vo: usize,
    hash: u64,
    seconds: f32,
}

impl MissionRun {
    fn first(&self, what: &str) -> String {
        self.log.iter().find(|l| l.contains(what)).cloned().unwrap_or_default()
    }
}

/// Scripted agent: a list of (goal NPC or marker, action) steps.
fn mission_run(maps: &MapContents, def: &MissionDef) -> Result<MissionRun> {
    let mc = maps.get(&def.map).context("mission map")?;
    maps.set.set_active(&def.map);
    let setup = dishonored_content::mission_setup(maps, def.clone()).map_err(|e| anyhow::anyhow!(e))?;
    let mut g = setup.build(&mc.sim);
    g.add_player(1, "agent", 0, &mc.collision, &mc.sim);
    let nav = NavGraph::build(&mc.collision, mc.ai_points.iter().map(|p| p.0).chain(mc.spawns.iter().map(|s| s.position)));
    let mut follow = Follower::default();
    // Plan: doorman (refused), Lydia (choices), Miss White, Brisby (approach, then use),
    // invitation, doorman again.
    #[derive(Clone, Copy, PartialEq)]
    enum Goal {
        Npc(&'static str),
        Marker(&'static str),
    }
    let plan: Vec<(Goal, &'static str)> = vec![
        (Goal::Npc("doorman"), "ended DisDialogOneShot_4/CantLetYouIn"),
        (Goal::Npc("lydia"), "ended DisDialogOneShot_10/Hunting Lydia"),
        (Goal::Npc("white"), "ended DisDialogOneShot_9/Hits_On_Player"),
        (Goal::Npc("brisby"), "ended DisDialogOneShot_6/Brisby_Boyle_Invite"),
        (Goal::Marker("invitation"), "flag Boyle_Has_Invitation"),
        (Goal::Npc("doorman"), "ended DisDialogOneShot_4/Come In"),
    ];
    let mut step = 0;
    let mut run = MissionRun { log: Vec::new(), flags: BTreeSet::new(), tasks: BTreeSet::new(), outcome: None, choices_taken: 0, view_choice: false, view_lines: 0, view_vo: 0, hash: 0, seconds: 0.0 };
    let mut seen_lines = BTreeSet::new();
    let mut logged = 0usize;
    let mut last_flags: BTreeMap<String, bool> = BTreeMap::new();
    let mut choice_cooldown = 0u32;
    let (mut last_pos, mut stuck, mut sidestep) = (glam::Vec3::ZERO, 0u32, 0u32);
    for tick in 0..(400 * 60) {
        let t = tick as f32 / 60.0;
        let Game::Mission(m) = &g else { anyhow::bail!("not a mission game") };
        let me = m.world.player(1).cloned().context("agent")?;
        let mut input = PlayerInput { left_hand: LeftHand::Empty, ..Default::default() };
        let busy = m.state.conv.as_ref().is_some_and(|r| r.active().any(|i| i.initiator == Some(1) && (i.line.is_some() || i.choice.is_some()) && i.conversation.is_some()));
        if let Some(c) = &m.state.view.choice {
            // Answer with the number keys (option 2 first time, then option 1), on a press edge.
            run.view_choice = true;
            if choice_cooldown == 0 {
                input.buttons |= if run.choices_taken == 0 { buttons::CHOICE_2 } else { buttons::CHOICE_1 };
                run.choices_taken += 1;
                choice_cooldown = 6;
                let _ = c;
            }
        } else if step < plan.len() && !busy {
            let goal = match plan[step].0 {
                Goal::Npc(n) => m.setup.npcs.iter().find(|x| x.0 == n).and_then(|(_, s)| m.world.player(s.id)).map(|p| p.position),
                Goal::Marker(k) => m.state.markers.get(k).map(|x| x.0),
            };
            if let Some(gp) = goal {
                let d = (gp - me.position).truncate().length();
                if std::env::var_os("CONV_DEBUG").is_some() && tick % 120 == 0 {
                    eprintln!("t {t:.0} step {step} me {:.0} goal {gp:.0} d {d:.0}", me.position);
                }
                let to = gp - me.position;
                if d < 120.0 {
                    input.yaw = to.y.atan2(to.x).to_degrees();
                    input.pitch = -15.0;
                    if tick % 20 == 0 {
                        input.buttons |= buttons::USE;
                    }
                } else {
                    let p = follow.steer(&nav, &mc.collision, me.position, gp);
                    let to = p - me.position;
                    input.yaw = to.y.atan2(to.x).to_degrees();
                    input.forward = 127;
                    // Unstick: sidestep and hop, then replan (as in `harness mission`).
                    if me.position.distance(last_pos) < 0.5 {
                        stuck += 1;
                    } else {
                        stuck = 0;
                    }
                    if stuck > 40 {
                        stuck = 0;
                        sidestep = 25;
                        follow.force_replan();
                    }
                    if sidestep > 0 {
                        sidestep -= 1;
                        input.yaw += 90.0;
                        if sidestep == 24 {
                            input.buttons |= buttons::JUMP;
                        }
                    }
                }
            }
        }
        choice_cooldown = choice_cooldown.saturating_sub(1);
        last_pos = me.position;
        g.step(&[(1, input)], &maps.set, &mc.sim);
        let Game::Mission(m) = &g else { unreachable!() };
        for e in &m.state.conv_log[logged.min(m.state.conv_log.len())..] {
            let s = match e {
                ConvEvent::Started { name, .. } => Some(format!("started {}", short(name))),
                ConvEvent::Ended { name, interrupted, .. } => Some(format!("ended {}{}", short(name), if *interrupted { " (interrupted)" } else { "" })),
                ConvEvent::Output { name, .. } => Some(format!("output {name}")),
                ConvEvent::Chosen { index, .. } => Some(format!("chose option {}", index + 1)),
                ConvEvent::Soiree { guid, matinee, .. } => Some(format!("soiree {guid} -> {}", matinee.as_deref().map(|m| m.rsplit('.').next().unwrap_or(m)).unwrap_or("-"))),
                _ => None,
            };
            if let Some(s) = s {
                run.log.push(format!("{t:.1} {s}"));
            }
        }
        logged = m.state.conv_log.len();
        for f in def.flags.iter() {
            let v = m.state.flag(f);
            if last_flags.insert(f.clone(), v) != Some(v) && v {
                run.flags.insert(f.clone());
                run.log.push(format!("{t:.1} flag {f}"));
            }
        }
        for o in &def.objectives {
            for x in &o.tasks {
                let p = format!("{}.{}", o.id, x.id);
                if m.state.task(&p) == Some(TaskState::Completed) && run.tasks.insert(p.clone()) {
                    run.log.push(format!("{t:.1} task {p} completed"));
                }
            }
        }
        if let Some(mv) = &g.view().mission {
            for l in &mv.dialogue {
                if seen_lines.insert(l.id) {
                    run.view_lines += 1;
                    run.view_vo += l.vo_event.is_some() as usize;
                }
            }
        }
        if step < plan.len() && run.log.iter().any(|l| l.contains(plan[step].1)) {
            step += 1;
            follow.force_replan();
        }
        if let Some(o) = m.state.outcome() {
            run.outcome = Some((o.success, o.text.clone()));
            run.seconds = t;
            break;
        }
    }
    run.hash = dishonored_sim::state_hash(g.world());
    Ok(run)
}

fn short(name: &str) -> &str {
    name.split_once(':').map(|x| x.1).unwrap_or(name)
}
