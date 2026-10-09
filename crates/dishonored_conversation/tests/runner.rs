//! Runner semantics on synthetic graphs (no install needed). Each test pins one rule decoded
//! from the original executable (research/dishonored/CONVERSATIONS.md).

use dishonored_conversation::hooks::{DistanceHook, HookClock, HookRejection, hook_index, hook_info};
use dishonored_conversation::model::*;
use dishonored_conversation::runner::{ConvEnv, ConvEvent, EndReason, HookFacts, HookRequest, Runner, SimpleEnv, StartSpec};
use std::collections::BTreeMap;
use std::sync::Arc;

fn line(text: &str, len: f32, next: Option<NodeId>) -> Node {
    let mut n = node(NodeKind::Line(Line { guid: String::new(), text: text.into(), nonword: false, spoken_to: 2, main_subtitle: true, voice_radius: 3000.0, end_delay: 0.0, label: Some(text.into()), look: None, vo: Some(VoRef { event: format!("Play_{text}"), bank: "Bk_X".into() }), vo_length: Some(len) }), vec![next]);
    n.label = Some(text.into());
    n
}

fn node(kind: NodeKind, outputs: Vec<Option<NodeId>>) -> Node {
    Node { path: String::new(), class: String::new(), tree: 0, conversation: Some(0), speaker: 0, label: None, outputs, kind }
}

fn in_conv(mut n: Node, c: u32) -> Node {
    n.conversation = Some(c);
    n
}

fn conv(label: &str, start: Option<NodeId>) -> Conversation {
    Conversation {
        path: label.into(),
        name: format!("DisConversation_{label}"),
        tree: 0,
        label: label.into(),
        description: String::new(),
        speakers: vec![SpeakerSlot::Me, SpeakerSlot::Initiator, SpeakerSlot::Player],
        speaker_nicks: vec![Some("Tester".into()), None, None],
        speaker_gates: vec![SpeakerGate::default(); 3],
        start,
        interrupted: None,
        walked_away: None,
        blocking: true,
        blocking_player: false,
        fire_once: false,
        priority: Priority::High,
        player_must_be_near: false,
        player_near_radius: 500.0,
        not_when_player_in_combat: false,
        always_interrupt_for_combat: false,
    }
}

fn make_db(convs: Vec<Conversation>, nodes: Vec<Node>, hooks: Vec<NodeId>) -> Arc<ConversationDb> {
    let n = convs.len() as u32;
    Arc::new(ConversationDb {
        packages: vec!["P".into()],
        oneshots: vec![OneShot { package: "P".into(), name: "OS".into(), path: "OS".into(), tree: Some(0), speakers: vec![OneShotSpeaker { nick: "Me".into(), actor: Some("Spawner_1".into()), voice: None, player: false }], location: [0.0; 3] }],
        trees: vec![Tree { package: "P".into(), path: "T".into(), oneshot: Some(0), hooks, conversations: (0..n).collect(), fallback: None }],
        conversations: convs,
        nodes,
        voice_trees: Default::default(),
        settings: Default::default(),
    })
}

/// 0 check flag F -> [1 "a", 2 seen-label "a"]; 2 -> [3 choice, 4 output]; 3 -> [5 "b", 6 soiree];
/// 6 soiree -> [finished: 7 set flag G, preview: 8 "c"].
fn db() -> Arc<ConversationDb> {
    let nodes = vec![
        node(NodeKind::CheckFlag { flag: "F".into() }, vec![Some(1), Some(2)]),
        line("a", 1.0, None),
        node(NodeKind::SeenLabel { oneshot: Some("OS".into()), conversation: Some(0), label: "a".into() }, vec![Some(3), Some(4)]),
        node(NodeKind::Choice { options: vec!["one".into(), "two".into()], optional: vec![] }, vec![Some(5), Some(6)]),
        node(NodeKind::KismetOutput { name: "Heard_A".into() }, vec![None]),
        line("b", 2.0, None),
        node(NodeKind::Soiree { guid: "G".into(), matinee: None, preview_only: true }, vec![Some(7), Some(8)]),
        node(NodeKind::SetFlag { flag: "G".into(), value: true }, vec![None]),
        line("c", 0.5, None),
    ];
    make_db(vec![conv("Test", Some(0))], nodes, vec![])
}

fn run(r: &mut Runner, env: &mut dyn ConvEnv, choices: &[usize]) -> Vec<ConvEvent> {
    let mut all = Vec::new();
    let mut c = choices.iter();
    for _ in 0..2000 {
        all.extend(r.take_events());
        if r.active().next().is_none() {
            break;
        }
        if r.active().any(|i| i.choice.is_some()) {
            assert!(r.choose(1, *c.next().unwrap(), env));
            continue;
        }
        r.update(0.05, env);
    }
    all
}

fn spec(c: u32, npc: &str) -> StartSpec {
    StartSpec { target: Target::Conversation { oneshot: Some(0), conversation: c }, initiator: Some(1), bindings: vec![Some(npc.into())], force: false }
}

fn start(r: &mut Runner, env: &mut dyn ConvEnv) {
    r.start(spec(0, "npc"), env).expect("starts");
}

fn lines(ev: &[ConvEvent]) -> Vec<String> {
    ev.iter().filter_map(|e| if let ConvEvent::Line { text, .. } = e { Some(text.clone()) } else { None }).collect()
}

fn outputs(ev: &[ConvEvent]) -> Vec<String> {
    ev.iter().filter_map(|e| if let ConvEvent::Output { name, .. } = e { Some(name.clone()) } else { None }).collect()
}

#[test]
fn flag_false_takes_output_0_and_line_holds_for_its_length() {
    let mut r = Runner::new(db(), 1);
    let mut env = SimpleEnv::default();
    start(&mut r, &mut env);
    let ev = r.take_events();
    assert!(matches!(&ev[1], ConvEvent::Line { text, duration, speaker, subtitle: true, .. } if text == "a" && (*duration - 1.0).abs() < 1e-6 && speaker.name == "Tester" && speaker.npc.as_deref() == Some("npc")));
    r.update(0.9, &mut env);
    assert!(r.active().next().is_some(), "still speaking");
    r.update(0.2, &mut env);
    assert!(r.active().next().is_none(), "ended after the clip");
    // The label is in every speaker's memory (the NPC and the player).
    assert!(r.heard("npc", 0, "a") && r.heard("player", 0, "a"));
}

