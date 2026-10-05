//! Gameplay session: an offline match (local authoritative sim + test bots) or an online
//! connection (predicted NetClient). Both feed the same `SessionView` for presentation.

use crate::input_map::LocalInput;
use bevy::prelude::*;
use dishonored_bots::{Bot, BotStyle};
use dishonored_content::MatchContent;
use dishonored_multiplayer::MatchView;
use dishonored_multiplayer::contracts::{ContractsRules, TargetInfo};
use dishonored_multiplayer::ffa::{FfaRules, SpawnPoint};
use dishonored_multiplayer::game::{CrowdSource, Game, GameSetup, crowd_center, default_crowd};
use dishonored_net::client::{ConnState, NetClient};
use dishonored_sim::events::SimEvent;
use dishonored_sim::input::PlayerInput;
use dishonored_sim::state::{PlayerId, PlayerState, ProjectileState};
use std::sync::Arc;

pub const LOCAL_ID: PlayerId = 1;

pub enum SessionKind {
    /// `spectator`: when set, the local slot is also driven by a bot (watch mode).
    Offline { game: Game, bots: Vec<Bot>, spectator: Option<Bot> },
    Online { client: NetClient },
}

#[derive(Resource)]
pub struct Session {
    pub kind: SessionKind,
    pub content: Arc<MatchContent>,
    /// Input actually applied on the last fixed tick (for HUD / harness).
    pub last_input: PlayerInput,
}

/// Presentation snapshot of the gameplay world (Dishonored units).
#[derive(Resource, Default, Clone)]
pub struct SessionView {
    pub me: Option<PlayerState>,
    pub others: Vec<PlayerState>,
    pub projectiles: Vec<ProjectileState>,
    pub matchv: MatchView,
    pub tick: u64,
    pub connected: bool,
    pub status: String,
    pub rtt_ms: f32,
    pub prediction_error: f32,
    pub correction: f32,
    pub dark_vision_reveals: Vec<PlayerId>,
    /// Recent events for presentation (kill feed, hit markers).
    pub feed: Vec<String>,
    pub respawn_in: Option<f32>,
    /// Contracts private info for the local player.
    pub target_info: Option<TargetInfo>,
}

impl Session {
    pub fn offline(content: Arc<MatchContent>, bots: usize, spectate: bool, contracts: bool) -> Self {
        let setup = if contracts {
            let rules = ContractsRules { warmup_time: 2.0, min_players: 2, crowd: Some(default_crowd(crowd_center(&content.spawns.iter().map(|s| (s.position, s.yaw)).collect::<Vec<_>>()))), ..Default::default() };
            let pts: Vec<(glam::Vec3, f32)> = content.spawns.iter().map(|s| (s.position, s.yaw)).collect();
            GameSetup::contracts(rules, pts, Some(CrowdSource { points: &content.ai_points, guard_spawns: &content.guard_spawns }), &content.collision)
        } else {
            let spawns: Vec<SpawnPoint> = content.spawns.iter().map(|s| SpawnPoint { position: s.position, yaw: s.yaw }).collect();
            GameSetup::Ffa { rules: FfaRules { warmup_time: 1.0, min_players: 1, ..Default::default() }, spawns }
        };
        let mut game = setup.build(&content.sim);
        let arch = |id: u32| dishonored_net::server::PLAYER_ARCHETYPES[id as usize % 3];
        game.add_player(LOCAL_ID, if spectate { "bot0" } else { "you" }, arch(LOCAL_ID), &content.collision, &content.sim);
        let nav = std::sync::Arc::new(dishonored_bots::NavGraph::build(&content.collision, content.ai_points.iter().map(|p| p.0).chain(content.spawns.iter().map(|s| s.position))));
        let mut bot_list = Vec::new();
        for i in 0..bots {
            let id = LOCAL_ID + 1 + i as u32;
            let style = match i % 3 {
                0 => BotStyle::Duelist,
                1 => BotStyle::Gunner,
                _ => BotStyle::Duelist,
            };
            game.add_player(id, &format!("bot{}", i + 1), arch(id), &content.collision, &content.sim);
            bot_list.push(Bot::new(id, style, 1000 + i as u64).with_nav(nav.clone()));
        }
        let spectator = spectate.then(|| Bot::new(LOCAL_ID, BotStyle::Duelist, 999).with_nav(nav.clone()));
        Self { kind: SessionKind::Offline { game, bots: bot_list, spectator }, content, last_input: PlayerInput::default() }
    }

    pub fn online(content: Arc<MatchContent>, client: NetClient) -> Self {
        Self { kind: SessionKind::Online { client }, content, last_input: PlayerInput::default() }
    }

