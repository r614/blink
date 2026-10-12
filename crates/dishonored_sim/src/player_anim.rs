//! The player's animation states' requests on its layers (research/ARCHITECTURE.md steps 6-7):
//! what `Twk_Pawn_Corvo`'s animation states (`pFullBodyTweaks`, `pUpperBodyTweaks`,
//! `pSpecialTweaks`: sequence, item prefix, `m_fBlendTime`, `m_bBlocksChildren`) play for the
//! player's state, as the runtime's former pickers chose them (research/LOG.md 2026-10-11, "the
//! runtime draws the simulation's animation state", and `git show 9523dd1:crates/
//! dishonored_runtime/src/viewmodel.rs`), now made by the simulation so that the renderer only
//! skins (`crate::anim::layer`):
//! - the full body (`FullBodyStatePicker`): choke and carry masters, the stun, the mantle, the
//!   slide, the climb, the swim, the air loop, the navigation (`Nav_Default_Ready`,
//!   `Nav_Sprinting_Ready`, `Nav_Crouched`); the choke, carry, stun, mantle, climb and swim
//!   states block the children (`m_bBlocksChildren`);
//! - the upper body (`UpperBodyFilter`): the block's in, loop, won parry and out, and the
//!   one-shots the player's transitions start (equip, crouch in / out, jump in, small land, the
//!   gunshot reaction); the swings and the paired masters are `crate::anim::sync_requests`';
//! - the left arm (`LeftArmFilter`): the possession and Blink aims, the Blink and Windblast
//!   casts, the equips and switches, the sword-slash and jump reactions, the left hand's
//!   navigation (`Ply_Nav_LeftHand_at`: idle, walk, sprint, air; `bNoNotifies`) in the full body's
//!   sync group; the fire, reload, cast and deploy actions are `crate::anim::left_requests`';
//! - the additive over the whole pose: the step-up's head (`StepUpMantle`).
//!
//! A state that continues plays a [`token::NAV`] sequence (asked again, it keeps playing); a
//! transition's one-shot a [`token::ACTION`] one, which plays until its end (on the left arm the
//! navigation takes over a blend time before the end, as the picker's `running_shot` did).

use crate::anim::{PawnAnim, REF_POSE, SeqId, SeqTable, layer, sync, token};
use crate::input::LeftHand;
use crate::state::{ChokePhase, LeftPhase, MoveMode, PlayerState, PossessStage, SwordPhase};
use crate::SimConfig;
use serde::{Deserialize, Serialize};

/// `m_fBlendTime` of the animation states (`Twk_Pawn_Corvo_Release` animation tweaks, as
/// `viewmodel.rs` listed them): the navigation states (`Nav_Default_Ready`, `Nav_Sprinting_Ready`,
/// `Nav_Crouched`, the left hand's `Nav_Default`), the climb and the swim.
pub const BLEND_NAV: f32 = 0.2;
/// `DishonoredAnimNodeBlendBySpeed` `BlendUpTime` / `BlendDownTime` inside a navigation tree
/// (idle / walk / run, the left hand's `WalkingSpeedBlend`).
pub const BLEND_SPEED: f32 = 0.4;
/// `Mantle`.
pub const BLEND_MANTLE: f32 = 0.25;
/// `Choke` (the choke masters cut in).
pub const BLEND_CHOKE: f32 = 0.0;
/// Equip, crouch in / out, small land, the gunshot reaction, stunned, the carry masters, the left
/// hand's sword-slash and jump reactions.
pub const BLEND_ACTION: f32 = 0.2;
/// `AnimMeleeBlock`, `Upper_Jump_In`, the slide.
pub const BLEND_MELEE: f32 = 0.1;
/// The power casts and aims (`CastBlink`, `CastPossess`, `CastWindBlast`), `AnimSwitchPower`.
pub const BLEND_CAST: f32 = 0.1;
/// `DisAnimNode3StateBlend`'s class default `m_fBlendTime` (the slide's children).
pub const BLEND_SLIDE: f32 = 0.2;
/// The climb's and the swim's speed nodes (`AnimNodeBlendBySpeed` class defaults: up / down 0.1).
pub const BLEND_CLIMB: f32 = 0.1;
pub const BLEND_SWIM: f32 = 0.1;
/// `DishonoredAnimNodeBlendByState`'s class default `m_fBlendTime` (a navigation tree's air child).
pub const BLEND_NAV_STATE: f32 = 0.3;
/// `DisAnimNodeBlendByLocoType`'s class default `m_fBlendTime` (the left hand's walk / sprint).
pub const BLEND_LOCO_TYPE: f32 = 0.2;
/// `StatePlayerMasterFalling.m_fAnimateFallTime`: a fall not from a jump shows the air child after
/// it.
pub const ANIMATE_FALL_TIME: f32 = 0.25;
/// `Ply_Player_Nav_Default_at`'s `WalkingSpeedBlend` constraint between walk and run (uu/s).
pub const WALK_SPEED_BLEND: f32 = 250.0;
/// `Ply_Player_Nav_Sprinting_at`'s `SprintSpeedBlend` constraint between idle and sprint (uu/s).
pub const SPRINT_SPEED_BLEND: f32 = 300.0;
/// `DisAnimNodeBlendByStepUpMantle.m_fLowMantleStepUpBlendTime`.
pub const STEP_UP_BLEND: f32 = 0.2;

const SWORD_IDLE_SET: &str = "Ply_Sword_Idle_as";
const SWORD_LOCO_SET: &str = "Ply_Sword_Locomotion_as";
const SWORD_HIT_SET: &str = "Ply_Sword_Hit_as";
const EMPTY_LOCO_SET: &str = "Ply_Empty_Locomotion_as";
const EMPTY_IDLE_SET: &str = "Ply_Empty_Idle_as";
const HEAD_LOCO_SET: &str = "Ply_Head_Locomotion_as";
const GADGETS_SET: &str = "Ply_Gadgets_as";

/// What the player's state was when the last tick ended (the transitions the pickers watched).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlayerPrev {
    pub mode: MoveMode,
    pub crouched: bool,
    pub alive: bool,
    pub swings: u8,
    pub sword: SwordPhase,
    pub item: LeftHand,
    pub left: LeftPhase,
    pub possess: PossessStage,
    pub step_ups: u8,
    pub parries: u8,
}

