//! Build [`GameplayConfig`] from the retail install: Arkane tweak objects (resolved
//! through class defaults), pawn class defaults, and layered INIs.
//!
//! Sources (all CONFIRMED as data; behavior that consumes them is tracked separately):
//! - `Twk_Pawn_Corvo.Twk_Pawn_Corvo_Release` (+ `.pAttributeTweaksNormal`, `.pMantleTweaks`)
//! - `Default__DishonoredPawn` (+ `.CollisionCylinder`) in DishonoredGame.upk
//! - `Twk_Powers.Blink.Twk_Blink`, `Default__DishonoredActivePowerComponent_Blink`
//! - `Twk_Powers.Windblast.Twk_WindBlast`, `Twk_Powers.DarkVision.Twk_DarkVision`
//! - weapon tweaks under `Twk_Inv_PlayerSpecific.*Corvo`, projectiles under `Twk_Projectiles`
//! - `DefaultGame.ini [Engine.WorldInfo] DefaultGravityZ`

use crate::content::Content;
use crate::ini::Ini;
use crate::tweaks::{TMap, TVal, TweakDb, get, get_f32};
use anyhow::{Context, Result};
use dishonored_ir::gameplay::*;
use std::collections::BTreeMap;

pub const CORVO: &str = "Twk_Pawn_Corvo.Twk_Pawn_Corvo_Release";
pub const DIFFICULTY_FIELD: &str = "m_fBaseValue2_Normal";
/// The player's input tweak (`DishonoredPlayerInput`): walk/sprint thresholds and the
/// strafe/backward angles the speed modifier uses.
pub const PLAYER_INPUT: &str = "Twk_Pawn_DefaultPlayer.Twk_Pawn_PlayerInput";

struct Loader<'a> {
    db: &'a TweakDb,
    prov: BTreeMap<String, String>,
}

impl Loader<'_> {
    fn obj(&self, path: &str) -> Result<TMap> {
        self.db.resolve(path).with_context(|| format!("resolving {path}"))
    }
    fn f(&mut self, m: &TMap, src: &str, field: &str, out: &str) -> Result<f32> {
        let v = get_f32(m, field).with_context(|| format!("{src}: missing {field}"))?;
        self.prov.insert(out.to_string(), format!("{src} :: {field} = {v}"));
        Ok(v)
    }
    /// A `DisAttribute` struct at Normal difficulty.
    fn attr(&mut self, m: &TMap, src: &str, field: &str, out: &str) -> Result<f32> {
        self.f(m, src, &format!("{field}.{DIFFICULTY_FIELD}"), out)
    }
}

/// Flatten every numeric/bool leaf of a tweak object into dotted keys.
pub fn flatten(m: &TMap) -> BTreeMap<String, f32> {
    fn walk(prefix: &str, m: &TMap, out: &mut BTreeMap<String, f32>) {
        for (k, v) in m {
            let key = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
            match v {
                TVal::F(f) => {
                    out.insert(key, *f);
                }
                TVal::I(i) => {
                    out.insert(key, *i as f32);
                }
                TVal::B(b) => {
                    out.insert(key, if *b { 1.0 } else { 0.0 });
                }
                TVal::Struct(s) => walk(&key, s, out),
                TVal::List(l) => {
                    for (i, e) in l.iter().enumerate() {
                        walk(&format!("{key}({i})"), e, out);
                    }
                }
                _ => {}
            }
        }
    }
    let mut out = BTreeMap::new();
    walk("", m, &mut out);
    out
}

