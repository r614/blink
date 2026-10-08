//! Scenario specs: the unit of the system oracles, as maps are the unit of the level-data
//! oracles. A scenario is a map, a setup, a fixed-step input script, checkpoints and what to
//! record; the same spec drives the original (phase 2, the engine hook's `lab` commands and
//! fixed time step) and the headless port runner, and the two traces are diffed tick by tick.
//!
//! Specs are RON files (`fixtures/oracle/scenarios/<system>/<name>.ron`). They are authored
//! test definitions, not game content: positions and expected values are numbers measured or
//! chosen by us.

use crate::cache::{Probe, fnv64};
use crate::trace::{Trace, TraceEvent};
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
    /// Name the trace uses for this NPC (`npc.<id>.x`, events `@<id>`); default: the spawner
    /// name, else `npc<index>`.
    #[serde(default)]
    pub id: Option<String>,
    /// An original spawner by name (`DishonoredSpawner_12`), or a pawn tweak to place.
    #[serde(default)]
    pub spawner: Option<String>,
    #[serde(default)]
    pub tweak: Option<String>,
    /// `Guard` (default) or `Civilian` when placed by position.
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub position: Option<[f32; 3]>,
    #[serde(default)]
    pub yaw: f32,
    /// Initial AI state: `Idle` (default), `Patrol` (along `route`), `Wander`, `Alerted`
    /// (combat against the player from the start), `Passive` (no AI: a training dummy).
    #[serde(default)]
    pub state: Option<String>,
    /// Faction tweak (`DisFaction_Defaults.Faction_Guard_Default`, the default of guards that
    /// are not passive); `None` for no faction (unaware of the player until alerted).
    #[serde(default)]
    pub faction: Option<String>,
    /// Patrol points (with `state: Patrol`).
    #[serde(default)]
    pub route: Vec<[f32; 3]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Setup {
    #[serde(default)]
    pub player: Option<PlayerSetup>,
    /// Items given (`lab give <Tweak>` on the original). Port side: `FullAmmo`, `AllPowers`,
    /// `Reset` (the default loadout first), `HealthElixir:<n>`, `ManaElixir:<n>`, `Coins:<n>`,
    /// `Runes:<n>`, `Upgrade:<name>`, or an ammo type (`Bullet`, `Bolt`, `SleepDart`,
    /// `IncendiaryBolt`, `SpringRazor`, `Grenade`, `StickyGrenade`, `BulletExplosive`) with an
    /// optional `:<count>` (default: to capacity).
    #[serde(default)]
    pub items: Vec<String>,
    /// Left-hand selection at the start (`dishonored_sim::input::LeftHand` name: `Blink`,
    /// `Windblast`, `Pistol`, `Crossbow`, `Grenade`, `SpringRazor`...).
    #[serde(default)]
    pub left_hand: Option<String>,
    /// Remote event of the global game sequence that starts the level's Kismet (with `kismet`).
    #[serde(default)]
    pub kismet_start: Option<String>,
    /// Spawn the level's original spawners that spawn on begin play (the populated level).
    #[serde(default)]
    pub original_npcs: bool,
    /// Ticks run before tick 0 (not recorded): the pawn lands and equips first.
    #[serde(default)]
    pub settle: u32,
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
        Self {
            player: None,
            items: Vec::new(),
            left_hand: None,
            kismet_start: None,
            original_npcs: false,
            settle: 0,
            equip: Vec::new(),
            powers: Vec::new(),
            npcs: Vec::new(),
            story_flags: Vec::new(),
            seed: 0,
            kismet: false,
            keep_alive: true,
        }
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
    /// A console command (original: `ConsoleCommand`; port: the matching debug hook, if any:
    /// `teleport X Y Z [YAW]`, `health N`, `mana N`, `power NAME LEVEL`, `alert NPC`,
    /// `calm NPC`, `give AMMOTYPE N`, `saveload` (save the match, load it back and continue from the copy; the
    /// event detail says `same_state=true|false`), `cansave`, `adddarkness N` (the chaos cheat:
    /// the darkness manager's Add); anything else is recorded as an
    /// unsupported-command event).
    Console(String),
    /// Select the left-hand item (power wheel / quick select), held until changed
    /// (`LeftHand` name: `Blink`, `Windblast`, `DarkVision`, `Possession`, `BendTime`,
    /// `DevouringSwarm`, `Pistol`, `Crossbow`, `SleepDart`, `IncendiaryBolt`, `Grenade`,
    /// `StickyGrenade`, `SpringRazor`, `Empty`).
    Hand(String),
    /// Drink an elixir this tick (`HealthElixir` / `ManaElixir`).
    Drink(String),
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
    /// The channel's last sampled value.
    Final(String),
    /// Value at the second tick minus the value at the first.
    Delta(String, u32, u32),
    /// Value at a tick minus the minimum from that tick on (a drop: damage taken, mana spent).
    DropFrom(String, u32),
    /// First channel minus the second, both at a tick (a Blink target ahead of the eye).
    Diff(String, String, u32),
    /// Number of trace events matching a pattern (see [`EventPattern`]).
    Count(String),
    /// Seconds from tick 0 to the first matching event.
    FirstAt(String),
    /// Seconds from the first event matching the first pattern to the first event matching
    /// the second at or after it.
    Between(String, String),
    /// A numeric `key=value` of the first matching event's detail.
    EventValue(String, String),
    /// Sum of a numeric `key=value` over the matching events.
    EventSum(String, String),
    /// Seconds from the first matching `Effect` spawn to the end of that instance
    /// (`EffectEnd` with the same `inst`): missing when it never ends within the run.
    Lifetime(String),
    /// Matching Effect spawns whose instance has no EffectEnd by the end of the run.
    Unended(String),
    /// First tick at which the channel drops below the threshold (<), in seconds.
    TimeBelow(String, f64),
    /// First tick at which the channel equals the value (within 0.5: enumerated channels), in
    /// seconds.
    TimeAt(String, f64),
    /// The second metric minus the first (the time between two channel events).
    Gap(Box<Metric>, Box<Metric>),
    /// The magnitude of a metric (quantities whose sign convention differs between the sides).
    Abs(Box<Metric>),
    /// Mean / variance of a numeric `key=value` over the matching events.
    EventMean(String, String),
    EventVar(String, String),
}

