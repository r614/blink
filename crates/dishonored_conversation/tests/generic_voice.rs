//! Generic per-voice bark trees (retail data; skipped without the install).

use dishonored_conversation::load::{NoVo, load_packages};
use dishonored_conversation::runner::{HookRequest, Runner, SimpleEnv};
use std::sync::Arc;

fn cooked() -> Option<std::path::PathBuf> {
    let p = std::path::PathBuf::from(r"C:\Program Files\GOG Galaxy\Games\Dishonored\DishonoredGame\CookedPCConsole");
    p.exists().then_some(p)
}

#[test]
fn elite_guard_voice_barks() {
    let Some(c) = cooked() else { return };
    let db = Arc::new(load_packages(&c, &["L_Boyle_Ext_Script".to_string()], &mut NoVo).unwrap());
    // `Default__DisTweaks_ConvSettings` read from DishonoredGame.upk equals the cooked values the
    // fallback carries.
    assert_eq!(db.settings, dishonored_conversation::hooks::ConvSettings::default());
    let trees = db.voice_trees.get("Dlg_EliteGuard.VoiceData.Voice_EliteGuard_A").cloned().expect("voice trees");
    let mut started = Vec::new();
    for hook in ["DDH_COMBAT_OUCH_SMALL", "DDH_COMBAT_THREAT", "DDH_SEARCH", "DDH_NOTICE_SUSPICIOUS_ACTIVITY"] {
        let mut r = Runner::new(db.clone(), 1);
        let mut env = SimpleEnv::default();
        let res = r.fire_hook(HookRequest { npc: "guard".into(), trees: trees.clone(), hook: hook.into(), initiator: Some(1), ..Default::default() }, &mut env);
        let ev: Vec<String> = r.take_events().iter().map(|e| format!("{e:?}")).collect();
        eprintln!("{hook}: {res:?} {ev:?}");
        if res.is_ok() {
            started.push(hook);
        }
    }
    assert!(!started.is_empty(), "no generic bark starts");
}

