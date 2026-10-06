//! The original music manager (`DisGlobalMusicManager`, native, decompiled from the executable;
//! research/dishonored/AUDIO_FORMAT.md "Music manager"): a state machine with Exploration,
//! Suspense, RatAttack, Combat and Chase, each posting the AkEvent of the map's
//! `DisTweaks_MusicEvents` when entered (Combat and Chase also when their level changes).
//!
//! State selection (`FUN_00c52a40`):
//! 1. combat intensity > 0 -> Combat;
//! 2. else attacked by rats -> RatAttack;
//! 3. else no living hostile NPC within `m_fEnemyRangeForSuspense` of the player -> Exploration;
//! 4. else, coming from Combat or RatAttack -> Chase (timer `m_fChaseTime`), otherwise Suspense.
//!
//! Exploration, Suspense and RatAttack re-select every tick; Combat re-selects every tick and then
//! updates its level from the intensity (`m_fCombatIntensity[1..3]` thresholds, `FUN_00c486d0`);
//! Chase re-selects only once its timer ran out or combat / rats resume, counts the timer down and
//! picks its level from the nearest hostile (`m_fEnemyRangeForChase[0]` near, `[1]` far; none in
//! range ends the chase: `FUN_00c48620`).
//!
//! Inputs are plain data, so the client runs it deterministically from the replicated view.

use ue3_package::properties::Value;
use ue3_package::{ObjRef, Package};

#[derive(Debug, Clone, Default)]
pub struct MusicTweaks {
    pub name: String,
    /// `m_fCombatIntensityContribution[EDisCombatMusicIntensityObjectType]` (None, Rat,
    /// Wolfhound, Thug, Guard, EliteGuard, Tallboy, Weeper).
    pub contribution: [f32; 8],
    /// `m_fCombatIntensity[Low, Medium, High, Tallboy]`.
    pub intensity: [f32; 4],
    pub suspense_range: f32,
    pub chase_time: f32,
    /// `m_fEnemyRangeForChase[EnemyNear, EnemyFar]`.
    pub chase_range: [f32; 2],
    pub start_exploration: Option<String>,
    pub start_suspense: Option<String>,
    pub start_rat_attack: Option<String>,
    pub start_combat: [Option<String>; 4],
    pub start_chase: [Option<String>; 2],
}

impl MusicTweaks {
    /// Read a `DisTweaks_MusicEvents` export (unset values keep `self`).
    pub fn read(&mut self, pkg: &Package, export: usize) {
        let Ok(h) = pkg.read_object(export) else { return };
        self.name = pkg.obj_path(ObjRef::Export(export));
        for p in &h.properties {
            let i = p.array_index as usize;
            let f = p.as_float();
            let ev = || match &p.value {
                Value::Object(o) if *o != 0 => Some(pkg.obj_name(ObjRef::from_index(*o))),
                _ => None,
            };
            match p.name.as_str() {
                "m_fCombatIntensityContribution" if i < 8 => self.contribution[i] = f.unwrap_or(0.0),
                "m_fCombatIntensity" if i < 4 => self.intensity[i] = f.unwrap_or(0.0),
                "m_fEnemyRangeForSuspense" => self.suspense_range = f.unwrap_or(0.0),
                "m_fChaseTime" => self.chase_time = f.unwrap_or(0.0),
                "m_fEnemyRangeForChase" if i < 2 => self.chase_range[i] = f.unwrap_or(0.0),
                "m_pStartExploration" => self.start_exploration = ev(),
                "m_pStartSuspense" => self.start_suspense = ev(),
                "m_pStartRatAttack" => self.start_rat_attack = ev(),
                "m_pStartCombat" if i < 4 => self.start_combat[i] = ev(),
                "m_pStartChase" if i < 2 => self.start_chase[i] = ev(),
                _ => {}
            }
        }
    }

    /// The map's music tweaks: the `DisTweaks_MusicEvents` cooked into its packages (one per
    /// level), over the `Twk_DefaultMusicEvents` defaults of DishonoredGame.
    pub fn for_packages(base: Option<&Package>, map_pkgs: &[&Package]) -> Option<Self> {
        let mut t = MusicTweaks::default();
        let mut found = false;
        let scan = |pkg: &Package, t: &mut MusicTweaks, want_default: bool| -> bool {
            for i in 0..pkg.exports.len() {
                if pkg.export_class(i) != "DisTweaks_MusicEvents" {
                    continue;
                }
                let path = pkg.obj_path(ObjRef::Export(i));
                if path.starts_with("Default__") {
                    continue;
                }
                let is_default = path.ends_with("Twk_DefaultMusicEvents.Twk_DefaultMusicEvents");
                if is_default == want_default {
                    t.read(pkg, i);
                    return true;
                }
            }
            false
        };
        if let Some(b) = base {
            scan(b, &mut t, true);
        }
        for p in map_pkgs {
            if scan(p, &mut t, false) {
                found = true;
                break;
            }
        }
        found.then_some(t)
    }