impl PlayerPrev {
    pub fn of(p: &PlayerState) -> Self {
        PlayerPrev { mode: p.mode, crouched: p.crouched, alive: p.alive, swings: p.sword.swings, sword: p.sword.phase, item: p.left.item, left: p.left.phase, possess: p.possess.stage, step_ups: p.traversal.step_ups, parries: p.sword.parries }
    }
}

/// A sequence a layer should play.
#[derive(Debug, Clone, Copy)]
struct Want {
    seq: SeqId,
    token: u32,
    rate: f32,
    looping: bool,
    blend: f32,
}

/// A continuing state's sequence (`set.name`), if the table has it.
fn nav(t: &SeqTable, set: &str, name: &str, group: u32, rate: f32, looping: bool, blend: f32) -> Option<Want> {
    let seq = t.id_of(set, name)?;
    Some(Want { seq, token: token::NAV | (group << 16) | seq as u32, rate, looping, blend })
}

/// A sequence of the player's sets by name (the first set holding it).
fn find(t: &SeqTable, name: &str) -> Option<SeqId> {
    const SETS: &[&str] = &[SWORD_IDLE_SET, SWORD_LOCO_SET, SWORD_HIT_SET, EMPTY_LOCO_SET, HEAD_LOCO_SET, "Ply_Sword_Versus_as", "Ply_Sword_Choke_as", "Ply_Empty_CarryCorpse_as", "Ply_Guns_as", "Ply_Powers_as", GADGETS_SET, "Ply_Pistol_as", "Ply_Crossbow_as"];
    SETS.iter().find_map(|s| t.id_of(s, name))
}

/// The layer's sequence name.
fn name_of<'a>(a: &PawnAnim, t: &'a SeqTable, l: usize) -> Option<&'a str> {
    a.layers[l].as_ref().filter(|s| s.out.is_none()).and_then(|s| t.name(s.seq))
}

/// Play `w` on the layer unless it already plays it (the same token); the rate follows.
fn apply(a: &mut PawnAnim, l: usize, w: Want, yaw: f32, quiet: bool) {
    match a.layers[l].as_mut() {
        Some(s) if s.token == w.token && s.out.is_none() => s.rate = w.rate,
        _ => a.play_state(l, w.seq, w.token, w.rate, w.looping, yaw, w.blend, quiet),
    }
}

/// Play a one-shot (a fresh [`token::ACTION`]).
fn shot(a: &mut PawnAnim, l: usize, seq: SeqId, rate: f32, blend: f32, yaw: f32) {
    let tok = a.next_shot();
    a.play_state(l, seq, tok, rate, false, yaw, blend, false);
}

/// "Ready" or "Sneak": the stance's item-prefixed sequences (`DisAnimNodeBlendByPlayerSneak`).
fn stance(p: &PlayerState) -> &'static str {
    if p.crouched { "Sneak" } else { "Ready" }
}

/// The left hand's item prefix (`m_ApplyItemEquipPrefix = EDisEquipUsage_Secondary`).
pub fn left_prefix(item: LeftHand) -> Option<&'static str> {
    match item {
        LeftHand::Empty => None,
        LeftHand::Pistol | LeftHand::Crossbow | LeftHand::SleepDart | LeftHand::IncendiaryBolt => Some("Guns_"),
        LeftHand::Grenade | LeftHand::StickyGrenade | LeftHand::SpringRazor => Some("Gadgets_"),
        _ => Some("Powers_"),
    }
}

