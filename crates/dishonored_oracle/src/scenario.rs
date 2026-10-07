//! Scenario specs: the unit of the system oracles, as maps are the unit of the level-data
//! oracles. A scenario is a map, a setup, a fixed-step input script, checkpoints and what to
//! record; the same spec drives the original (phase 2, the engine hook's `lab` commands and
//! fixed time step) and the headless port runner, and the two traces are diffed tick by tick.
//!
//! Specs are RON files (`fixtures/oracle/scenarios/<system>/<name>.ron`). They are authored
//! test definitions, not game content: positions and expected values are numbers measured or
//! chosen by us.

use crate::cache::{Probe, fnv64};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The game systems the oracle covers (README "the whole game").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum System {
    Movement,
    Combat,
    Powers,
    Stealth,
    Ai,
    Kismet,
    Conversation,
    Cutscene,
    Inventory,
    Economy,
    Chaos,
    SaveLoad,
    Ui,
    Audio,
    Physics,
    Particles,
    /// A level-data sweep (collision, navigation, light...) of one map.
    MapSweep,
}

impl System {
    pub const ALL: [System; 17] = [
        System::Movement,
        System::Combat,
        System::Powers,
        System::Stealth,
        System::Ai,
        System::Kismet,
        System::Conversation,
        System::Cutscene,
        System::Inventory,
        System::Economy,
        System::Chaos,
        System::SaveLoad,
        System::Ui,
        System::Audio,
        System::Physics,
        System::Particles,
        System::MapSweep,
    ];
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlayerSetup {
    /// Feet-on-floor position is found by the runner when `snap_to_floor` is set; otherwise the
    /// pawn's centre.
    pub position: [f32; 3],
    #[serde(default)]
    pub yaw: f32,
    #[serde(default)]
    pub pitch: f32,
    #[serde(default)]
    pub snap_to_floor: bool,
    #[serde(default)]
    pub sword_drawn: bool,
    #[serde(default)]
    pub health: Option<f32>,
    #[serde(default)]
    pub mana: Option<f32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NpcSetup {
    /// An original spawner by name (`DishonoredNPCSpawner_12`), or a pawn tweak to place.
    #[serde(default)]
    pub spawner: Option<String>,
    #[serde(default)]
    pub tweak: Option<String>,
    #[serde(default)]
    pub position: Option<[f32; 3]>,
    #[serde(default)]
    pub yaw: f32,
    /// Initial AI state (`Idle`, `Patrol`, `Alerted`...).
    #[serde(default)]
    pub state: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Setup {
    #[serde(default)]
    pub player: Option<PlayerSetup>,
    /// Items given (`lab give <Tweak>` on the original).
    #[serde(default)]
    pub items: Vec<String>,
    /// Items equipped: (item class, hand 1 = right, 2 = left).
    #[serde(default)]
    pub equip: Vec<(String, u8)>,
    /// Powers: (name, 1-based level).
    #[serde(default)]
    pub powers: Vec<(String, u8)>,
    #[serde(default)]
    pub npcs: Vec<NpcSetup>,
    #[serde(default)]
    pub story_flags: Vec<(String, bool)>,
    #[serde(default)]
    pub seed: u64,
    /// Run the level's Kismet (off: a quiet level, only what the scenario sets up).
    #[serde(default)]
    pub kismet: bool,
    /// Keep the player alive (the original's `m_MinimumScriptedHealth` = 1; damage is still
    /// recorded).
    #[serde(default = "yes")]
    pub keep_alive: bool,
}

fn yes() -> bool {
    true
}

impl Default for Setup {
    fn default() -> Self {
        Self { player: None, items: Vec::new(), equip: Vec::new(), powers: Vec::new(), npcs: Vec::new(), story_flags: Vec::new(), seed: 0, kismet: false, keep_alive: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Button {
    Jump,
    Crouch,
    Sprint,
    Walk,
    Primary,
    Secondary,
    Block,
    LeanLeft,
    LeanRight,
    Use,
    Reload,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Action {
    /// Movement axes, -127..127, held until changed.
    Move { forward: i8, right: i8 },
    /// View angles in degrees (Dishonored convention), held until changed.
    Look { yaw: f32, pitch: f32 },
    /// Button down for `hold` ticks (at least one).
    Press(Button),
    /// A console command (original: `ConsoleCommand`; port: the matching debug hook, if any).
    Console(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputStep {
    pub tick: u32,
    /// Ticks a `Press` is held (0 or 1: one tick).
    #[serde(default)]
    pub hold: u32,
    pub action: Action,
}

/// A value computed from a trace channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Metric {
    /// The channel at a tick.
    At(String, u32),
    Max(String),
    Min(String),
    /// Maximum minus the value at tick 0 (a jump's apex height).
    Rise(String),
    /// Maximum from a tick on, minus the value at that tick.
    RiseFrom(String, u32),
    /// First tick at which the channel reaches the threshold (>=), in seconds.
    TimeToReach(String, f64),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Expect {
    pub metric: Metric,
    pub value: f64,
    pub tol: f64,
    /// Where the reference value comes from (a measurement, a doc and date).
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub name: String,
    #[serde(default)]
    pub expect: Vec<Expect>,
}

/// What to record per tick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Record {
    /// `pawn.x/y/z`, `pawn.vx/vy/vz`, `pawn.speed2d`, `pawn.physics`, `pawn.crouched`.
    Pawn,
    /// `pawn.health`, `pawn.mana`.
    Vitals,
    /// The camera (eye) location and rotation.
    Camera,
    /// An NPC by spawner name: location, AI state, awareness.
    Npc(String),
    /// Kismet op activations (events).
    Kismet,
    /// ProcessEvent calls whose function name contains one of the filters (original only).
    ProcessEvent(Vec<String>),
    /// The port's simulation events (sounds, hits, deaths).
    SimEvents,
}

/// A level-data sweep instead of a scripted run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sweep {
    pub probe: Probe,
    /// Query generator and version (`harness-collision-v1`).
    pub generator: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scenario {
    /// `<system>/<name>`, unique.
    pub name: String,
    pub system: System,
    pub map: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub setup: Setup,
    #[serde(default = "sixty")]
    pub tick_hz: u32,
    #[serde(default)]
    pub ticks: u32,
    #[serde(default)]
    pub input: Vec<InputStep>,
    #[serde(default)]
    pub checkpoints: Vec<Checkpoint>,
    #[serde(default)]
    pub record: Vec<Record>,
    /// Absolute tolerance per channel-name prefix (longest prefix wins); default 1e-3.
    #[serde(default)]
    pub tolerances: Vec<(String, f64)>,
    #[serde(default)]
    pub sweep: Option<Sweep>,
}

fn sixty() -> u32 {
    60
}

impl Scenario {
    pub fn parse(text: &str) -> Result<Self> {
        let s: Scenario = ron::from_str(text)?;
        s.validate()?;
        Ok(s)
    }

    pub fn load(path: &Path) -> Result<Self> {
        Self::parse(&std::fs::read_to_string(path)?).with_context(|| format!("{}", path.display()))
    }

    pub fn validate(&self) -> Result<()> {
        if self.name.is_empty() || self.map.is_empty() {
            bail!("scenario needs a name and a map");
        }
        if self.sweep.is_none() && self.ticks == 0 {
            bail!("{}: a scripted scenario needs ticks > 0", self.name);
        }
        if self.tick_hz == 0 {
            bail!("{}: tick_hz 0", self.name);
        }
        let mut last = 0;
        for s in &self.input {
            if s.tick < last {
                bail!("{}: input steps out of order at tick {}", self.name, s.tick);
            }
            if self.sweep.is_none() && s.tick >= self.ticks {
                bail!("{}: input at tick {} after the end ({})", self.name, s.tick, self.ticks);
            }
            last = s.tick;
        }
        Ok(())
    }

    /// Hash of the canonical spec: the cache key's `query_hash` for the scenario's traces.
    pub fn spec_hash(&self) -> u64 {
        fnv64(ron::to_string(self).unwrap_or_default().as_bytes())
    }

    /// Tolerance for a channel (longest matching prefix).
    pub fn tolerance(&self, channel: &str) -> f64 {
        self.tolerances.iter().filter(|(p, _)| channel.starts_with(p.as_str())).max_by_key(|(p, _)| p.len()).map(|x| x.1).unwrap_or(1e-3)
    }

    /// The input state at every tick: (forward, right, yaw, pitch, held buttons).
    pub fn input_timeline(&self) -> Vec<(i8, i8, f32, f32, Vec<Button>)> {
        let mut out = Vec::with_capacity(self.ticks as usize);
        let (mut fw, mut rt) = (0i8, 0i8);
        let (mut yaw, mut pitch) = self.setup.player.as_ref().map(|p| (p.yaw, p.pitch)).unwrap_or((0.0, 0.0));
        let mut held: Vec<(Button, u32)> = Vec::new();
        let mut steps = self.input.iter().peekable();
        for t in 0..self.ticks {
            while let Some(s) = steps.peek() {
                if s.tick != t {
                    break;
                }
                match &s.action {
                    Action::Move { forward, right } => (fw, rt) = (*forward, *right),
                    Action::Look { yaw: y, pitch: p } => (yaw, pitch) = (*y, *p),
                    Action::Press(b) => held.push((*b, t + s.hold.max(1))),
                    Action::Console(_) => {}
                }
                steps.next();
            }
            held.retain(|(_, until)| *until > t);
            out.push((fw, rt, yaw, pitch, held.iter().map(|h| h.0).collect()));
        }
        out
    }
}

/// Every `.ron` scenario under a directory (recursively), sorted by name.
pub fn load_dir(dir: &Path) -> Result<Vec<(PathBuf, Scenario)>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.filter_map(|e| e.ok()) {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "ron") {
                out.push((p.clone(), Scenario::load(&p)?));
            }
        }
    }
    out.sort_by(|a, b| a.1.name.cmp(&b.1.name));
    for w in out.windows(2) {
        if w[0].1.name == w[1].1.name {
            bail!("duplicate scenario name {}", w[0].1.name);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const JUMP: &str = r#"(
        name: "movement/jump_test",
        system: Movement,
        map: "L_Boyle_Ext_P",
        setup: (player: Some((position: (0, 0, 0), yaw: 270))),
        ticks: 10,
        input: [
            (tick: 1, action: Move(forward: 127, right: 0)),
            (tick: 2, hold: 3, action: Press(Jump)),
            (tick: 6, action: Look(yaw: 90, pitch: -10)),
        ],
        checkpoints: [(name: "apex", expect: [(metric: Rise("pawn.z"), value: 106.67, tol: 0.5, source: "test")])],
        record: [Pawn, Vitals],
        tolerances: [("pawn.", 0.5), ("pawn.z", 0.1)],
    )"#;

    #[test]
    fn parse_timeline_tolerance_hash() {
        let s = Scenario::parse(JUMP).unwrap();
        assert!(s.setup.keep_alive);
        assert_eq!(s.tick_hz, 60);
        let tl = s.input_timeline();
        assert_eq!(tl.len(), 10);
        assert_eq!(tl[0].0, 0);
        assert_eq!(tl[1].0, 127);
        assert!(tl[1].4.is_empty());
        assert_eq!(tl[2].4, vec![Button::Jump]);
        assert_eq!(tl[4].4, vec![Button::Jump]);
        assert!(tl[5].4.is_empty());
        assert_eq!((tl[5].2, tl[6].2, tl[6].3), (270.0, 90.0, -10.0));
        assert_eq!(s.tolerance("pawn.z"), 0.1);
        assert_eq!(s.tolerance("pawn.x"), 0.5);
        assert_eq!(s.tolerance("npc.x"), 1e-3);
        let h = s.spec_hash();
        let mut t = s.clone();
        t.ticks = 11;
        assert_ne!(t.spec_hash(), h);
        // Round trip through RON keeps the spec.
        assert_eq!(Scenario::parse(&ron::to_string(&s).unwrap()).unwrap(), s);
    }

    #[test]
    fn validation() {
        assert!(Scenario::parse(r#"(name: "x", system: Movement, map: "M", ticks: 0)"#).is_err());
        assert!(Scenario::parse(r#"(name: "x", system: Movement, map: "M", ticks: 5, input: [(tick: 7, action: Press(Jump))])"#).is_err());
        let sweep = Scenario::parse(r#"(name: "sweep/x", system: MapSweep, map: "M", sweep: Some((probe: Collision, generator: "g")))"#).unwrap();
        assert_eq!(sweep.sweep.unwrap().probe, Probe::Collision);
    }
}