/// How an expectation compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Cmp {
    /// |actual - value| <= tol.
    #[default]
    Eq,
    /// actual >= value - tol.
    AtLeast,
    /// actual <= value + tol.
    AtMost,
}

/// What the reference value is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Reference {
    /// Measured on the running original (the engine hook: INSTRUMENTATION.md, HOOK_TRIAGE.md).
    #[default]
    Measured,
    /// Read from the original's data or code (tweaks, decompiled constants), not run.
    Static,
    /// Not measured yet: the port's value is reported and the expectation is a capture
    /// request for the hook (`value` is a placeholder and never fails the scenario).
    Unmeasured,
    /// Measured on the original, but not known to be sampled like the port (another sampling
    /// point, frame of reference or unit): reported with the port's value, never judged, and
    /// listed as a capture request until the hook samples it comparably.
    Unverified,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Expect {
    pub metric: Metric,
    pub value: f64,
    pub tol: f64,
    /// Where the reference value comes from (a measurement, a doc and date).
    pub source: String,
    #[serde(default)]
    pub cmp: Cmp,
    #[serde(default)]
    pub status: Reference,
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
    /// Powers and left hand: item, phase, ammo, dispersion, Blink target, Dark Vision, Bend
    /// Time, possession.
    Powers,
    /// Projectiles (count, the first one's position) and their spawn / end events.
    Projectiles,
    /// Particle systems the presentation spawns for the events (`Effect` / `EffectEnd`).
    Effects,
    /// Sound events the presentation posts (`Sound`).
    Sounds,
    /// Animation inputs (hits, deaths, swings: `Anim`) and NPC stances.
    Anim,
}