/// The full body's state (`FullBodyStatePicker`) and whether it blocks the children; `None`
/// while a paired move's master owns the layer.
fn full_body(p: &PlayerState, a: &PawnAnim, cfg: &SimConfig) -> (Option<Want>, bool) {
    let t = &cfg.anim;
    let mv = &cfg.gameplay.movement;
    if a.layers[layer::FULL_BODY].as_ref().is_some_and(|s| s.token & token::KIND == token::PAIRED) {
        return (None, true);
    }
    let cur = name_of(a, t, layer::FULL_BODY).unwrap_or("");
    let cur_done = a.layers[layer::FULL_BODY].as_ref().is_none_or(|s| s.done());
    // Choke (`Ply_Sword_Choke_as` masters by `eChokeState`, blend 0), whole rig.
    let choke = match p.choke.phase {
        ChokePhase::In => Some(("Sword_Choke_In_Master", false)),
        ChokePhase::Loop => Some(("Sword_Choke_Loop_Master", true)),
        ChokePhase::Win => Some(("Sword_Choke_Win_Master", false)),
        ChokePhase::Lose => Some(("Sword_Choke_Lose_Master", false)),
        ChokePhase::Cancel => Some(("Sword_Choke_Cancel_Master", false)),
        ChokePhase::None => None,
    };
    if let Some((n, looping)) = choke {
        return (nav(t, crate::anim::CHOKE_SET, n, sync::NONE, 1.0, looping, BLEND_CHOKE), true);
    }
    let speed = p.velocity.truncate().length();
    // Carrying a body (`Ply_Empty_CarryCorpse_as` masters): the lift (from a won choke the
    // choke's `CarryCorpse` master), then the walk or idle loop.
    if p.carrying.is_some() {
        if p.carry_anim > 0.0 {
            let from_choke = cur.starts_with("Sword_Choke");
            let w = if from_choke { nav(t, crate::anim::CHOKE_SET, "Sword_Choke_CarryCorpse_Master", sync::NONE, 1.0, false, BLEND_CHOKE) } else { nav(t, crate::anim::CARRY_SET, "Empty_CarryCorpse_In_Master", sync::NONE, 1.0, false, BLEND_ACTION) };
            return (w, true);
        }
        let walking = p.mode == MoveMode::Walking && speed > 30.0;
        let w = if walking { nav(t, crate::anim::CARRY_SET, "Empty_CarryCorpse_Walk_Master", sync::NONE, (speed / 200.0).clamp(0.6, 1.4), true, BLEND_NAV) } else { nav(t, crate::anim::CARRY_SET, "Empty_CarryCorpse_Idle_Master", sync::NONE, 1.0, true, BLEND_NAV) };
        return (w, true);
    }
    // The low drop plays out over the action lock that follows it.
    if p.action_lock > 0.0 && cur.starts_with("Empty_CarryCorpse") {
        return (nav(t, crate::anim::CARRY_SET, "Empty_CarryCorpse_DropLow_Master", sync::NONE, 1.0, false, BLEND_ACTION), true);
    }
    // `AnimStunned` (`Ply_Player_Stunned_at`, `DishonoredAnimNodeBlendByStunState` 0x00A89BC0): a
    // versus stun's intro (`Stun_In`, cut in: 0x00A686A0 gives states 0-6 no blend), then the loop
    // (state 7 over `m_fStunBlendTime`, not loaded: the state tweak's 0.2).
    if p.sword.phase == SwordPhase::Staggered {
        let in_done = cur == "Sword_Stun_Loop" || (cur == "Sword_Stun_In" && cur_done);
        let w = if in_done { nav(t, SWORD_HIT_SET, "Sword_Stun_Loop", sync::NONE, 1.0, true, BLEND_ACTION) } else { nav(t, SWORD_HIT_SET, "Sword_Stun_In", sync::NONE, 1.0, false, 0.0) };
        if w.is_some() {
            return (w, true);
        }
    }
    // `Mantle` (`Ply_Player_Mantle_at`): `DishonoredAnimNodeBlendByMantleType` (0x00A89810) picks
    // low / medium / high from the mantle state's `m_fCurrentMantleHeight` against
    // `m_LowMantleMaxEdgeHeight` / `m_MediumMantleMaxEdgeHeight` (120 / 170); under it
    // `DisAnimNodeBlendByPlayerSneak` (0x00A88E10) the sneaking sequences, and over it
    // `DisAnimNodeBlendByPlayerStance` (0x00A9A3E0) the empty-handed ones when the sword is not
    // ready; `DishonoredAnimNodeSeqMantle`'s rate is `Attribute_MantleAnimRate` (0x00A97E30; 1 for
    // Corvo).
    if p.mode == MoveMode::Mantling {
        let e = p.traversal.edge_height;
        let h = if e <= mv.mantle.low_max_edge_height {
            "Low"
        } else if e <= mv.mantle.medium_max_edge_height {
            "Medium"
        } else {
            "High"
        };
        let ready = crate::movement::sword_in_right_hand(p);
        let (set, name) = match (ready, p.crouched) {
            (true, false) => (SWORD_LOCO_SET, format!("Sword_Ready_Mantle{h}")),
            (true, true) => (SWORD_LOCO_SET, format!("Sword_Sneak_Mantle{h}")),
            (false, false) => (EMPTY_LOCO_SET, format!("Empty_Mantle{h}")),
            (false, true) => (EMPTY_LOCO_SET, format!("Empty_Sneak_Mantle{h}")),
        };
        let rate = crate::powers::attribute(p, &cfg.gameplay, "Attribute_MantleAnimRate", 1.0);
        if let Some(w) = nav(t, set, &name, sync::NONE, if rate > 0.0 { rate } else { 1.0 }, false, BLEND_MANTLE) {
            return (Some(w), true);
        }
    }
    // `Slide` (`Ply_Player_Slide_at`, `DisAnimNode3StateBlend` 0x00A7D810, the `Sword_` item
    // prefix, children blended over its 0.2): in, then the loop; when the slide ends the outro's
    // `DisAnimNodeBlendByLocoType` picks the sprint's or the sneak's out. The 3-state blend's intro
    // and loop times are not decoded: the in plays to its end.
    if p.slide.active {
        let in_done = cur == "Sword_SlideLoop" || (cur == "Sword_SlideIn" && cur_done);
        let w = if in_done { nav(t, SWORD_LOCO_SET, "Sword_SlideLoop", sync::NONE, 1.0, true, BLEND_SLIDE) } else { nav(t, SWORD_LOCO_SET, "Sword_SlideIn", sync::NONE, 1.0, false, if cur.is_empty() || !cur.starts_with("Sword_Slide") { BLEND_MELEE } else { BLEND_SLIDE }) };
        if w.is_some() {
            return (w, false);
        }
    }
    if p.mode == MoveMode::Walking && cur.starts_with("Sword_Slide") {
        let out = if cur.starts_with("Sword_SlideOut") { cur.to_string() } else if p.sprinting { "Sword_SlideOutSprint".into() } else { "Sword_SlideOutSneak".into() };
        if !(cur == out && cur_done) {
            if let Some(w) = nav(t, SWORD_LOCO_SET, &out, sync::NONE, 1.0, false, BLEND_SLIDE) {
                return (Some(w), false);
            }
        }
    }
    // `Climb` (`Ply_Player_Climb_at`): a `DishonoredAnimNodeBlendBySpeed` on the 3D acceleration
    // (`bUseAcceleration`) with the class defaults (constraints [0, 180], `BlendDownPerc` 0.2, 0.1
    // s): its empty child (the reference pose) up to 36, `Head_Rope_Climb` above. The climb's
    // acceleration is the input's direction at the ladder's rate: any input is above 36.
    if p.mode == MoveMode::Climbing {
        let pushing = p.last_input.forward != 0 || p.last_input.right != 0;
        let climbing = cur == "Head_Rope_Climb" || a.layers[layer::FULL_BODY].as_ref().is_some_and(|s| s.seq == REF_POSE && s.out.is_none());
        let blend = if climbing { BLEND_CLIMB } else { BLEND_NAV };
        if pushing {
            if let Some(w) = nav(t, HEAD_LOCO_SET, "Head_Rope_Climb", sync::NONE, 1.0, true, blend) {
                return (Some(w), true);
            }
        }
        return (Some(Want { seq: REF_POSE, token: token::NAV | REF_POSE as u32, rate: 1.0, looping: true, blend }), true);
    }
    // `Nav_Swim` (`Ply_Player_Nav_Swimming_at`): `DisAnimNodeBlendBySwimState` (0x00A89040) then a
    // `DishonoredAnimNodeBlendBySpeed` on the controller's input (`m_bUseControllerInputSpeed`, 2D;
    // constraints [0.2, 1.0], `BlendDownPerc` 0.2): `Empty_SwimIdle` up to an input of 0.36, above it
    // a `DisAnimNodeBlendDirectional` of `Empty_SwimN` / `S` / `W` / `E` by the input's direction.
    // A layer plays one sequence: the direction's dominant loop (the directional blend's
    // neighbours are not mixed); the speed node's 0.2 s delays are not modelled.
    if p.mode == MoveMode::Swimming {
        let (f, r) = (p.last_input.forward as f32 / 127.0, p.last_input.right as f32 / 127.0);
        let w = if (f * f + r * r).sqrt() <= 0.36 {
            nav(t, EMPTY_IDLE_SET, "Empty_SwimIdle", sync::NONE, 1.0, true, BLEND_SWIM)
        } else {
            let d = if f.abs() >= r.abs() { if f >= 0.0 { "N" } else { "S" } } else if r >= 0.0 { "E" } else { "W" };
            nav(t, EMPTY_LOCO_SET, &format!("Empty_Swim{d}"), sync::NONE, 1.0, true, BLEND_SWIM)
        };
        if w.is_some() {
            return (w, true);
        }
    }
    // The navigation states (`Nav_Default_Ready`, `Nav_Sprinting_Ready`, `Nav_Crouched`, entered
    // over their `m_fBlendTime` 0.2): `DishonoredAnimNodeBlendByState` (0x00A89A50, 0.3 s) shows the
    // air child (`CustomAirIdle`) in the jump state or once a fall not from a jump is older than
    // `m_fAnimateFallTime` (0.25 s); else the speed blend (`DishonoredAnimNodeBlendBySpeed`
    // 0x00594BA0 over the 2D speed of 0x00A89900; the crouched tree's stock node over the 3D
    // speed): `Nav_Default` idle at 0, walk up to 250, run above; `Nav_Sprinting` idle up to 300,
    // sprint above; `Nav_Crouched` idle at 0, the sneak walk above; switches over 0.4 s. The loops
    // play at rate 1 (no node scales them by speed; the groups' rate scales are 1).
    let tree_now = if p.crouched { 1 } else if p.sprinting { 2 } else { 0 };
    let air = p.mode == MoveMode::Falling && (p.fall_from_jump || p.fall_time > ANIMATE_FALL_TIME);
    let (set, name, group) = if air {
        (SWORD_LOCO_SET, if p.crouched { "Sword_Sneak_Jump" } else { "Sword_Ready_Jump" }, sync::NONE)
    } else if p.crouched {
        if p.velocity.length() > 0.0 { (SWORD_LOCO_SET, "Sword_Sneak_Walk", sync::NONE) } else { (SWORD_IDLE_SET, "Sword_Sneak_Idle", sync::IDLE) }
    } else if p.sprinting {
        if speed > SPRINT_SPEED_BLEND { (SWORD_LOCO_SET, "Sword_Ready_Sprint", sync::SPRINT) } else { (SWORD_IDLE_SET, "Sword_Ready_Idle", sync::IDLE) }
    } else if speed <= 0.0 {
        (SWORD_IDLE_SET, "Sword_Ready_Idle", sync::IDLE)
    } else if speed <= WALK_SPEED_BLEND {
        (SWORD_LOCO_SET, "Sword_Ready_Walk", sync::NONE)
    } else {
        (SWORD_LOCO_SET, "Sword_Ready_Run", sync::RUN)
    };
    let prev_tree = a.nav_tree;
    let was_air = cur.ends_with("_Jump");
    let blend = if prev_tree != Some(tree_now) { BLEND_NAV } else if air != was_air { BLEND_NAV_STATE } else { BLEND_SPEED };
    (nav(t, set, name, group, 1.0, true, blend), false)
}