    fn level_event(&self, s: MusicState, level: u8) -> Option<&str> {
        match s {
            MusicState::Exploration => self.start_exploration.as_deref(),
            MusicState::Suspense => self.start_suspense.as_deref(),
            MusicState::RatAttack => self.start_rat_attack.as_deref(),
            MusicState::Combat => self.start_combat.get(level as usize).and_then(|e| e.as_deref()),
            MusicState::Chase => self.start_chase.get(level as usize).and_then(|e| e.as_deref()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MusicState {
    #[default]
    Exploration,
    Suspense,
    RatAttack,
    Combat,
    Chase,
}

/// One NPC as the manager sees it.
#[derive(Debug, Clone, Copy)]
pub struct MusicNpc {
    pub pos: [f32; 3],
    pub alive: bool,
    /// Relationship to the player is hostile.
    pub hostile: bool,
    /// Fighting the player.
    pub combat: bool,
    /// `EDisCombatMusicIntensityObjectType`.
    pub music_type: u8,
}

#[derive(Debug, Clone, Default)]
pub struct MusicManager {
    pub tweaks: MusicTweaks,
    pub state: MusicState,
    started: bool,
    /// Combat: last intensity value and level; Chase: timer and level.
    value: f32,
    level: u8,
    pub intensity: f32,
    pub attacked_by_rats: bool,
    /// State changes (for the harness).
    pub transitions: Vec<(MusicState, u8)>,
}

fn within(player: [f32; 3], npcs: &[MusicNpc], r: f32) -> bool {
    npcs.iter().any(|n| {
        let d = [n.pos[0] - player[0], n.pos[1] - player[1], n.pos[2] - player[2]];
        n.alive && n.hostile && d[0] * d[0] + d[1] * d[1] + d[2] * d[2] <= r * r
    })
}

impl MusicManager {
    pub fn new(tweaks: MusicTweaks) -> Self {
        MusicManager { tweaks, ..Default::default() }
    }

    /// Combat intensity: the contribution of every NPC fighting the player (U-160: the native
    /// writer of `m_fCombatIntensity` was not found; this is the sum the tweak layout implies).
    pub fn compute_intensity(&self, npcs: &[MusicNpc]) -> f32 {
        npcs.iter().filter(|n| n.alive && n.combat).map(|n| self.tweaks.contribution.get(n.music_type as usize).copied().unwrap_or(0.0)).sum()
    }

    fn select(&self, player: [f32; 3], npcs: &[MusicNpc]) -> MusicState {
        if self.intensity > 0.0 {
            MusicState::Combat
        } else if self.attacked_by_rats {
            MusicState::RatAttack
        } else if !within(player, npcs, self.tweaks.suspense_range) {
            MusicState::Exploration
        } else if matches!(self.state, MusicState::Combat | MusicState::RatAttack) {
            MusicState::Chase
        } else {
            MusicState::Suspense
        }
    }

    fn enter(&mut self, s: MusicState, posts: &mut Vec<String>) {
        self.state = s;
        match s {
            MusicState::Combat => {
                self.level = 4;
                self.value = self.intensity;
            }
            MusicState::Chase => {
                self.level = 2;
                self.value = self.tweaks.chase_time;
            }
            _ => {
                if let Some(e) = self.tweaks.level_event(s, 0) {
                    posts.push(e.to_string());
                }
            }
        }
        self.transitions.push((s, self.level));
    }

    fn combat_level(&mut self, posts: &mut Vec<String>) {
        let t = &self.tweaks.intensity;
        let mut l = (t[1] <= self.value) as u8;
        if t[2] <= self.value {
            l = 2;
        }
        if t[3] <= self.value {
            l = 3;
        }
        if l != self.level {
            self.level = l;
            self.transitions.push((MusicState::Combat, l));
            if let Some(e) = self.tweaks.level_event(MusicState::Combat, l) {
                posts.push(e.to_string());
            }
        }
    }

    fn chase_level(&mut self, player: [f32; 3], npcs: &[MusicNpc], posts: &mut Vec<String>) {
        for i in 0..2u8 {
            if within(player, npcs, self.tweaks.chase_range[i as usize]) {
                if i != self.level {
                    self.level = i;
                    self.transitions.push((MusicState::Chase, i));
                    if let Some(e) = self.tweaks.level_event(MusicState::Chase, i) {
                        posts.push(e.to_string());
                    }
                }
                return;
            }
        }
        self.value = 0.0;
    }

    /// One game tick; returns the AkEvents to post (on the global object).
    pub fn tick(&mut self, dt: f32, player: [f32; 3], npcs: &[MusicNpc]) -> Vec<String> {
        let mut posts = Vec::new();
        self.intensity = self.compute_intensity(npcs);
        if !self.started {
            self.started = true;
            self.enter(MusicState::Exploration, &mut posts);
        }
        let reselect = match self.state {
            MusicState::Chase => self.value <= 0.0 || self.intensity > 0.0 || self.attacked_by_rats,
            _ => true,
        };
        if reselect {
            let s = self.select(player, npcs);
            if s != self.state {
                self.enter(s, &mut posts);
            }
        }
        match self.state {
            MusicState::Combat => {
                self.value = self.intensity;
                self.combat_level(&mut posts);
            }
            MusicState::Chase => {
                self.value -= dt;
                self.chase_level(player, npcs, &mut posts);
            }
            _ => {}
        }
        posts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tw() -> MusicTweaks {
        MusicTweaks {
            contribution: [0.0, 0.5, 0.5, 1.0, 1.0, 2.0, 100.0, 1.0],
            intensity: [0.1, 2.0, 5.0, 100.0],
            suspense_range: 800.0,
            chase_time: 15.0,
            chase_range: [2000.0, 4500.0],
            start_exploration: Some("Explor".into()),
            start_suspense: Some("Suspens".into()),
            start_combat: [Some("F1".into()), Some("F2".into()), Some("F3".into()), Some("F4".into())],
            start_chase: [Some("EscHigh".into()), Some("EscLow".into())],
            ..Default::default()
        }
    }

    fn npc(x: f32, combat: bool) -> MusicNpc {
        MusicNpc { pos: [x, 0.0, 0.0], alive: true, hostile: true, combat, music_type: 4 }
    }

    #[test]
    fn exploration_suspense_combat_chase() {
        let mut m = MusicManager::new(tw());
        assert_eq!(m.tick(0.1, [0.0; 3], &[npc(3000.0, false)]), vec!["Explor"]);
        assert_eq!(m.tick(0.1, [0.0; 3], &[npc(700.0, false)]), vec!["Suspens"]);
        // One guard fighting: intensity 1 < 2 -> level 0.
        assert_eq!(m.tick(0.1, [0.0; 3], &[npc(700.0, true)]), vec!["F1"]);
        // Three guards: 3 >= 2 -> level 1.
        assert_eq!(m.tick(0.1, [0.0; 3], &[npc(700.0, true), npc(600.0, true), npc(500.0, true)]), vec!["F2"]);
        // Combat ends with a hostile near: chase, near level.
        assert_eq!(m.tick(0.1, [0.0; 3], &[npc(700.0, false)]), vec!["EscHigh"]);
        assert_eq!(m.state, MusicState::Chase);
        // Far hostile: far level; nobody within 4500 ends the chase.
        assert_eq!(m.tick(0.1, [0.0; 3], &[npc(3000.0, false)]), vec!["EscLow"]);
        assert!(m.tick(0.1, [0.0; 3], &[npc(9000.0, false)]).is_empty());
        assert_eq!(m.tick(0.1, [0.0; 3], &[npc(9000.0, false)]), vec!["Explor"]);
    }

    #[test]
    fn chase_times_out_to_suspense() {
        let mut m = MusicManager::new(tw());
        m.tick(0.1, [0.0; 3], &[npc(700.0, true)]);
        m.tick(0.1, [0.0; 3], &[npc(700.0, false)]);
        assert_eq!(m.state, MusicState::Chase);
        for _ in 0..149 {
            m.tick(0.1, [0.0; 3], &[npc(700.0, false)]);
        }
        assert_eq!(m.state, MusicState::Chase);
        m.tick(0.1, [0.0; 3], &[npc(700.0, false)]);
        m.tick(0.1, [0.0; 3], &[npc(700.0, false)]);
        assert_eq!(m.state, MusicState::Suspense);
    }
}
