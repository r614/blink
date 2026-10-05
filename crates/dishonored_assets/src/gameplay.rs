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
    l.prov.insert("movement.ground_friction".into(), "UE3 stock PhysicsVolume.GroundFriction = 8.0 (LIKELY)".into());
    l.prov.insert("movement.terminal_velocity".into(), "UE3 stock PhysicsVolume.TerminalVelocity = 4000.0 (LIKELY)".into());
    l.prov.insert("movement.walkable_floor_z".into(), "UE3 stock Pawn.WalkableFloorZ = 0.7 (LIKELY)".into());

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
        walkable_floor_z: 0.7,
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
        },
        slow_walk_speed: l.attr(&attrs, a, "m_GroundSpeedSlowWalk", "movement.slow_walk_speed")?,
        sword_sheathed_speed_factor: l.attr(&attrs, a, "m_SwordSheathedSpeedFactor", "movement.sword_sheathed_speed_factor")?,
        speed_blend_down_speed: l.f(&corvo, CORVO, "m_fSpeedBlendDownSpeed", "movement.speed_blend_down_speed")?,
        crawl_half_height: l.f(&corvo, CORVO, "m_fCrouchHeight", "movement.crawl_half_height")?,
        slide_time: l.f(&corvo, CORVO, "m_fSlideTime", "movement.slide_time")?,
        slide_deactivate_angle: l.f(&corvo, CORVO, "m_fSlideVelocityDeactivateAngle", "movement.slide_deactivate_angle")?,
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
    };

    let power = |l: &mut Loader, path: &str, name: &str| -> Result<PowerConfig> {
        let m = l.obj(path)?;
        let mana_cost = l.f(&m, path, "m_ManaCost", &format!("{name}.mana_cost"))?;
        l.prov.insert(format!("{name}.values"), format!("{path} (flattened)"));
        Ok(PowerConfig { mana_cost, values: flatten(&m) })
    };
    let windblast = power(&mut l, "Twk_Powers.Windblast.Twk_WindBlast", "windblast")?;
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
        ],
        "pistol",
    )?;
    let crossbow = weapon(
        &mut l,
        &[
            ("item", "Twk_Inv_PlayerSpecific.Twk_Inv_CrossbowCorvo"),
            ("fire", "Twk_Inv_PlayerSpecific.Twk_Inv_CrossbowCorvo.DisTweaks_InventoryItem_PlayerSpecific_1.DisTweaks_FireCrossbow_0"),
            ("projectile", "Twk_Projectiles.Twk_Proj_Arrow"),
        ],
        "crossbow",
    )?;

    Ok(GameplayConfig {
        movement,
        vitals,
        blink,
        windblast,
        dark_vision,
        sword,
        pistol,
        crossbow,
        melee,
        ai: crate::ai_config::load_ai_config(content, &[]),
        provenance: l.prov,
    })
}
