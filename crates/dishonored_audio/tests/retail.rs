//! Regression tests against the canonical GOG build (Dishonored.exe SHA-256 70da9613…c6fb).
//! Skipped automatically when the install is not present. Nothing is written to disk.

use dishonored_audio::library::SoundLibrary;
use dishonored_audio::vorbis::CodebookLibrary;
use std::path::PathBuf;

fn install() -> Option<PathBuf> {
    let root = std::env::var_os("DISHONORED_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files\GOG Galaxy\Games\Dishonored"));
    root.join("Binaries/Win32/Dishonored.exe").is_file().then_some(root)
}

#[test]
fn event_ids_are_fnv1_of_lowercase_names() {
    // Ids observed in the retail banks.
    assert_eq!(dishonored_audio::fnv1_32("Play_Amb_LVL_Boyle_Estate"), 415855026);
    assert_eq!(dishonored_audio::fnv1_32("Snd_Sword_Corvo_Out"), 795089353);
    assert_eq!(dishonored_audio::fnv1_32("snd_sword_corvo_out"), 795089353);
}

#[test]
fn codebooks_come_from_the_executable() {
    let Some(root) = install() else {
        eprintln!("Dishonored install not found; skipping");
        return;
    };
    let lib = CodebookLibrary::from_install(&root).expect("codebook table");
    assert_eq!(lib.len(), 598);
}

#[test]
fn sword_event_resolves_and_decodes() {
    let Some(root) = install() else {
        eprintln!("Dishonored install not found; skipping");
        return;
    };
    let mut lib = SoundLibrary::new(&root).unwrap();
    lib.load_bank("Bank_Weapon_Sword_Corvo").unwrap();
    let cue = lib.resolve("Snd_Sword_Corvo_Out").expect("event resolves");
    let mut leaves = Vec::new();
    cue.leaves(&mut leaves);
    assert_eq!(leaves.len(), 1);
    assert_eq!(leaves[0].media.id, 645675558);
    assert!((leaves[0].volume_db + 5.0).abs() < 1e-3);
    assert_eq!(leaves[0].max_distance, Some(3000.0));
    let pcm = lib.decode(leaves[0].media).unwrap();
    assert_eq!((pcm.channels, pcm.sample_rate), (2, 44100));
    assert!(pcm.peak() > 0.1 && pcm.samples.iter().all(|s| s.is_finite()));
    assert!((pcm.duration() - 0.87).abs() < 0.02, "duration {}", pcm.duration());
}

#[test]
fn boyle_ambience_follows_the_level_state() {
    let Some(root) = install() else {
        eprintln!("Dishonored install not found; skipping");
        return;
    };
    let mut lib = SoundLibrary::new(&root).unwrap();
    lib.load_bank("Bank_Amb_LVL_Boyle_Estate").unwrap();
    let default_leaves = {
        let mut v = Vec::new();
        lib.resolve("Play_Amb_LVL_Boyle_Estate").unwrap().leaves(&mut v);
        v.iter().map(|l| l.media.id).collect::<Vec<_>>()
    };
    assert!(lib.apply_state_event("Set_State_Amb_LVL_Boyle_Ext_Streets"));
    let mut v = Vec::new();
    lib.resolve("Play_Amb_LVL_Boyle_Estate").unwrap().leaves(&mut v);
    let streets: Vec<u32> = v.iter().map(|l| l.media.id).collect();
    assert_ne!(default_leaves, streets, "state must select another switch branch");
    assert!(v.iter().all(|l| l.media.streamed));
}