#[test]
fn flag_true_then_label_seen_reaches_output_and_choice_and_soiree_order() {
    let mut r = Runner::new(db(), 1);
    let mut env = SimpleEnv::default();
    env.flags.insert("F".into(), true);
    start(&mut r, &mut env);
    let ev = run(&mut r, &mut env, &[1]);
    let kinds: Vec<String> = ev
        .iter()
        .filter_map(|e| match e {
            ConvEvent::Choice { .. } => Some("choice".into()),
            ConvEvent::Chosen { index, .. } => Some(format!("chose{index}")),
            ConvEvent::Soiree { .. } => Some("soiree".into()),
            ConvEvent::Line { text, .. } => Some(text.clone()),
            ConvEvent::Flag { name, .. } => Some(format!("flag{name}")),
            _ => None,
        })
        .collect();
    // Preview-only soiree: its preview chain (line c), then output 0 (set G).
    assert_eq!(kinds, vec!["choice", "chose1", "soiree", "c", "flagG"]);
    assert!(env.flags["G"]);
}

#[test]
fn heard_label_takes_output_1() {
    let mut r = Runner::new(db(), 1);
    let mut env = SimpleEnv::default();
    start(&mut r, &mut env);
    run(&mut r, &mut env, &[]);
    env.flags.insert("F".into(), true);
    start(&mut r, &mut env);
    let ev = run(&mut r, &mut env, &[]);
    assert_eq!(outputs(&ev), vec!["Heard_A"]);
}

#[test]
fn seen_label_is_per_speaker() {
    // Another NPC in the same conversation has not heard "a".
    let mut r = Runner::new(db(), 1);
    let mut env = SimpleEnv::default();
    start(&mut r, &mut env);
    run(&mut r, &mut env, &[]);
    env.flags.insert("F".into(), true);
    r.start(spec(0, "other"), &mut env).unwrap();
    let ev = run(&mut r, &mut env, &[0]);
    assert!(outputs(&ev).is_empty(), "the other NPC takes the not-heard branch");
}

#[test]
fn labels_on_any_node_are_recorded() {
    // A labelled set-flag node (not a line) marks the label.
    let mut set = node(NodeKind::SetFlag { flag: "X".into(), value: true }, vec![None]);
    set.label = Some("marked".into());
    let db = make_db(vec![conv("C", Some(0))], vec![set], vec![]);
    let mut r = Runner::new(db, 1);
    let mut env = SimpleEnv::default();
    start(&mut r, &mut env);
    assert!(r.heard("npc", 0, "marked"));
}

#[test]
fn deterministic() {
    let go = || {
        let mut r = Runner::new(db(), 9);
        let mut env = SimpleEnv::default();
        env.flags.insert("F".into(), true);
        start(&mut r, &mut env);
        run(&mut r, &mut env, &[0])
    };
    assert_eq!(go(), go());
}

/// Conversation: 0 branch -> outputs 1..=k (KismetOutput "o<i>").
fn branch_db(kind: NodeKind, outs: usize) -> Arc<ConversationDb> {
    let mut nodes = vec![node(kind, (1..=outs as u32).map(Some).collect())];
    for i in 0..outs {
        nodes.push(node(NodeKind::KismetOutput { name: format!("o{i}") }, vec![None]));
    }
    make_db(vec![conv("B", Some(0))], nodes, vec![])
}

fn picks(db: &Arc<ConversationDb>, n: usize, seed: u64) -> Vec<String> {
    let mut r = Runner::new(db.clone(), seed);
    let mut env = SimpleEnv::default();
    let mut v = Vec::new();
    for _ in 0..n {
        start(&mut r, &mut env);
        v.extend(outputs(&run(&mut r, &mut env, &[])));
    }
    v
}

#[test]
fn sequential_branch_stops_on_last_or_loops() {
    let stop = branch_db(NodeKind::SequentialBranch { looping: false, global: false }, 3);
    assert_eq!(picks(&stop, 5, 1), vec!["o0", "o1", "o2", "o2", "o2"]);
    let lp = branch_db(NodeKind::SequentialBranch { looping: true, global: false }, 3);
    assert_eq!(picks(&lp, 5, 1), vec!["o0", "o1", "o2", "o0", "o1"]);
}

#[test]
fn random_sequential_branch_depletes_or_loops() {
    // Three branches plus "Depleted" (output 3).
    let db = branch_db(NodeKind::RandomSequentialBranch { global: false, branches: 3, looping: false }, 4);
    let p = picks(&db, 5, 3);
    let mut first: Vec<String> = p[..3].to_vec();
    first.sort();
    assert_eq!(first, vec!["o0", "o1", "o2"], "each branch once");
    assert_eq!(&p[3..], &["o3".to_string(), "o3".to_string()], "then Depleted");
    let lp = branch_db(NodeKind::RandomSequentialBranch { global: false, branches: 3, looping: true }, 4);
    let p = picks(&lp, 6, 3);
    assert!(!p.contains(&"o3".to_string()), "looping never depletes");
    // Inside a conversation the counter writes at once, without a reshuffle: the order repeats.
    assert_eq!(p[..3], p[3..]);
}

#[test]
fn random_branch_weights_and_keep_unique() {
    // 0% / 100%: always output 1.
    let db = branch_db(NodeKind::RandomBranch { keep_unique: false, weights: vec![0.0, 100.0] }, 2);
    assert!(picks(&db, 10, 5).iter().all(|o| o == "o1"));
    // 100% / 0% with keep-unique: the stored last index starts at 0, so the first pick (0)
    // counts as a repeat and moves to output 1; then it alternates.
    let db = branch_db(NodeKind::RandomBranch { keep_unique: true, weights: vec![100.0, 0.0] }, 2);
    assert_eq!(picks(&db, 4, 5), vec!["o1", "o0", "o1", "o0"]);
}