/// How a sweep picks its queries (several can be combined).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Sampling {
    /// The navmesh grid / foot-waist-head / link sweeps of `harness collision`.
    Navmesh,
    /// Every collision primitive from every approach direction (`reach` uu each side).
    PrimitiveDirected { reach: f32 },
    /// After a first pass: `per` queries within `radius` uu of each disagreement, `rounds`
    /// times (each round around the previous round's disagreements).
    Densify { rounds: u32, per: u32, radius: f32 },
}

/// Which trace profiles the queries run with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Profiles {
    /// Every trace call site of the exe (from the Ghidra index, phase 3).
    Enumerated,
    /// A listed subset.
    Listed(Vec<crate::query::TraceProfile>),
}

/// A world state to answer the queries in (one cache entry per state).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldStateSpec {
    pub name: String,
    /// Actor settings (`<package>:<actor>` -> state); empty: the map's initial state.
    #[serde(default)]
    pub settings: Vec<crate::query::StateSetting>,
    /// Or every toggleable actor of a class in each of its states (`DisDoor`): expanded by the
    /// runner into one state per (actor, state).
    #[serde(default)]
    pub each_state_of: Option<String>,
}

/// A level-data sweep instead of a scripted run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sweep {
    pub probe: Probe,
    /// Query generator and version (`harness-collision-v1`).
    pub generator: String,
    #[serde(default)]
    pub sampling: Vec<Sampling>,
    #[serde(default)]
    pub profiles: Option<Profiles>,
    /// States to re-run the targeted queries in (empty: the initial state only).
    #[serde(default)]
    pub states: Vec<WorldStateSpec>,
    /// Run a structural diff of the system's objects first (the primary proof).
    #[serde(default)]
    pub structure: bool,
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
    /// How to capture the original's side (hook commands), for the capture requests of
    /// unmeasured expectations.
    #[serde(default)]
    pub capture: Option<String>,
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

    /// Hash of what defines the run (map, setup, step, ticks, input, what is recorded, the
    /// sweep): the cache key's `query_hash` for the scenario's traces. Descriptions,
    /// checkpoints, tolerances and capture notes are left out, so correcting an expected value
    /// keeps the captured traces.
    pub fn spec_hash(&self) -> u64 {
        let run = (&self.name, &self.map, &self.setup, self.tick_hz, self.ticks, &self.input, &self.record, &self.sweep);
        fnv64(ron::to_string(&run).unwrap_or_default().as_bytes())
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
                    Action::Console(_) | Action::Hand(_) | Action::Drink(_) => {}
                }
                steps.next();
            }
            held.retain(|(_, until)| *until > t);
            out.push((fw, rt, yaw, pitch, held.iter().map(|h| h.0).collect()));
        }
        out
    }

    /// The full input at every tick, with left-hand selections, drinks and console commands.
    pub fn timeline(&self) -> Vec<TickInput> {
        let base = self.input_timeline();
        let mut hand = self.setup.left_hand.clone();
        let mut out = Vec::with_capacity(base.len());
        let mut steps = self.input.iter().peekable();
        for (t, (forward, right, yaw, pitch, held)) in base.into_iter().enumerate() {
            let mut ti = TickInput { forward, right, yaw, pitch, held, hand: None, drink: None, console: Vec::new() };
            while let Some(s) = steps.peek() {
                if s.tick != t as u32 {
                    break;
                }
                match &s.action {
                    Action::Hand(h) => hand = Some(h.clone()),
                    Action::Drink(d) => ti.drink = Some(d.clone()),
                    Action::Console(c) => ti.console.push(c.clone()),
                    _ => {}
                }
                steps.next();
            }
            ti.hand = hand.clone();
            out.push(ti);
        }
        out
    }

    /// Every expectation evaluated on a trace, with its status.
    pub fn evaluate(&self, t: &Trace) -> Vec<Check> {
        let mut out = Vec::new();
        for cp in &self.checkpoints {
            for e in &cp.expect {
                let actual = crate::diff::metric_value(t, &e.metric);
                let ok = actual.is_some_and(|a| match e.cmp {
                    Cmp::Eq => (a - e.value).abs() <= e.tol,
                    Cmp::AtLeast => a >= e.value - e.tol,
                    Cmp::AtMost => a <= e.value + e.tol,
                });
                let status = match (e.status, ok) {
                    (Reference::Unmeasured, _) => CheckStatus::Unmeasured,
                    (Reference::Unverified, _) => CheckStatus::Unverified,
                    (_, true) => CheckStatus::Pass,
                    (_, false) => CheckStatus::Fail,
                };
                let first_event = match &e.metric {
                    Metric::Count(p) | Metric::FirstAt(p) | Metric::EventValue(p, _) | Metric::EventSum(p, _) | Metric::Lifetime(p) | Metric::Unended(p) | Metric::Between(p, _) => EventPattern::parse(p).first(t).map(|ev| ev.tick),
                    _ => None,
                };
                out.push(Check { checkpoint: cp.name.clone(), metric: metric_label(&e.metric), cmp: e.cmp, expected: e.value, tol: e.tol, actual, status, reference: e.status, source: e.source.clone(), tick: first_event });
            }
        }
        out
    }
}

