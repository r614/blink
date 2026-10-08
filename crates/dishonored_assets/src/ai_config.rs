//! NPC perception and behaviour tuning from the install: Arkane AI tweaks plus
//! `DefaultAI.ini` / `DefaultNPC.ini`. Every field that is read records its source in
//! `AiConfig::provenance`; fields not found keep the `AiConfig::default()` value (which was
//! read from the same objects) and are marked as such.

use crate::content::Content;
use crate::ini::Ini;
use crate::tweaks::{TMap, TVal, TweakDb, get_f32, get_obj};
use dishonored_ir::ai::{AiConfig, AttentionConfig, AttentionIncrease, MeleeContextConfig, VisTuning};

/// Attention profiles: (field, tweak object). The civilian profiles are cooked into level
/// script packages (`L_Boyle_Ext_Script`); without them their fallback (the guard profile)
/// applies, which has the same values.
const GUARD_UNS: &str = "Twk_Attn_Guard_Unsuspecting.Twk_Attn_Guard_Unsuspecting";
const GUARD_SUS: &str = "Twk_Attn_Guard_Suspecting.Twk_Attn_Guard_Suspecting";
const CIV_UNS: &str = "Twk_Attn_Noncombatant_Uns.Twk_Attn_Noncombatant_Uns";
const CIV_SUS: &str = "Twk_Attn_Noncombatant_Sus.Twk_Attn_Noncombatant_Sus";
const NEUTRAL: &str = "Twk_Attn_Default_Neutral.Twk_Attn_Default_Neutral";
const VISION: &str = "Twk_Vision_Default.Twk_Vision_Default";
/// Level script packages that cook the NPC locomotion configs and animation sets (as the
/// runtime's animation bank reads them).
pub(crate) const NPC_ANIM_PACKAGES: &[&str] = &["L_Boyle_Ext_Script", "L_Boyle_Int_Script", "L_BoyleStreet_Ext_Script"];
const GUARD_SWORD_SET: &str = "Npc_EliteGuard_Sword_as.Npc_EliteGuard_Sword_as";

fn level_index(name: &str) -> Option<u8> {
    Some(match name {
        "DALFI_None" => 0,
        "DALFI_HeadTrack" => 1,
        "DALFI_TurnToFace" => 2,
        "DALFI_Investigate" => 3,
        "DALFI_Busted" => 4,
        "DALFI_MaxAttention" => 5,
        _ => return None,
    })
}

fn increase(m: &TMap, key: &str, default: AttentionIncrease) -> AttentionIncrease {
    match m.get(key) {
        Some(TVal::Struct(s)) => AttentionIncrease {
            amount: get_f32(s, "m_fByAmount").unwrap_or(0.0),
            to_level: match s.get("m_ToThreshold") {
                Some(TVal::Name(n)) => level_index(n).unwrap_or(default.to_level),
                _ => 0,
            },
        },
        _ => default,
    }
}

fn attention(m: &TMap, d: &AttentionConfig) -> AttentionConfig {
    let f = |k: &str, dv: f32| get_f32(m, k).unwrap_or(dv);
    let mut start = d.start;
    let mut stop = d.stop;
    for i in 0..4 {
        let key = if i == 0 { "m_AttentionThresholds".to_string() } else { format!("m_AttentionThresholds[{i}]") };
        if let Some(TVal::Struct(s)) = m.get(&key) {
            start[i] = get_f32(s, "m_fAttentionLevelToStart").unwrap_or(start[i]);
            stop[i] = get_f32(s, "m_fAttentionLevelToStop").unwrap_or(stop[i]);
        }
    }
    let seeing_scalar_player = match m.get("m_SeeingPawnAttentionIncreaseScalar_ToPlayer") {
        Some(TVal::Struct(s)) => get_f32(s, "m_fBaseValue2_Normal").unwrap_or(d.seeing_scalar_player),
        _ => d.seeing_scalar_player,
    };
    let distance_tuning = match m.get("m_DistanceValueTuning") {
        Some(TVal::Struct(s)) => tuning(s, &d.distance_tuning),
        _ => d.distance_tuning.clone(),
    };
    let mut stopping_variance = d.stopping_variance;
    for (i, v) in stopping_variance.iter_mut().enumerate() {
        let key = if i == 0 { "m_AttentionThresholds".to_string() } else { format!("m_AttentionThresholds[{i}]") };
        if let Some(TVal::Struct(s)) = m.get(&key) {
            *v = get_f32(s, "m_fStoppingVariance").unwrap_or(0.0);
        }
    }
    // `m_ReasonSettings[i].m_bCanIncreaseToBusted` (struct default false).
    let mut can_bust = 0u32;
    let mut any_reason = false;
    for i in 0..21 {
        let key = if i == 0 { "m_ReasonSettings".to_string() } else { format!("m_ReasonSettings[{i}]") };
        if let Some(TVal::Struct(s)) = m.get(&key) {
            any_reason = true;
            if matches!(s.get("m_bCanIncreaseToBusted"), Some(TVal::B(true))) {
                can_bust |= 1 << i;
            }
        }
    }
    if !any_reason {
        can_bust = d.can_bust;
    }
    let clamp_target_visibility = match m.get("m_bClampTargetVisibility") {
        Some(TVal::B(b)) => *b,
        _ => d.clamp_target_visibility,
    };
    AttentionConfig {
        leak_rate: f("m_fAttentionLeakRate", d.leak_rate),
        max: f("m_fMaxAttention", d.max),
        start,
        stop,
        danger_sound: increase(m, "m_IncreaseForDangerSound", d.danger_sound),
        threatening_sound: increase(m, "m_IncreaseForThreateningSound", d.threatening_sound),
        anomaly_sound: increase(m, "m_IncreaseForAnomalySound", d.anomaly_sound),
        help_sound: increase(m, "m_IncreaseForHelpSound", d.help_sound),
        alarm_sound: increase(m, "m_IncreaseForAlarmSound", d.alarm_sound),
        death_rattle: increase(m, "m_IncreaseForDeathRattleSound", d.death_rattle),
        attacked_by: increase(m, "m_IncreaseForAttackedBy", d.attacked_by),
        attacked_enemy: increase(m, "m_IncreaseForAttackedEnemy", d.attacked_enemy),
        discovered_corpse: increase(m, "m_IncreaseForDiscoveredCorpse", d.discovered_corpse),
        ally_combat_target: increase(m, "m_IncreaseForAllyCombatTarget", d.ally_combat_target),
        ally_panic_target: increase(m, "m_IncreaseForAllyPanicTarget", d.ally_panic_target),
        ally_search_target: increase(m, "m_IncreaseForAllySearchTarget", d.ally_search_target),
        touching_rate: f("m_fTouchingPawnAttentionIncreaseRate", d.touching_rate),
        seeing_scalar_player,
        seeing_scalar_npc: f("m_fSeeingPawnAttentionIncreaseScalar_ToLiveNPC", d.seeing_scalar_npc),
        distance_to_lose_sight: f("m_fDistanceToLoseSight", d.distance_to_lose_sight),
        distance_tuning,
        clamp_target_visibility,
        stopping_variance,
        can_bust,
        seeing_scalar_dead_npc: f("m_fSeeingPawnAttentionIncreaseScalar_ToDeadNPC", d.seeing_scalar_dead_npc),
        min_range_unsighted_busted_proxy: f("m_fMinRangeToUnsightedBustedProxy", d.min_range_unsighted_busted_proxy),
        magic: increase(m, "m_IncreaseForMagic", d.magic),
    }
}