/// The thrown-NPC config (`ThrowConfig`, research/dishonored/systems/physics.md): the NPC hit
/// reaction tweak, the throw state's class defaults, the default physics volume's fluid
/// friction and the Windblast throw animations' ragdoll notifies.
fn load_throw(l: &mut Loader, content: &Content) -> Result<ThrowConfig> {
    let tp = "Twk_Component.Twk_Component_HitReactionThrow_NPC";
    let t = l.obj(tp)?;
    let c = "m_ComponentConfig";
    let effective_mass = l.f(&t, tp, &format!("{c}.m_fEffectiveMassForThrowing"), "throw.effective_mass")?;
    let strong_hit_react_threshold = l.f(&t, tp, &format!("{c}.m_fStrongHitReactThreshold"), "throw.strong_hit_react_threshold")?;
    let anim_ragdoll_threshold = l.f(&t, tp, &format!("{c}.m_fAnimRagdollThreshold"), "throw.anim_ragdoll_threshold")?;
    let full_ragdoll_threshold = l.f(&t, tp, &format!("{c}.m_fFullRagdollThreshold"), "throw.full_ragdoll_threshold")?;
    let ragdoll_move_end_threshold = l.f(&t, tp, &format!("{c}.m_fRagdollMoveEndThreshold"), "throw.ragdoll_move_end_threshold")?;
    // `StateNPCMasterThrown` class defaults; a field the defaults leave out is zero.
    let sp = "Default__StateNPCMasterThrown";
    let st = l.db.class_defaults("StateNPCMasterThrown")?;
    let mut sf = |field: &str, out: &str| {
        let v = get_f32(&st, field).unwrap_or(0.0);
        l.prov.insert(out.to_string(), format!("{sp} :: {field} = {v}"));
        v
    };
    let min_momentum_2d = sf("m_fMinMomentum2DMagnitude", "throw.min_momentum_2d");
    let min_momentum_z = sf("m_fMinMomentumZ", "throw.min_momentum_z");
    let min_momentum_2d_stunned = sf("m_fMinMomentum2DMagnitudeWhenStunned", "throw.min_momentum_2d_stunned");
    let min_momentum_z_stunned = sf("m_fMinMomentumZWhenStunned", "throw.min_momentum_z_stunned");
    let pv = l.db.class_defaults("PhysicsVolume")?;
    let fluid_friction = match get_f32(&pv, "FluidFriction") {
        Some(v) => {
            l.prov.insert("throw.fluid_friction".into(), format!("Default__PhysicsVolume :: FluidFriction = {v}"));
            v
        }
        None => {
            // UE3 stock, as `movement.ground_friction` (LIKELY; the Engine CDO is not parsed).
            l.prov.insert("throw.fluid_friction".into(), "Engine.upk Default__PhysicsVolume.FluidFriction = 0.3 (UE3 stock)".into());
            0.3
        }
    };
    const SET: &str = "Npc_EliteGuard_Generic_as.Npc_EliteGuard_Generic_as";
    let mut windblast_ragdoll_notify = [0.0; 4];
    for (i, dir) in ["Front", "Left", "Right", "Back"].iter().enumerate() {
        let seq = format!("Generic_WindBlast_{dir}");
        let (pkg, v) = crate::ai_config::NPC_ANIM_PACKAGES
            .iter()
            .find_map(|p| crate::npc_anim::sequence_notify_time(content, p, SET, &seq, "DishonoredNotify_Ragdoll").map(|v| (*p, v)))
            .with_context(|| format!("{SET} {seq}: no DishonoredNotify_Ragdoll"))?;
        windblast_ragdoll_notify[i] = v;
        l.prov.insert(format!("throw.windblast_ragdoll_notify[{i}]"), format!("{pkg} {SET} {seq} DishonoredNotify_Ragdoll Time / RateScale = {v}"));
    }
    // The rest test's speed (0x00A98DD0 reads the pawn tweak's body tweak +0x1F4).
    let body = l.db.class_defaults("DisTweaks_NPCPawn_Body")?;
    let max_ragdoll_speed_to_recover = l.f(&body, "Default__DisTweaks_NPCPawn_Body", "m_fMaxRagdollSpeedToRecover", "throw.max_ragdoll_speed_to_recover")?;
    // The get-up: `StateNPCMasterStunned` plays the `Stun` chain's group `m_Animation` = 1
    // (`eStunEnterAnim_RagdollFront`, the value the throw state's rest hands it, 0x00A814B0).
    const DEFS: &str = "DisNPCAnimDefinitions_EliteGuard";
    const SETS: [&str; 2] = ["Npc_EliteGuard_Generic_as.Npc_EliteGuard_Generic_as", "Npc_EliteGuard_Sword_as.Npc_EliteGuard_Sword_as"];
    let pkgs: Vec<String> = crate::ai_config::NPC_ANIM_PACKAGES.iter().map(|s| s.to_string()).collect();
    let defs = crate::npc_anim::load_anim_defs(content, &pkgs, DEFS)?;
    let group = defs
        .chain(dishonored_ir::npc_anim::NpcAnim::Stun)
        .and_then(|c| c.groups.get(1))
        .with_context(|| format!("{DEFS}: no Stun group 1"))?;
    // A step's sequence: the name as authored, or with the set's prefix (`Generic_`; the
    // primary-item prefix of a guard is `Sword_`).
    let len = |name: &str, prefix: u8| -> Option<(String, f32)> {
        let cands = [name.to_string(), format!("Generic_{name}"), if prefix == 2 { format!("Sword_{name}") } else { name.to_string() }];
        cands.iter().find_map(|n| {
            crate::ai_config::NPC_ANIM_PACKAGES.iter().find_map(|p| SETS.iter().find_map(|s| crate::npc_anim::sequence_play_length(content, p, s, n).map(|v| (n.clone(), v))))
        })
    };
    let (first, last) = (group.steps.first().context("empty Stun group")?, group.steps.last().context("empty Stun group")?);
    let (n0, getup_anim) = first.names.first().and_then(|n| len(n, first.item_prefix)).context("Stun group 1: first sequence not found")?;
    let rec: Vec<(String, f32)> = last.names.iter().filter_map(|n| len(n, last.item_prefix)).collect();
    anyhow::ensure!(!rec.is_empty(), "Stun group 1: no recovery sequence found");
    l.prov.insert("throw.getup_anim".into(), format!("{DEFS} Stun group 1 step 0 {n0} play length = {getup_anim}"));
    l.prov.insert("throw.getup_recover".into(), format!("{DEFS} Stun group 1 last step: {rec:?}"));
    let getup_recover = rec.into_iter().map(|r| r.1).collect();
    Ok(ThrowConfig {
        max_ragdoll_speed_to_recover,
        getup_anim,
        getup_recover,
        effective_mass,
        strong_hit_react_threshold,
        anim_ragdoll_threshold,
        full_ragdoll_threshold,
        ragdoll_move_end_threshold,
        min_momentum_2d,
        min_momentum_z,
        min_momentum_2d_stunned,
        min_momentum_z_stunned,
        fluid_friction,
        windblast_ragdoll_notify,
    })
}