/// One tick of input ([`Scenario::timeline`]).
#[derive(Debug, Clone, PartialEq)]
pub struct TickInput {
    pub forward: i8,
    pub right: i8,
    pub yaw: f32,
    pub pitch: f32,
    pub held: Vec<Button>,
    /// Left-hand selection in force (None: the runner's default).
    pub hand: Option<String>,
    /// Elixir drunk this tick.
    pub drink: Option<String>,
    pub console: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckStatus {
    Pass,
    Fail,
    /// Unmeasured reference: the port's value is reported, nothing is judged.
    Unmeasured,
    /// Measured, but not known to be sampled comparably: reported, not judged.
    Unverified,
}

/// An expectation's outcome ([`Scenario::evaluate`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Check {
    pub checkpoint: String,
    pub metric: String,
    pub cmp: Cmp,
    pub expected: f64,
    pub tol: f64,
    pub actual: Option<f64>,
    pub status: CheckStatus,
    pub reference: Reference,
    pub source: String,
    /// Tick of the first matching event (event metrics).
    pub tick: Option<u32>,
}

/// A compact metric label: `Count(Effect:Vfx_*)`.
pub fn metric_label(m: &Metric) -> String {
    match m {
        Metric::At(c, t) => format!("At({c}, {t})"),
        Metric::Max(c) => format!("Max({c})"),
        Metric::Min(c) => format!("Min({c})"),
        Metric::Rise(c) => format!("Rise({c})"),
        Metric::RiseFrom(c, t) => format!("RiseFrom({c}, {t})"),
        Metric::TimeToReach(c, v) => format!("TimeToReach({c}, {v})"),
        Metric::Final(c) => format!("Final({c})"),
        Metric::Delta(c, a, b) => format!("Delta({c}, {a}, {b})"),
        Metric::DropFrom(c, t) => format!("DropFrom({c}, {t})"),
        Metric::Diff(a, b, t) => format!("Diff({a}, {b}, {t})"),
        Metric::Count(p) => format!("Count({p})"),
        Metric::FirstAt(p) => format!("FirstAt({p})"),
        Metric::Between(a, b) => format!("Between({a}, {b})"),
        Metric::EventValue(p, k) => format!("EventValue({p}, {k})"),
        Metric::EventSum(p, k) => format!("EventSum({p}, {k})"),
        Metric::Lifetime(p) => format!("Lifetime({p})"),
        Metric::Unended(p) => format!("Unended({p})"),
        Metric::TimeBelow(c, v) => format!("TimeBelow({c}, {v})"),
        Metric::TimeAt(c, v) => format!("TimeAt({c}, {v})"),
        Metric::Gap(a, b) => format!("Gap({}, {})", metric_label(a), metric_label(b)),
        Metric::Abs(a) => format!("Abs({})", metric_label(a)),
        Metric::EventMean(p, k) => format!("EventMean({p}, {k})"),
        Metric::EventVar(p, k) => format!("EventVar({p}, {k})"),
    }
}

