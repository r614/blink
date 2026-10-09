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
        out.vision.bone_nullzone_deg = g("m_fVision_BoneNullzone_Deg", d.bone_nullzone_deg);
        prov.insert("ai.vision".into(), VISION.into());
    }
    let beh = |path: &str| db.try_resolve(path);
    // The player target's vision regions (LOS test points, 0x00C77FC0) and its LOS check period.
    if let Some(m) = beh("Twk_DefaultVisRegion.Twk_DefaultVisRegion") {
        if let Some(TVal::List(items)) = m.get("m_VisRegionSettings") {
            for (i, it) in items.iter().take(3).enumerate() {
                let d = out.vision.vis_regions[i];
                out.vision.vis_regions[i] = [
                    get_f32(it, "m_fWidth").unwrap_or(d[0]),
                    get_f32(it, "m_fOffsetTowardsNPC").unwrap_or(d[1]),
                    get_f32(it, "m_fTopHeight").unwrap_or(d[2]),
                    get_f32(it, "m_fBottomHeight").unwrap_or(d[3]),
                ];
            }
            prov.insert("ai.vision.vis_regions".into(), "Twk_DefaultVisRegion.Twk_DefaultVisRegion :: m_VisRegionSettings".into());
        }
    }
    if let Some(TVal::Struct(c)) = beh("Twk_Component.Twk_Component_Observable_Player").as_ref().and_then(|m| m.get("m_ComponentConfig")) {
        if let Some(p) = get_f32(c, "m_fLOSCheckPeriod") {
            out.vision.los_period = p;
            prov.insert("ai.vision.los_period".into(), "Twk_Component.Twk_Component_Observable_Player :: m_ComponentConfig.m_fLOSCheckPeriod".into());
        }
    }
    // Ambient barks of the patrol / guard behaviours (`DisTweaks_AISubProcess_AmbientBarks`).
    const AMBIENT_ANIMS: &str = "AI_BehaviorTweaks_Patrol.TweaksBehavior_Patrol_Default.DisTweaks_AISubProcess_AmbientAnims_0";
    if let Some(m) = beh(AMBIENT_ANIMS) {
        let d = out.ambient_anim_freq;
        out.ambient_anim_freq = [get_f32(&m, "m_fMinAmbientAnimFrequency").unwrap_or(d[0]), get_f32(&m, "m_fMaxAmbientAnimFrequency").unwrap_or(d[1])];
        prov.insert("ai.ambient_anim_freq".into(), AMBIENT_ANIMS.into());
    }
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
    // `DisBehaviorEscapeExplosion` (slot Preserve): unset values keep the archetype's and class
    // defaults the config defaults hold.
    const ESCAPE: &str = "AI_BhvrTwk_EscapeExplosion.TweakBehaviors_EscapeExplosion_Default";
    if let Some(m) = beh(ESCAPE) {
        let e = &mut out.escape_explosion;
        e.clearance = get_f32(&m, "m_fExplosionClearance").unwrap_or(e.clearance);
        e.discovery_angle_deg = get_f32(&m, "m_fDiscoveryAngle").unwrap_or(e.discovery_angle_deg);
        e.end_timer = get_f32(&m, "m_fEscapeEndTimer").unwrap_or(e.end_timer);
        e.threat_radius = get_f32(&m, "m_fEscapeThreatRadius").unwrap_or(e.threat_radius);
        e.threat_timer = get_f32(&m, "m_fEscapeThreatTimer").unwrap_or(e.threat_timer);
        e.after_explosion = get_f32(&m, "m_fEscapeThreatTimerAfterExplosion").unwrap_or(e.after_explosion);
        e.surprise_chance = get_f32(&m, "m_fSurpriseChance").unwrap_or(e.surprise_chance);
        prov.insert("ai.escape_explosion".into(), ESCAPE.into());
    }
    if let Some(m) = beh(&format!("{ESCAPE}.pDisTweaks_AISubState_Flee")) {
        let f = &mut out.escape_explosion.flee;
        let flag = |k: &str, d: bool| match m.get(k) {
            Some(TVal::B(b)) => *b,
            _ => d,
        };
        f.use_flee_points = flag("m_bUseFleePoints", f.use_flee_points);
        f.use_systemic = flag("m_bUseSystemicFlee", f.use_systemic);
        f.systemic_min = get_f32(&m, "m_fSystemicFleeMinEnvellope").unwrap_or(f.systemic_min);
        f.systemic_max = get_f32(&m, "m_fSystemicFleeMaxEnvellope").unwrap_or(f.systemic_max);
        f.forbidden_angle_deg = get_f32(&m, "m_fForbiddenAngle").unwrap_or(f.forbidden_angle_deg);
        f.forbidden_radius = get_f32(&m, "m_fForbiddenRadius").unwrap_or(f.forbidden_radius);
        prov.insert("ai.escape_explosion.flee".into(), format!("{ESCAPE}.pDisTweaks_AISubState_Flee"));
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
    // The wolfhound's (`DisWolfhoundLocomotionConfig.WolfHoundLocomotionConfig`), cooked into
    // Startup (with the city guard's).
    for c in crate::npc_anim::package_loco_configs(content, &["Startup"]) {
        if !out.locomotion.iter().any(|x| x.name == c.name) {
            out.locomotion.push(c);
        }
    }
    if !out.locomotion.is_empty() {
        prov.insert("ai.locomotion".into(), format!("ArkComponentLocomotionConfig exports of {NPC_ANIM_PACKAGES:?}: {}", out.locomotion.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")));
    }
    // The vision head (stealth.md section 13): the look-at config the head model reads, the
    // HeadTrack request's torso weight, and `camera_jnt` baked per archetype.
    let lookat_ref = ("DishonoredGame".to_string(), "DisLookatConfig.DefaultLookatConfig".to_string());
    match crate::npc_anim::load_lookat_config(content, &lookat_ref) {
        Ok(l) => {
            out.lookat = l;
            prov.insert("ai.lookat".into(), "DishonoredGame DisLookatConfig.DefaultLookatConfig (DisTweaks_NPCPawn.m_pComponentLookatConfig default)".into());
        }
        Err(e) => {
            prov.insert("ai.lookat".into(), format!("missing ({e:#})"));
        }
    }
    let ratio = db.try_resolve(GUARD_UNS).and_then(|m| get_f32(&m, "m_fHeadTrackTorsoRatio"));
    out.head_track_torso_ratio = ratio.unwrap_or(0.0);
    prov.insert("ai.head_track_torso_ratio".into(), match ratio {
        Some(v) => format!("{GUARD_UNS} :: m_fHeadTrackTorsoRatio = {v}"),
        None => "DisTweaks_PawnAttention class default 0 (no shipped attention tweak sets m_fHeadTrackTorsoRatio)".into(),
    });
    // The built-in archetypes' `Attribute_HealthMax` (their party pawn tweaks are cooked into the
    // Boyle script packages).
    {
        use crate::characters::ARCHETYPE_TWEAKS;
        let pkgs: Vec<String> = SPECIAL_NPC_PACKAGES.iter().filter(|p| content.has_package(p)).map(|s| s.to_string()).collect();
        if let Ok(pdb) = TweakDb::load_with(content, &pkgs) {
            let hs: Vec<Option<f32>> = ARCHETYPE_TWEAKS.iter().map(|t| pawn_health(&pdb, t)).collect();
            if hs.iter().all(Option::is_some) {
                out.archetype_health_max = hs.into_iter().flatten().collect();
                prov.insert("ai.archetype_health_max".into(), format!("{ARCHETYPE_TWEAKS:?} :: m_pAttributeTweaks[1].m_HealthMax (Normal) = {:?}", out.archetype_health_max));
            }
        }
    }
    {
        use crate::characters::{ARCHETYPES, ARCHETYPE_TWEAKS, ARISTO_SETS, FEMALE_LOCO, FEMALE_SETS, GUARD_LOCO, GUARD_SETS};
        const DEFAULT_NPC_BODY: &str = "Twk_Pawn_DefaultNPC.Twk_Pawn_DefaultNPC.DisTweaks_NPCPawn_Body_0";
        for (i, (name, parts)) in ARCHETYPES.iter().enumerate() {
            let (sets, loco_name) = match i {
                1 => (GUARD_SETS, GUARD_LOCO),
                3 => (FEMALE_SETS, FEMALE_LOCO),
                _ => (ARISTO_SETS, GUARD_LOCO),
            };
            // `DisTweaks_Pawn_Body.m_fBone_AngleOffset_*` of the archetype's pawn tweak, else
            // the default NPC body tweak (pitch -90).
            let body_path = ARCHETYPE_TWEAKS.get(i).and_then(|t| db.try_resolve(t)).and_then(|m| get_obj(&m, "m_pBodyTweaks").map(|s| s.to_string())).filter(|p| db.try_resolve(p).is_some()).unwrap_or_else(|| DEFAULT_NPC_BODY.to_string());
            let body = db.try_resolve(&body_path).unwrap_or_default();
            let offset = [get_f32(&body, "m_fBone_AngleOffset_Yaw").unwrap_or(0.0), get_f32(&body, "m_fBone_AngleOffset_Pitch").unwrap_or(0.0), get_f32(&body, "m_fBone_AngleOffset_Roll").unwrap_or(0.0)];
            let Some(loco) = out.locomotion.iter().find(|l| l.name.eq_ignore_ascii_case(loco_name)) else {
                prov.insert(format!("ai.head_poses.{i}"), format!("{name}: no {loco_name}"));
                out.head_poses.push(dishonored_ir::ai::HeadPose { archetype: name.to_string(), ..Default::default() });
                continue;
            };
            match parts.first().map(|b| crate::head_pose::bake(content, name, *b, loco, sets, NPC_ANIM_PACKAGES, offset)) {
                Some(Ok(h)) => {
                    prov.insert(format!("ai.head_poses.{i}"), format!("{name}: {} camera_jnt over the {} idle / walk loops of {loco_name}; vision forward = X after {body_path} m_fBone_AngleOffset (yaw, pitch, roll) = {offset:?}", h.mesh, h.loops.len()));
                    out.head_poses.push(h);
                }
                other => {
                    let why = match other {
                        Some(Err(e)) => format!("{e:#}"),
                        _ => "no body part".into(),
                    };
                    prov.insert(format!("ai.head_poses.{i}"), format!("{name}: bake failed ({why})"));
                    out.head_poses.push(dishonored_ir::ai::HeadPose { archetype: name.to_string(), ..Default::default() });
                }
            }
        }
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
    // EscapeExplosion's surprise: its GenericAction plays `DisNPCAnim_Panic` group 0 (0x00AF1600 ->
    // 0x00B2DED0(5, 0)), its steps in order (a looped step plays once: the action ends with it).
    let pkgs: Vec<String> = NPC_ANIM_PACKAGES.iter().map(|s| s.to_string()).collect();
    if let Ok(defs) = crate::npc_anim::load_anim_defs(content, &pkgs, "DisNPCAnimDefinitions_EliteGuard") {
        const SETS: [&str; 2] = ["Npc_EliteGuard_Generic_as.Npc_EliteGuard_Generic_as", GUARD_SWORD_SET];
        let len = |name: &str| -> Option<(String, f32)> {
            [name.to_string(), format!("Generic_{name}"), format!("Sword_{name}")]
                .into_iter()
                .find_map(|n| NPC_ANIM_PACKAGES.iter().find_map(|p| SETS.iter().find_map(|s| crate::npc_anim::sequence_play_length(content, p, s, &n).map(|v| (n.clone(), v)))))
        };
        if let Some(group) = defs.chain(dishonored_ir::npc_anim::NpcAnim::Panic).and_then(|c| c.groups.first()) {
            let steps: Vec<(String, f32)> = group.steps.iter().filter_map(|s| s.names.first().and_then(|n| len(n))).collect();
            if !steps.is_empty() {
                out.escape_explosion.anim_surprise = steps.iter().map(|s| s.1).sum();
                prov.insert("ai.escape_explosion.anim_surprise".into(), format!("DisNPCAnimDefinitions_EliteGuard Panic group 0: {steps:?}"));
            }
        }
    }
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
    // The special NPC classes (research/dishonored/systems/ai.md F6).
    out.special = special_npcs(content, &out, &mut prov);
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
            set("environment", &mut t.environment, "Environment", ENV);
            // Physics props (a movable's contact type) striking the world: every `Env_*` striker.
            let mut strikers: std::collections::BTreeSet<String> = cs.ai_noises().map(|(s, _, _)| s.to_string()).collect();
            strikers.extend(cs.parents.keys().cloned());
            t.prop_contacts.clear();
            for s in strikers.into_iter().filter(|s| s.starts_with("DisContactType_Env_")) {
                if let Some(n) = cs.ai_noise(&s, "DisContactType_Environment") {
                    let r = if n.loudness == "EAINoiseSilent" { Some(0.0) } else { radius(&n.loudness) };
                    if let (Some(r), Some(ctx)) = (r, NoiseContext::from_name(&n.context)) {
                        let noise = AiNoise::new(r, ctx, if n.volume.scale_with_speed { n.volume.min_speed } else { 0.0 });
                        // The types that make the plain `Environment` noise (800 uu, most of them) need no entry.
                        if noise != t.environment {
                            t.prop_contacts.push((s.clone(), noise));
                        }
                        prov.insert(format!("ai.noise.prop_contacts.{s}"), format!("Dis_ContactSystem ({s}, Environment) DisContactSub_AINoise {} {} min speed {} (scaled {})", n.loudness, n.context, n.volume.min_speed, n.volume.scale_with_speed));
                    }
                }
            }
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

/// Level script packages that cook the special NPCs' tweaks: the Boyle street (tallboys,
/// weepers), the Boyle interior (music-box Overseers) and the Overseer kennels (wolfhounds).
pub const SPECIAL_NPC_PACKAGES: &[&str] = &["L_Boyle_Ext_Script", "L_Boyle_Int_Script", "L_BoyleStreet_Ext_Script", "L_Ovrsr_Kennel_Script", "L_Ovrsr_Script", "L_Ovrsr_Back_Script"];

fn ranged(m: &TMap, k: &str, d: [f32; 2]) -> [f32; 2] {
    match m.get(k) {
        Some(TVal::Struct(s)) => [get_f32(s, "m_fMinValue").unwrap_or(d[0]), get_f32(s, "m_fMaxValue").unwrap_or(d[1])],
        _ => d,
    }
}

fn int_ranged(m: &TMap, k: &str, d: [i32; 2]) -> [i32; 2] {
    match m.get(k) {
        Some(TVal::Struct(s)) => {
            let g = |n: &str, d: i32| match s.get(n) {
                Some(TVal::I(v)) => *v,
                _ => d,
            };
            [g("m_MinValue", d[0]), g("m_MaxValue", d[1])]
        }
        _ => d,
    }
}

/// An attack item context (`DisTweaks_ItemContext` / `DisTweaks_MeleeAttack`) with the
/// weapon's `Attribute_MeleeDamage` (Normal): one hit deals `ceil(multiplier x damage)`.
fn special_attack(db: &TweakDb, path: &str, kind: u8, inhibition: f32, melee_damage: f32) -> Option<dishonored_ir::ai::SpecialAttack> {
    let m = db.try_resolve(path)?;
    let name = get_obj(&m, "m_pSpawnedObjectClass").map(|c| c.rsplit('.').next().unwrap_or(c).to_string()).unwrap_or_default();
    let h = get_f32(&m, "m_fMaxContextHeightDifference").unwrap_or(-1.0);
    let mult = get_f32(&m, "m_fAttackDamageMultiplier").unwrap_or(1.0);
    Some(dishonored_ir::ai::SpecialAttack {
        name,
        kind,
        min_range: get_f32(&m, "m_fMinContextRange").unwrap_or(0.0),
        max_range: get_f32(&m, "m_fMaxContextRange").unwrap_or(f32::MAX),
        ideal_range: get_f32(&m, "m_fIdealContextRange").unwrap_or(0.0),
        max_height: if h < 0.0 { f32::MAX } else { h },
        angle: get_f32(&m, "m_fAllowedAngle").unwrap_or(180.0),
        cooldown: ranged(&m, "m_ContextCooldown", [0.0, 0.0]),
        inhibition,
        damage: (mult * melee_damage).ceil(),
        unblockable: matches!(m.get("m_MainAttack"), Some(TVal::Struct(s)) if matches!(s.get("m_bUnblockable"), Some(TVal::B(true)))),
    })
}

fn manage_attacks(db: &TweakDb, behaviour: &str, d: &dishonored_ir::ai::ManageAttacksConfig) -> dishonored_ir::ai::ManageAttacksConfig {
    let Some(b) = db.try_resolve(behaviour) else { return d.clone() };
    let subs: Vec<String> = match b.get("m_SubProcessTweaks") {
        Some(TVal::List(l)) => l.iter().filter_map(|e| get_obj(e, "obj").map(str::to_string)).collect(),
        _ => Vec::new(),
    };
    for p in subs {
        let Some(s) = db.try_resolve(&p) else { continue };
        if !s.contains_key("m_AttacksPerFlurry") && !s.contains_key("m_StaminaRecoveryDuration") {
            continue;
        }
        return dishonored_ir::ai::ManageAttacksConfig {
            attacks_per_flurry: int_ranged(&s, "m_AttacksPerFlurry", d.attacks_per_flurry),
            stamina_recovery: ranged(&s, "m_StaminaRecoveryDuration", d.stamina_recovery),
            aggression_regen_rate: get_f32(&s, "m_fAggressionRegenRate").unwrap_or(d.aggression_regen_rate),
            aggression_delta_per_damage: get_f32(&s, "m_fAggressionDeltaPerDamage").unwrap_or(d.aggression_delta_per_damage),
            cooldown_factor_at_min_aggression: get_f32(&s, "m_fCooldownFactorAtMinAggression").unwrap_or(d.cooldown_factor_at_min_aggression),
        };
    }
    d.clone()
}

/// `Attribute_HealthMax` (Normal) of a pawn tweak (`m_pAttributeTweaks[1]`).
fn pawn_health(db: &TweakDb, pawn: &str) -> Option<f32> {
    let m = db.try_resolve(pawn)?;
    let a = db.try_resolve(get_obj(&m, "m_pAttributeTweaks[1]")?)?;
    get_f32(&a, "m_HealthMax.m_fBaseValue2_Normal")
}

/// `Attribute_MeleeDamage` (Normal) of an item tweak (`m_pAttributeTweak[1]`).
fn item_melee_damage(db: &TweakDb, item: &str) -> Option<f32> {
    let m = db.try_resolve(item)?;
    let a = db.try_resolve(get_obj(&m, "m_pAttributeTweak[1]")?)?;
    get_f32(&a, "m_MeleeDamage.m_fBaseValue2_Normal")
}

/// The special NPC classes' tuning from their pawn, brain, behaviour and weapon tweaks.
fn special_npcs(content: &Content, base: &AiConfig, prov: &mut std::collections::BTreeMap<String, String>) -> dishonored_ir::ai::SpecialNpcConfig {
    use dishonored_ir::ai::{SpecialNpcConfig, WeeperConfig};
    use dishonored_ir::ai::special_moves as mv;
    let mut out = SpecialNpcConfig::default();
    let pkgs: Vec<String> = SPECIAL_NPC_PACKAGES.iter().filter(|p| content.has_package(p)).map(|s| s.to_string()).collect();
    let Ok(db) = TweakDb::load_with(content, &pkgs) else {
        prov.insert("ai.special".into(), "defaults (tweak database failed)".into());
        return out;
    };
    let ini = Ini::load_layered(&content.root, "AI");
    let inhibit = |k: &str| ini.get_f32("DishonoredGame.DisGlobalCombatManager", k).unwrap_or(0.0);
    // Wolfhound: `Pwn_WolfHound_0` (the base tweak is a fallback-only template).
    const HOUND: &str = "Twk_Pawn_WolfHound.Pwn_WolfHound_0";
    const BITE: &str = "Twk_Inv_Wolfhound.Twk_Inv_WolfhoundBite";
    const HOUND_COMBAT: &str = "AI_BehaviorTweaks_WolfHound.AI_Tweak_Behavior_WolfhoundCombat";
    let h = &mut out.wolfhound;
    h.health_max = pawn_health(&db, HOUND).unwrap_or(20.0);
    let bite = item_melee_damage(&db, BITE).unwrap_or(15.0);
    let ctx = |n: &str| format!("{BITE}.pNPCSpecific.{n}");
    // The bite's contexts the port plays, in the item's slot order: the medium bite and the jump
    // attack. Not ported: the arm attack (a sync grab: 5 damage per 0.5 s tick until the player
    // escapes, which needs the sync-attack lock), the 90-degree turn bites, the attack from below
    // and the rat chomp.
    for (n, kind, inh) in [("pNPCAttackMedium", mv::WH_BITE, inhibit("m_fInhibitionTimeMediumAttack")), ("DisTweaks_WHJumpAttack_0", mv::WH_JUMP_ATTACK, inhibit("m_fInhibitionTimeWHJumpAttack"))] {
        if let Some(mut a) = special_attack(&db, &ctx(n), kind, inh, bite) {
            if kind == mv::WH_JUMP_ATTACK {
                // `m_DamageOnKnockDown[1]` (Normal): the knockdown's damage.
                a.damage = db.try_resolve(&ctx(n)).and_then(|m| match m.get("m_DamageOnKnockDown[1]") {
                    Some(TVal::I(v)) => Some(*v as f32),
                    _ => None,
                }).unwrap_or(0.0);
            }
            h.attacks.push(a);
        }
    }
    h.manage = manage_attacks(&db, HOUND_COMBAT, &h.manage);
    h.walk_distance = db.try_resolve(&format!("{HOUND_COMBAT}.DisTweaks_AISubState_WH_CombatShortDistance_0")).and_then(|m| get_f32(&m, "m_fWalkDistance")).unwrap_or(300.0);
    let attn = |p: &str, d: &AttentionConfig| db.try_resolve(p).map(|m| attention(&m, d));
    h.unsuspecting = attn("Twk_Attn_Wolfhound.Twk_Attn_Wolfhound_Unsuspecting", &base.guard_unsuspecting);
    h.suspecting = attn("Twk_Attn_Wolfhound.Twk_Attn_Wolfhound_Suspecting", &base.guard_suspecting);
    prov.insert("ai.special.wolfhound".into(), format!("{HOUND} (health), {BITE} (contexts, MeleeDamage), {HOUND_COMBAT} (ManageAttacks, WH_CombatShortDistance m_fWalkDistance), Twk_Attn_Wolfhound_*; packages {pkgs:?}"));
    // Weepers: `Pwn_WeeperMiddle_MTall_1` (the three Boyle weeper tweaks share these values).
    const WEEPER: &str = "Pwn_WeeperMiddle_MTall_1.Pwn_WeeperMiddle_MTall_1";
    const WEEP: &str = "AI_BehaviorTweaks_Weep.TweakBehaviors_Weep_Default";
    const GRAB: &str = "Twk_EmptyHands_NPC.Twk_EmptyHands_NPC_Weeper.DisTweaks_InventoryItem_PawnSpecific_0.DisTweaks_WeeperGrab_2";
    let d = WeeperConfig::default();
    let w = &mut out.weeper;
    if let Some(p) = db.try_resolve(WEEPER) {
        w.plague_radius = get_f32(&p, "m_fPlagueRadius").unwrap_or(d.plague_radius);
        w.plague_angle = get_f32(&p, "m_fPlagueDmgAngle").unwrap_or(d.plague_angle);
        w.plague_period = get_f32(&p, "m_fPlagueDmgePeriod").unwrap_or(d.plague_period);
        w.plague_damage = match p.get("m_iPlagueDmgPerTick") {
            Some(TVal::I(v)) => *v as f32,
            _ => d.plague_damage,
        };
        w.plague_length = ranged(&p, "m_fPlagueDmgLength", d.plague_length);
    }
    w.health_max = pawn_health(&db, WEEPER).unwrap_or(d.health_max);
    if let Some(b) = db.try_resolve(WEEP) {
        w.home_radius = get_f32(&b, "m_fHomeRadius").unwrap_or(d.home_radius);
        w.lost_timer = [get_f32(&b, "m_fLostTimer_Min").unwrap_or(d.lost_timer[0]), get_f32(&b, "m_fLostTimer_Max").unwrap_or(d.lost_timer[1])];
        w.coughing_cooldown = ranged(&b, "m_fCoughingCooldown", d.coughing_cooldown);
        w.runners_allowed = match b.get("m_iSimultaneousRunnerAllowed") {
            Some(TVal::I(v)) => (*v).max(0) as u32,
            _ => d.runners_allowed,
        };
        w.time_before_running = ranged(&b, "m_TimeBeforeRunning", [d.time_before_running; 2])[0];
        w.stand_still = [get_f32(&b, "m_fStandStillWeeping_Min").unwrap_or(d.stand_still[0]), get_f32(&b, "m_fStandStillWeeping_Max").unwrap_or(d.stand_still[1])];
    }
    if let Some(g) = db.try_resolve(GRAB) {
        w.grab_range = get_f32(&g, "m_fMaxContextRange").unwrap_or(d.grab_range);
        w.grab_height = get_f32(&g, "m_fMaxContextHeightDifference").unwrap_or(d.grab_height);
        w.grab_angle = get_f32(&g, "m_fAllowedAngle").unwrap_or(d.grab_angle);
    }
    w.grab_inhibition = ini.get_f32("DishonoredGame.DisGlobalCombatManager", "m_fInhibitionTimeWeeperArmGrab").unwrap_or(d.grab_inhibition);
    prov.insert("ai.special.weeper".into(), format!("{WEEPER} (plague, health), {WEEP}, {GRAB}, DefaultAI.ini m_fInhibitionTimeWeeperArmGrab"));
    special_tallboy(&db, &Ini::load_layered(&content.root, "NPC"), &base.loudness_distances, &mut out, prov);
    special_overseer(&db, &mut out, prov);
    out
}

/// Tallboys: `Twk_Pawn_Tallboy.Pwn_Tallboy_Wood`, its combat behaviour and bow.
fn special_tallboy(db: &TweakDb, npc_ini: &Ini, loudness: &[f32], out: &mut dishonored_ir::ai::SpecialNpcConfig, prov: &mut std::collections::BTreeMap<String, String>) {
    use dishonored_ir::ai::special_moves as mv;
    const PAWN: &str = "Twk_Pawn_Tallboy.Pwn_Tallboy_Wood";
    const BOW: &str = "Twk_Inv_Tallboy.TallboyBowWithMelee";
    const COMBAT: &str = "AI_BehaviorTweaks_TallBoy.AI_BehaviorTweaks_TallBoy_Combat";
    const ARROW: &str = "Twk_Inv_Tallboy.TallboyExplosiveArrow_twk";
    let tb = &mut out.tallboy;
    tb.health_max = pawn_health(db, PAWN).unwrap_or(150.0);
    let dmg = item_melee_damage(db, BOW).unwrap_or(15.0);
    let ctx = |n: &str| format!("{BOW}.pTallboyBowRangedNPCSpecific.{n}");
    if let Some(a) = special_attack(db, &ctx("DisTweaks_AttackNPCTallboyMelee_1"), mv::TALLBOY_STOMP, 0.0, dmg) {
        tb.attacks = vec![a];
    }
    let fire = ctx("DisTweaks_NPCFireBow_1");
    if let Some(a) = special_attack(db, &fire, mv::TALLBOY_FIRE_BOW, 0.0, 0.0) {
        tb.bow = a;
    }
    if let Some(m) = db.try_resolve(&fire) {
        tb.arrow_speed = get_f32(&m, "m_fProjectileSpeed").unwrap_or(2000.0);
        tb.allies_hold_fire_radius = get_f32(&m, "m_fAlliesHoldFireRadius").unwrap_or(0.0);
        tb.miss_angle = get_f32(&m, "m_fCheatAngleDeviation").unwrap_or(30.0);
    }
    tb.manage = manage_attacks(db, COMBAT, &tb.manage);
    if let Some(s) = db.try_resolve(&format!("{COMBAT}.DisTweaks_AISubState_FindShootingPosition_0")) {
        tb.fsp_avoidance = get_f32(&s, "m_fAvoidanceRadius").unwrap_or(250.0);
        tb.fsp_give_up = get_f32(&s, "m_fTimeBeforeGivingUp").unwrap_or(3.0);
    }
    if let Some(a) = db.try_resolve(PAWN).and_then(|m| get_obj(&m, "m_pAttributeTweaks[1]").map(str::to_string)).and_then(|p| db.try_resolve(&p)) {
        tb.accuracy = [get_f32(&a, "m_MaxAccuracy.m_fBaseValue2_Normal").unwrap_or(1.0), get_f32(&a, "m_MinAccuracy.m_fBaseValue2_Normal").unwrap_or(0.1)];
    }
    let arrow = db.try_resolve(ARROW);
    tb.arrow_gravity_mult = arrow.as_ref().and_then(|m| get_f32(m, "m_fGravityMultiplier")).unwrap_or(0.5);
    tb.arrow_speed *= arrow.as_ref().and_then(|m| get_f32(m, "m_fSpeedMultiplier")).unwrap_or(1.0);
    if let Some(e) = arrow.as_ref().and_then(|m| get_obj(m, "m_pExplosionTweak").map(str::to_string)).and_then(|p| db.try_resolve(&p)) {
        let d = dishonored_ir::items::ExplosionConfig::default();
        let x = &mut tb.explosion;
        x.radial_damage = get_f32(&e, "m_RadialDamage.m_fBaseValue2_Normal").unwrap_or(d.radial_damage);
        x.damage_radius = get_f32(&e, "m_fDamageRadius").unwrap_or(d.damage_radius);
        x.full_damage_radius = get_f32(&e, "m_fFullDamageRadius").unwrap_or(d.full_damage_radius);
        x.player_damage_radius = get_f32(&e, "m_fPlayerDamageRadius").unwrap_or(d.player_damage_radius);
        x.player_full_damage_radius = get_f32(&e, "m_fPlayerFullDamageRadius").unwrap_or(d.player_full_damage_radius);
        x.radial_momentum = get_f32(&e, "m_fRadialMomentum").unwrap_or(d.radial_momentum);
        x.extra_up_force = get_f32(&e, "m_fExtraUpForce").unwrap_or(d.extra_up_force);
        x.attached_damage = 0.0;
        x.ai_noise_radius = match e.get("m_AINoiseLoudness") {
            Some(TVal::Name(n)) => crate::ai_level::LOUDNESS_NAMES.iter().position(|l| l == n).and_then(|i| loudness.get(i).copied()).unwrap_or(d.ai_noise_radius),
            _ => d.ai_noise_radius,
        };
    }
    let npc = |k: &str, d: f32| -> f32 { npc_ini.get_f32("DishonoredGame.StateNPCMasterTallboyAim", k).unwrap_or(d) };
    tb.aim_align_deg = npc("m_fAimAlignYawDegrees", 20.0);
    tb.aim_time = npc("m_fAimAlignTimeBeforeShooting", 0.25);
    prov.insert("ai.special.tallboy".into(), format!("{PAWN} (health, accuracy), {BOW} (MeleeDamage, Fire Bow and Leg Attack contexts), {COMBAT} (ManageAttacks, FindShootingPosition), {ARROW} and its m_pExplosionTweak; StateNPCMasterTallboyAim m_fAimAlign*"));
}

/// Music-box Overseers: `Twk_Pawn_Boyle_Party.Pwn_MusicOverseer_Party`, the music box.
fn special_overseer(db: &TweakDb, out: &mut dishonored_ir::ai::SpecialNpcConfig, prov: &mut std::collections::BTreeMap<String, String>) {
    use dishonored_ir::ai::special_moves as mv;
    const PAWN: &str = "Twk_Pawn_Boyle_Party.Pwn_MusicOverseer_Party";
    const BOX: &str = "Twk_Inv_NPC_Overseer.Musical.Twk_Weapon_MusicBox_Default";
    let o = &mut out.overseer;
    o.health_max = pawn_health(db, PAWN).unwrap_or(40.0);
    let dmg = item_melee_damage(db, BOX).unwrap_or(10.0);
    let ctx = |n: &str| format!("{BOX}.pNPCSpecific.{n}");
    if let Some(a) = special_attack(db, &ctx("DisTweaks_NPCBash_8"), mv::OVERSEER_BASH, 0.0, dmg) {
        o.attacks = vec![a];
    }
    let combat = ctx("DisTweaks_NPCTune_Combat_0");
    if let Some(t) = special_attack(db, &combat, mv::OVERSEER_TUNE, 0.0, 0.0) {
        o.tune = t;
    }
    if let Some(m) = db.try_resolve(&combat) {
        o.tune_damage = match m.get("m_iMusicEffectDamage") {
            Some(TVal::I(v)) => *v as f32,
            _ => 0.0,
        };
        o.stop_not_seeing = get_f32(&m, "m_fStopWhenNotSeeingTimer").unwrap_or(5.0);
        o.stop_out_of_range = get_f32(&m, "m_fStopWhenOutOfRangeTimer").unwrap_or(1.0);
        o.repulse = ranged(&m, "m_fRepulseForceRange", [1500.0, 6000.0]);
        o.tune_period = get_obj(&m, "m_pTuneTweak").and_then(|t| db.try_resolve(t)).and_then(|t| get_f32(&t, "m_fMusicEffectPeriod")).unwrap_or(0.5);
    }
    let prot = ctx("DisTweaks_NPCTune_Protection_0");
    if let Some(m) = db.try_resolve(&prot) {
        o.ideal_distance = get_f32(&m, "m_fIdealContextRange").unwrap_or(1000.0);
        o.inhibit_powers_radius = get_obj(&m, "m_pTuneTweak").and_then(|t| db.try_resolve(t)).and_then(|t| get_f32(&t, "m_fMusicEffectRadius_InhibitPowers")).unwrap_or(1500.0);
    }
    o.player_music_reduction = db.try_resolve("Twk_Pawn_Corvo.Twk_Pawn_Corvo_Release.pAttributeTweaksNormal").and_then(|m| get_f32(&m, "m_MusicDamageResilience.m_fBaseValue2_Normal")).unwrap_or(0.0);
    prov.insert("ai.special.overseer".into(), format!("{PAWN} (health), {BOX} (MeleeDamage; Bash, Combat Tune and its DisTweaks_Tune, Protection Tune and its DisTweaks_Tune_Protection)"));
}