//! Finding the Dishonored install at startup. Order: `--game-dir DIR`, `DISHONORED_DIR`, the folder
//! saved on an earlier launch, the default GOG path; when none is an install, a folder picker asks
//! for it and the choice is saved for next time. The result is exported as `DISHONORED_DIR`, which
//! every content loader (and a game the launcher starts) reads.

use dishonored_assets::content::{find_install, install_root};
use std::path::PathBuf;

/// Resolve the install before anything else runs (single-threaded start of `main`). Exits the
/// process when no install is found and the player cancels the picker.
pub fn ensure() {
    let args: Vec<String> = std::env::args().collect();
    let flag = args.iter().position(|a| a == "--game-dir").and_then(|i| args.get(i + 1)).map(PathBuf::from);
    if let Some(dir) = flag {
        match install_root(&dir) {
            Some(root) => return export(root, true),
            None => fail(&format!("--game-dir {}: no DishonoredGame\\CookedPCConsole there or above.", dir.display())),
        }
    }
    if let Some(root) = find_install(dishonored_settings::saved_game_dir().as_deref()) {
        return export(root, false);
    }
    loop {
        let picked = rfd::FileDialog::new().set_title("Blink: select your Dishonored install folder").pick_folder();
        let Some(dir) = picked else {
            fail("Blink needs your own copy of Dishonored: Definitive Edition. Run it again and select the folder the game is installed in.");
        };
        if let Some(root) = install_root(&dir) {
            return export(root, true);
        }
        let retry = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Blink")
            .set_description(format!("{} is not a Dishonored install (it has no DishonoredGame\\CookedPCConsole folder).\n\nSelect the folder Dishonored is installed in, e.g. C:\\Program Files\\GOG Galaxy\\Games\\Dishonored.", dir.display()))
            .set_buttons(rfd::MessageButtons::OkCancel)
            .show();
        if retry != rfd::MessageDialogResult::Ok {
            fail("No Dishonored install selected.");
        }
    }
}

fn export(root: PathBuf, save: bool) {
    if save {
        if let Err(e) = dishonored_settings::save_game_dir(&root) {
            eprintln!("saving the game folder: {e:#}");
        }
    }
    // SAFETY: called first thing in `main`, before any other thread exists.
    unsafe { std::env::set_var("DISHONORED_DIR", &root) };
}

fn fail(msg: &str) -> ! {
    eprintln!("{msg}");
    rfd::MessageDialog::new().set_level(rfd::MessageLevel::Error).set_title("Blink").set_description(msg).show();
    std::process::exit(1);
}
