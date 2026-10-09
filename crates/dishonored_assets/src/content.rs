//! Locating and caching retail content from the local installation.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use ue3_package::Package;

pub const DEFAULT_GOG_DIR: &str = r"C:\Program Files\GOG Galaxy\Games\Dishonored";

/// The install root at or above `path` (the folder holding `DishonoredGame/CookedPCConsole`), so
/// picking the install folder or any folder inside it works.
pub fn install_root(path: &Path) -> Option<PathBuf> {
    path.ancestors().find(|a| a.join("DishonoredGame").join("CookedPCConsole").is_dir()).map(Path::to_path_buf)
}

/// The install to use: `DISHONORED_DIR` if it is one, else `saved` (the folder chosen on first
/// launch), else the default GOG path. `None` when none of them is an install.
pub fn find_install(saved: Option<&Path>) -> Option<PathBuf> {
    let env = std::env::var_os("DISHONORED_DIR").map(PathBuf::from);
    env.iter().map(PathBuf::as_path).chain(saved).chain([Path::new(DEFAULT_GOG_DIR)]).find_map(install_root)
}

/// Read-only view of a Dishonored installation.
pub struct Content {
    pub root: PathBuf,
    pub cooked: PathBuf,
    /// The DLC folders (`DishonoredGame/DLC/PCConsole/DLC05..07`): their packages and texture
    /// caches are found there when `CookedPCConsole` has none of the name.
    pub dlc: Vec<PathBuf>,
    packages: Mutex<HashMap<String, Arc<Package>>>,
    tfc: Mutex<HashMap<String, File>>,
}

impl Content {
    /// Resolve the install from `DISHONORED_DIR`, else the default GOG path.
    pub fn locate() -> Result<Self> {
        let root = std::env::var_os("DISHONORED_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_GOG_DIR));
        Self::open(root)
    }

    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        let cooked = root.join("DishonoredGame").join("CookedPCConsole");
        anyhow::ensure!(
            cooked.is_dir(),
            "{} does not look like a Dishonored install (no DishonoredGame/CookedPCConsole)",
            root.display()
        );
        let mut dlc: Vec<PathBuf> = std::fs::read_dir(root.join("DishonoredGame").join("DLC").join("PCConsole"))
            .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
            .unwrap_or_default();
        dlc.sort();
        Ok(Self {
            root,
            cooked,
            dlc,
            packages: Mutex::new(HashMap::new()),
            tfc: Mutex::new(HashMap::new()),
        })
    }

    /// A package's file: `CookedPCConsole` first, then the DLC folders (the cooked one when
    /// neither has it, for the error message).
    pub fn package_path(&self, name: &str) -> PathBuf {
        self.find_file(&format!("{name}.upk"))
    }

    fn find_file(&self, file: &str) -> PathBuf {
        let cooked = self.cooked.join(file);
        if cooked.is_file() {
            return cooked;
        }
        self.dlc.iter().map(|d| d.join(file)).find(|p| p.is_file()).unwrap_or(cooked)
    }

    pub fn has_package(&self, name: &str) -> bool {
        self.package_path(name).is_file()
    }

    /// Open (and cache) a package by name without extension.
    pub fn package(&self, name: &str) -> Result<Arc<Package>> {
        let key = name.to_ascii_lowercase();
        if let Some(p) = self.packages.lock().unwrap().get(&key) {
            return Ok(p.clone());
        }
        let pkg = Arc::new(Package::open(self.package_path(name))?);
        self.packages.lock().unwrap().insert(key, pkg.clone());
        Ok(pkg)
    }

    pub fn read_tfc(&self, name: &str, offset: u64, len: usize) -> Result<Vec<u8>> {
        let mut map = self.tfc.lock().unwrap();
        let key = name.to_ascii_lowercase();
        if !map.contains_key(&key) {
            let path = self.find_file(&format!("{name}.tfc"));
            let f = File::open(&path).with_context(|| format!("opening {}", path.display()))?;
            map.insert(key.clone(), f);
        }
        let f = map.get_mut(&key).unwrap();
        f.seek(SeekFrom::Start(offset))?;
        let mut buf = vec![0u8; len];
        f.read_exact(&mut buf)
            .with_context(|| format!("reading {len} bytes at {offset:#x} from {name}.tfc"))?;
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_root_accepts_the_install_or_a_folder_inside_it() {
        let root = std::env::temp_dir().join(format!("blink_install_root_{}", std::process::id()));
        let cooked = root.join("DishonoredGame").join("CookedPCConsole");
        std::fs::create_dir_all(&cooked).unwrap();
        std::fs::create_dir_all(root.join("Binaries").join("Win32")).unwrap();
        assert_eq!(install_root(&root).as_deref(), Some(root.as_path()));
        assert_eq!(install_root(&cooked).as_deref(), Some(root.as_path()));
        assert_eq!(install_root(&root.join("Binaries").join("Win32")).as_deref(), Some(root.as_path()));
        assert_eq!(install_root(&std::env::temp_dir().join("blink_not_an_install")), None);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
