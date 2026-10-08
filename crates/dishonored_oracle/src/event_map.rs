//! Cross-side event mapping: the original's trace events (the hook's actor `Spawn` scan,
//! `NpcBehavior` changes of the AI brain) and the port's (`Sim`, `Projectile`, the brain state
//! channel) brought to one vocabulary of labels, so the diff can compare them as events (count
//! and timing). Every mapping lives in the tables below; the AI's behaviour table is the one to
//! edit when the port's brain is rebuilt around the original's behaviour slots.
//!
//! Both sides give an event the tick of the frame that first shows it (ORACLE.md "Trace
//! alignment and comparison"): the hook's spawn scan at boundary k + 1 finds an actor spawned
//! during the world tick that produces frame k + 1; the port's step for frame k + 1 emits its
//! events with tick k + 1. Derived events (damage, behaviour from a channel) take the tick of
//! the first frame with the new value, on both sides alike.

use crate::trace::{Trace, TraceEvent};

/// Which side a rule reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Original,
    Port,
}

/// One mapping: events of `kind` whose name matches `name` (exact, or a prefix when it ends in
/// `*`) on `side` become `label`.
pub struct EventRule {
    pub side: Side,
    pub kind: &'static str,
    pub name: &'static str,
    pub label: &'static str,
    /// Why the two sides' events are the same thing (or what is not known yet).
    pub note: &'static str,
}

/// Actor spawns and simulation events, both sides.
pub const EVENT_RULES: &[EventRule] = &[
    // Projectiles: the original spawns one projectile actor per shot; the port emits
    // `Projectile` with its kind.
    EventRule { side: Side::Original, kind: "Spawn", name: "DisBullet", label: "Projectile:Bullet", note: "pistol shot (DishonoredWepPistol fires a DisBullet)" },
    EventRule { side: Side::Port, kind: "Projectile", name: "Bullet", label: "Projectile:Bullet", note: "" },
    EventRule { side: Side::Original, kind: "Spawn", name: "DisProjectile_Arrow", label: "Projectile:Arrow", note: "crossbow bolt; sleep darts too (same class, Twk_Proj_Arrow_Sleep tweak)" },
    EventRule { side: Side::Port, kind: "Projectile", name: "Bolt", label: "Projectile:Arrow", note: "" },
    EventRule { side: Side::Port, kind: "Projectile", name: "SleepDart", label: "Projectile:Arrow", note: "" },
    EventRule {
        side: Side::Original,
        kind: "Spawn",
        name: "DisProjectile_Arrow_Explosive",
        label: "Projectile:ArrowExplosive",
        note: "explosive arrow: also fired by the host level's Tallboys (spawns without an owner in the trace), so the player's incendiary bolt is not told apart yet",
    },
    EventRule { side: Side::Port, kind: "Projectile", name: "IncendiaryBolt", label: "Projectile:ArrowExplosive", note: "" },
    EventRule { side: Side::Original, kind: "Spawn", name: "DisProjectile_Grenade", label: "Projectile:Grenade", note: "thrown grenade (DisGrenade, spawned later, is not the projectile)" },
    EventRule { side: Side::Port, kind: "Projectile", name: "Grenade", label: "Projectile:Grenade", note: "" },
    EventRule { side: Side::Port, kind: "Projectile", name: "StickyGrenade", label: "Projectile:Grenade", note: "DisProjectile_StickyGrenade is a subclass: the hook records its own class name" },
    EventRule { side: Side::Original, kind: "Spawn", name: "DisProjectile_StickyGrenade", label: "Projectile:Grenade", note: "" },
    // Explosions: an explosion actor on the original, `Explosion` on the port (which spawns the
    // grenade explosion effect from it).
    EventRule { side: Side::Original, kind: "Spawn", name: "DisExplosion", label: "Explosion", note: "DisExplosion actor (grenades, explosive arrows)" },
    EventRule { side: Side::Port, kind: "Sim", name: "Explosion", label: "Explosion", note: "" },
    // Gadgets and powers that spawn actors.
    EventRule { side: Side::Original, kind: "Spawn", name: "DisGadget_SpringRazorPlaced", label: "SpringRazorPlaced", note: "placed Spring Razor actor" },
    EventRule { side: Side::Port, kind: "Sim", name: "TrapPlaced", label: "SpringRazorPlaced", note: "" },
    EventRule { side: Side::Original, kind: "Spawn", name: "DisRatSwarm", label: "SwarmSummoned", note: "Devouring Swarm actor" },
    EventRule { side: Side::Port, kind: "Sim", name: "SwarmSummoned", label: "SwarmSummoned", note: "" },
    // Effects. Pooled ParticleSystemComponents (`EmitterPool.SpawnEmitter`) are components, not
    // actors, so the spawn scan sees only emitter actors: the camera lens effects. The port's
    // runner does not present lens effects yet, so these stay one-sided.
    EventRule { side: Side::Original, kind: "Spawn", name: "DisEmitterCameraLensEffect*", label: "Effect:CameraLens", note: "screen lens effect actor (blink, blood); not presented by the port's runner" },
];