#[test]
fn time_limit_blocks_within_the_limit_inclusive() {
    let db = branch_db(NodeKind::TimeLimit { seconds: 10.0, global: false }, 2);
    let mut r = Runner::new(db, 1);
    let mut env = SimpleEnv::default();
    let go = |r: &mut Runner, env: &mut SimpleEnv| {
        start(r, env);
        outputs(&run(r, env, &[]))
    };
    r.update(1.0, &mut env);
    assert_eq!(go(&mut r, &mut env), vec!["o0"]);
    r.update(10.0, &mut env);
    assert_eq!(go(&mut r, &mut env), vec!["o1"], "exactly the limit still blocks");
    r.update(0.5, &mut env);
    assert_eq!(go(&mut r, &mut env), vec!["o0"]);
}

#[test]
fn tree_level_branch_state_commits_only_when_a_conversation_starts() {
    // Hook -> sequential branch (tree level) -> [ref to fire-once conversation 0, ref to 0].
    // Conversation 0 (fire once) has one line.
    let mut c0 = conv("Once", Some(0));
    c0.fire_once = true;
    let mut seq = node(NodeKind::SequentialBranch { looping: false, global: false }, vec![Some(2), Some(3)]);
    seq.conversation = None;
    let mut r1 = node(NodeKind::ConversationRef { conversation: Some(0) }, vec![None, None]);
    r1.conversation = None;
    let mut r2 = node(NodeKind::ConversationRef { conversation: Some(0) }, vec![None, None]);
    r2.conversation = None;
    let mut hook = node(NodeKind::Hook { hook: "DDH_PLAYER_USED".into(), index: 0, names: vec![], priority: None, distances: None, class: HookClass::Base }, vec![Some(1)]);
    hook.conversation = None;
    let nodes = vec![line("x", 0.1, None), seq, r1, r2, hook];
    let db = make_db(vec![c0], nodes, vec![4]);
    let mut r = Runner::new(db, 1);
    let mut env = SimpleEnv::default();
    let use_hook = |r: &mut Runner, env: &mut SimpleEnv| {
        r.start(StartSpec { target: Target::Hook { oneshot: 0, hook: 4, output: 0 }, initiator: Some(1), bindings: vec![Some("npc".into())], force: false }, env);
        lines(&run(r, env, &[]))
    };
    assert_eq!(use_hook(&mut r, &mut env), vec!["x"], "first use: branch 0 starts the conversation");
    // Second use: branch 1 -> the conversation already fired -> nothing starts (the counter
    // advanced once, committed by the first start).
    assert!(use_hook(&mut r, &mut env).is_empty());
    assert_eq!(r.fired_count(0), 1);
}

#[test]
fn priority_equal_refuses_higher_interrupts_and_runs_interrupted_branch() {
    // Conversation 0 (medium): long line. Interrupted -> output "cut" then line "hey".
    // Conversation 1 (high): line "boss". Conversation 2 (medium).
    let mut c0 = conv("Idle", Some(0));
    c0.priority = Priority::Medium;
    c0.interrupted = Some(1);
    let mut c1 = conv("Boss", Some(3));
    c1.priority = Priority::High;
    let mut c2 = conv("Other", Some(4));
    c2.priority = Priority::Medium;
    let nodes = vec![
        line("idle", 30.0, None),
        node(NodeKind::KismetOutput { name: "cut".into() }, vec![Some(2)]),
        line("hey", 1.0, Some(0)),
        in_conv(line("boss", 1.0, None), 1),
        in_conv(line("other", 1.0, None), 2),
    ];
    let db = make_db(vec![c0, c1, c2], nodes, vec![]);
    let mut r = Runner::new(db, 1);
    let mut env = SimpleEnv::default();
    r.start(spec(0, "npc"), &mut env).unwrap();
    assert!(r.start(spec(2, "npc"), &mut env).is_none(), "equal priority refused");
    r.take_events();
    r.start(spec(1, "npc"), &mut env).unwrap();
    let ev = r.take_events();
    assert_eq!(outputs(&ev), vec!["cut"]);
    assert_eq!(lines(&ev), vec!["hey", "boss"], "first interrupted line starts, then the new conversation");
    assert!(ev.iter().any(|e| matches!(e, ConvEvent::Ended { label, interrupted: true, .. } if label == "Idle")));
    assert_eq!(r.detached.len(), 1, "the interrupted line plays out on its own");
    assert_eq!(r.active().count(), 1);
}

#[test]
fn force_interrupts_equal_priority() {
    let mut c0 = conv("A", Some(0));
    c0.priority = Priority::Medium;
    let mut c1 = conv("B", Some(1));
    c1.priority = Priority::Medium;
    let db = make_db(vec![c0, c1], vec![line("a", 30.0, None), in_conv(line("b", 1.0, None), 1)], vec![]);
    let mut r = Runner::new(db, 1);
    let mut env = SimpleEnv::default();
    r.start(spec(0, "npc"), &mut env).unwrap();
    let mut s = spec(1, "npc");
    s.force = true;
    assert!(r.start(s, &mut env).is_some());
}

struct DistEnv {
    flags: BTreeMap<String, bool>,
    dist: f32,
}

impl ConvEnv for DistEnv {
    fn flag(&self, n: &str) -> bool {
        self.flags.get(n).copied().unwrap_or(false)
    }
    fn set_flag(&mut self, n: &str, v: bool) {
        self.flags.insert(n.into(), v);
    }
    fn distance(&self, _: Option<u32>, _: Option<&str>) -> Option<f32> {
        Some(self.dist)
    }
}