/// The left hand's navigation (`Ply_Nav_LeftHand_at`'s `Nav_Default`, `bNoNotifies`): its
/// `DishonoredAnimNodeBlendByState` shows `CustomAirIdle` falling (as the full body's); its speed
/// blend (constraints [0, 250] on the 2D speed) the idle at 0, else `DisAnimNodeBlendByLocoType`
/// (0x00A88D10, 0.2 s): `CustomSprint` sprinting, `CustomWalk` walking or running (there is no
/// `Run` loop for a left-hand item).
fn left_nav(p: &PlayerState, a: &PawnAnim, t: &SeqTable, pre: &str) -> Option<Want> {
    let speed = p.velocity.truncate().length();
    let air = p.mode == MoveMode::Falling && (p.fall_from_jump || p.fall_time > ANIMATE_FALL_TIME);
    let (suffix, group) = if air {
        ("Jump", sync::NONE)
    } else if speed <= 0.0 {
        ("Idle", sync::IDLE)
    } else if p.sprinting {
        ("Sprint", sync::SPRINT)
    } else {
        ("Walk", sync::RUN)
    };
    let seq = find(t, &format!("{pre}{suffix}")).or_else(|| find(t, &format!("{pre}Idle")))?;
    let cur = a.layers[layer::LEFT_ARM].as_ref().filter(|s| s.token & token::KIND == token::NAV && s.out.is_none());
    let cur_name = cur.and_then(|s| t.name(s.seq)).unwrap_or("");
    let blend = match cur {
        None => BLEND_NAV,
        Some(_) if cur_name.ends_with("Jump") != air => BLEND_NAV_STATE,
        Some(_) if cur_name.ends_with("Idle") || suffix == "Idle" => BLEND_SPEED,
        Some(_) => BLEND_LOCO_TYPE,
    };
    Some(Want { seq, token: token::NAV | (group << 16) | seq as u32, rate: 1.0, looping: true, blend })
}