/// An event pattern: `Kind:Name`, `*` wildcards in both, then optional `@object` and
/// `[key=value,...]` detail filters (`Sim:Damage@dummy_a[cause=Sword]`,
/// `Effect:Vfx_GamePlay.Windblast.*`). A bare `Name` matches any kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventPattern {
    pub kind: String,
    pub name: String,
    pub object: Option<String>,
    pub detail: Vec<(String, String)>,
}

fn wild(p: &str, s: &str) -> bool {
    // `*` matches any run of characters, case-insensitive.
    let (p, s) = (p.to_ascii_lowercase(), s.to_ascii_lowercase());
    let parts: Vec<&str> = p.split('*').collect();
    if parts.len() == 1 {
        return p == s;
    }
    let mut rest = s.as_str();
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            if !rest.starts_with(part) {
                return false;
            }
            rest = &rest[part.len()..];
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else if let Some(k) = rest.find(part) {
            rest = &rest[k + part.len()..];
        } else {
            return false;
        }
    }
    true
}

/// `key=value` pairs of an event detail (space separated).
pub fn detail_fields(d: &str) -> Vec<(&str, &str)> {
    d.split_whitespace().filter_map(|kv| kv.split_once('=')).collect()
}

impl EventPattern {
    pub fn parse(p: &str) -> Self {
        let (p, detail) = match p.split_once('[') {
            Some((a, b)) => (a, b.trim_end_matches(']').split(',').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect()),
            None => (p, Vec::new()),
        };
        let (p, object) = match p.rsplit_once('@') {
            Some((a, o)) => (a, Some(o.to_string())),
            None => (p, None),
        };
        let (kind, name) = match p.split_once(':') {
            Some((k, n)) => (k.to_string(), n.to_string()),
            None => ("*".to_string(), p.to_string()),
        };
        Self { kind, name, object, detail }
    }

    pub fn matches(&self, e: &TraceEvent) -> bool {
        wild(&self.kind, &e.kind)
            && wild(&self.name, &e.name)
            && self.object.as_ref().is_none_or(|o| wild(o, &e.object))
            && self.detail.iter().all(|(k, v)| detail_fields(&e.detail).iter().any(|(dk, dv)| dk.eq_ignore_ascii_case(k) && wild(v, dv)))
    }

    pub fn first<'a>(&self, t: &'a Trace) -> Option<&'a TraceEvent> {
        t.events.iter().find(|e| self.matches(e))
    }
}

fn field(e: &TraceEvent, key: &str) -> Option<f64> {
    detail_fields(&e.detail).into_iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).and_then(|(_, v)| v.parse().ok())
}

