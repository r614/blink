//! Runner semantics on a synthetic graph (no install needed).

use dishonored_conversation::model::*;
use dishonored_conversation::runner::{ConvEvent, Runner, SimpleEnv, StartSpec};
use std::sync::Arc;

fn line(text: &str, len: f32, next: Option<NodeId>) -> Node {
    node(NodeKind::Line(Line { guid: String::new(), text: text.into(), nonword: false, spoken_to: 2, main_subtitle: true, voice_radius: 3000.0, end_delay: 0.0, label: Some(text.into()), look: None, vo: Some(VoRef { event: format!("Play_{text}"), bank: "Bk_X".into() }), vo_length: Some(len) }), vec![next])
}

fn node(kind: NodeKind, outputs: Vec<Option<NodeId>>) -> Node {
    Node { path: String::new(), class: String::new(), tree: 0, conversation: Some(0), speaker: 0, outputs, kind }
}

/// 0 check flag F -> [1 "a", 2 seen-label "a"]; 2 -> [3 choice, 4 output]; 3 -> [5 "b", 6 soiree];
/// 6 soiree -> [finished: 7 set flag G, started: 8 "c"].
fn db() -> Arc<ConversationDb> {
    let nodes = vec![
        node(NodeKind::CheckFlag { flag: "F".into() }, vec![Some(1), Some(2)]),
        line("a", 1.0, None),
        node(NodeKind::SeenLabel { oneshot: Some("OS".into()), label: "a".into() }, vec![Some(3), Some(4)]),
        node(NodeKind::Choice { options: vec!["one".into(), "two".into()] }, vec![Some(5), Some(6)]),
        node(NodeKind::KismetOutput { name: "Heard_A".into() }, vec![None]),
        line("b", 2.0, None),
        node(NodeKind::Soiree { guid: "G".into(), matinee: None }, vec![Some(7), Some(8)]),
        node(NodeKind::SetFlag { flag: "G".into(), value: true }, vec![None]),
        line("c", 0.5, None),
    ];
    let conv = Conversation {
        path: "C".into(),
        name: "DisConversation_0".into(),
        tree: 0,
        label: "Test".into(),
        description: String::new(),
        speakers: vec![SpeakerSlot::Me, SpeakerSlot::Initiator, SpeakerSlot::Player],
        speaker_nicks: vec![Some("Tester".into()), None, None],
        speaker_gates: vec![(false, false, false); 3],
        start: Some(0),
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
    };
    Arc::new(ConversationDb {
        packages: vec!["P".into()],
        oneshots: vec![OneShot { package: "P".into(), name: "OS".into(), path: "OS".into(), tree: Some(0), speakers: vec![OneShotSpeaker { nick: "Me".into(), actor: Some("Spawner_1".into()), voice: None }], location: [0.0; 3] }],
        trees: vec![Tree { package: "P".into(), path: "T".into(), oneshot: Some(0), hooks: vec![], conversations: vec![0] }],
        conversations: vec![conv],
        nodes,
    })
}

fn run(r: &mut Runner, env: &mut SimpleEnv, choices: &[usize]) -> Vec<ConvEvent> {
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

fn start(r: &mut Runner, env: &mut SimpleEnv) {
    r.start(StartSpec { target: Target::Conversation { oneshot: Some(0), conversation: 0 }, initiator: Some(1), bindings: vec![Some("npc".into())], force: true }, env).expect("starts");
}

#[test]
fn flag_false_takes_output_0_and_line_holds_for_its_length() {
    let db = db();
    let mut r = Runner::new(db, 1);
    let mut env = SimpleEnv::default();
    start(&mut r, &mut env);
    let ev = r.take_events();
    assert!(matches!(&ev[1], ConvEvent::Line { text, duration, speaker, .. } if text == "a" && (*duration - 1.0).abs() < 1e-6 && speaker.name == "Tester" && speaker.npc.as_deref() == Some("npc")));
    r.update(0.9, &mut env);
    assert!(r.active().next().is_some(), "still speaking");
    r.update(0.2, &mut env);
    assert!(r.active().next().is_none(), "ended after the clip");
    assert!(r.heard("OS", "a"));
}

#[test]
fn flag_true_then_label_seen_reaches_output_and_choice_and_soiree_order() {
    let db = db();
    let mut r = Runner::new(db, 1);
    let mut env = SimpleEnv::default();
    env.flags.insert("F".into(), true);
    // Label "a" not heard yet: the choice (seen-label output 0).
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
    // Soiree: "started" branch (line c) first, "finished" branch (set G) after it.
    assert_eq!(kinds, vec!["choice", "chose1", "soiree", "c", "flagG"]);
    assert!(env.flags["G"]);
}

#[test]
fn heard_label_takes_output_1() {
    let db = db();
    let mut r = Runner::new(db, 1);
    let mut env = SimpleEnv::default();
    start(&mut r, &mut env);
    run(&mut r, &mut env, &[]);
    env.flags.insert("F".into(), true);
    start(&mut r, &mut env);
    let ev = run(&mut r, &mut env, &[]);
    assert!(ev.iter().any(|e| matches!(e, ConvEvent::Output { name, .. } if name == "Heard_A")));
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