/// The player's states' requests this tick (after its states ran, before its advance), around
/// the gameplay sequences `crate::anim::sync_requests` keeps (the swings, the paired masters, the
/// left hand's actions, the crossbow mesh).
pub fn requests(p: &PlayerState, cfg: &SimConfig, a: &mut PawnAnim) {
    let t = &cfg.anim;
    let prev = a.prev.unwrap_or_else(|| PlayerPrev::of(p));
    a.prev = Some(PlayerPrev::of(p));
    let yaw = p.yaw;
    let st = stance(p);
    let pre = left_prefix(p.left.item);

    // The full body.
    let (fb, blocks) = full_body(p, a, cfg);
    if let Some(w) = fb {
        apply(a, layer::FULL_BODY, w, yaw, false);
    }
    const NAV_LOOPS: [&str; 8] = ["Sword_Ready_Idle", "Sword_Ready_Walk", "Sword_Ready_Run", "Sword_Ready_Sprint", "Sword_Ready_Jump", "Sword_Sneak_Idle", "Sword_Sneak_Walk", "Sword_Sneak_Jump"];
    a.nav_tree = fb.and_then(|w| t.name(w.seq)).filter(|n| NAV_LOOPS.contains(n)).map(|_| if p.crouched { 1 } else if p.sprinting { 2 } else { 0 });

    // The upper body: the block (`AnimMeleeBlock`, `Ply_Player_MeleeBlock_at`): its in
    // (`Sword_Ready_ParryIdle_In`, crouched `Sword_Sneak_ParryIdle_In`, whose
    // `DisNotify_BlockParryZone` is the parry window), then the loop; a won parry plays
    // `Blocking Parry` over it (an `AnimNodeRandom` of `Sword_Ready_Parry_A` / `B` / `C` at
    // 0.33 / 0.33 / 0.34, rolled from the pawn's stream: the node's own roll is UE3's unseeded
    // `appFrand`, LIKELY).
    let paired_upper = a.layers[layer::UPPER_BODY].as_ref().is_some_and(|s| s.token & token::KIND == token::PAIRED);
    let swinging = matches!(p.sword.phase, SwordPhase::Startup | SwordPhase::Active | SwordPhase::Recovery);
    let block_kind = |a: &PawnAnim| a.layers[layer::UPPER_BODY].as_ref().filter(|s| s.token & token::KIND == token::BLOCK && s.out.is_none()).map(|s| (s.token & !token::KIND, s.done()));
    if blocks && !paired_upper {
        a.release(layer::UPPER_BODY);
    } else if p.sword.phase == SwordPhase::Blocking && !paired_upper {
        let won = p.sword.parries != prev.parries;
        match block_kind(a) {
            _ if won => {
                let r = a.roll();
                let n = if r < 0.33 { "Sword_Ready_Parry_A" } else if r < 0.66 { "Sword_Ready_Parry_B" } else { "Sword_Ready_Parry_C" };
                if let Some(seq) = find(t, n) {
                    a.shots = a.shots.wrapping_add(1);
                    a.play_state(layer::UPPER_BODY, seq, token::BLOCK | block::PARRY | (a.shots & 0xFFFF), 1.0, false, yaw, BLEND_MELEE, false);
                }
            }
            Some((k, false)) if k == block::IN || k & block::PARRY != 0 => {}
            Some((block::LOOP, _)) => {}
            Some(_) => {
                if let Some(seq) = find(t, "Sword_Ready_ParryIdle") {
                    a.play_state(layer::UPPER_BODY, seq, token::BLOCK | block::LOOP, 1.0, true, yaw, BLEND_MELEE, false);
                }
            }
            None => {
                if let Some(seq) = t.id_of(crate::anim::SWORD_VERSUS_SET, &format!("Sword_{st}_ParryIdle_In")) {
                    a.play_state(layer::UPPER_BODY, seq, token::BLOCK | block::IN, 1.0, false, yaw, BLEND_MELEE, false);
                }
            }
        }
    } else {
        if block_kind(a).is_some() {
            a.stop(layer::UPPER_BODY);
        }
        // The transitions' one-shots (the last one asked wins, as the picker's `ub_shot`): respawn
        // `EquipItem_Ready`, `Upper_GoTo_Crouch` / `Upper_Leave_Crouch`, `Upper_Jump_In`,
        // `Upper_Jump_Land_Small`, the shot's `PistolReaction` / `CrossbowReaction`, the block's
        // `Block Canceled`.
        let mut want: Option<(String, f32)> = None;
        if p.alive && !prev.alive {
            want = Some(("Sword_Ready_Equip".into(), BLEND_ACTION));
        }
        if p.crouched != prev.crouched && p.mode == MoveMode::Walking {
            want = Some((if p.crouched { "Sword_Sneak_In" } else { "Sword_Sneak_Out" }.into(), BLEND_ACTION));
        }
        if prev.mode == MoveMode::Walking && p.mode == MoveMode::Falling && p.velocity.z > 0.0 {
            want = Some((format!("Sword_{st}_JumpIn"), BLEND_MELEE));
        }
        if prev.mode == MoveMode::Falling && p.mode == MoveMode::Walking {
            want = Some((format!("Sword_{st}_JumpLandSmall"), BLEND_ACTION));
        }
        let gun = p.left.item == LeftHand::Pistol || p.left.item.is_crossbow();
        if gun && p.left.phase == LeftPhase::Recovering && prev.left != LeftPhase::Recovering {
            want = Some((format!("Sword_{st}_GunshotReaction"), BLEND_ACTION));
        }
        if prev.sword == SwordPhase::Blocking && p.sword.phase == SwordPhase::Idle {
            want = Some((format!("Sword_{st}_ParryIdle_Out"), BLEND_MELEE));
        }
        let free = !swinging && !paired_upper && !blocks;
        if let (true, Some((n, blend))) = (free, want) {
            if let Some(seq) = find(t, &n) {
                shot(a, layer::UPPER_BODY, seq, 1.0, blend, yaw);
            }
        }
        // A one-shot that ended leaves the layer.
        if a.layers[layer::UPPER_BODY].as_ref().is_some_and(|s| s.token & token::KIND == token::ACTION && !s.playing) {
            a.stop(layer::UPPER_BODY);
        }
    }

    // The left arm.
    // A left-hand action holds the arm until its state is done with it (a reload's in holds its
    // last frame through the timed stage).
    let acting = matches!(p.left.phase, LeftPhase::Recovering | LeftPhase::Reloading);
    let left_busy = a.layers[layer::LEFT_ARM].as_ref().is_some_and(|s| s.token & token::KIND == token::LEFT && s.out.is_none() && (acting || !(s.done() || s.advanced)));
    match pre {
        _ if blocks => {
            if !left_busy {
                a.release(layer::LEFT_ARM);
            }
        }
        None => {
            if !left_busy {
                a.release(layer::LEFT_ARM);
            }
        }
        Some(_) if left_busy => {}
        Some(pre) => left_arm(p, &prev, a, t, pre, yaw),
    }

    // The step-up's head (`StepUpMantle`, a step-up 0x00AD35E0): `ADD_Head_Mantle_Low` over the
    // whole pose, blended in and out over `m_fLowMantleStepUpBlendTime`.
    if p.traversal.step_ups != prev.step_ups {
        if let Some(seq) = t.id_of(HEAD_LOCO_SET, "ADD_Head_Mantle_Low") {
            shot(a, layer::ADDITIVE, seq, 1.0, STEP_UP_BLEND, yaw);
        }
    } else if a.layers[layer::ADDITIVE].as_ref().is_some_and(|s| !s.playing) {
        a.stop(layer::ADDITIVE);
    }
}

