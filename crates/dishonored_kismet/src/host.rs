//! What Kismet asks of the game.
//!
//! The interpreter ([`crate::runtime::Kismet`]) runs the engine's own sequence ops (events,
//! sequences, gates, switches, delays, variables, conditions, remote events, matinee timing).
//! Everything that touches the world or Dishonored's systems (story flags, objectives,
//! spawners and AI, travel, conversations, sounds, presentation) goes through [`Host`].

use crate::model::Obj;
use crate::runtime::Ctx;

/// Outcome of a host op activation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostResult {
    /// Finished now. Outputs set with [`Ctx::fire`] go out, plus the class's automatic outputs
    /// (`bAutoActivateOutputLinks`) as the engine's `DeActivated` does.
    Done,
    /// Keeps running: [`Host::update`] is called every tick until it returns true. Outputs
    /// fired meanwhile go out at once (latent ops propagate outputs while active).
    Latent,
    /// Not supported yet: logged once per class and counted in the coverage report; the op
    /// finishes like [`HostResult::Done`] (its automatic outputs still fire, so the graph
    /// continues as it would after a presentation-only action).
    Unhandled,
}

/// Matinee (`SeqAct_Interp`) notifications.
#[derive(Debug, Clone, PartialEq)]
pub enum Matinee {
    Started { length: f32, reverse: bool },
    Event(String),
    Finished { position: f32, completed: bool },
}

pub trait Host {
    /// Story flag by original name (`SF_*` sets), shared with the mission flags.
    fn flag(&mut self, name: &str) -> bool;
    fn set_flag(&mut self, name: &str, value: bool);
    /// Whether an object is a player-controlled pawn (`bPlayerOnly` events).
    fn is_player(&self, o: &Obj) -> bool;
    /// A game op (any class the interpreter does not run itself).
    fn activated(&mut self, cx: &mut Ctx) -> HostResult;
    /// Latent update of an op that returned [`HostResult::Latent`]; true = finished.
    fn update(&mut self, _cx: &mut Ctx, _dt: f32) -> bool {
        true
    }
    /// `SeqAct_Toggle` on actors (`Targets`): 0 on, 1 off, 2 toggle.
    fn toggle_actors(&mut self, _cx: &mut Ctx, _targets: &[Obj], _mode: u8) {}
    /// Matinee start / event key / finish (presentation and AI scenes are not played).
    fn matinee(&mut self, _cx: &mut Ctx, _what: Matinee) {}
    /// `SeqAct_Log` text.
    fn log(&mut self, _text: &str) {}
    /// Whether the game activates events of this class (or one of its ancestors) at all
    /// (touches, uses, deaths...). Events the interpreter raises itself (level loaded, remote,
    /// sequence activated, console) are not asked. A level's events of a class no one fires are
    /// reported once as unsupported when the level loads.
    fn fires_event(&self, _class: &str) -> bool {
        true
    }
}

/// A host that supports nothing (unit tests and data checks).
#[derive(Debug, Default, Clone)]
pub struct NullHost {
    pub flags: std::collections::BTreeMap<String, bool>,
    pub players: Vec<u32>,
    pub calls: Vec<String>,
}

impl Host for NullHost {
    fn flag(&mut self, name: &str) -> bool {
        self.flags.get(name).copied().unwrap_or(false)
    }
    fn set_flag(&mut self, name: &str, value: bool) {
        self.flags.insert(name.to_string(), value);
    }
    fn is_player(&self, o: &Obj) -> bool {
        matches!(o, Obj::Pawn(p) if self.players.contains(p))
    }
    fn activated(&mut self, cx: &mut Ctx) -> HostResult {
        self.calls.push(cx.class().to_string());
        HostResult::Unhandled
    }
}