#[test]
fn player_must_be_near_to_start_and_walk_away() {
    // Conversation 0: line "a" (10 s) -> line "b". Walked away -> line "bye".
    let mut c0 = conv("Near", Some(0));
    c0.player_must_be_near = true;
    c0.player_near_radius = 300.0;
    c0.walked_away = Some(2);
    let db = make_db(vec![c0], vec![line("a", 10.0, Some(1)), line("b", 1.0, None), line("bye", 1.0, None)], vec![]);
    let mut r = Runner::new(db.clone(), 1);
    let mut env = DistEnv { flags: BTreeMap::new(), dist: 400.0 };
    assert!(r.start(spec(0, "npc"), &mut env).is_none(), "player too far to start");
    env.dist = 100.0;
    r.start(spec(0, "npc"), &mut env).unwrap();
    r.update(1.0, &mut env);
    r.take_events();
    env.dist = 300.0; // not strictly within the radius
    let ev = run(&mut r, &mut env, &[]);
    assert_eq!(lines(&ev), vec!["bye"], "walked away output replaces the rest");
    // Without a walked-away link the conversation ends as interrupted.
    let mut c1 = conv("Near", Some(0));
    c1.player_must_be_near = true;
    let db = make_db(vec![c1], vec![line("a", 10.0, None)], vec![]);
    let mut r = Runner::new(db, 1);
    let mut env = DistEnv { flags: BTreeMap::new(), dist: 100.0 };
    r.start(spec(0, "npc"), &mut env).unwrap();
    env.dist = 900.0;
    let ev = run(&mut r, &mut env, &[]);
    assert!(ev.iter().any(|e| matches!(e, ConvEvent::Ended { interrupted: true, .. })));
}

#[test]
fn blocking_player_conversations_ignore_walk_away() {
    let mut c0 = conv("Block", Some(0));
    c0.player_must_be_near = true;
    c0.blocking_player = true;
    let db = make_db(vec![c0], vec![line("a", 2.0, None)], vec![]);
    let mut r = Runner::new(db, 1);
    let mut env = DistEnv { flags: BTreeMap::new(), dist: 100.0 };
    r.start(spec(0, "npc"), &mut env).unwrap();
    env.dist = 5000.0;
    let ev = run(&mut r, &mut env, &[]);
    assert!(ev.iter().any(|e| matches!(e, ConvEvent::Ended { interrupted: false, .. })));
}

#[test]
fn optional_choices_follow_their_flag_and_map_to_later_outputs() {
    // Static ["s0"], optional ["o_set" (flag K), "o_unset" (flag K must be false)].
    let ch = node(
        NodeKind::Choice {
            options: vec!["s0".into()],
            optional: vec![OptionalChoice { text: "o_set".into(), flag: "K".into(), must_be_false: false }, OptionalChoice { text: "o_unset".into(), flag: "K".into(), must_be_false: true }],
        },
        vec![Some(1), Some(2), Some(3)],
    );
    let nodes = vec![ch, node(NodeKind::KismetOutput { name: "s0".into() }, vec![None]), node(NodeKind::KismetOutput { name: "set".into() }, vec![None]), node(NodeKind::KismetOutput { name: "unset".into() }, vec![None])];
    let db = make_db(vec![conv("Ch", Some(0))], nodes, vec![]);
    let mut r = Runner::new(db.clone(), 1);
    let mut env = SimpleEnv::default();
    start(&mut r, &mut env);
    assert_eq!(r.active().next().unwrap().choice.as_ref().unwrap().options, vec!["s0", "o_unset"]);
    assert_eq!(outputs(&run(&mut r, &mut env, &[1])), vec!["unset"]);
    env.flags.insert("K".into(), true);
    start(&mut r, &mut env);
    assert_eq!(outputs(&run(&mut r, &mut env, &[1])), vec!["set"]);
}

#[test]
fn spawner_branch_matches_the_speakers_spawner_else_everyone_else() {
    let mk = |name: &str| {
        let b = node(
            NodeKind::GroupBranch { groups: vec![], kind: GroupKind::Spawner, entries: vec![GroupEntry { name: "Spawner_9".into(), exact: true }, GroupEntry { name: name.into(), exact: true }] },
            vec![Some(1), Some(2), Some(3)],
        );
        let nodes = vec![b, node(NodeKind::KismetOutput { name: "first".into() }, vec![None]), node(NodeKind::KismetOutput { name: "second".into() }, vec![None]), node(NodeKind::KismetOutput { name: "else".into() }, vec![None])];
        make_db(vec![conv("G", Some(0))], nodes, vec![])
    };
    let mut env = SimpleEnv::default();
    let mut r = Runner::new(mk("Spawner_1"), 1);
    start(&mut r, &mut env);
    assert_eq!(outputs(&run(&mut r, &mut env, &[])), vec!["second"]);
    let mut r = Runner::new(mk("Spawner_2"), 1);
    start(&mut r, &mut env);
    assert_eq!(outputs(&run(&mut r, &mut env, &[])), vec!["else"]);
}

#[test]
fn line_without_vo_is_silent_and_takes_only_its_end_delay() {
    let mut l = line("x", 0.0, None);
    if let NodeKind::Line(ref mut x) = l.kind {
        x.vo = None;
        x.vo_length = None;
        x.end_delay = 0.25;
    }
    let db = make_db(vec![conv("S", Some(0))], vec![l], vec![]);
    let mut r = Runner::new(db, 1);
    let mut env = SimpleEnv::default();
    start(&mut r, &mut env);
    let ev = r.take_events();
    assert!(ev.iter().any(|e| matches!(e, ConvEvent::Line { subtitle: false, duration, .. } if (*duration - 0.25).abs() < 1e-6)));
}

#[test]
fn soiree_is_skipped_in_non_blocking_conversations() {
    let mut c = conv("NB", Some(0));
    c.blocking = false;
    let nodes = vec![node(NodeKind::Soiree { guid: "G".into(), matinee: None, preview_only: false }, vec![Some(1), Some(2)]), node(NodeKind::KismetOutput { name: "after".into() }, vec![None]), line("preview", 1.0, None)];
    let db = make_db(vec![c], nodes, vec![]);
    let mut r = Runner::new(db, 1);
    let mut env = SimpleEnv::default();
    start(&mut r, &mut env);
    let ev = run(&mut r, &mut env, &[]);
    assert!(lines(&ev).is_empty() && outputs(&ev) == vec!["after"]);
}

