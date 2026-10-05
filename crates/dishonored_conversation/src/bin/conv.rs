//! Research CLI over the conversation loader and runner. Output contains retail text: keep
//! it under harness_out/.
//!
//! conv stats <pkg>...                      node classes, lines, VO coverage
//! conv list <pkg>...                       one-shots, conversations, hooks, outputs
//! conv survey <pkg>...                     branch / condition / conversation settings (counts)
//! conv flags <pkg>...                      story flags checked / set by conversations
//! conv play <pkg>... -- <ref> [choice...]  run a conversation headless (choices 1-based)
use anyhow::Result;
use dishonored_conversation::load::{Loader, NoVo, VoLengths, class_histogram};
use dishonored_conversation::model::*;
use dishonored_conversation::runner::{ConvEvent, Runner, SimpleEnv, StartSpec};
use std::sync::Arc;

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mode = a.first().cloned().unwrap_or_default();
    if mode == "findevent" {
        // conv findevent <event>...: which Bk_*.<lang> files define these events.
        let loader = Loader::locate()?;
        let want: Vec<(u32, &String)> = a[1..].iter().map(|e| (dishonored_audio::fnv1_32(e), e)).collect();
        for e in std::fs::read_dir(&loader.cooked)?.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if !n.starts_with("Bk_") {
                continue;
            }
            if let Ok(ids) = dishonored_audio::index::event_ids(&e.path()) {
                for (id, ev) in &want {
                    if ids.contains(id) {
                        println!("{ev} in {n}");
                    }
                }
            }
        }
        return Ok(());
    }
    let split = a.iter().position(|x| x == "--").unwrap_or(a.len());
    let pkgs: Vec<String> = a[1..split].to_vec();
    let rest: Vec<String> = a.get(split + 1..).map(|s| s.to_vec()).unwrap_or_default();
    let mut loader = Loader::locate()?;
    let want_vo = mode == "stats" || mode == "play";
    let mut vo: Box<dyn VoLengths> = if want_vo { Box::new(dishonored_audio::vo::VoResolver::open(loader.cooked.parent().unwrap().parent().unwrap())?) } else { Box::new(NoVo) };
    for p in &pkgs {
        loader.add_package(p, vo.as_mut())?;
    }
    let db = loader.finish();
    match mode.as_str() {
        "stats" => {
            for (c, n) in class_histogram(&db) {
                println!("{c:<40} {n}");
            }
            let (lines, vo, len) = db.line_stats();
            let conv_lines = db.nodes.iter().filter(|n| n.conversation.is_some() && matches!(n.kind, NodeKind::Line(_))).count();
            let conv_len = db.nodes.iter().filter(|n| n.conversation.is_some() && matches!(&n.kind, NodeKind::Line(l) if l.vo_length.is_some())).count();
            println!("one-shots {} trees {} conversations {}", db.oneshots.len(), db.trees.len(), db.conversations.len());
            println!("line nodes {lines}: with VO event {vo}, decodable length {len}; in conversations {conv_lines} ({conv_len} with VO length)");
            for n in &db.nodes {
                if let (Some(c), NodeKind::Line(l)) = (n.conversation, &n.kind) {
                    if l.vo_length.is_none() {
                        println!("  no VO: {} [{}] {:?}", db.conversation_name(c), n.path.rsplit('.').next().unwrap_or(""), l.vo.as_ref().map(|v| (&v.event, &v.bank)));
                    }
                }
            }
        }
        "list" => {
            for (oi, o) in db.oneshots.iter().enumerate() {
                let sp: Vec<String> = o.speakers.iter().map(|s| format!("{}@{}", s.nick, s.actor.clone().unwrap_or_default())).collect();
                println!("{}:{} speakers [{}]", o.package, o.name, sp.join(", "));
                let Some(t) = o.tree else { continue };
                for h in &db.trees[t as usize].hooks {
                    if let NodeKind::Hook { hook, names, .. } = &db.node(*h).kind {
                        println!("  hook {hook} {names:?}");
                    }
                }
                for c in &db.trees[t as usize].conversations {
                    let cv = &db.conversations[*c as usize];
                    println!("  conv {} '{}' outputs {:?} once={} prio={:?}", cv.name, cv.label, db.outputs_of(*c), cv.fire_once, cv.priority);
                }
                let _ = oi;
            }
        }
        "survey" => {
            // Node settings that decide branch semantics (no retail text printed).
            let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
            let mut bump = |k: String| *counts.entry(k).or_default() += 1;
            for n in &db.nodes {
                let outs = n.outputs.len();
                match &n.kind {
                    NodeKind::RandomBranch { keep_unique, weights } => bump(format!("random keep_unique={keep_unique} weights={weights:?} outputs={outs}")),
                    NodeKind::SequentialBranch { looping, global } => bump(format!("sequential looping={looping} global={global} outputs={outs}")),
                    NodeKind::RandomSequentialBranch { global, branches, looping } => bump(format!("randseq looping={looping} global={global} branches={branches} outputs={outs}")),
                    NodeKind::TimeLimit { seconds, global } => bump(format!("timelimit {seconds}s global={global} tree_level={}", n.conversation.is_none())),
                    NodeKind::Soiree { preview_only, matinee, .. } => bump(format!("soiree preview_only={preview_only} matinee={} preview_link={}", matinee.is_some(), n.outputs.get(1).copied().flatten().is_some())),
                    NodeKind::Choice { options, optional } => bump(format!("choice static={} optional={}", options.len(), optional.len())),
                    NodeKind::GroupBranch { kind, entries, .. } => bump(format!("group {kind:?} entries={} outputs={outs}", entries.len())),
                    NodeKind::CheckSuspicion | NodeKind::IsPossessed | NodeKind::CombatStatus | NodeKind::SpeakerAvailable => bump(format!("{} outputs={outs}", n.class)),
                    NodeKind::SeenLabel { conversation, .. } => bump(format!("seenlabel conversation_set={}", conversation.is_some())),
                    NodeKind::HadConversation { .. } | NodeKind::ConversationFired { .. } => bump(n.class.clone()),
                    NodeKind::Line(l) => bump(format!("line vo={} length={} labelled={}", l.vo.is_some(), l.vo_length.is_some(), n.label.is_some())),
                    _ if n.label.is_some() => bump(format!("labelled {}", n.class)),
                    _ => {}
                }
            }
            for c in &db.conversations {
                bump(format!("conversation prio={:?} blocking={} blocking_player={} near={} once={} walked_away_link={} interrupted_link={}", c.priority, c.blocking, c.blocking_player, c.player_must_be_near, c.fire_once, c.walked_away.is_some(), c.interrupted.is_some()));
            }
            for (k, v) in counts {
                println!("{v:6} {k}");
            }
        }
        "flags" => {
            for n in &db.nodes {
                match &n.kind {
                    NodeKind::CheckFlag { flag } => println!("check {flag}\t{}", n.path),
                    NodeKind::SetFlag { flag, value } => println!("set {flag}={value}\t{}", n.path),
                    _ => {}
                }
            }
        }
        "play" => {
            let db = Arc::new(db);
            let target = db.resolve(&rest[0]).map_err(anyhow::Error::msg)?;
            let mut env = SimpleEnv::default();
            for f in rest.iter().skip(1).filter(|s| s.starts_with('+')) {
                env.flags.insert(f[1..].to_string(), true);
            }
            let mut choices: Vec<usize> = rest.iter().skip(1).filter_map(|s| s.parse::<usize>().ok()).collect();
            choices.reverse();
            let mut r = Runner::new(db.clone(), 1);
            let id = r.start(StartSpec { target, initiator: Some(1), bindings: vec![], force: true }, &mut env);
            println!("start -> {id:?}");
            let mut t = 0.0;
            loop {
                for e in r.take_events() {
                    match &e {
                        ConvEvent::Line { speaker, text, vo, duration, .. } => println!("{t:7.2} [{}] {text}  ({duration:.2}s {:?})", speaker.name, vo.as_ref().map(|v| &v.event)),
                        ConvEvent::Choice { options, .. } => println!("{t:7.2} CHOICE {options:?}"),
                        other => println!("{t:7.2} {other:?}"),
                    }
                }
                if r.active().next().is_none() || t > 600.0 {
                    break;
                }
                if r.active().any(|i| i.choice.is_some()) {
                    let c = choices.pop().unwrap_or(1);
                    r.choose(1, c - 1, &mut env);
                    continue;
                }
                r.update(0.1, &mut env);
                t += 0.1;
            }
            println!("flags {:?}", env.flags);
        }
        _ => eprintln!("usage: conv stats|list|survey|flags|play <pkg>... [-- <ref> [choices]]"),
    }
    Ok(())
}