/// The player's mantle sequences, low / medium / high (`Ply_Sword_Locomotion_as`, the
/// `Sword_` item prefix of the `Mantle` state's tree `Ply_Player_Mantle_at`): the root bone's
/// translation per frame taken to pawn space through `Ply_Player.Skm_Player`'s mesh transform,
/// and each node's `m_fMantleTravelDistanceZ`.
fn mantle_anims(content: &Content, prov: &mut BTreeMap<String, String>) -> Result<Vec<MantleAnim>> {
    use ue3_package::properties::{Value, find};
    const SET: &str = "Ply_Sword_Locomotion_as.Ply_Sword_Locomotion_as";
    const NAMES: [&str; 3] = ["Sword_Ready_MantleLow", "Sword_Ready_MantleMedium", "Sword_Ready_MantleHigh"];
    let rig = crate::characters::load_model(content, "mantle_rig", &[("Engine", "Ply_Player.Skm_Player")], None, 16)?;
    let part = rig.parts.first().context("Skm_Player has no part")?;
    let pkg = content.package("Startup")?;
    let mut travel: BTreeMap<String, f32> = BTreeMap::new();
    for i in 0..pkg.exports.len() {
        if pkg.export_class(i) != "DishonoredAnimNodeSeqMantle" {
            continue;
        }
        let h = pkg.read_object(i)?;
        let Some(Value::Name(name)) = find(&h.properties, "AnimSeqName").map(|p| &p.value) else { continue };
        if let Some(z) = find(&h.properties, "m_fMantleTravelDistanceZ").and_then(|p| p.as_float()) {
            travel.insert(name.clone(), z);
        }
    }
    let mut lib = crate::characters::load_anim_set(content, "Startup", SET, &NAMES)?;
    lib.edge_order(&part.bones);
    let root_name = &part.bones.first().context("no bones")?.name;
    let root = lib.tracks.iter().position(|t| t.eq_ignore_ascii_case(root_name)).context("no root track")?;
    let mut out = Vec::new();
    for n in NAMES {
        let clip = lib.clips.get(n).with_context(|| format!("{n} not decoded"))?;
        let at = |f: &Vec<Option<glam::Vec3>>| f.get(root).copied().flatten();
        let t0 = clip.translations.first().and_then(at).unwrap_or(glam::Vec3::ZERO);
        let mut last = t0;
        let pts: Vec<[f32; 2]> = clip
            .translations
            .iter()
            .map(|f| {
                let t = at(f).unwrap_or(last);
                last = t;
                let d = part.mesh_to_pawn.transform_vector3(t - t0);
                [d.x, d.z]
            })
            .collect();
        let travel_z = *travel.get(n).with_context(|| format!("no DishonoredAnimNodeSeqMantle plays {n}"))?;
        prov.insert(format!("mantle.anims.{n}"), format!("Startup {SET} {n}: {} frames at {} Hz, {} s, root end {:?}, Ply_Player_Mantle_at m_fMantleTravelDistanceZ = {travel_z}", pts.len(), clip.sample_rate, clip.duration, pts.last()));
        out.push(MantleAnim { sequence: n.to_string(), travel_z, sample_rate: clip.sample_rate, duration: clip.duration, root: pts });
    }
    Ok(out)
}