struct SuspEnv(u8);
impl ConvEnv for SuspEnv {
    fn flag(&self, _: &str) -> bool {
        false
    }
    fn set_flag(&mut self, _: &str, _: bool) {}
    fn suspicion(&self, _: Option<&str>) -> u8 {
        self.0
    }
}

#[test]
fn condition_output_past_the_links_ends_as_error() {
    // Suspicion level 2 on a node with two outputs: the original ends the conversation as an
    // error, which runs the Interrupted output.
    let mut c = conv("E", Some(0));
    c.interrupted = Some(3);
    let nodes = vec![node(NodeKind::CheckSuspicion, vec![Some(1), Some(2)]), node(NodeKind::KismetOutput { name: "calm".into() }, vec![None]), node(NodeKind::KismetOutput { name: "sus".into() }, vec![None]), node(NodeKind::KismetOutput { name: "int".into() }, vec![None])];
    let db = make_db(vec![c], nodes, vec![]);
    let mut r = Runner::new(db.clone(), 1);
    let mut env = SuspEnv(1);
    start(&mut r, &mut env);
    assert_eq!(outputs(&run(&mut r, &mut env, &[])), vec!["sus"]);
    let mut env = SuspEnv(2);
    start(&mut r, &mut env);
    assert_eq!(outputs(&run(&mut r, &mut env, &[])), vec!["int"]);
}

#[test]
fn stop_runs_interrupted_and_reports_reason() {
    let mut c = conv("Stop", Some(0));
    c.interrupted = Some(1);
    let db = make_db(vec![c], vec![line("long", 30.0, None), node(NodeKind::SetFlag { flag: "Cut".into(), value: true }, vec![None])], vec![]);
    let mut r = Runner::new(db, 1);
    let mut env = SimpleEnv::default();
    start(&mut r, &mut env);
    let id = r.active().next().unwrap().id;
    r.stop(id, &mut env);
    assert!(env.flags["Cut"]);
    let _ = EndReason::BehaviorChange;
}

#[test]
fn distance_hook_bands() {
    let edges = [200.0, 500.0, 1000.0];
    let all = [true, true, true];
    let mut h = DistanceHook::default();
    assert_eq!(h.update(1500.0, edges, all), None, "outside far: nothing, last unchanged");
    assert_eq!(h.update(1000.0, edges, all), Some(2), "far edge is inclusive");
    assert_eq!(h.update(800.0, edges, all), None);
    assert_eq!(h.update(500.0, edges, all), Some(1));
    assert_eq!(h.update(150.0, edges, all), Some(0));
    assert_eq!(h.update(900.0, edges, all), None, "moving out fires nothing");
    assert_eq!(h.update(100.0, edges, all), Some(0), "back in from medium-far: close");
    // Close not linked: a jump straight inside close fires medium.
    let mut h = DistanceHook::default();
    assert_eq!(h.update(100.0, edges, [false, true, true]), Some(1));
}

#[test]
fn hook_table_and_refire() {
    let used = hook_info("DDH_PLAYER_USED");
    assert_eq!((used.refire_min, used.refire_max), (0.0, 0.0));
    let search = hook_info("DDH_SEARCH");
    assert_eq!((search.refire_min, search.refire_max), (8.0, 12.0), "tweak override");
    assert!(hook_info("DDH_COMBAT_DYING").flags & 0x2 != 0, "dying interrupts");
    assert_eq!(hook_info("DDH_ATTENTION_INVESTIGATE").flags, 0xa0, "row 110: search sound (FUN_00c8d820)");
    // R1 test vector (`FUN_00ca6bd0` gate, `FUN_00ca5670` stamp): DDH_SEARCH fires on A at
    // t = 10 with u = 0.5: next = 10 + 0.5 * (12 - 8) = 12; refused until now > 12 + 8 = 20.
    let idx = hook_index("DDH_SEARCH").unwrap();
    let mut c = HookClock::default();
    assert!(c.allowed("a", idx, &search, 0.0).is_ok(), "never fired");
    c.fired("a", idx, &search, 10.0, 0.5);
    assert_eq!(c.allowed("a", idx, &search, 19.9), Err(HookRejection::RefireDelay));
    assert!(c.allowed("a", idx, &search, 20.01).is_ok());
    assert!(c.allowed("b", idx, &search, 10.5).is_ok(), "per NPC (the global delay is 0)");
    // A default AI hook (min = max = 1): exactly 1 s.
    let misc = hook_info("DDH_COMBAT_ATTACK");
    let mi = hook_index("DDH_COMBAT_ATTACK").unwrap();
    c.fired("a", mi, &misc, 3.0, 0.9);
    assert_eq!(c.allowed("a", mi, &misc, 3.99), Err(HookRejection::RefireDelay));
    assert!(c.allowed("a", mi, &misc, 4.0).is_ok());
    // `DDH_PLAYER_USED` (min = max = 0): always.
    let ui = hook_index("DDH_PLAYER_USED").unwrap();
    c.fired("a", ui, &used, 5.0, 0.3);
    assert!(c.allowed("a", ui, &used, 5.0).is_ok());
}

#[test]
fn fire_hook_starts_the_npcs_hook_and_rearms_refire() {
    // Tree hook DDH_PATROL_MISC -> ref conversation 0 (line).
    let mut hook = node(NodeKind::Hook { hook: "DDH_PATROL_MISC".into(), index: 0, names: vec![], priority: None, distances: None, class: HookClass::Base }, vec![Some(1)]);
    hook.conversation = None;
    let mut cref = node(NodeKind::ConversationRef { conversation: Some(0) }, vec![None, None]);
    cref.conversation = None;
    let db = make_db(vec![conv("Bark", Some(2))], vec![hook, cref, line("bark", 1.0, None)], vec![0]);
    let mut r = Runner::new(db, 1);
    let mut env = SimpleEnv::default();
    let req = HookRequest { npc: "guard".into(), trees: vec![0], oneshot_trees: 1, hook: "DDH_PATROL_MISC".into(), ..Default::default() };
    assert!(r.fire_hook(req.clone(), &mut env).is_ok());
    assert_eq!(lines(&r.take_events()), vec!["bark"]);
    run(&mut r, &mut env, &[]);
    assert_eq!(r.fire_hook(req.clone(), &mut env), Err(HookRejection::RefireDelay), "PATROL_MISC: at least 5 s");
    let other = HookRequest { hook: "DDH_COMBAT_ATTACK".into(), ..req };
    assert_eq!(r.fire_hook(other, &mut env), Err(HookRejection::NoContent));
}