/// The event and channel metrics added for the scenario library (the movement metrics are in
/// [`crate::diff::metric_value`]).
pub fn extra_metric(t: &Trace, m: &Metric) -> Option<f64> {
    let secs = |tick: u32| tick as f64 / t.tick_hz.max(1) as f64;
    let at = |c: &str, tick: u32| t.series(c).into_iter().find(|x| x.0 == tick).map(|x| x.1);
    match m {
        Metric::Final(c) => t.series(c).last().map(|x| x.1),
        Metric::Delta(c, a, b) => Some(at(c, *b)? - at(c, *a)?),
        Metric::DropFrom(c, tick) => {
            let v0 = at(c, *tick)?;
            t.series(c).into_iter().filter(|x| x.0 >= *tick).map(|x| x.1).reduce(f64::min).map(|m| v0 - m)
        }
        Metric::Diff(a, b, tick) => Some(at(a, *tick)? - at(b, *tick)?),
        Metric::Count(p) => {
            let p = EventPattern::parse(p);
            Some(t.events.iter().filter(|e| p.matches(e)).count() as f64)
        }
        Metric::FirstAt(p) => EventPattern::parse(p).first(t).map(|e| secs(e.tick)),
        Metric::Between(a, b) => {
            let (a, b) = (EventPattern::parse(a), EventPattern::parse(b));
            let ea = a.first(t)?;
            let eb = t.events.iter().find(|e| e.tick >= ea.tick && b.matches(e))?;
            Some(secs(eb.tick - ea.tick))
        }
        Metric::EventValue(p, k) => {
            let p = EventPattern::parse(p);
            t.events.iter().filter(|e| p.matches(e)).find_map(|e| field(e, k))
        }
        Metric::EventSum(p, k) => {
            let p = EventPattern::parse(p);
            Some(t.events.iter().filter(|e| p.matches(e)).filter_map(|e| field(e, k)).sum())
        }
        Metric::Lifetime(p) => {
            let p = EventPattern::parse(p);
            let spawn = t.events.iter().find(|e| e.kind == "Effect" && p.matches(e))?;
            let inst = detail_fields(&spawn.detail).into_iter().find(|(k, _)| *k == "inst")?.1.to_string();
            let end = t.events.iter().find(|e| e.kind == "EffectEnd" && detail_fields(&e.detail).iter().any(|(k, v)| *k == "inst" && *v == inst))?;
            Some(secs(end.tick - spawn.tick))
        }
        Metric::Unended(p) => {
            let p = EventPattern::parse(p);
            let inst = |e: &TraceEvent| detail_fields(&e.detail).into_iter().find(|(k, _)| *k == "inst").map(|x| x.1.to_string());
            let ended: Vec<String> = t.events.iter().filter(|e| e.kind == "EffectEnd").filter_map(inst).collect();
            Some(t.events.iter().filter(|e| e.kind == "Effect" && p.matches(e)).filter(|e| inst(e).is_none_or(|i| !ended.contains(&i))).count() as f64)
        }
        Metric::TimeBelow(c, thr) => t.series(c).into_iter().find(|x| x.1 < *thr).map(|x| secs(x.0)),
        Metric::TimeAt(c, v) => t.series(c).into_iter().find(|x| (x.1 - v).abs() < 0.5).map(|x| secs(x.0)),
        Metric::Gap(a, b) => Some(crate::diff::metric_value(t, b)? - crate::diff::metric_value(t, a)?),
        Metric::Abs(a) => crate::diff::metric_value(t, a).map(f64::abs),
        Metric::EventMean(p, k) | Metric::EventVar(p, k) => {
            let p = EventPattern::parse(p);
            let v: Vec<f64> = t.events.iter().filter(|e| p.matches(e)).filter_map(|e| field(e, k)).collect();
            if v.is_empty() {
                return None;
            }
            let mean = v.iter().sum::<f64>() / v.len() as f64;
            Some(if matches!(m, Metric::EventMean(..)) { mean } else { v.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / v.len() as f64 })
        }
        _ => None,
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
    fn event_metrics_and_statuses() {
        let mut t = Trace::new("s", "port", 60, &[("pawn.health", "pt")]);
        for k in 0..120u32 {
            t.push(k, vec![if k < 70 { 70.0 } else { 60.0 }]);
        }
        let ev = |tick: u32, kind: &str, object: &str, name: &str, detail: &str| TraceEvent { tick, kind: kind.into(), object: object.into(), name: name.into(), detail: detail.into() };
        t.events = vec![
            ev(6, "Sim", "player", "SwordSwing", "id=player"),
            ev(24, "Sim", "dummy", "Damage", "victim=dummy amount=14 cause=Sword"),
            ev(24, "Effect", "dummy", "Vfx_PhysMat.Blood.Pfx_BloodMotion", "inst=1 untextured=0"),
            ev(30, "Effect", "player", "Vfx_GamePlay.Blink.Blink_Fall_01", "inst=2"),
            ev(84, "EffectEnd", "", "Vfx_PhysMat.Blood.Pfx_BloodMotion", "inst=1"),
        ];
        let m = |x: Metric| crate::diff::metric_value(&t, &x);
        assert_eq!(m(Metric::Count("Sim:Damage@dummy[cause=Sword]".into())), Some(1.0));
        assert_eq!(m(Metric::Count("Sim:Damage@dummy[cause=Pistol]".into())), Some(0.0));
        assert_eq!(m(Metric::Count("Effect:Vfx_PhysMat.Blood.*".into())), Some(1.0));
        assert_eq!(m(Metric::Between("Sim:SwordSwing".into(), "Sim:Damage@dummy".into())), Some(0.3));
        assert_eq!(m(Metric::EventValue("Sim:Damage".into(), "amount".into())), Some(14.0));
        assert_eq!(m(Metric::Lifetime("Vfx_PhysMat.Blood.*".into())), Some(1.0));
        assert_eq!(m(Metric::Lifetime("Vfx_GamePlay.Blink.*".into())), None);
        assert_eq!(m(Metric::Unended("*".into())), Some(1.0));
        assert_eq!(m(Metric::DropFrom("pawn.health".into(), 0)), Some(10.0));
        assert_eq!(m(Metric::Gap(Box::new(Metric::FirstAt("Sim:SwordSwing".into())), Box::new(Metric::TimeBelow("pawn.health".into(), 65.0)))).map(|v| (v * 60.0).round()), Some(64.0));
        assert_eq!(m(Metric::Abs(Box::new(Metric::Delta("pawn.health".into(), 0, 100)))), Some(10.0));
        let s = Scenario::parse(
            r#"(name: "combat/x", system: Combat, map: "M", ticks: 10, checkpoints: [(name: "c", expect: [
                (metric: Count("Sim:Damage"), value: 1, tol: 0, source: "m"),
                (metric: Count("Sim:Damage"), value: 2, tol: 0, source: "m", status: Static),
                (metric: Count("Sim:Damage"), value: 5, tol: 0, source: "m", status: Unmeasured),
                (metric: Count("Sim:Damage"), value: 5, tol: 0, source: "m", status: Unverified),
                (metric: Count("Effect:*"), value: 1, tol: 0, cmp: AtLeast, source: "m"),
            ])])"#,
        )
        .unwrap();
        let st: Vec<CheckStatus> = s.evaluate(&t).iter().map(|c| c.status).collect();
        assert_eq!(st, vec![CheckStatus::Pass, CheckStatus::Fail, CheckStatus::Unmeasured, CheckStatus::Unverified, CheckStatus::Pass]);
        // The spec hash ignores checkpoints (correcting a reference keeps captured traces).
        let mut s2 = s.clone();
        s2.checkpoints.clear();
        assert_eq!(s.spec_hash(), s2.spec_hash());
    }

    #[test]
    fn validation() {
        assert!(Scenario::parse(r#"(name: "x", system: Movement, map: "M", ticks: 0)"#).is_err());
        assert!(Scenario::parse(r#"(name: "x", system: Movement, map: "M", ticks: 5, input: [(tick: 7, action: Press(Jump))])"#).is_err());
        let sweep = Scenario::parse(r#"(name: "sweep/x", system: MapSweep, map: "M", sweep: Some((probe: Collision, generator: "g")))"#).unwrap();
        assert_eq!(sweep.sweep.unwrap().probe, Probe::Collision);
        let full = Scenario::parse(
            r#"(name: "sweep/full", system: MapSweep, map: "M", sweep: Some((probe: Collision, generator: "g", structure: true,
                sampling: [Navmesh, PrimitiveDirected(reach: 64), Densify(rounds: 2, per: 8, radius: 16)],
                profiles: Some(Listed([(name: "vision", tag: "V", flags: 0x8023086, extent: (0, 0, 0))])),
                states: [(name: "initial"), (name: "doors", each_state_of: Some("DisDoor")),
                         (name: "gate open", settings: [(actor: "L_X:DisDoor_3", state: "2")])])))"#,
        )
        .unwrap();
        let sw = full.sweep.unwrap();
        assert_eq!(sw.sampling.len(), 3);
        assert_eq!(sw.states[2].settings[0].state, "2");
        assert!(matches!(sw.profiles, Some(Profiles::Listed(ref v)) if v[0].flags == 0x0802_3086));
    }
}