pub fn load_gameplay_config(content: &Content) -> Result<GameplayConfig> {
    let db = TweakDb::load(content)?;
    let mut l = Loader { db: &db, prov: BTreeMap::new() };
    let game_ini = Ini::load_layered(&content.root, "Game");

    let corvo = l.obj(CORVO)?;
    let attrs_path = format!("{CORVO}.pAttributeTweaksNormal");
    let attrs = l.obj(&attrs_path)?;
    let mantle_path = format!("{CORVO}.pMantleTweaks");
    let mantle = l.obj(&mantle_path)?;
    let pawn = l.db.class_defaults("DishonoredPawn")?;
    // Subobject templates in a subclass CDO are deltas against the parent class CDO's
    // template of the same name: player radius 35 over the base pawn cylinder.
    let mut cyl = l.obj("Default__DishonoredPawn.CollisionCylinder")?;
    crate::tweaks::merge(&mut cyl, &l.db.delta("Default__DishonoredPlayerPawn.CollisionCylinder")?);

    let gravity_z = game_ini
        .get_f32("Engine.WorldInfo", "DefaultGravityZ")
        .context("DefaultGravityZ missing from DefaultGame.ini")?;
    l.prov.insert(
        "movement.gravity_z".into(),
        format!("DefaultGame.ini [Engine.WorldInfo] DefaultGravityZ = {gravity_z}"),
    );
    // UE3 stock values not overridden anywhere we have found (LIKELY; see UNKNOWNS).
    l.prov.insert("movement.ground_friction".into(), "Engine.upk Default__PhysicsVolume.GroundFriction = 8.0".into());
    l.prov.insert("movement.terminal_velocity".into(), "Engine.upk Default__PhysicsVolume.TerminalVelocity = 4000.0".into());
    // The attribute update (0x00B56880) sets Pawn.WalkableFloorZ to cos(m_fMaxWalkableFloorInDegrees).
    let walk_deg = l.f(&corvo, CORVO, "m_fMaxWalkableFloorInDegrees", "movement.walkable_floor_z")?;

    let a = &attrs_path;
    let movement = MovementConfig {
        gravity_z,
        run_speed: l.attr(&attrs, a, "m_GroundSpeed", "movement.run_speed")?,
        sprint_speed: l.attr(&attrs, a, "m_GroundSpeedSprint", "movement.sprint_speed")?,
        walk_speed: l.attr(&attrs, a, "m_GroundSpeedWalk", "movement.walk_speed")?,
        crouch_speed: l.attr(&attrs, a, "m_GroundSpeedCrouch", "movement.crouch_speed")?,
        accel_rate: l.attr(&attrs, a, "m_AccelerationRate", "movement.accel_rate")?,
        ground_friction: 8.0,
        jump_z: l.attr(&attrs, a, "m_JumpZ", "movement.jump_z")?,
        air_control: l.f(&corvo, CORVO, "m_fAirControl", "movement.air_control")?,
        max_fall_speed: l.f(&corvo, CORVO, "m_fMaxFallSpeed", "movement.max_fall_speed")?,
        terminal_velocity: 4000.0,
        max_step_height: l.f(&pawn, "Default__DishonoredPawn", "MaxStepHeight", "movement.max_step_height")?,
        collision_radius: l.f(&cyl, "Default__DishonoredPlayerPawn.CollisionCylinder (over DishonoredPawn template)", "CollisionRadius", "movement.collision_radius")?,
        collision_half_height: l.f(&cyl, "Default__DishonoredPawn.CollisionCylinder (inherited by DishonoredPlayerPawn)", "CollisionHeight", "movement.collision_half_height")?,
        crouch_half_height: l.f(&corvo, CORVO, "m_fSneakHeight", "movement.crouch_half_height")?,
        base_eye_height: l.f(&pawn, "Default__DishonoredPawn", "BaseEyeHeight", "movement.base_eye_height")?,
        strafe_mult_run: l.attr(&attrs, a, "m_GroundStrafeMultiplierRun", "movement.strafe_mult_run")?,
        strafe_mult_sprint: l.attr(&attrs, a, "m_GroundStrafeMultiplierSprint", "movement.strafe_mult_sprint")?,
        strafe_mult_sneak: l.attr(&attrs, a, "m_GroundStrafeMultiplierSneak", "movement.strafe_mult_sneak")?,
        back_mult_run: l.attr(&attrs, a, "m_GroundBackwardMultiplierRun", "movement.back_mult_run")?,
        back_mult_sprint: l.attr(&attrs, a, "m_GroundBackwardMultiplierSprint", "movement.back_mult_sprint")?,
        back_mult_sneak: l.attr(&attrs, a, "m_GroundBackwardMultiplierSneak", "movement.back_mult_sneak")?,
        sword_unsheathed_speed_factor: l.attr(&attrs, a, "m_SwordUnsheathedSpeedFactor", "movement.sword_unsheathed_speed_factor")?,
        walkable_floor_z: walk_deg.to_radians().cos(),
        max_speed_before_falling_damage: l.attr(&attrs, a, "m_MaxSpeedBeforeFallingDamage", "movement.max_speed_before_falling_damage")?,
        max_speed_before_falling_death: l.attr(&attrs, a, "m_MaxSpeedBeforeFallingDeath", "movement.max_speed_before_falling_death")?,
        mantle: MantleConfig {
            min_edge_height: l.f(&mantle, &mantle_path, "m_MantleMinEdgeHeight", "mantle.min_edge_height")?,
            low_max_edge_height: l.f(&mantle, &mantle_path, "m_LowMantleMaxEdgeHeight", "mantle.low_max_edge_height")?,
            medium_max_edge_height: l.f(&mantle, &mantle_path, "m_MediumMantleMaxEdgeHeight", "mantle.medium_max_edge_height")?,
            max_edge_height: l.f(&mantle, &mantle_path, "m_MantleMaxEdgeHeight", "mantle.max_edge_height")?,
            edge_search_dist: l.f(&mantle, &mantle_path, "m_MantleEdgeSearchDist", "mantle.edge_search_dist")?,
            max_slope_angle_edge_top: l.f(&mantle, &mantle_path, "m_MaxSlopeAngleForEdgeTop", "mantle.max_slope_angle_edge_top")?,
            forward_move_amount: l.f(&mantle, &mantle_path, "m_fMantleForwardMoveAmount", "mantle.forward_move_amount")?,
            max_fall_speed_for_mantle: l.f(&mantle, &mantle_path, "m_fMaxFallSpeedForMantle", "mantle.max_fall_speed_for_mantle")?,
            fall_speed_for_ledge_grab: l.f(&mantle, &mantle_path, "m_fFallSpeedForLedgeGrab", "mantle.fall_speed_for_ledge_grab")?,
            line_check_step_size: l.f(&mantle, &mantle_path, "m_LineCheckStepSize", "mantle.line_check_step_size")?,
            max_vertical_angle_edge_face: l.f(&mantle, &mantle_path, "m_MaxVerticalAngleForEdgeFace", "mantle.max_vertical_angle_edge_face")?,
            max_horizontal_angle_edge_face: l.f(&mantle, &mantle_path, "m_MaxHorizontalAngleForEdgeFace", "mantle.max_horizontal_angle_edge_face")?,
            ledge_grab_min_edge_height: l.f(&mantle, &mantle_path, "m_fLedgeGrabMantleMinEdgeHeight", "mantle.ledge_grab_min_edge_height")?,
            low_use_step_up: {
                let v = matches!(get(&mantle, "m_bLowMantleUseStepUp"), Some(TVal::B(true)));
                l.prov.insert("mantle.low_use_step_up".into(), format!("{mantle_path} :: m_bLowMantleUseStepUp = {v}"));
                v
            },
            anims: mantle_anims(content, &mut l.prov)?,
            anim_rate: match get_f32(&attrs, &format!("m_MantleAnimRate.{DIFFICULTY_FIELD}")) {
                Some(v) => {
                    l.prov.insert("mantle.anim_rate".into(), format!("{a} :: m_MantleAnimRate.{DIFFICULTY_FIELD} = {v}"));
                    v
                }
                None => {
                    l.prov.insert("mantle.anim_rate".into(), "DisAttribute default 1 (m_MantleAnimRate not in the resolved tweak)".into());
                    1.0
                }
            },
        },
        slow_walk_speed: l.attr(&attrs, a, "m_GroundSpeedSlowWalk", "movement.slow_walk_speed")?,
        sword_sheathed_speed_factor: l.attr(&attrs, a, "m_SwordSheathedSpeedFactor", "movement.sword_sheathed_speed_factor")?,
        speed_blend_down_speed: l.f(&corvo, CORVO, "m_fSpeedBlendDownSpeed", "movement.speed_blend_down_speed")?,
        crawl_half_height: l.f(&corvo, CORVO, "m_fCrouchHeight", "movement.crawl_half_height")?,
        slide_time: l.f(&corvo, CORVO, "m_fSlideTime", "movement.slide_time")?,
        slide_deactivate_angle: l.f(&corvo, CORVO, "m_fSlideVelocityDeactivateAngle", "movement.slide_deactivate_angle")?,
        slide_percent_not_cancelable: l.f(&corvo, CORVO, "m_fSlidePercentNotCancelable", "movement.slide_percent_not_cancelable")?,
        slide_damage_test_distance: l.f(&corvo, CORVO, "m_fSlideDamageTestDistance", "movement.slide_damage_test_distance")?,
        slide_impact_damage_vel_ratio: l.f(&corvo, CORVO, "m_fSlideImpactDamageVelRatio", "movement.slide_impact_damage_vel_ratio")?,
        slide_damage_ignores_sleepers: true,
        slide_damage_npc: 0.0,
        slide_damage_rat: 0.0,
        slide_damage_other: 5.0,
        lean: {
            let cam = Ini::load_layered(&content.root, "Camera");
            let sec = "DishonoredGame.DishonoredCamera_Lean";
            let d = LeanConfig::default();
            let spring = cam.get(sec, "m_LeanConstants").unwrap_or("");
            let field = |name: &str| -> Option<f32> {
                let i = spring.find(name)? + name.len() + 1;
                spring[i..].split([',', ')']).next()?.trim().parse().ok()
            };
            l.prov.insert("movement.lean".into(), format!("DefaultCamera.ini [{sec}]"));
            LeanConfig {
                height_pct: cam.get_f32(sec, "m_fLeanHeightPct").unwrap_or(d.height_pct),
                max_angle: cam.get_f32(sec, "m_fMaxLeanAngle").unwrap_or(d.max_angle),
                max_angle_crouched: cam.get_f32(sec, "m_fMaxLeanAngle_Crouched").unwrap_or(d.max_angle_crouched),
                springiness: field("m_Springiness").unwrap_or(d.springiness),
                damping: field("m_Damping").unwrap_or(d.damping),
                soften_angle: cam.get_f32(sec, "m_fMaxLeanSoftenAngle").unwrap_or(d.soften_angle),
                soften_speed: cam.get_f32(sec, "m_fMaxLeanSoftenSpeed").unwrap_or(d.soften_speed),
                camera_tilt_percent: cam.get_f32(sec, "m_fCameraTiltPercent").unwrap_or(d.camera_tilt_percent),
            }
        },
        input: {
            let ip = PLAYER_INPUT;
            let pi = l.obj(ip)?;
            PlayerInputConfig {
                slow_walk_threshold: l.f(&pi, ip, "m_fSlowWalk_Trigger_Threshold", "input.slow_walk_threshold")?,
                walk_threshold: l.f(&pi, ip, "m_fWalk_Trigger_Threshold", "input.walk_threshold")?,
                stop_sprint_threshold: l.f(&pi, ip, "m_fStopSprint_Threshold", "input.stop_sprint_threshold")?,
                strafe_angle_forward: l.f(&pi, ip, "m_fStrafeAngle_Forward", "input.strafe_angle_forward")?,
                strafe_angle_back: l.f(&pi, ip, "m_fStrafeAngle_Back", "input.strafe_angle_back")?,
                backwards_angle: l.f(&pi, ip, "m_fBackwardsAngle", "input.backwards_angle")?,
                min_speed_to_slide: l.f(&pi, ip, "m_fMinSpeedToSlide", "input.min_speed_to_slide")?,
            }
        },
        water: {
            let d = WaterMoveConfig::default();
            let state = Ini::load_layered(&content.root, "PlayerState");
            let sec = "DishonoredGame.StatePlayerMasterSwim";
            let sf = |k: &str, v: f32| state.get_f32(sec, k).unwrap_or(v);
            l.prov.insert("movement.water.swim".into(), format!("DefaultPlayerState.ini [{sec}]"));
            WaterMoveConfig {
                water_speed: l.attr(&attrs, a, "m_WaterSpeed", "movement.water.water_speed").unwrap_or(d.water_speed),
                buoyancy: l.f(&pawn, "Default__DishonoredPawn", "Buoyancy", "movement.water.buoyancy").unwrap_or(d.buoyancy),
                out_of_water_z: l.f(&pawn, "Default__DishonoredPawn", "OutofWaterZ", "movement.water.out_of_water_z").unwrap_or(d.out_of_water_z),
                min_swim_accel: sf("m_fMinSwimAccel", d.min_swim_accel),
                max_swim_accel: sf("m_fMaxSwimAccel", d.max_swim_accel),
                max_speed_no_stroke: sf("m_fMaxSpeedNoStroke", d.max_speed_no_stroke),
                stroke_time: sf("m_fStrokeTime", d.stroke_time),
                surface_stroke_delay: sf("m_fSurfaceSwimDelayBetweenStrokes", d.surface_stroke_delay),
                constant_speed_at_surface: state.get(sec, "m_bConstantSpeedAtSurface").map_or(d.constant_speed_at_surface, |v| v.eq_ignore_ascii_case("true")),
                apnea_max: l.f(&corvo, CORVO, "m_fApneaMaxDuration", "movement.water.apnea_max").unwrap_or(d.apnea_max),
                apnea_recovery: l.f(&corvo, CORVO, "m_fApneaRecoveryDuration", "movement.water.apnea_recovery").unwrap_or(d.apnea_recovery),
                drowning_damage: l.attr(&attrs, a, "m_DrowningDamageAmount", "movement.water.drowning_damage").unwrap_or(d.drowning_damage),
                drowning_step: l.attr(&attrs, a, "m_DrowningDamageStep", "movement.water.drowning_step").unwrap_or(d.drowning_step),
            }
        },
        power_jump_z: l.attr(&attrs, a, "m_JumpZ_PowerJump", "movement.power_jump_z").unwrap_or(0.0),
        climb: {
            let d = ClimbConfig::default();
            let player = Ini::load_layered(&content.root, "Player");
            l.prov.insert("movement.climb.aim_threshold".into(), "DefaultPlayer.ini [DishonoredGame.DishonoredPlayerPawn] m_ClimbingAimThreshold".into());
            let pi = l.obj(PLAYER_INPUT).ok();
            ClimbConfig {
                speed: l.f(&corvo, CORVO, "m_fClimbingSpeed", "movement.climb.speed").unwrap_or(d.speed),
                jump_impulse: l.f(&corvo, CORVO, "m_fJumpImpulse", "movement.climb.jump_impulse").unwrap_or(d.jump_impulse),
                sound_min_delay: l.f(&corvo, CORVO, "m_fClimbingSoundMinDelay", "movement.climb.sound_min_delay").unwrap_or(d.sound_min_delay),
                sound_max_delay: l.f(&corvo, CORVO, "m_fClimbingSoundMaxDelay", "movement.climb.sound_max_delay").unwrap_or(d.sound_max_delay),
                reuse_delay: pi.as_ref().and_then(|pi| l.f(pi, PLAYER_INPUT, "m_fClimbReuseDelay", "movement.climb.reuse_delay").ok()).unwrap_or(d.reuse_delay),
                reuse_delay_jump: pi.as_ref().and_then(|pi| get(pi, "m_bClimbReuseDelayJump")).map_or(d.reuse_delay_jump, |v| matches!(v, TVal::B(true))),
                aim_threshold: player.get("DishonoredGame.DishonoredPlayerPawn", "m_ClimbingAimThreshold").and_then(|v| v.trim().parse().ok()).unwrap_or(d.aim_threshold),
                collision_restoration_delay: d.collision_restoration_delay,
                camera_relative: d.camera_relative,
            }
        },
    };

    let vitals = VitalsConfig {
        health_max: l.attr(&attrs, a, "m_HealthMax", "vitals.health_max")?,
        health_regen_rate: l.attr(&attrs, a, "m_HealthRegenRate", "vitals.health_regen_rate")?,
        health_regen_amount: l.attr(&attrs, a, "m_HealthRegenAmount", "vitals.health_regen_amount")?,
        health_regen_limit: l.attr(&attrs, a, "m_HealthRegenLimit", "vitals.health_regen_limit")?,
        health_regen_initial_delay: l.attr(&attrs, a, "m_HealthRegenInitialDelay", "vitals.health_regen_initial_delay")?,
        mana_max: l.attr(&attrs, a, "m_ManaMax", "vitals.mana_max")?,
        mana_regen_amount: l.attr(&attrs, a, "m_ManaRegenAmount", "vitals.mana_regen_amount")?,
        mana_regen_additive_portion: l.attr(&attrs, a, "m_ManaRegenAdditivePortion", "vitals.mana_regen_additive_portion")?,
        mana_regen_step_time: l.attr(&attrs, a, "m_ManaRegenStepTime", "vitals.mana_regen_step_time")?,
        mana_regen_initial_delay: l.attr(&attrs, a, "m_ManaRegenInitialDelay", "vitals.mana_regen_initial_delay")?,
    };

    let bp = "Twk_Powers.Blink.Twk_Blink";
    let blink_t = l.obj(bp)?;
    // A per-level field of the Blink tweak (`m_Levels` element 0, then 1).
    let per_level = |l: &mut Loader, m: &TMap, src: &str, field: &str, out: &str| -> Result<Vec<f32>> {
        let mut v = Vec::new();
        for (i, key) in ["m_Levels", "m_Levels[1]"].iter().enumerate() {
            v.push(l.f(m, src, &format!("{key}.{field}"), &format!("{out}[{i}]"))?);
        }
        Ok(v)
    };
    let comp_p = "Default__DishonoredActivePowerComponent_Blink";
    let blink_c = l.obj(comp_p)?;
    let mut horiz = Vec::new();
    let mut vert = Vec::new();
    for (i, key) in ["m_Levels", "m_Levels[1]"].iter().enumerate() {
        horiz.push(l.f(&blink_t, bp, &format!("{key}.m_HorizDistance"), &format!("blink.horiz_distance[{i}]"))?);
        vert.push(l.f(&blink_t, bp, &format!("{key}.m_VertDistance"), &format!("blink.vert_distance[{i}]"))?);
    }
    let extent = match get(&blink_t, "m_TargetTestExtent") {
        Some(TVal::Vec3(v)) => *v,
        _ => [20.0; 3],
    };
    l.prov.insert("blink.target_test_extent".into(), format!("{bp} :: m_TargetTestExtent = {extent:?}"));
    let blink = BlinkConfig {
        horiz_distance: horiz,
        vert_distance: vert,
        step_distance: l.f(&blink_t, bp, "m_Levels.m_fDefaultBlinkStepDistance", "blink.step_distance")?,
        target_test_extent: extent,
        close_collision_distance: l.f(&blink_t, bp, "m_fCloseCollisionDistance", "blink.close_collision_distance")?,
        close_collision_offset_step: l.f(&blink_t, bp, "m_fCloseCollisionOffsetStep", "blink.close_collision_offset_step")?,
        mana_cost: l.f(&blink_t, bp, "m_ManaCost", "blink.mana_cost")?,
        ledge_check_distance_scale: l.f(&blink_c, comp_p, "m_fLedgeCheckDistanceScale", "blink.ledge_check_distance_scale")?,
        max_angle_big_step: l.f(&blink_c, comp_p, "m_fMaxAngle_BigStep", "blink.max_angle_big_step")?,
        max_angle_small_step: l.f(&blink_c, comp_p, "m_fMaxAngle_SmallStep", "blink.max_angle_small_step")?,
        step_length_big: l.f(&blink_c, comp_p, "m_fStepLength_Big", "blink.step_length_big")?,
        step_length_small: l.f(&blink_c, comp_p, "m_fStepLength_Small", "blink.step_length_small")?,
        impulse_radius: l.f(&blink_c, comp_p, "m_fImpulseRadius", "blink.impulse_radius")?,
        time_between_steps: {
            let mut v = Vec::new();
            for (i, key) in ["m_Levels", "m_Levels[1]"].iter().enumerate() {
                v.push(l.f(&blink_t, bp, &format!("{key}.m_fDefaultTimeBetweenBlinkSteps"), &format!("blink.time_between_steps[{i}]"))?);
            }
            v
        },
        limit_vertical_from_ground: matches!(get(&blink_t, "m_bLimitVerticalDistanceFromGround"), Some(TVal::B(true))),
        bend_time_down_time: per_level(&mut l, &blink_t, bp, "m_fBendTimeDownTime", "blink.bend_time_down_time")?,
        bend_time_up_time: per_level(&mut l, &blink_t, bp, "m_fBendTimeUpTime", "blink.bend_time_up_time")?,
        cooldown_time: per_level(&mut l, &blink_t, bp, "m_fCooldownTime", "blink.cooldown_time")?,
    };

    let power = |l: &mut Loader, path: &str, name: &str| -> Result<PowerConfig> {
        let m = l.obj(path)?;
        let mana_cost = l.f(&m, path, "m_ManaCost", &format!("{name}.mana_cost"))?;
        l.prov.insert(format!("{name}.values"), format!("{path} (flattened)"));
        Ok(PowerConfig { mana_cost, values: flatten(&m) })
    };
    let mut windblast = power(&mut l, "Twk_Powers.Windblast.Twk_WindBlast", "windblast")?;
    // The blast starts at the cast animation's `DisNotify_WindBlast` (0x00A96AE0 -> 0x00C44E10
    // -> 0x00BF9570); the power's fire mode (0x00BF9720) waits for it while the cast action plays.
    {
        let (pkg, set, seq) = ("Startup", "Ply_Powers_as.Ply_Powers_as", "Powers_Cast_Windblast");
        let t = crate::npc_anim::sequence_notify_time(content, pkg, set, seq, "DisNotify_WindBlast").with_context(|| format!("{pkg} {set} {seq}: no DisNotify_WindBlast"))?;
        windblast.values.insert("cast.notify_time".into(), t);
        l.prov.insert("windblast.cast.notify_time".into(), format!("{pkg}.upk {set} {seq} DisNotify_WindBlast Time / RateScale = {t}"));
    }
    let throw = load_throw(&mut l, content)?;
    let dark_vision = power(&mut l, "Twk_Powers.DarkVision.Twk_DarkVision", "dark_vision")?;

    let weapon = |l: &mut Loader, parts: &[(&str, &str)], name: &str| -> Result<WeaponConfig> {
        let mut values = BTreeMap::new();
        let mut srcs = Vec::new();
        for (prefix, path) in parts {
            let m = l.obj(path)?;
            for (k, v) in flatten(&m) {
                values.insert(format!("{prefix}.{k}"), v);
            }
            srcs.push(format!("{prefix}={path}"));
        }
        l.prov.insert(format!("{name}.values"), srcs.join("; "));
        Ok(WeaponConfig { values })
    };
    let sword_root = "Twk_Inv_PlayerSpecific.Twk_Inv_SwordCorvo";
    let sword_ps = format!("{sword_root}.DisTweaks_InventoryItem_PlayerSpecific_1");
    let sword = weapon(&mut l, &[("item", sword_root)], "sword")?;
    let melee = weapon(
        &mut l,
        &[
            ("attack", &format!("{sword_ps}.DisTweaks_MeleeAttackPlayer_1")),
            ("block", &format!("{sword_ps}.DisTweaks_MeleeBlockPlayer_1")),
            ("assassinate", &format!("{sword_ps}.DisTweaks_Assassinate_1")),
            ("drop_assassinate", &format!("{sword_ps}.DisTweaks_DropAssassinate_1")),
        ],
        "melee",
    )?;
    let pistol = weapon(
        &mut l,
        &[
            ("item", "Twk_Inv_PlayerSpecific.Twk_Inv_PistolCorvo"),
            ("fire", "Twk_Inv_PlayerSpecific.Twk_Inv_PistolCorvo.DisTweaks_InventoryItem_PlayerSpecific_0.DisTweaks_FirePistol_1"),
            ("projectile", "Twk_Projectiles.Twk_Proj_Bullet"),
            // `DisTweaks_WeaponRanged.m_pAttributeTweaks`: Attribute_DispersionMax / Min /
            // InterpolationSpeed (U-247).
            ("disp", "Twk_Inv_PlayerSpecific.Twk_Inv_PistolCorvo.DisTweaks_WeaponRanged_Attributes_5"),
        ],
        "pistol",
    )?;
    let crossbow = weapon(
        &mut l,
        &[
            ("item", "Twk_Inv_PlayerSpecific.Twk_Inv_CrossbowCorvo"),
            ("fire", "Twk_Inv_PlayerSpecific.Twk_Inv_CrossbowCorvo.DisTweaks_InventoryItem_PlayerSpecific_1.DisTweaks_FireCrossbow_0"),
            ("projectile", "Twk_Projectiles.Twk_Proj_Arrow"),
            ("disp", "Twk_Inv_PlayerSpecific.Twk_Inv_CrossbowCorvo.DisTweaks_WeaponRanged_Attributes_4"),
        ],
        "crossbow",
    )?;

    // The player visibility's speed value divides by `Attribute_GroundSpeedSprint` (0x00ac33b0).
    let mut ai = crate::ai_config::load_ai_config(content, &[]);
    ai.player_visibility.ground_speed_sprint = movement.sprint_speed;
    // The Tallboy's ballistic solve uses the world's GravityZ (0x00c3f2a0).
    ai.special.tallboy.gravity_z = movement.gravity_z;
    // The stealth light environment's bounds are the collision cylinder's.
    ai.player_visibility.cylinder = [movement.collision_radius, movement.collision_half_height, movement.crouch_half_height];
    // A look-at request follows the player's eyes (`GetActorEyesViewPoint`).
    ai.player_eye = [movement.base_eye_height, movement.collision_half_height];

    let items = crate::items_config::load_items_config(content, &db, &mut l.prov, &ai.loudness_distances);
    // `DisTweaks_FirePistol.m_AINoiseLoudnessWhenFiring` (an `EAINoiseLoudness`; the class
    // default `EAINoiseLoudness4` when the tweak leaves it) as a distance.
    let mut pistol = pistol;
    {
        let fire_p = "Twk_Inv_PlayerSpecific.Twk_Inv_PistolCorvo.DisTweaks_InventoryItem_PlayerSpecific_0.DisTweaks_FirePistol_1";
        let fire = l.obj(fire_p)?;
        let loud4 = ai.loudness_distances.get(15).copied().unwrap_or(6000.0);
        let r = crate::items_config::loudness_radius(&fire, "m_AINoiseLoudnessWhenFiring", &ai.loudness_distances).unwrap_or(loud4);
        pistol.values.insert("fire.ai_noise_radius".into(), r);
        l.prov.insert("pistol.fire.ai_noise_radius".into(), format!("{fire_p} :: m_AINoiseLoudnessWhenFiring -> m_LoudnessDistanceArray = {r}"));
    }
    let adrenaline = {
        let d = AdrenalineConfig::default();
        let mv = |k: &str, v: f32| melee.values.get(k).copied().unwrap_or(v);
        AdrenalineConfig {
            max: l.attr(&attrs, a, "m_AdrenalineMax", "adrenaline.max").unwrap_or(d.max),
            burn_rate: l.attr(&attrs, a, "m_AdrenalineBurnRate", "adrenaline.burn_rate").unwrap_or(d.burn_rate),
            cooldown: l.attr(&attrs, a, "m_AdrenalineCooldown", "adrenaline.cooldown").unwrap_or(d.cooldown),
            mult_taking_damage: l.attr(&attrs, a, "m_AdrenalineMultWhenTakingDamage", "adrenaline.mult_taking_damage").unwrap_or(d.mult_taking_damage),
            mult_on_hit: mv("attack.m_fAdrenalineMultOnHit", d.mult_on_hit),
            mult_assassinate: mv("assassinate.m_fAdrenalineMultOnHit", d.mult_assassinate),
            mult_drop_assassinate: mv("drop_assassinate.m_fAdrenalineMultOnHit", d.mult_drop_assassinate),
            parry_win: mv("block.m_fAdrenalineAddedParryWin", d.parry_win),
        }
    };
    Ok(GameplayConfig {
        adrenaline,
        throw,
        items,
        movement,
        vitals,
        blink,
        windblast,
        dark_vision,
        sword,
        pistol,
        crossbow,
        melee,
        ai,
        provenance: l.prov,
    })
}