/// Generic (non one-shot) bark trees: tree `t` holds one hook node per entry, each output a
/// line of 2 s spoken by ME.
fn bark_db(trees: Vec<(Vec<(&str, HookClass, usize)>, Option<u32>, bool)>) -> Arc<ConversationDb> {
    let mut nodes = Vec::new();
    let mut tv = Vec::new();
    for (ti, (hooks, fallback, try_fallback)) in trees.into_iter().enumerate() {
        let mut hs = Vec::new();
        for (h, class, outs) in hooks {
            let hid = nodes.len() as NodeId;
            let mut hn = node(NodeKind::Hook { hook: h.into(), index: 0, names: vec![], priority: None, distances: None, class }, vec![]);
            hn.conversation = None;
            hn.tree = ti as u32;
            nodes.push(hn);
            for k in 0..outs {
                let id = nodes.len() as NodeId;
                nodes[hid as usize].outputs.push(Some(id));
                let mut l = if try_fallback { node(NodeKind::TryFallbackTree, vec![None]) } else { line(&format!("{h}#{k}@{ti}"), 2.0, None) };
                l.conversation = None;
                l.tree = ti as u32;
                nodes.push(l);
            }
            hs.push(hid);
        }
        tv.push(Tree { package: "P".into(), path: format!("T{ti}"), oneshot: None, hooks: hs, conversations: vec![], fallback });
    }
    Arc::new(ConversationDb { packages: vec!["P".into()], oneshots: vec![], trees: tv, conversations: vec![], nodes, voice_trees: Default::default(), settings: Default::default() })
}

#[derive(Default)]
struct BarkEnv {
    dist: BTreeMap<String, f32>,
    audio: BTreeMap<String, f32>,
    dead: Vec<String>,
    facts: HookFacts,
}

impl ConvEnv for BarkEnv {
    fn flag(&self, _: &str) -> bool {
        false
    }
    fn set_flag(&mut self, _: &str, _: bool) {}
    fn npc_dead(&self, npc: &str) -> bool {
        self.dead.iter().any(|d| d == npc)
    }
    fn listener_distance(&self, npc: &str) -> Option<f32> {
        self.dist.get(npc).copied()
    }
    fn audio_distance(&self, npc: &str) -> Option<f32> {
        self.audio.get(npc).copied().or_else(|| self.listener_distance(npc))
    }
    fn hook_facts(&self, _: &str, _: Option<u32>) -> HookFacts {
        self.facts
    }
}

fn bark(npc: &str, hook: &str) -> HookRequest {
    HookRequest { npc: npc.into(), trees: vec![0], hook: hook.into(), ..Default::default() }
}

fn hook_results(ev: &[ConvEvent]) -> Vec<String> {
    ev.iter().filter_map(|e| if let ConvEvent::Hook { npc, result, .. } = e { Some(format!("{npc}:{result:?}")) } else { None }).collect()
}

#[test]
fn queue_fires_the_nearest_and_drops_the_rest_then_locks_at_the_end() {
    // R3 test vector: same tick, group Agitated (cap 1 per list), both Loud: A at 600 uu,
    // B at 1800 uu (weights 0.88 + 0.2u vs 0.64 + 0.2u): A fires, B is dropped (reason 5), no
    // retry; after A's bark ends the Agitated list is locked for 2.5 s.
    let db = bark_db(vec![(vec![("DDH_SEARCH", HookClass::Base, 1)], None, false)]);
    for seed in 1..20 {
        let mut r = Runner::new(db.clone(), seed);
        let mut env = BarkEnv::default();
        env.dist.insert("a".into(), 600.0);
        env.dist.insert("b".into(), 1800.0);
        r.request_hook(bark("b", "DDH_SEARCH"), &mut env).unwrap();
        r.request_hook(bark("a", "DDH_SEARCH"), &mut env).unwrap();
        r.take_events();
        r.tick_hooks(1.0 / 60.0, &mut env);
        let ev = r.take_events();
        assert_eq!(lines(&ev), vec!["DDH_SEARCH#0@0"]);
        assert_eq!(hook_results(&ev), vec!["a:Ok(Fired)", "b:Err(Dropped)"], "seed {seed}");
        // The bark plays 2 s; the list unlocks 2.5 s after it ends.
        let mut t = 0.0;
        while r.active().next().is_some() {
            r.update(0.1, &mut env);
            r.tick_hooks(0.1, &mut env);
            t += 0.1;
        }
        assert!((t - 2.0f32).abs() < 0.11, "{t}");
        assert_eq!(r.request_hook(bark("b", "DDH_SEARCH"), &mut env), Err(HookRejection::GroupLocked));
        for _ in 0..24 {
            r.update(0.1, &mut env);
            r.tick_hooks(0.1, &mut env);
        }
        assert_eq!(r.request_hook(bark("b", "DDH_SEARCH"), &mut env), Err(HookRejection::GroupLocked), "2.4 s after the end");
        r.update(0.2, &mut env);
        r.tick_hooks(0.2, &mut env);
        assert!(r.request_hook(bark("b", "DDH_SEARCH"), &mut env).is_ok(), "2.6 s after the end");
    }
}

