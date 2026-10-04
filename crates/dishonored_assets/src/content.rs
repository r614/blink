//! Locating and caching retail content from the local installation.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use ue3_package::Package;

pub const DEFAULT_GOG_DIR: &str = r"C:\Program Files\GOG Galaxy\Games\Dishonored";

/// Read-only view of a Dishonored installation.
pub struct Content {
    pub root: PathBuf,
    pub cooked: PathBuf,
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
        Ok(Self {
            root,
            cooked,
            packages: Mutex::new(HashMap::new()),
            tfc: Mutex::new(HashMap::new()),
        })
    }

    pub fn package_path(&self, name: &str) -> PathBuf {
        self.cooked.join(format!("{name}.upk"))
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
            let path = self.cooked.join(format!("{name}.tfc"));
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