/// The original's behaviour classes (`DishonoredAIBrain.m_pCurrentBehavior`) and the port's
/// brain state with the same role (`dishonored_ai` `BrainState`). A class not listed keeps its
/// own name (`orig:<class>`) so it shows up as a difference instead of being dropped.
pub const BEHAVIOUR_STATES: &[(&str, &str)] = &[
    ("DisBehaviorIdle", "Idle"),
    ("DisBehaviorSoiree", "GroupIdle"),
    ("DisBehaviorWander", "Wander"),
    ("DisBehaviorPatrol", "Patrol"),
    ("DisBehaviorNotice", "Notice"),
    ("DisBehaviorInvestigate", "Investigate"),
    ("DisBehaviorSearch", "Search"),
    ("DisBehaviorReact", "React"),
    ("DisBehaviorMagicResponse", "React"),
    ("DisBehaviorCombat", "Combat"),
    ("DisBehaviorCombatEliteGuard", "Combat"),
    ("DisBehaviorFlee", "Flee"),
    ("DisBehaviorCower", "Cower"),
    ("DisBehaviorRingAlarm", "RingAlarm"),
    ("None", "None"),
];

/// The port's state name of an original behaviour class.
pub fn behaviour_state(class: &str) -> String {
    let c = class.trim();
    BEHAVIOUR_STATES.iter().find(|x| x.0 == c).map(|x| x.1.to_string()).unwrap_or_else(|| format!("orig:{c}"))
}

/// An event in the shared vocabulary.
#[derive(Debug, Clone, PartialEq)]
pub struct CanonEvent {
    pub tick: u32,
    /// `Projectile:Bullet`, `Damage`, `Behaviour`...
    pub label: String,
    /// The object when both sides name it the same way (`player`, `npc.<n>`), else empty.
    pub object: String,
    /// What happened (`Patrol`, the source event), for reports.
    pub what: String,
    /// A value compared between paired events (the damage amount), NaN when none.
    pub value: f64,
}

fn matches(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(p) => name.starts_with(p),
        None => pattern == name,
    }
}

fn from_rules(t: &Trace, side: Side, out: &mut Vec<CanonEvent>) {
    for e in &t.events {
        // The longest matching pattern wins (`DisProjectile_Arrow_Explosive` over a prefix).
        let rule = EVENT_RULES.iter().filter(|r| r.side == side && r.kind == e.kind && matches(r.name, &e.name)).max_by_key(|r| r.name.len());
        if let Some(r) = rule {
            out.push(CanonEvent { tick: e.tick, label: r.label.into(), object: String::new(), what: format!("{} {}", e.kind, e.name), value: f64::NAN });
        }
    }
}

/// Health drops of `pawn.health` (`player`) and `npc.<n>.health`, from the channels on both
/// sides (the same measurement: the first frame showing the lower value).
fn damage(t: &Trace, out: &mut Vec<CanonEvent>) {
    for (i, c) in t.channels.iter().enumerate() {
        let who = if c.name == "pawn.health" {
            "player".to_string()
        } else if let Some(n) = c.name.strip_prefix("npc.").and_then(|r| r.strip_suffix(".health")) {
            format!("npc.{n}")
        } else {
            continue;
        };
        let mut prev = f64::NAN;
        for f in &t.frames {
            let v = f.values[i];
            if v.is_nan() {
                continue;
            }
            if !prev.is_nan() && v < prev - 1e-3 {
                out.push(CanonEvent { tick: f.tick, label: "Damage".into(), object: who.clone(), what: format!("{prev:.1} -> {v:.1}"), value: prev - v });
            }
            prev = v;
        }
    }
}