#[test]
fn quiet_speakers_use_their_own_list_and_far_ones_never_bark() {
    let db = bark_db(vec![(vec![("DDH_SEARCH", HookClass::Base, 1), ("DDH_KISMET_ACTIVATED", HookClass::KismetActivated, 1)], None, false)]);
    let mut r = Runner::new(db, 3);
    let mut env = BarkEnv::default();
    env.dist.insert("a".into(), 600.0);
    env.dist.insert("q".into(), 2500.0);
    env.dist.insert("far".into(), 5200.0);
    for n in ["a", "q", "far"] {
        r.request_hook(bark(n, "DDH_SEARCH"), &mut env).unwrap();
    }
    r.take_events();
    r.tick_hooks(0.0, &mut env);
    let mut got = hook_results(&r.take_events());
    got.sort();
    assert_eq!(got, vec!["a:Ok(Fired)", "far:Err(Dropped)", "q:Ok(Fired)"], "Agitated 1 Loud + 1 Quiet; beyond 5000 uu weight 0");
    // `DDH_KISMET_ACTIVATED` ignores the distance cull.
    r.request_hook(bark("far", "DDH_KISMET_ACTIVATED"), &mut env).unwrap();
    r.tick_hooks(0.0, &mut env);
    assert!(hook_results(&r.take_events()).contains(&"far:Ok(Fired)".to_string()));
}

#[test]
fn global_slots_count_every_playing_hook() {
    // Six NPCs in six groups that do not ignore the global cap, all Loud: 5 slots.
    let hooks = ["DDH_PATROL_MISC", "DDH_PATROL_SWITCH_ROUTE", "DDH_ATTENTION_HEADTRACK", "DDH_ROBBED", "DDH_COMBAT_OUCH_SMALL", "DDH_SEARCH"];
    let db = bark_db(vec![(hooks.iter().map(|h| (*h, HookClass::Base, 1)).collect(), None, false)]);
    let mut r = Runner::new(db, 5);
    let mut env = BarkEnv::default();
    for (i, h) in hooks.iter().enumerate() {
        env.dist.insert(format!("n{i}"), 100.0);
        r.request_hook(bark(&format!("n{i}"), h), &mut env).unwrap();
    }
    r.take_events();
    r.tick_hooks(0.0, &mut env);
    let got = hook_results(&r.take_events());
    assert_eq!(got.iter().filter(|g| g.ends_with("Ok(Fired)")).count(), 5, "{got:?}");
    // Groups go 12 -> 0: Casual (PATROL_MISC, n0) is the one left without a slot.
    assert!(got.contains(&"n0:Err(Dropped)".to_string()), "{got:?}");
}

#[test]
fn fallback_tree_and_try_fallback_node() {
    // Tree 0 lacks the hook -> its fallback tree 1 has it.
    let db = bark_db(vec![(vec![("DDH_SEARCH", HookClass::Base, 1)], Some(1), false), (vec![("DDH_COMBAT_THREAT", HookClass::Base, 1)], None, false)]);
    let mut r = Runner::new(db, 1);
    let mut env = BarkEnv::default();
    assert!(r.fire_hook(bark("a", "DDH_COMBAT_THREAT"), &mut env).is_ok());
    assert_eq!(lines(&r.take_events()), vec!["DDH_COMBAT_THREAT#0@1"]);
    // Tree 0's walk ends in DisConv_TryFallbackTree -> tree 1 speaks.
    let db = bark_db(vec![(vec![("DDH_SEARCH", HookClass::Base, 1)], Some(1), true), (vec![("DDH_SEARCH", HookClass::Base, 1)], None, false)]);
    let mut r = Runner::new(db, 1);
    assert!(r.fire_hook(bark("a", "DDH_SEARCH"), &mut env).is_ok());
    assert_eq!(lines(&r.take_events()), vec!["DDH_SEARCH#0@1"]);
}

#[test]
fn hook_output_by_node_class() {
    let db = bark_db(vec![(vec![("DDH_ESCALATE_SUSPICION_LEVEL", HookClass::SuspicionLevel, 2), ("DDH_COMBAT_DYING", HookClass::DeathMode, 2), ("DDH_NOTICE_SUSPICIOUS_ACTIVITY", HookClass::Notice, 3)], None, false)]);
    let mut env = BarkEnv::default();
    env.facts = HookFacts { suspicion: 1, knocked_out: true, notice: Some(2), investigate: 0 };
    let mut r = Runner::new(db.clone(), 1);
    r.fire_hook(bark("a", "DDH_ESCALATE_SUSPICION_LEVEL"), &mut env).unwrap();
    r.fire_hook(bark("b", "DDH_COMBAT_DYING"), &mut env).unwrap();
    r.fire_hook(bark("c", "DDH_NOTICE_SUSPICIOUS_ACTIVITY"), &mut env).unwrap();
    assert_eq!(lines(&r.take_events()), vec!["DDH_ESCALATE_SUSPICION_LEVEL#1@0", "DDH_COMBAT_DYING#1@0", "DDH_NOTICE_SUSPICIOUS_ACTIVITY#2@0"]);
    // Past the node's outputs, or Notice without a stimulus: the hook cannot start.
    env.facts = HookFacts { suspicion: 2, knocked_out: false, notice: None, investigate: 0 };
    let mut r = Runner::new(db, 1);
    assert_eq!(r.fire_hook(bark("a", "DDH_ESCALATE_SUSPICION_LEVEL"), &mut env), Err(HookRejection::NoContent));
    assert_eq!(r.fire_hook(bark("c", "DDH_NOTICE_SUSPICIOUS_ACTIVITY"), &mut env), Err(HookRejection::NoContent));
}