/// The block's sequences on the upper body ([`token::BLOCK`]'s low bits).
pub mod block {
    /// `Sword_Ready_ParryIdle_In` / `Sword_Sneak_ParryIdle_In`.
    pub const IN: u32 = 0;
    /// `Sword_Ready_ParryIdle`.
    pub const LOOP: u32 = 1;
    /// A won parry (`Sword_Ready_Parry_A` / `B` / `C`), with its serial below.
    pub const PARRY: u32 = 1 << 16;
}

/// The left arm's states besides the actions (`LeftArmStatePicker`).
fn left_arm(p: &PlayerState, prev: &PlayerPrev, a: &mut PawnAnim, t: &SeqTable, pre: &str, yaw: f32) {
    let item = p.left.item;
    let cur = name_of(a, t, layer::LEFT_ARM).unwrap_or("").to_string();
    let cur_done = a.layers[layer::LEFT_ARM].as_ref().is_none_or(|s| s.done());
    // A continuing state: the possession's aim (`CastPossess`: in, the loop while targeting) and
    // the Blink's (`CastBlink`: in, the loop while aiming).
    let aim = match (item, p.possess.stage, p.left.phase) {
        (LeftHand::Possession, PossessStage::Targeting, _) => Some(("Powers_Cast_Possession_In", "Powers_Cast_Possession_Loop")),
        (LeftHand::Blink, _, LeftPhase::Aiming) => Some(("Powers_Cast_Blink_In", "Powers_Cast_Blink_Loop")),
        _ => None,
    };
    if let Some((intro, lp)) = aim {
        let in_done = cur == lp || (cur == intro && cur_done);
        let w = if in_done { nav(t, "Ply_Powers_as", lp, sync::NONE, 1.0, true, BLEND_CAST) } else { nav(t, "Ply_Powers_as", intro, sync::NONE, 1.0, false, BLEND_CAST) };
        if let Some(w) = w {
            apply(a, layer::LEFT_ARM, w, yaw, false);
            return;
        }
    }
    // The transitions' one-shots (the last one asked wins, as the picker's `la_shot`).
    let st_new = p.left.phase != prev.left || item != prev.item;
    let mut want: Option<(String, f32, f32)> = None;
    if prev.mode == MoveMode::Walking && p.mode == MoveMode::Falling && p.velocity.z > 0.0 {
        want = Some((format!("{pre}JumpIn"), 1.0, BLEND_ACTION));
    }
    if prev.mode == MoveMode::Falling && p.mode == MoveMode::Walking {
        want = Some((format!("{pre}JumpLandSmall"), 1.0, BLEND_ACTION));
    }
    // A new swing: `SwordSlash_Left` / `SwordSlash_Right` by the swing's side.
    if p.sword.swings != prev.swings && matches!(p.sword.phase, SwordPhase::Startup | SwordPhase::Active | SwordPhase::Recovery) {
        let side = if matches!(p.sword.swing_clip, 1 | 3) { "Left" } else { "Right" };
        want = Some((format!("{pre}SwordSlash_{side}"), 1.0, BLEND_ACTION));
    }
    // Item change: `AnimSwitchPower` between items of one kind, else `EquipItem_Ready`.
    if item != prev.item {
        let switch = format!("{pre}Switch");
        want = Some(if left_prefix(prev.item) == Some(pre) && find(t, &switch).is_some() { (switch, 1.0, BLEND_CAST) } else { (format!("{pre}Ready_Equip"), 1.0, BLEND_ACTION) });
    }
    // The casts the left hand's state plays without a gameplay sequence (their effects are
    // timed by the powers: the Windblast's `DisNotify_WindBlast` is `CombatTuning::
    // windblast_start_delay`, the Blink's travel its own state): `CastWindBlast`, `CastBlink`'s out.
    if st_new && p.left.phase == LeftPhase::Recovering {
        match item {
            LeftHand::Blink => want = Some(("Powers_Cast_Blink_Out".into(), 1.0, BLEND_CAST)),
            LeftHand::Windblast => want = Some(("Powers_Cast_Windblast".into(), 1.0, BLEND_CAST)),
            _ => {}
        }
    }
    // Cancels: `CastPossess` Cancel, `CastBlink` Cancel (`Powers_Cast_Blink_Out`).
    if prev.possess == PossessStage::Targeting && p.possess.stage == PossessStage::Inactive && item == LeftHand::Possession {
        want = Some(("Powers_Cast_Possession_Cancel".into(), 1.0, BLEND_CAST));
    }
    if item == LeftHand::Blink && prev.item == LeftHand::Blink && prev.left == LeftPhase::Aiming && p.left.phase == LeftPhase::Idle {
        want = Some(("Powers_Cast_Blink_Out".into(), 1.0, BLEND_CAST));
    }
    if let Some((n, rate, blend)) = want {
        if let Some(seq) = find(t, &n) {
            shot(a, layer::LEFT_ARM, seq, rate, blend, yaw);
            return;
        }
    }
    // A one-shot plays until a blend time before its end (`running_shot`).
    if let Some(s) = a.layers[layer::LEFT_ARM].as_ref().filter(|s| s.token & token::KIND == token::ACTION && s.out.is_none()) {
        let left = t.info(s.seq).map_or(0.0, |i| s.time_left(i));
        if s.playing && left > s.blend {
            return;
        }
    }
    // An action whose state is done with it (its exit crossed: `left_hand` ended the state) gives
    // the arm back to the navigation, which blends in from the action's pose.
    if let Some(w) = left_nav(p, a, t, pre) {
        apply(a, layer::LEFT_ARM, w, yaw, true);
    }
}

/// The left hand's navigation loop in the full body's sync group takes its relative position
/// (UE3 `UpdateAnimNodeSeqGroups`: the group's master is the heaviest member, the full body's
/// loop), after every advance; without notifies (`bNoNotifies`).
pub fn sync_groups(a: &mut PawnAnim, t: &SeqTable) {
    let group = |s: &crate::anim::SeqPlay| if s.token & token::KIND == token::NAV { (s.token >> 16) & 0xFF } else { sync::NONE };
    let Some(fb) = a.layers[layer::FULL_BODY].as_ref().filter(|s| s.out.is_none()) else { return };
    let g = group(fb);
    if g == sync::NONE {
        return;
    }
    let Some(lf) = t.info(fb.seq).map(|i| i.duration).filter(|l| *l > 0.0) else { return };
    let phase = (fb.time / lf).rem_euclid(1.0);
    if let Some(la) = a.layers[layer::LEFT_ARM].as_mut().filter(|s| s.out.is_none() && group(s) == g) {
        if let Some(ll) = t.info(la.seq).map(|i| i.duration).filter(|l| *l > 0.0) {
            la.time = phase * ll;
        }
    }
}