/// A `DisVisCalcTuningParams` struct.
fn tuning(s: &TMap, d: &VisTuning) -> VisTuning {
    let points = match s.get("m_Points") {
        Some(TVal::List(pts)) => pts.iter().map(|p| [get_f32(p, "m_fInputValue").unwrap_or(0.0), get_f32(p, "m_fOutputValue").unwrap_or(0.0)]).collect(),
        _ => d.points.clone(),
    };
    VisTuning {
        multiply: match s.get("m_ApplyOp") {
            Some(TVal::Name(n)) => n == "DVCTPAO_Multiply",
            _ => d.multiply,
        },
        scalar: get_f32(s, "m_fScalar").unwrap_or(d.scalar),
        bias: get_f32(s, "m_fBias").unwrap_or(d.bias),
        exponent: get_f32(s, "m_fExponent").unwrap_or(d.exponent),
        use_points: match s.get("m_bUsePointsInsteadOfFormula") {
            Some(TVal::B(b)) => *b,
            _ => d.use_points,
        },
        points,
    }
}

/// Load the AI tuning. `extra` names level packages holding level-cooked tweaks.
pub fn load_ai_config(content: &Content, extra: &[String]) -> AiConfig {
    let mut out = AiConfig::default();
    let db = match TweakDb::load_with(content, extra) {
        Ok(db) => db,
        Err(e) => {
            out.provenance.insert("ai".into(), format!("defaults (tweak database failed: {e:#})"));
            return out;
        }
    };
    let mut prov = std::mem::take(&mut out.provenance);
    let mut profile = |path: &str, field: &str, d: &AttentionConfig| -> AttentionConfig {
        match db.try_resolve(path) {
            Some(m) => {
                prov.insert(format!("ai.{field}"), path.to_string());
                attention(&m, d)
            }
            None => {
                prov.insert(format!("ai.{field}"), format!("fallback value (not in the loaded packages: {path})"));
                d.clone()
            }
        }
    };
    out.guard_unsuspecting = profile(GUARD_UNS, "guard_unsuspecting", &out.guard_unsuspecting);
    out.guard_suspecting = profile(GUARD_SUS, "guard_suspecting", &out.guard_suspecting);
    out.civilian_unsuspecting = profile(CIV_UNS, "civilian_unsuspecting", &out.guard_unsuspecting);
    out.civilian_suspecting = profile(CIV_SUS, "civilian_suspecting", &out.guard_suspecting);
    out.neutral = profile(NEUTRAL, "neutral", &out.neutral);
    if let Some(v) = db.try_resolve(VISION) {
        let g = |k: &str, d: f32| get_f32(&v, k).unwrap_or(d);
        let d = out.vision.clone();
        out.vision.inner_fov_deg = g("m_fVision_InnerFOV_Deg", d.inner_fov_deg);
        out.vision.outer_fov_deg = g("m_fVision_OuterFOV_Deg", d.outer_fov_deg);
        out.vision.inner_range = g("m_fVision_InnerMaxRange", d.inner_range);
        out.vision.outer_range = g("m_fVision_OuterMaxRange", d.outer_range);
        out.vision.inner_up = g("m_fVision_InnerVerticalFOVScale_Up", d.inner_up);
        out.vision.inner_down = g("m_fVision_InnerVerticalFOVScale_Down", d.inner_down);
        out.vision.outer_up = g("m_fVision_OuterVerticalFOVScale_Up", d.outer_up);
        out.vision.outer_down = g("m_fVision_OuterVerticalFOVScale_Down", d.outer_down);
        prov.insert("ai.vision".into(), VISION.into());
    }
    let beh = |path: &str| db.try_resolve(path);
    // Ambient barks of the patrol / guard behaviours (`DisTweaks_AISubProcess_AmbientBarks`).
    const AMBIENT: &str = "AI_BehaviorTweaks_Patrol.TweaksBehavior_Patrol_Default.DisTweaks_AISubProcess_AmbientBarks_0";
    if let Some(m) = beh(AMBIENT) {
        out.ambient_bark_inhibition = get_f32(&m, "m_fBarkInhibitionDuration").unwrap_or(out.ambient_bark_inhibition);
        prov.insert("ai.ambient_bark_inhibition".into(), format!("{AMBIENT}.m_fBarkInhibitionDuration"));
    }
    if let Some(m) = beh("AI_BehaviorTweaks_Search.TweaksBehavior_Search_Default") {
        out.search.cancel_investigate_wait = [get_f32(&m, "m_fCancelInvestigateWaitTimeMin").unwrap_or(0.5), get_f32(&m, "m_fCancelInvestigateWaitTimeMax").unwrap_or(1.0)];
        out.search.reached_broadcast_range = get_f32(&m, "m_fSearchReachedProxyBroadcastRange").unwrap_or(out.search.reached_broadcast_range);
        out.search.stare_radius = get_f32(&m, "m_fInvestigateStareAtUnreachableRadius").unwrap_or(out.search.stare_radius);
        out.search.stare_max_time = get_f32(&m, "m_fInvestigateStareAtUnreachableMaxTime_s").unwrap_or(out.search.stare_max_time);
        out.search.anim_cooldown = get_f32(&m, "m_fSearchAnimCooldown").unwrap_or(out.search.anim_cooldown);
        prov.insert("ai.search.cancel_investigate_wait".into(), "AI_BehaviorTweaks_Search.TweaksBehavior_Search_Default".into());
        prov.insert("ai.search.stare".into(), "AI_BehaviorTweaks_Search.TweaksBehavior_Search_Default.m_fInvestigateStareAtUnreachable{Radius,MaxTime_s}, m_fSearchAnimCooldown".into());
    }
    if let Some(m) = beh("AI_BehaviorTweaks_Search.TweaksBehavior_Search_Default.DisTweaks_AISubState_Investigate_0") {
        let s = &mut out.search;
        s.proximity = get_f32(&m, "m_fProximityThresholds").unwrap_or(s.proximity);
        s.pathing_fail_retries = match m.get("m_PathingFailRetryCount") {
            Some(TVal::I(v)) => (*v).max(0) as u32,
            _ => s.pathing_fail_retries,
        };
        s.pathing_fail_scale = get_f32(&m, "m_fPathingFailDistanceScale").unwrap_or(s.pathing_fail_scale);
        s.pathing_fail_range = [
            get_f32(&m, "m_fPathingFailPointerSearchRange_Horiz").unwrap_or(s.pathing_fail_range[0]),
            get_f32(&m, "m_fPathingFailPointerSearchRange_Up").unwrap_or(s.pathing_fail_range[1]),
            get_f32(&m, "m_fPathingFailPointerSearchRange_Down").unwrap_or(s.pathing_fail_range[2]),
        ];
        prov.insert("ai.search.proximity".into(), "TweaksBehavior_Search_Default.DisTweaks_AISubState_Investigate_0.m_fProximityThresholds".into());
        prov.insert("ai.search.pathing_fail".into(), "TweaksBehavior_Search_Default.DisTweaks_AISubState_Investigate_0.m_PathingFail*".into());
    }
    if let Some(m) = beh("AI_BehaviorTweaks_Search.TweaksBehavior_Search_Default.DisTweaks_AISubState_TrackTarget_1") {
        out.search.track_proximity = get_f32(&m, "m_fProximityThreshold").unwrap_or(out.search.track_proximity);
        prov.insert("ai.search.track_proximity".into(), "TweaksBehavior_Search_Default.DisTweaks_AISubState_TrackTarget_1.m_fProximityThreshold".into());
    }
    if let Some(m) = beh("AI_BehaviorTweaks_Guard.TweaksBehavior_Guard_Default") {
        out.guard_attention_span = get_f32(&m, "m_fAttentionSpan").unwrap_or(out.guard_attention_span);
        prov.insert("ai.guard_attention_span".into(), "AI_BehaviorTweaks_Guard.TweaksBehavior_Guard_Default.m_fAttentionSpan".into());
    }
    if let Some(m) = beh("AI_BehaviorTweaks_Panic.TweaksBehavior_Panic_Civilian") {
        let p = &mut out.panic;
        p.cower_distance = get_f32(&m, "m_fCowerReactionDistance").unwrap_or(p.cower_distance);
        p.flee_point_claim_radius = get_f32(&m, "m_fFleePointClaimRadius").unwrap_or(p.flee_point_claim_radius);
        p.flee_interception_distance = get_f32(&m, "m_fFleeInterceptionDistance").unwrap_or(p.flee_interception_distance);
        p.free_flight_time = get_f32(&m, "m_fFreeFlightTimer").unwrap_or(p.free_flight_time);
        prov.insert("ai.panic".into(), "AI_BehaviorTweaks_Panic.TweaksBehavior_Panic_Civilian".into());
    }
    if let Some(m) = beh("AI_BehaviorTweaks_Panic.TweaksBehavior_Panic_Civilian.DisTweaks_AISubState_Flee_0") {
        out.panic.forbidden_angle_deg = get_f32(&m, "m_fForbiddenAngle").unwrap_or(out.panic.forbidden_angle_deg);
        out.panic.forbidden_radius = get_f32(&m, "m_fForbiddenRadius").unwrap_or(out.panic.forbidden_radius);
    }
    if let Some(m) = beh("AI_BehaviorTweaks_TriggerAlarm.TweaksBehavior_TriggerAlarm_Party") {
        out.alarm_companion_range = get_f32(&m, "m_fCompanionRange").unwrap_or(out.alarm_companion_range);
        out.alarm_fight_radius = get_f32(&m, "m_fFightRadius").unwrap_or(out.alarm_fight_radius);
        // Unset chances keep the class default 0.
        let ta = &mut out.trigger_alarm;
        let g = |k: &str| get_f32(&m, k).unwrap_or(0.0);
        ta.chance_sighting_alone = g("m_fAlarmChanceUponSighting_Alone");
        ta.chance_sighting_in_group = g("m_fAlarmChanceUponSighting_InGroup");
        ta.chance_found_corpse = g("m_fAlarmChanceUponFoundCorpse");
        ta.chance_witness_death = g("m_fAlarmChanceUponWitnessDeath");
        ta.chance_help = g("m_fAlarmChanceUponHelp");
        ta.fight_radius = out.alarm_fight_radius;
        ta.companion_range = out.alarm_companion_range;
        ta.wait_before_ring = get_f32(&m, "m_fWaitTimeBeforeRing").unwrap_or(ta.wait_before_ring);
        ta.companion_must_be_ally = matches!(m.get("m_bCompanionMustBeAlly"), Some(TVal::B(true)));
        ta.companion_must_match_squad = matches!(m.get("m_bCompanionMustMatchSquad"), Some(TVal::B(true)));
        prov.insert("ai.alarm".into(), "AI_BehaviorTweaks_TriggerAlarm.TweaksBehavior_TriggerAlarm_Party".into());
    }
    if let Some(m) = beh("AI_BehaviorTweaks_EliteGuard.TweaksBehavior_CombatEliteGuard") {
        let rng = |k: &str, d: [f32; 2]| match m.get(k) {
            Some(TVal::Struct(s)) => [get_f32(s, "m_fMinValue").unwrap_or(0.0), get_f32(s, "m_fMaxValue").unwrap_or(d[1])],
            _ => d,
        };
        out.combat.gun_usage_start = rng("m_GunUsageStart", out.combat.gun_usage_start);
        out.combat.gun_usage_cooldown = rng("m_GunUsageCooldown", out.combat.gun_usage_cooldown);
        prov.insert("ai.combat.gun_usage".into(), "AI_BehaviorTweaks_EliteGuard.TweaksBehavior_CombatEliteGuard".into());
        // Its sub-process / sub-state tweaks: ManageAttacks (pacing, dodges) and FirePistol.
        let frng = |s: &TMap, k: &str, d: [f32; 2]| match s.get(k) {
            Some(TVal::Struct(r)) => [get_f32(r, "m_fMinValue").unwrap_or(d[0]), get_f32(r, "m_fMaxValue").unwrap_or(d[1])],
            _ => d,
        };
        let irng = |s: &TMap, k: &str, d: [i32; 2]| match s.get(k) {
            Some(TVal::Struct(r)) => {
                let g = |n: &str, d: i32| match r.get(n) {
                    Some(TVal::I(v)) => *v,
                    _ => d,
                };
                [g("m_MinValue", d[0]), g("m_MaxValue", d[1])]
            }
            _ => d,
        };
        let subs: Vec<String> = ["m_SubProcessTweaks", "m_SubStateTweaks"]
            .iter()
            .flat_map(|k| match m.get(*k) {
                Some(TVal::List(l)) => l.iter().filter_map(|e| get_obj(e, "obj").map(str::to_string)).collect(),
                _ => Vec::new(),
            })
            .collect();
        for path in subs {
            let Some(s) = db.try_resolve(&path) else { continue };
            if s.contains_key("m_AttacksPerFlurry") || s.contains_key("m_IncomingAttacksBeforeBackStep") {
                let d = &mut out.combat.dodge;
                d.attacks_per_flurry = irng(&s, "m_AttacksPerFlurry", d.attacks_per_flurry);
                d.stamina_recovery = frng(&s, "m_StaminaRecoveryDuration", d.stamina_recovery);
                d.time_for_threat_side_step = frng(&s, "m_TimeForThreatToCauseSideStep", d.time_for_threat_side_step);
                d.angle_melee_threat = get_f32(&s, "m_fAngleOfPerceivedMeleeThreat").unwrap_or(d.angle_melee_threat);
                d.angle_ranged_threat = get_f32(&s, "m_fAngleOfPerceivedRangedThreat").unwrap_or(d.angle_ranged_threat);
                d.incoming_attacks_before_back_step = irng(&s, "m_IncomingAttacksBeforeBackStep", d.incoming_attacks_before_back_step);
                d.forgive_incoming_attacks = get_f32(&s, "m_fTimeToForgiveIncomingAttacks").unwrap_or(d.forgive_incoming_attacks);
                d.aggression_delta_per_damage = get_f32(&s, "m_fAggressionDeltaPerDamage").unwrap_or(d.aggression_delta_per_damage);
                d.aggression_regen_rate = get_f32(&s, "m_fAggressionRegenRate").unwrap_or(d.aggression_regen_rate);
                d.cooldown_factor_at_min_aggression = get_f32(&s, "m_fCooldownFactorAtMinAggression").unwrap_or(d.cooldown_factor_at_min_aggression);
                d.stalemate_allowance = get_f32(&s, "m_fStalemateAllowanceTime").unwrap_or(d.stalemate_allowance);
                d.blocked_attack_sensitivity = get_f32(&s, "m_fBlockedAttackSensitivity").unwrap_or(d.blocked_attack_sensitivity);
                prov.insert("ai.combat.dodge".into(), path.clone());
            }
            if s.contains_key("m_TimeToFirstShot") || s.contains_key("m_TimeBeforeGivingUp") {
                let p = &mut out.combat.pistol;
                p.time_to_first_shot = frng(&s, "m_TimeToFirstShot", p.time_to_first_shot);
                p.time_before_giving_up = frng(&s, "m_TimeBeforeGivingUp", p.time_before_giving_up);
                p.point_blank_distance = get_f32(&s, "m_fPointBlankDistance").unwrap_or(p.point_blank_distance);
                p.chance_fire_again = get_f32(&s, "m_fChanceOfFiringAgain").unwrap_or(p.chance_fire_again);
                prov.insert("ai.combat.pistol".into(), path.clone());
            }
        }
    }
    let ini_ai = Ini::load_layered(&content.root, "AI");
    out.combat.dodge.side_step_inhibition = ini_ai.get_f32("DishonoredGame.DisGlobalCombatManager", "m_fInhibitionTimeSideStep").unwrap_or(out.combat.dodge.side_step_inhibition);
    out.combat.dodge.back_step_inhibition = ini_ai.get_f32("DishonoredGame.DisGlobalCombatManager", "m_fInhibitionTimeBackStep").unwrap_or(out.combat.dodge.back_step_inhibition);
    // The elite guard's item contexts: the pawn-specific contexts of its sword and pistol
    // (`Twk_Inv_SwordEliteGuard` / `Twk_Inv_PistolEliteGuard`, `DisTweaks_InventoryItem_PawnSpecific`)
    // give the ranges, the dodge distances and the gun's range.
    let item_ctx = |item: &str, class: &str| db.paths_with_class(class).into_iter().find(|p| p.starts_with(item)).and_then(|p| db.try_resolve(&p).map(|m| (p, m)));
    const SWORD: &str = "Twk_Inv_EliteGuard.Twk_Inv_SwordEliteGuard.";
    const PISTOL: &str = "Twk_Inv_EliteGuard.Twk_Inv_PistolEliteGuard.";
    if let Some((_, s)) = item_ctx(SWORD, "DisTweaks_NPCSideStep") {
        out.combat.dodge.side_step_distance = get_f32(&s, "m_fSideStepDistance").unwrap_or(out.combat.dodge.side_step_distance);
    }
    if let Some((_, s)) = item_ctx(SWORD, "DisTweaks_NPCBackStep") {
        out.combat.dodge.back_step_distance = get_f32(&s, "m_fBackStepDistance").unwrap_or(out.combat.dodge.back_step_distance);
    }
    if let Some((p, s)) = item_ctx(PISTOL, "DisTweaks_NPCFireGun") {
        out.combat.pistol.min_range = get_f32(&s, "m_fMinContextRange").unwrap_or(out.combat.pistol.min_range);
        out.combat.pistol.max_range = get_f32(&s, "m_fMaxContextRange").unwrap_or(out.combat.pistol.max_range);
        prov.insert("ai.combat.pistol.range".into(), p);
    }
    for c in out.combat.melee_contexts.iter_mut() {
        let class = c.name.replace("DisItemContext_", "DisTweaks_");
        if let Some((_, s)) = item_ctx(SWORD, &class) {
            c.min_range = get_f32(&s, "m_fMinContextRange").unwrap_or(c.min_range);
            c.max_range = get_f32(&s, "m_fMaxContextRange").unwrap_or(c.max_range);
            // A negative height difference is no limit.
            c.max_height = get_f32(&s, "m_fMaxContextHeightDifference").map(|h| if h < 0.0 { f32::MAX } else { h }).unwrap_or(c.max_height);
            c.damage_multiplier = get_f32(&s, "m_fAttackDamageMultiplier").or(c.damage_multiplier);
        }
    }
    // The sword's `Attribute_MeleeDamage` per difficulty (its `m_pAttributeTweak[1]`), the damage
    // of every NPC slash (0x00C33DD0 -> 0x00C07C80).
    let sword_attrs = db.try_resolve(SWORD.trim_end_matches('.')).and_then(|m| get_obj(&m, "m_pAttributeTweak[1]").map(str::to_string));
    if let Some(m) = sword_attrs.as_deref().and_then(|p| db.try_resolve(p)) {
        for (i, k) in ["m_fBaseValue1_Easy", "m_fBaseValue2_Normal", "m_fBaseValue3_Hard", "m_fBaseValue4_VeryHard"].iter().enumerate() {
            out.combat.npc_sword_damage[i] = get_f32(&m, &format!("m_MeleeDamage.{k}")).unwrap_or(out.combat.npc_sword_damage[i]);
        }
        prov.insert("ai.combat.npc_sword_damage".into(), format!("{} :: m_MeleeDamage", sword_attrs.unwrap_or_default()));
    }
    // The guard archetype's attributes (Normal): HealthMax and MeleeDamageBonus.
    const GUARD_PAWN: &str = "Pwn_EliteGuard_MTall_0.Pwn_EliteGuard_MTall_0";
    let pawn_attrs = db.try_resolve(GUARD_PAWN).and_then(|m| get_obj(&m, "m_pAttributeTweaks[1]").map(str::to_string));
    if let Some(m) = pawn_attrs.as_deref().and_then(|p| db.try_resolve(p)) {
        let normal = |k: &str| get_f32(&m, &format!("{k}.m_fBaseValue2_Normal"));
        out.combat.guard_health_max = normal("m_HealthMax").unwrap_or(out.combat.guard_health_max);
        out.combat.npc_melee_damage_bonus = normal("m_MeleeDamageBonus").unwrap_or(out.combat.npc_melee_damage_bonus);
        prov.insert("ai.combat.guard_health_max".into(), format!("{} :: m_HealthMax / m_MeleeDamageBonus", pawn_attrs.unwrap_or_default()));
    }
    prov.insert("ai.combat.melee_ranges".into(), format!("{SWORD}DisTweaks_InventoryItem_PawnSpecific_1.DisTweaks_NPC*"));
    // Rat swarms: the default swarm tweak and the Devouring Swarm's.
    if let Some(s) = beh("Default__DisTweaks_RatSwarm") {
        out.rats = rat_swarm(&s, &out.rats);
    }
    if let Some(s) = beh("Twk_RatSwarm.Twk_RandomRatSwarm") {
        out.rats = rat_swarm(&s, &out.rats);
        prov.insert("ai.rats".into(), "Twk_RatSwarm.Twk_RandomRatSwarm".into());
    }
    if let Some(p) = beh("Twk_Powers_DevouringSwarm.Twk_DevouringSwarm").and_then(|m| match m.get("m_Levels") {
        Some(TVal::Struct(l)) => get_obj(l, "m_pRatSwarmTweaks").map(str::to_string),
        _ => None,
    }) {
        if let Some(s) = beh(&p) {
            out.devouring_swarm = rat_swarm(&s, &out.devouring_swarm);
            prov.insert("ai.devouring_swarm".into(), p);
        }
    }
    if let Some(m) = beh("AI_BrainTweaks_Elite.BrainTweaks_Guard_Elite_Party").or_else(|| beh("AI_BrainTweaks_Guard.BrainTweaks_Guard")) {
        out.combat.reachable_melee_range = get_f32(&m, "m_fReachableMeleeRange").unwrap_or(out.combat.reachable_melee_range);
        out.combat.reachable_melee_height = get_f32(&m, "m_fReachableMeleeHeight").unwrap_or(out.combat.reachable_melee_height);
        prov.insert("ai.combat.reachable_melee".into(), "guard brain tweak m_fReachableMelee{Range,Height}".into());
    }
    // Player visibility (`DishonoredVisibilityComponent` tuning).
    if let Some(m) = beh("Twk_PlayerVisibility.Twk_PlayerVisibility") {
        let pv = &mut out.player_visibility;
        if let Some(TVal::Struct(s)) = m.get("m_LightValueTuning") {
            pv.light = tuning(s, &pv.light);
        }
        if let Some(TVal::Struct(s)) = m.get("m_SpeedValueTuning") {
            pv.speed = tuning(s, &pv.speed);
        }
        // Unset bonuses keep the class default 0.
        let b = |k: &str| get_f32(&m, k).unwrap_or(0.0);
        pv.bonus_autocrouching = b("m_fVisLevelBonus_Autocrouching");
        pv.bonus_sneaking = b("m_fVisLevelBonus_Sneaking");
        pv.bonus_sliding = b("m_fVisLevelBonus_Sliding");
        pv.bonus_standing = b("m_fVisLevelBonus_Standing");
        pv.bonus_walking = b("m_fVisLevelBonus_Walking");
        pv.bonus_sprinting = b("m_fVisLevelBonus_Sprinting");
        pv.bonus_swimming = b("m_fVisLevelBonus_Swimming");
        pv.bonus_swimming_surface = b("m_fVisLevelBonus_SwimmingOnSurface");
        pv.zero_speed_sneaking = matches!(m.get("m_bForceZeroSpeedWhileSneaking"), Some(TVal::B(true)));
        prov.insert("ai.player_visibility".into(), "Twk_PlayerVisibility.Twk_PlayerVisibility".into());
    }
    if let Some(m) = beh("Twk_GlobalPlayerVisSettings.Twk_GlobalPlayerVisSettings") {
        out.player_visibility.light_darkest = get_f32(&m, "m_fLightValueDarkestVal").unwrap_or(out.player_visibility.light_darkest);
        out.player_visibility.light_brightest = get_f32(&m, "m_fLightValueBrightestVal").unwrap_or(out.player_visibility.light_brightest);
    }
    // INIs.
    let ai = Ini::load_layered(&content.root, "AI");
    if let Some(v) = ai.get_f32("DishonoredGame.DishonoredGlobalAIManager", "m_fGroupDistance") {
        out.group_distance = v;
        prov.insert("ai.group_distance".into(), "DefaultAI.ini [DishonoredGame.DishonoredGlobalAIManager] m_fGroupDistance".into());
    }
    // Melee contexts: the elite guard attack pattern's cooldowns (cooked into the level script
    // packages; without them the defaults hold the same values) and the combat manager's
    // inhibition times.
    let path = "AI_AttackPatterns_Shared.TwkAttackPattern_BerserkerElite";
    if let Some(m) = beh(path) {
        if let Some(TVal::List(items)) = m.get("m_ItemContextOverrides") {
            let inhibit = |class: &str| -> f32 {
                let key = match class {
                    "DisItemContext_NPCAttackShort" => "m_fInhibitionTimeShortAttack",
                    "DisItemContext_NPCAttackMedium" => "m_fInhibitionTimeMediumAttack",
                    "DisItemContext_NPCAttackStepBack" => "m_fInhibitionTimeStepBackAttack",
                    "DisItemContext_NPCAttackLong" => "m_fInhibitionTimeLongAttack",
                    "DisItemContext_NPCJumpAttack" => "m_fInhibitionTimeJumpAttack",
                    _ => return 0.0,
                };
                ai.get_f32("DishonoredGame.DisGlobalCombatManager", key).unwrap_or(0.0)
            };
            let mut ctx = Vec::new();
            // Pattern order is the tweak's; the melee attacks (the dodges and the gun have
            // their own rules), with their item-context class ranges.
            for it in items {
                let class = match it.get("m_pContextToOverride") {
                    Some(TVal::Obj(o)) => o.rsplit('.').next().unwrap_or(o).to_string(),
                    _ => continue,
                };
                if !matches!(it.get("m_bContextEnabled"), Some(TVal::B(true))) || inhibit(&class) == 0.0 {
                    continue;
                }
                let cd = match it.get("m_ContextCooldown") {
                    Some(TVal::Struct(s)) => [get_f32(s, "m_fMinValue").unwrap_or(0.0), get_f32(s, "m_fMaxValue").unwrap_or(0.0)],
                    _ => [0.0, 0.0],
                };
                ctx.push(MeleeContextConfig::of_class(&class, cd, inhibit(&class)));
            }
            if !ctx.is_empty() {
                out.combat.melee_contexts = ctx;
                prov.insert("ai.combat.melee_contexts".into(), format!("{path}.m_ItemContextOverrides + DefaultAI.ini m_fInhibitionTime*"));
            }
        }
    }
    if let Some(v) = ai.sections.get("DishonoredGame.DisAINoiseManager").and_then(|s| s.get("m_LoudnessDistanceArray")) {
        let d: Vec<f32> = v.iter().filter_map(|x| x.parse().ok()).collect();
        if !d.is_empty() {
            out.loudness_distances = d;
            prov.insert("ai.loudness_distances".into(), "DefaultAI.ini [DishonoredGame.DisAINoiseManager] m_LoudnessDistanceArray".into());
        }
    }
    let range = |s: Option<&str>| -> Option<[f32; 2]> {
        let s = s?;
        let lo = s.split("m_fMinValue=").nth(1)?.split([',', ')']).next()?.parse().ok()?;
        let hi = s.split("m_fMaxValue=").nth(1)?.split([',', ')']).next()?.parse().ok()?;
        Some([lo, hi])
    };
    if let Some(r) = range(ai.get("DishonoredGame.DishonoredGlobalAIManager", "m_ReactionDelay")) {
        out.reaction_delay = r;
        prov.insert("ai.reaction_delay".into(), "DefaultAI.ini [DishonoredGame.DishonoredGlobalAIManager] m_ReactionDelay".into());
    }
    if let Some(r) = range(ai.get("DishonoredGame.DishonoredGlobalAIManager", "m_MagicReactionDelay")) {
        out.magic_reaction_delay = r;
        prov.insert("ai.magic_reaction_delay".into(), "DefaultAI.ini [DishonoredGame.DishonoredGlobalAIManager] m_MagicReactionDelay".into());
    }
    // NPC locomotion configs (authored loop speeds per stance modifier) and the elite guard's
    // GenericAction animation lengths, from the level script packages that cook them.
    out.locomotion = crate::npc_anim::package_loco_configs(content, NPC_ANIM_PACKAGES);
    if !out.locomotion.is_empty() {
        prov.insert("ai.locomotion".into(), format!("ArkComponentLocomotionConfig exports of {NPC_ANIM_PACKAGES:?}: {}", out.locomotion.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")));
    }
    let seq = |name: &str| NPC_ANIM_PACKAGES.iter().find_map(|p| crate::npc_anim::sequence_play_length(content, p, GUARD_SWORD_SET, name));
    // `DisNPCAnimDefinitions_EliteGuard`: SearchLostTarget = [Sword_SwordToPatrol -> <p2>OutOfSight],
    // CorpseCheck = [<p2>CorpseCheck], MagicLostTarget = [<p2>Magic_OutOfSight]; the search keeps
    // the sword out (`m_bAlwaysSearchWithSwordOut`), so the primary prefix is `Sword_`.
    if let (Some(a), Some(b)) = (seq("Sword_SwordToPatrol"), seq("Sword_OutOfSight")) {
        out.search.anim_lost_target = a + b;
    }
    if let Some(a) = seq("Sword_CorpseCheck") {
        out.search.anim_corpse_check = a;
    }
    if let Some(a) = seq("Sword_Magic_OutOfSight") {
        out.anim_magic_lost_target = a;
    }
    prov.insert("ai.search.anim_lengths".into(), format!("{GUARD_SWORD_SET} SequenceLength / RateScale: SearchLostTarget {} s, CorpseCheck {} s, MagicLostTarget {} s", out.search.anim_lost_target, out.search.anim_corpse_check, out.anim_magic_lost_target));
    let crumbs = "DishonoredGame.DishonoredSearchCrumbsComponent";
    let s = &mut out.search;
    s.crumbs = ai.get(crumbs, "m_iNumSearchCrumbs").and_then(|v| v.parse().ok()).unwrap_or(s.crumbs);
    s.crumb_spacing = ai.get_f32(crumbs, "m_fDistanceBetweenCrumbs").unwrap_or(s.crumb_spacing);
    s.crumb_lifetime = ai.get_f32(crumbs, "m_fCrumbLifetime").unwrap_or(s.crumb_lifetime);
    s.heartbeat = ai.get_f32(crumbs, "m_fHeartbeatInterval").unwrap_or(s.heartbeat);
    s.wander_min = ai.get_f32(crumbs, "m_fMinWanderStep").unwrap_or(s.wander_min);
    s.wander_max = ai.get_f32(crumbs, "m_fMaxWanderStep").unwrap_or(s.wander_max);
    s.wander_angle_deg = ai.get_f32(crumbs, "m_fWanderAngularRange").unwrap_or(s.wander_angle_deg);
    prov.insert("ai.search.crumbs".into(), format!("DefaultAI.ini [{crumbs}]"));
    let cm = "DishonoredGame.DisGlobalCombatManager";
    if let Some(v) = ai.get(cm, "m_InnerSkirmishCaps[1]").and_then(|v| v.parse().ok()) {
        out.combat.skirmish_inner = v;
    }
    if let Some(v) = ai.get(cm, "m_TotalSkirmishCaps[1]").and_then(|v| v.parse().ok()) {
        out.combat.skirmish_total = v;
    }
    prov.insert("ai.combat.skirmish".into(), format!("DefaultAI.ini [{cm}] m_InnerSkirmishCaps[1] / m_TotalSkirmishCaps[1] (Normal)"));
    let npc = Ini::load_layered(&content.root, "NPC");
    let ctl = "DishonoredGame.DishonoredNPCController";
    out.auto_glance_radius = npc.get_f32(ctl, "m_fAutoGlanceRadius").unwrap_or(out.auto_glance_radius);
    out.auto_look_radius = npc.get_f32(ctl, "m_fAutoLookRadius").unwrap_or(out.auto_look_radius);
    out.glance_time = npc.get_f32(ctl, "m_fGlanceTime").unwrap_or(out.glance_time);
    out.combat.aim_time_before_shooting = npc.get_f32("DishonoredGame.StateNPCMasterAim", "m_fTimeAimingRequiresBeforeShooting").unwrap_or(out.combat.aim_time_before_shooting);
    prov.insert("ai.glance".into(), format!("DefaultNPC.ini [{ctl}]"));
    // Loudness enum values named in the tweaks (EAINoiseLoudness2 = index 9, Whisper = 2).
    if let Some(&d) = out.loudness_distances.get(9) {
        out.civilian_talking_loudness = d;
        prov.insert("ai.civilian_talking_loudness".into(), "BrainTweaks_Civilian.m_TalkingLoudness = EAINoiseLoudness2 -> m_LoudnessDistanceArray[9]".into());
    }
    if let Some(&d) = out.loudness_distances.get(2) {
        out.death_rattle_loudness = d;
        prov.insert("ai.death_rattle_loudness".into(), "Pwn_EliteGuard_*.m_DeathRattleLoudness = EAINoiseWhisper -> m_LoudnessDistanceArray[2]".into());
    }
    // The AI noises (contact system, weapon tweaks, footfall notifies).
    noise_table(content, &db, &mut out, &mut prov);
    out.provenance = prov;
    out
}

/// The player's pistol fire tweak (`DisTweaks_FirePistol`, `m_AINoiseLoudnessWhenFiring`).
const FIRE_PISTOL: &str = "Twk_Inv_PlayerSpecific.Twk_Inv_PistolCorvo.DisTweaks_InventoryItem_PlayerSpecific_0.DisTweaks_FirePistol_1";
/// The player's locomotion anim set (its loops carry `DishonoredNotify_Footfall`).
const LOCOMOTION_SET: &str = "Ply_Sword_Locomotion_as.Ply_Sword_Locomotion_as";

/// `AiConfig::noise` from the install: the contact system's `DisContactSub_AINoise` objects
/// (loudness -> `m_LoudnessDistanceArray`, context, speed gate), `m_AINoiseLoudnessWhenFiring`
/// of the pistol fire tweak, `m_fFall_Stun_Vel` (DefaultPlayerState.ini) and the footfall
/// notify times of the player's locomotion loops. Entries not found keep the default (the same
/// retail values) and say so in the provenance.
fn noise_table(content: &Content, db: &TweakDb, out: &mut AiConfig, prov: &mut std::collections::BTreeMap<String, String>) {
    use dishonored_ir::ai::{AiNoise, FootfallClip, NoiseContext};
    let dist = out.loudness_distances.clone();
    let radius = |name: &str| crate::ai_level::LOUDNESS_NAMES.iter().position(|n| *n == name).and_then(|i| dist.get(i).copied());
    let game = content.package("DishonoredGame").ok();
    let game_content = content.package("DishonoredGameContent").ok();
    let cs = game.as_ref().and_then(|g| dishonored_audio::contact::ContactSystem::load(g, &game_content.iter().map(|p| &**p).collect::<Vec<_>>()));
    let t = &mut out.noise;
    match &cs {
        Some(cs) => {
            const ENV: &str = "Environment";
            let mut set = |field: &str, slot: &mut AiNoise, striker: &str, struck: &str| {
                let s = format!("DisContactType_{striker}");
                let k = format!("DisContactType_{struck}");
                match cs.ai_noise(&s, &k) {
                    Some(n) => {
                        let r = if n.loudness == "EAINoiseSilent" { Some(0.0) } else { radius(&n.loudness) };
                        let ctx = NoiseContext::from_name(&n.context);
                        if let (Some(r), Some(ctx)) = (r, ctx) {
                            *slot = AiNoise::new(r, ctx, if n.volume.scale_with_speed { n.volume.min_speed } else { 0.0 });
                            prov.insert(format!("ai.noise.{field}"), format!("Dis_ContactSystem ({s}, {k}) DisContactSub_AINoise {} {} min speed {} (scaled {})", n.loudness, n.context, n.volume.min_speed, n.volume.scale_with_speed));
                        } else {
                            prov.insert(format!("ai.noise.{field}"), format!("default (unknown loudness / context {} {})", n.loudness, n.context));
                        }
                    }
                    // No AI noise sub on the pair or its ancestors: the contact is silent.
                    None => {
                        *slot = AiNoise::SILENT;
                        prov.insert(format!("ai.noise.{field}"), format!("Dis_ContactSystem ({s}, {k}): no DisContactSub_AINoise"));
                    }
                }
            };
            set("footfall_run", &mut t.footfall_run, "Footfall_Run_Player", ENV);
            set("footfall_sprint", &mut t.footfall_sprint, "Footfall_Sprint_Player", ENV);
            set("footfall_sneak", &mut t.footfall_sneak, "Footfall_Sneak_Player", ENV);
            set("footfall_silent", &mut t.footfall_silent, "Footfall_Run_Player_Silent", ENV);
            set("land", &mut t.land, "JumpLand", ENV);
            set("land_stealth", &mut t.land_stealth, "JumpLand_Stealth", ENV);
            set("land_big", &mut t.land_big, "JumpLand_Big", ENV);
            set("sword_body", &mut t.sword_body, "Sword", "Body");
            set("sword_weapon", &mut t.sword_weapon, "Sword", "Weapon");
            set("bullet_impact", &mut t.bullet_impact, "Bullet", ENV);
            set("corpse", &mut t.corpse, "Corpse", ENV);
            set("sleeping_body", &mut t.sleeping_body, "SleepingBody", ENV);
        }
        None => {
            prov.insert("ai.noise".into(), "defaults (Dis_ContactSystem not loaded)".into());
        }
    }
    match db.try_resolve(FIRE_PISTOL).and_then(|m| match m.get("m_AINoiseLoudnessWhenFiring") {
        Some(TVal::Name(n)) => Some(n.clone()),
        _ => None,
    }) {
        Some(n) => {
            if let Some(r) = radius(&n) {
                t.pistol_fire = AiNoise::new(r, NoiseContext::Danger, 0.0);
                prov.insert("ai.noise.pistol_fire".into(), format!("{FIRE_PISTOL}.m_AINoiseLoudnessWhenFiring = {n}, context Danger (0x00C2A3E0)"));
            }
        }
        None => {
            prov.insert("ai.noise.pistol_fire".into(), format!("default (m_AINoiseLoudnessWhenFiring not in {FIRE_PISTOL})"));
        }
    }
    let state = Ini::load_layered(&content.root, "PlayerState");
    if let Some(v) = state.get_f32("DishonoredGame.StatePlayerMasterFalling", "m_fFall_Stun_Vel") {
        t.fall_stun_speed = v;
        prov.insert("ai.noise.fall_stun_speed".into(), "DefaultPlayerState.ini [DishonoredGame.StatePlayerMasterFalling] m_fFall_Stun_Vel".into());
    }
    if let Some(v) = state.get_f32("DishonoredGame.StatePlayerMasterFalling", "m_fAnimateFallTime") {
        t.animate_fall_time = v;
        prov.insert("ai.noise.animate_fall_time".into(), "DefaultPlayerState.ini [DishonoredGame.StatePlayerMasterFalling] m_fAnimateFallTime".into());
    }
    if let Ok(pkg) = content.package("Startup") {
        if let Some(set) = (0..pkg.exports.len()).find(|&i| pkg.obj_path(ue3_package::ObjRef::Export(i)).eq_ignore_ascii_case(LOCOMOTION_SET)) {
            let clips = dishonored_audio::anim_notify::read_anim_set(&pkg, set);
            let clip = |name: &str, slot: &mut FootfallClip| {
                if let Some(c) = clips.get(name) {
                    let times: Vec<f32> = c.notifies.iter().filter(|n| matches!(n.kind, dishonored_audio::anim_notify::NotifyKind::Footfall { .. })).map(|n| n.time).collect();
                    if c.length > 0.0 && !times.is_empty() {
                        *slot = FootfallClip { length: c.length / c.rate.max(1e-6), footfalls: times.iter().map(|x| x / c.rate.max(1e-6)).collect() };
                        return true;
                    }
                }
                false
            };
            let ok = clip("Sword_Ready_Walk", &mut t.walk_clip) & clip("Sword_Ready_Run", &mut t.run_clip) & clip("Sword_Ready_Sprint", &mut t.sprint_clip);
            prov.insert("ai.noise.footfall_clips".into(), format!("Startup {LOCOMOTION_SET} Sword_Ready_{{Walk,Run,Sprint}} DishonoredNotify_Footfall times{}", if ok { "" } else { " (some not found: defaults)" }));
        }
    }
}

/// A `DisTweaks_RatSwarm` over defaults.
fn rat_swarm(s: &TMap, d: &dishonored_ir::ai::RatSwarmConfig) -> dishonored_ir::ai::RatSwarmConfig {
    let f = |k: &str, v: f32| get_f32(s, k).unwrap_or(v);
    let i = |k: &str, v: i32| match s.get(k) {
        Some(TVal::I(x)) => *x,
        _ => v,
    };
    let swarm_type = match s.get("m_SwarmType") {
        Some(TVal::Name(n)) => ["DRST_AmbientSwarm", "DRST_RandomSwarm", "DRST_ScriptedSwarm", "DRST_DevouringSwarm"].iter().position(|x| x == n).map(|p| p as u8).unwrap_or(d.swarm_type),
        Some(TVal::I(v)) => *v as u8,
        _ => d.swarm_type,
    };
    dishonored_ir::ai::RatSwarmConfig {
        swarm_type,
        initial_rat_count_to_be_aggressive: i("m_InitialRatCountToBeAggressive", d.initial_rat_count_to_be_aggressive),
        pawn_detection_radius: f("m_fPawnDetectionRadius", d.pawn_detection_radius),
        pawn_detection_radius_when_busy: f("m_fPawnDetectionRadiusWhenBusy", d.pawn_detection_radius_when_busy),
        pawn_escape_radius: f("m_fPawnEscapeRadius", d.pawn_escape_radius),
        player_detection_delay_after_spawn: f("m_fPlayerDetectionDelayAfterSpawn", d.player_detection_delay_after_spawn),
        pawn_unreachable_delay: f("m_fPawnUnreachableDelay", d.pawn_unreachable_delay),
        destination_radius: f("m_fDestinationRadius", d.destination_radius),
        pawn_visibility_check_interval: f("m_fPawnVisibilityCheckInterval", d.pawn_visibility_check_interval),
        idle_away_from_pawn_trigger_dist: f("m_fIdleAwayFromPawnTriggerDist", d.idle_away_from_pawn_trigger_dist),
        idle_away_from_pawn_trigger_duration: f("m_fIdleAwayFromPawnTriggerDuration", d.idle_away_from_pawn_trigger_duration),
        idle_away_from_pawn_move_dist: f("m_fIdleAwayFromPawnMoveDist", d.idle_away_from_pawn_move_dist),
        idle_away_from_pawn_count: [i("m_IdleAwayFromPawnMinCount", d.idle_away_from_pawn_count[0]), i("m_IdleAwayFromPawnMaxCount", d.idle_away_from_pawn_count[1])],
        idle_roaming_radius: f("m_fIdleRoamingRadius", d.idle_roaming_radius),
        idle_roaming_max_pause: f("m_fIdleRoamingMaxPause", d.idle_roaming_max_pause),
        min_rat_count_per_attack_group: i("m_MinRatCountPerAttackGroup", d.min_rat_count_per_attack_group),
        min_rat_count_for_new_attack_group: i("m_MinRatCountForNewAttackGroup", d.min_rat_count_for_new_attack_group),
        forced_attack_detection_radius: f("m_fForcedAttackDetectionRadius", d.forced_attack_detection_radius),
        initial_delay_before_dealing_damage: f("m_fInitialDelayBeforeDealingDamage", d.initial_delay_before_dealing_damage),
        damage_per_bite: f("m_fDamagePerBite", d.damage_per_bite),
        min_damage: f("m_fMinDamage", d.min_damage),
        max_damage: f("m_fMaxDamage", d.max_damage),
        damage_time_interval: f("m_fDamageTimeInterval", d.damage_time_interval),
        pawn_slowdown_percent: f("m_fPawnSlowdownPercent", d.pawn_slowdown_percent),
        pawn_slowdown_duration: f("m_fPawnSlowdownDuration", d.pawn_slowdown_duration),
        eat_required_rat_count: i("m_EatRequiredRatCount", d.eat_required_rat_count),
        eat_startup_duration: f("m_fEatStartupDuration", d.eat_startup_duration),
        eat_startup_delay_after_kill: f("m_fEatStartupDelayAfterKill", d.eat_startup_delay_after_kill),
        eat_per_limb_duration: f("m_fEatPerLimbDuration", d.eat_per_limb_duration),
        never_flee_systemically: match s.get("m_bNeverFleeSystemicaly") {
            Some(TVal::B(b)) => *b,
            _ => d.never_flee_systemically,
        },
        flee_attempt_distance: [f("m_fFleeAttemptMinDistance", d.flee_attempt_distance[0]), f("m_fFleeAttemptMaxDistance", d.flee_attempt_distance[1])],
        flee_retry_delay: f("m_fFleeRetryDelay", d.flee_retry_delay),
    }
}