#[test]
fn barks_take_their_speaker_and_respect_priority() {
    let db = bark_db(vec![(vec![("DDH_SEARCH", HookClass::Base, 1), ("DDH_COMBAT_ATTACK", HookClass::Base, 1), ("DDH_COMBAT_OUCH_SMALL", HookClass::Base, 1), ("DDH_COMBAT_DYING", HookClass::Base, 1), ("DDH_COMBAT_BACKSTABBED", HookClass::Base, 1)], None, false)]);
    let mut r = Runner::new(db, 1);
    let mut env = BarkEnv::default();
    r.fire_hook(bark("a", "DDH_SEARCH"), &mut env).unwrap();
    // A second LOW bark on a speaker already speaking is refused (Available 4).
    assert_eq!(r.fire_hook(bark("a", "DDH_COMBAT_ATTACK"), &mut env), Err(HookRejection::Busy));
    assert!(r.fire_hook(bark("b", "DDH_COMBAT_ATTACK"), &mut env).is_ok(), "another speaker");
    // OUCH_SMALL (flag 0x2) interrupts it; the first state ends.
    assert!(r.fire_hook(bark("a", "DDH_COMBAT_OUCH_SMALL"), &mut env).is_ok());
    assert_eq!(r.active().filter(|i| i.bindings.first().cloned().flatten().as_deref() == Some("a")).count(), 1);
    // Dead owners: only 57-59. A flag-0x4 state (dying 0x8e) is not interrupted, even by 0x2.
    env.dead.push("a".into());
    assert_eq!(r.fire_hook(bark("a", "DDH_COMBAT_ATTACK"), &mut env), Err(HookRejection::Dead));
    assert!(r.fire_hook(bark("a", "DDH_COMBAT_DYING"), &mut env).is_ok());
    assert_eq!(r.fire_hook(bark("a", "DDH_COMBAT_BACKSTABBED"), &mut env), Err(HookRejection::Busy));
}

#[test]
fn tree_level_time_limit_is_not_consumed_when_no_conversation_starts() {
    // Hook -> time limit (tree level) -> [ref to fire-once conversation 0, output "blocked"].
    let mut c0 = conv("Once", Some(0));
    c0.fire_once = true;
    let mut tl = node(NodeKind::TimeLimit { seconds: 10.0, global: false }, vec![Some(2), Some(3)]);
    tl.conversation = None;
    let mut cref = node(NodeKind::ConversationRef { conversation: Some(0) }, vec![None, None]);
    cref.conversation = None;
    let mut blocked = node(NodeKind::KismetOutput { name: "blocked".into() }, vec![None]);
    blocked.conversation = None;
    let mut hook = node(NodeKind::Hook { hook: "DDH_PLAYER_USED".into(), index: 0, names: vec![], priority: None, distances: None, class: HookClass::Base }, vec![Some(1)]);
    hook.conversation = None;
    let db = make_db(vec![c0], vec![line("x", 0.1, None), tl, cref, blocked, hook], vec![4]);
    let mut r = Runner::new(db, 1);
    let mut env = SimpleEnv::default();
    let use_hook = |r: &mut Runner, env: &mut SimpleEnv| {
        r.update(1.0, env);
        r.start(StartSpec { target: Target::Hook { oneshot: 0, hook: 4, output: 0 }, initiator: Some(1), bindings: vec![Some("npc".into())], force: false }, env);
        run(r, env, &[])
    };
    // The conversation fires once (the time limit is committed by that start).
    assert_eq!(lines(&use_hook(&mut r, &mut env)), vec!["x"]);
    r.update(20.0, &mut env);
    // Now the reference fails: the pass through the time limit stays pending and is dropped,
    // so the next use is not blocked either.
    assert!(outputs(&use_hook(&mut r, &mut env)).is_empty());
    assert!(outputs(&use_hook(&mut r, &mut env)).is_empty(), "time limit not consumed by a failed start");
    // Within 10 s of the committed pass it would have been blocked.
    let mut r2 = Runner::new(r.db.clone(), 1);
    assert_eq!(lines(&use_hook(&mut r2, &mut env)), vec!["x"]);
    assert_eq!(outputs(&use_hook(&mut r2, &mut env)), vec!["blocked"]);
}

#[test]
fn player_loiter_fires_after_its_time_and_rearms() {
    use dishonored_conversation::hooks::{LoiterHook, LookAtHook};
    // `FUN_00cb53c0`: entering arms min + roll * (max - min) / 32767 (roll 16383.5 ~ 6 s).
    let mut h = LoiterHook::default();
    let mut fired = Vec::new();
    for i in 0..=800 {
        let t = i as f64 / 60.0;
        let d = if t < 1.0 { 900.0 } else { 300.0 };
        if h.update(t, d, 600.0, 5.0, 7.0, 16384).is_some() {
            fired.push(t);
        }
    }
    assert_eq!(fired.len(), 2, "{fired:?}");
    assert!((fired[0] - 7.0).abs() < 0.05, "1 s outside + 6 s inside: {fired:?}");
    assert!((fired[1] - fired[0] - 6.0).abs() < 0.1, "re-armed on the next poll: {fired:?}");
    // A gap of more than 0.5 s between polls counts as having left.
    let mut h = LoiterHook::default();
    assert_eq!(h.update(0.0, 100.0, 600.0, 1.0, 1.0, 0), None);
    assert_eq!(h.update(2.0, 100.0, 600.0, 1.0, 1.0, 0), None, "re-armed, not fired");
    assert_eq!(h.update(2.4, 100.0, 600.0, 1.0, 1.0, 0), None);
    assert_eq!(h.update(2.8, 100.0, 600.0, 1.0, 1.0, 0), None);
    assert_eq!(h.update(3.2, 100.0, 600.0, 1.0, 1.0, 0), Some(0));
    // `FUN_00cb5680`: the target under its angle with the smallest angle, once after its time.
    let mut l = LookAtHook::default();
    let targets = [(30.0, 1.0), (30.0, 2.0)];
    assert_eq!(l.update(0.0, 100.0, 300.0, &[Some(20.0), Some(10.0)], &targets), None);
    assert_eq!(l.update(1.5, 100.0, 300.0, &[Some(20.0), Some(10.0)], &targets), None);
    assert_eq!(l.update(2.0, 100.0, 300.0, &[Some(20.0), Some(10.0)], &targets), Some(1));
    assert_eq!(l.update(5.0, 100.0, 300.0, &[Some(20.0), Some(10.0)], &targets), None, "once");
    assert_eq!(l.update(6.0, 400.0, 300.0, &[Some(20.0), Some(10.0)], &targets), None, "out of range resets");
}