// ---- the third-person body of a human player ----

/// A human player's third-person body on `NPC_Human_at` (the NPC profile's tree and
/// `DisAnimDefinitions` chains, as the runtime's former third-person picker played them through
/// the NPC profile, `git show 9523dd1:crates/dishonored_runtime/src/npc_anim.rs`): the chain
/// playing on `FullBodySlot`, its sequence's player, and the player's state the last tick.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Body {
    pub layer: Option<crate::anim::SeqPlay>,
    pub chain: Option<BodyChain>,
    pub prev: Option<PlayerPrev>,
    /// Chains played so far (wrapping): the layer token's serial.
    pub serial: u32,
}

/// A chain on the body: `eDisNPCAnimations`, group, step, and what holds a looping step.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BodyChain {
    pub anim: u8,
    pub group: u8,
    pub step: u8,
    pub hold: u8,
}

/// What holds a body chain's looping step ([`BodyChain::hold`]); [`hold::DEAD`] holds the last
/// frame of a death (the slot's negative blend-out).
pub mod hold {
    pub const NONE: u8 = 0;
    pub const BLOCK: u8 = 1;
    pub const STAGGER: u8 = 2;
    pub const CROUCH: u8 = 3;
    pub const DEAD: u8 = 4;
}

/// The cover loop's pseudo chain (`Sword_Cover_Loop`, held while an armed player crouches still:
/// the NPC sets have no crouched locomotion).
const COVER: u8 = 0xFF;

/// Whether a body chain's hold still holds.
fn holds(h: u8, p: &PlayerState) -> bool {
    match h {
        hold::BLOCK => p.sword.phase == SwordPhase::Blocking,
        hold::STAGGER => p.sword.phase == SwordPhase::Staggered,
        hold::CROUCH => p.crouched && p.velocity.truncate().length() < 20.0,
        hold::DEAD => true,
        _ => false,
    }
}

/// Whether the player holds its sword (the `Sword` modifier and item prefix of its body).
pub fn body_armed(p: &PlayerState) -> bool {
    p.carrying.is_none() && crate::inventory::owns(p, crate::inventory::weapon::SWORD)
}

/// Start a chain's step on the body (`step` of `group`, the group at random when `group` < 0);
/// false when it has no sequence.
fn body_play(a: &mut PawnAnim, cfg: &SimConfig, armed: bool, chain: BodyChain, group: i8, yaw: f32) -> bool {
    let Some(ch) = cfg.gameplay.npc_anim.defs.chains.get(chain.anim as usize).filter(|c| !c.groups.is_empty()) else { return false };
    let g = if group < 0 { ((a.roll() * ch.groups.len() as f32) as usize).min(ch.groups.len() - 1) } else { group as usize };
    let Some(grp) = ch.groups.get(g) else { return false };
    let k = if (chain.step as usize) < grp.steps.len() { chain.step as usize } else { 0 };
    let r = a.roll();
    let Some(seq) = crate::pawn_states::step_sequence(cfg, &grp.steps[k], r, armed) else { return false };
    let (looping, blend_in, blend_out) = (grp.steps[k].looped, grp.steps[k].blend_in, grp.blend_out);
    let body = a.body.get_or_insert_with(Body::default);
    body.serial = body.serial.wrapping_add(1);
    let mut s = crate::anim::SeqPlay { seq, token: token::BODY | (body.serial & 0x00FF_FFFF), time: 0.0, prev: 0.0, rate: 1.0, looping, playing: true, yaw, exited: false, interruptible: false, advanced: false, windows: Vec::new(), elapsed: 0.0, blend_in, blend_out, blend: blend_in, out: None, quiet: true };
    if chain.hold == hold::DEAD {
        // A death holds its last frame (`blend_out` negative on the slot).
        s.blend_out = -1.0;
    }
    body.layer = Some(s);
    body.chain = Some(BodyChain { group: g as u8, step: k as u8, ..chain });
    true
}