/// Behaviour changes: the original's `NpcBehavior` events of the current behaviour; the port's
/// `npc.<n>.state` channel (its legend in the unit `enum:Idle,Patrol,...`), first value included.
fn behaviour(t: &Trace, side: Side, out: &mut Vec<CanonEvent>) {
    match side {
        Side::Original => {
            for e in t.events.iter().filter(|e| e.kind == "NpcBehavior" && e.detail == "current") {
                let s = behaviour_state(&e.name);
                out.push(CanonEvent { tick: e.tick, label: "Behaviour".into(), object: e.object.clone(), what: format!("{s} ({})", e.name.trim()), value: f64::NAN });
            }
        }
        Side::Port => {
            for (i, c) in t.channels.iter().enumerate() {
                let Some(n) = c.name.strip_prefix("npc.").and_then(|r| r.strip_suffix(".state")) else { continue };
                let legend: Vec<&str> = c.unit.strip_prefix("enum:").map(|l| l.split(',').collect()).unwrap_or_default();
                let mut prev = f64::NAN;
                for f in &t.frames {
                    let v = f.values[i];
                    if v.is_nan() || v == prev {
                        continue;
                    }
                    prev = v;
                    let s = legend.get(v as usize).copied().unwrap_or("?");
                    out.push(CanonEvent { tick: f.tick, label: "Behaviour".into(), object: format!("npc.{n}"), what: s.to_string(), value: f64::NAN });
                }
            }
        }
    }
}

/// Every mapped event of a trace, by tick.
pub fn canonical_events(t: &Trace, side: Side) -> Vec<CanonEvent> {
    let mut out = Vec::new();
    from_rules(t, side, &mut out);
    damage(t, &mut out);
    behaviour(t, side, &mut out);
    out.sort_by(|a, b| a.tick.cmp(&b.tick).then(a.label.cmp(&b.label)));
    out
}

/// Whether a raw event takes part in the mapping (so it is not also listed as one-sided).
pub fn is_mapped(e: &TraceEvent, side: Side) -> bool {
    (side == Side::Original && e.kind == "NpcBehavior") || EVENT_RULES.iter().any(|r| r.side == side && r.kind == e.kind && matches(r.name, &e.name))
}

/// The key two paired events must share: the label, the object when named on both sides, and
/// for behaviour the state.
pub fn pair_key(e: &CanonEvent) -> String {
    let what = if e.label == "Behaviour" { e.what.split(' ').next().unwrap_or("") } else { "" };
    format!("{}|{}|{what}", e.label, e.object)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawns_and_channels_map_to_labels() {
        let mut o = Trace::new("s", "original", 60, &[("npc.g.health", "pt")]);
        let mut p = Trace::new("s", "port", 60, &[("npc.g.health", "pt"), ("npc.g.state", "enum:Idle,Wander,WalkToPoint,GroupIdle,React,Flee,Patrol")]);
        for k in 0..4u32 {
            o.push(k, vec![if k >= 2 { 35.0 } else { 50.0 }]);
            p.push(k, vec![if k >= 2 { 56.0 } else { 70.0 }, if k >= 3 { 4.0 } else { 6.0 }]);
        }
        let ev = |tick, kind: &str, name: &str, detail: &str| TraceEvent { tick, kind: kind.into(), object: "x".into(), name: name.into(), detail: detail.into() };
        o.events = vec![ev(0, "NpcBehavior", "DisBehaviorPatrol", "current"), ev(1, "Spawn", "DisProjectile_Arrow_Explosive", ""), ev(1, "Spawn", "DishonoredDecalActor", ""), ev(3, "NpcBehavior", "DisBehaviorMagicResponse", "current")];
        o.events[0].object = "npc.g".into();
        o.events[3].object = "npc.g".into();
        p.events = vec![ev(1, "Projectile", "IncendiaryBolt", "")];
        let a = canonical_events(&o, Side::Original);
        let b = canonical_events(&p, Side::Port);
        let keys = |v: &[CanonEvent]| v.iter().map(|e| (e.tick, pair_key(e))).collect::<Vec<_>>();
        assert_eq!(keys(&a), keys(&b));
        assert_eq!(a.iter().find(|e| e.label == "Damage").unwrap().value, 15.0);
        assert_eq!(behaviour_state("DisBehaviorSomethingNew"), "orig:DisBehaviorSomethingNew");
    }
}