    pub fn local_id(&self) -> Option<PlayerId> {
        match &self.kind {
            SessionKind::Offline { .. } => Some(LOCAL_ID),
            SessionKind::Online { client } => client.player_id,
        }
    }
}

fn describe(e: &SimEvent, names: &MatchView) -> Option<String> {
    let name = |id: PlayerId| names.scores.iter().find(|s| s.id == id).map(|s| s.name.clone()).unwrap_or(format!("#{id}"));
    match e {
        SimEvent::Death { victim, killer: Some(k), cause } if k != victim => Some(format!("{} killed {} ({cause:?})", name(*k), name(*victim))),
        SimEvent::Death { victim, cause, .. } => Some(format!("{} died ({cause:?})", name(*victim))),
        SimEvent::SwordParried { attacker, defender } => Some(format!("{} parried {}", name(*defender), name(*attacker))),
        _ => None,
    }
}

/// One fixed simulation tick (60 Hz).
pub fn fixed_tick(mut session: ResMut<Session>, mut local: ResMut<LocalInput>, mut view: ResMut<SessionView>) {
    let input = local.take_tick_input();
    let session = &mut *session;
    session.last_input = input;
    let content = session.content.clone();
    let world = &content.collision;
    let sim = &content.sim;
    match &mut session.kind {
        SessionKind::Offline { game, bots, spectator } => {
            let snapshot: Vec<PlayerState> = game.world().players.clone();
            let contracts = matches!(game, Game::Contracts(_));
            let mut think = |b: &mut Bot, me: &PlayerState| -> PlayerInput {
                if contracts {
                    let target = game.target_info(b.id).and_then(|t| t.target);
                    b.think_contract(me, &snapshot, world, target)
                } else {
                    b.think(me, &snapshot, world)
                }
            };
            let local_input = match (spectator.as_mut(), snapshot.iter().find(|p| p.id == LOCAL_ID)) {
                (Some(b), Some(me)) => think(b, me),
                _ => input,
            };
            let mut inputs = vec![(LOCAL_ID, local_input)];
            for b in bots.iter_mut() {
                if let Some(me) = snapshot.iter().find(|p| p.id == b.id) {
                    inputs.push((b.id, think(b, me)));
                }
            }
            let out = game.step(&inputs, world, sim);
            for e in &out.events {
                if let Some(s) = describe(e, game.view()) {
                    view.feed.push(s);
                }
            }
            let w = game.world();
            view.me = w.player(LOCAL_ID).copied();
            view.others = w.players.iter().filter(|p| p.id != LOCAL_ID).copied().collect();
            view.projectiles = w.projectiles.clone();
            view.tick = w.tick;
            view.dark_vision_reveals = dishonored_sim::left_hand::dark_vision_reveals(w, LOCAL_ID, &sim.tuning);
            view.matchv = game.view().clone();
            view.target_info = game.target_info(LOCAL_ID);
            view.connected = true;
            let npcs = view.others.iter().filter(|p| p.kind != dishonored_sim::state::PawnKind::Human).count();
            view.status = if spectator.is_some() { format!("spectating {} bots, {npcs} NPCs (N: next, V: camera)", bots.len() + 1) } else { format!("offline vs {} bots, {npcs} NPCs", bots.len()) };
            view.respawn_in = game.respawn_time_left(LOCAL_ID);
        }
        SessionKind::Online { client } => {
            client.poll(world, sim);
            client.send_input(input, world, sim);
            let me = client.player_id;
            for (_, e) in client.events.drain(..) {
                if let Some(s) = describe(&e, &client.matchv) {
                    view.feed.push(s);
                }
            }
            view.me = client.me().copied();
            view.others = client.remote_players(6.0);
            view.projectiles = client.predicted.projectiles.clone();
            view.matchv = client.matchv.clone();
            view.tick = client.metrics.server_tick;
            view.connected = client.state == ConnState::Connected;
            view.status = match client.state {
                ConnState::Connecting => format!("connecting to {}...", client.server),
                ConnState::Connected => format!("online {} ({})", client.server, client.map),
                ConnState::Rejected => format!("rejected: {}", client.reject_reason.clone().unwrap_or_default()),
            };
            view.rtt_ms = client.metrics.rtt_ms;
            view.prediction_error = client.metrics.prediction_error;
            view.correction = client.metrics.correction;
            view.dark_vision_reveals = me.map(|id| dishonored_sim::left_hand::dark_vision_reveals(&client.predicted, id, &sim.tuning)).unwrap_or_default();
            view.respawn_in = None;
            view.target_info = client.target_info.clone();
        }
    }
    let n = view.feed.len();
    if n > 6 {
        view.feed.drain(0..n - 6);
    }
}