/// The body's requests after the pawn group (with this tick's events), for every human player:
/// the reactions to the tick's damage and death (`HitReactStrong`, `HitReactProjectile`,
/// `HitReactWindblast` by direction, `Death` held), then its states (a new swing's `AttackShort`
/// group, group 5 in a paired move; a won parry's `Parry` group 0; the block's
/// `Parry_AwaitingImpact`; the stagger's `Stun` group 0; the gun's `AimIn` / `AimShoot` /
/// `AimReload`; an armed crouch's cover loop). They play from the next tick's advance.
pub fn body_requests(state: &mut crate::state::WorldState, cfg: &SimConfig, events: &[crate::events::SimEvent]) {
    use crate::events::{DamageCause as C, SimEvent as E};
    use dishonored_ir::npc_anim::NpcAnim;
    for i in 0..state.players.len() {
        let p = state.players[i];
        if p.kind != crate::state::PawnKind::Human {
            continue;
        }
        let angle_from = |state: &crate::state::WorldState, from: Option<u32>| {
            from.and_then(|k| state.player(k)).map_or(0.0, |k| {
                let d = (k.position - p.position).truncate();
                let fwd = crate::movement::forward_vector(p.yaw).truncate();
                let right = crate::movement::right_vector(p.yaw).truncate();
                d.dot(right).atan2(d.dot(fwd)).to_degrees()
            })
        };
        let mut next: Option<(NpcAnim, i8, u8, u8)> = None;
        let mut dead_now = false;
        for e in events {
            match *e {
                E::Damage { victim, attacker, cause, .. } if victim == p.id && p.alive => {
                    let q = angle_from(state, attacker);
                    next = match cause {
                        C::Sword | C::SwordCounter => Some((NpcAnim::HitReactStrong, -1, cardinal(q), hold::NONE)),
                        C::Windblast => Some((NpcAnim::HitReactWindblast, -1, cardinal(q), hold::NONE)),
                        // Steps: (none), head, stomach, left leg, right leg, front left, front right.
                        C::PistolHeadshot | C::CrossbowHeadshot => Some((NpcAnim::HitReactProjectile, 0, 1, hold::NONE)),
                        C::Pistol | C::Crossbow => {
                            let r = state.anim.entry(p.id).or_insert_with(|| PawnAnim::new(p.id)).roll();
                            Some((NpcAnim::HitReactProjectile, 0, 2 + ((r * 5.0) as u8).min(4), hold::NONE))
                        }
                        _ => next,
                    };
                }
                E::Death { victim, killer, .. } if victim == p.id => {
                    let q = angle_from(state, killer);
                    next = Some((NpcAnim::Death, -1, crate::anim::eight_way(q) as u8, hold::DEAD));
                    dead_now = true;
                }
                _ => {}
            }
        }
        let a = state.anim.entry(p.id).or_insert_with(|| PawnAnim::new(p.id));
        let body = a.body.get_or_insert_with(Body::default);
        let prev = body.prev.unwrap_or_else(|| PlayerPrev::of(&p));
        body.prev = Some(PlayerPrev::of(&p));
        let cur = body.chain;
        // Back alive (a respawn): the body leaves its death.
        if p.alive && !prev.alive {
            body.layer = None;
            body.chain = None;
        }
        if cur.is_some_and(|c| c.hold == hold::DEAD) && !dead_now {
            continue;
        }
        if next.is_none() && p.alive {
            let swinging = matches!(p.sword.phase, SwordPhase::Startup | SwordPhase::Active | SwordPhase::Recovery);
            let gun = p.left.item == LeftHand::Pistol || p.left.item.is_crossbow();
            let armed = body_armed(&p);
            if p.sword.swings != prev.swings && swinging {
                next = Some((NpcAnim::AttackShort, -1, 0, hold::NONE));
            } else if p.sword.phase == SwordPhase::Assassinating && prev.sword != SwordPhase::Assassinating {
                next = Some((NpcAnim::AttackShort, 5, 0, hold::NONE));
            } else if p.sword.parries != prev.parries {
                next = Some((NpcAnim::Parry, 0, 0, hold::NONE));
            } else if p.sword.phase == SwordPhase::Blocking && cur.is_none_or(|c| c.hold != hold::BLOCK) && cur.is_none() {
                next = Some((NpcAnim::ParryAwaitingImpact, -1, 0, hold::BLOCK));
            } else if p.sword.phase == SwordPhase::Staggered && prev.sword != SwordPhase::Staggered {
                next = Some((NpcAnim::Stun, 0, 0, hold::STAGGER));
            } else if p.left.phase != prev.left {
                next = match p.left.phase {
                    LeftPhase::Recovering if gun => Some((NpcAnim::AimShoot, -1, 0, hold::NONE)),
                    LeftPhase::Reloading => Some((NpcAnim::AimReload, -1, 0, hold::NONE)),
                    LeftPhase::Equipping if gun && armed => Some((NpcAnim::AimIn, -1, 0, hold::NONE)),
                    _ => None,
                };
            }
            if next.is_none() && cur.is_none() && armed && holds(hold::CROUCH, &p) {
                if let Some(seq) = cfg.anim.find(&cfg.gameplay.npc_anim.sets, "Sword_Cover_Loop") {
                    let body = a.body.get_or_insert_with(Body::default);
                    body.serial = body.serial.wrapping_add(1);
                    body.layer = Some(crate::anim::SeqPlay { seq, token: token::BODY | (body.serial & 0x00FF_FFFF), time: 0.0, prev: 0.0, rate: 1.0, looping: true, playing: true, yaw: p.yaw, exited: false, interruptible: false, advanced: false, windows: Vec::new(), elapsed: 0.0, blend_in: 0.2, blend_out: 0.2, blend: 0.2, out: None, quiet: true });
                    body.chain = Some(BodyChain { anim: COVER, group: 0, step: 0, hold: hold::CROUCH });
                }
                continue;
            }
        }
        if let Some((anim, group, step, h)) = next {
            let armed = body_armed(&p) || anim as u8 == NpcAnim::AttackShort as u8;
            body_play(a, cfg, armed, BodyChain { anim: anim as u8, group: 0, step, hold: h }, group, p.yaw);
        }
    }
}

/// `ECardinalDirection` (Forward, Left, Right, Back) of an angle from the facing (degrees, +90
/// right): 90-degree sectors (thresholds LIKELY, U-143).
pub fn cardinal(angle: f32) -> u8 {
    let a = angle.abs();
    if a <= 45.0 {
        0
    } else if a >= 135.0 {
        3
    } else if angle < 0.0 {
        1
    } else {
        2
    }
}

/// The body's advance with the pawn's animation (quiet: the player's own layers fire its
/// notifies): a step that ended goes on to the group's next step (not after an end-after-one
/// group's), a looping step goes on when its hold lets go, the chain ends after its last step; a
/// death holds.
pub fn body_advance(a: &mut PawnAnim, p: &PlayerState, cfg: &SimConfig, dt: f32) {
    let Some(body) = a.body.as_mut() else { return };
    if let Some(s) = body.layer.as_mut() {
        if let Some(info) = cfg.anim.info(s.seq) {
            crate::anim::advance_quiet(s, info, dt);
        }
    }
    let Some(c) = body.chain else { return };
    let Some(s) = body.layer.as_ref() else {
        body.chain = None;
        return;
    };
    if c.hold == hold::DEAD {
        return;
    }
    let ended = if s.looping { !holds(c.hold, p) } else { !s.playing };
    if !ended {
        return;
    }
    let grp = cfg.gameplay.npc_anim.defs.chains.get(c.anim as usize).and_then(|ch| ch.groups.get(c.group as usize));
    let next = c.step as usize + 1;
    let last = c.anim == COVER || grp.is_none_or(|g| g.end_after_one || next >= g.steps.len());
    if last {
        body.layer = None;
        body.chain = None;
        return;
    }
    let armed = body_armed(p) || c.anim == dishonored_ir::npc_anim::NpcAnim::AttackShort as u8;
    let yaw = p.yaw;
    if !body_play(a, cfg, armed, BodyChain { step: next as u8, ..c }, c.group as i8, yaw) {
        if let Some(b) = a.body.as_mut() {
            b.layer = None;
            b.chain = None;
        }
    }
}