//! UE3 localization files (`DishonoredGame/Localization/<LANG>/*.int`), read at runtime.
//!
//! Format: INI files named after a package; sections `[ObjectPath:SubobjectName Class]` or
//! `[ObjectPath Class]`; values `key="text"`. Example (Boyle_Twk.int):
//! `[Door_Transitions.LTD_Garden_to_Ballroom:DisTweaks_InteractableInterface_16 DisTweaks_InteractableInterface]`
//! with `m_InteractText="Press `k to go to ..."` (`` `k `` stands for the bound key).

use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Default, Clone)]
pub struct Localization {
    /// (package lowercase, section lowercase) -> key lowercase -> value
    entries: HashMap<(String, String), HashMap<String, String>>,
}

impl Localization {
    /// Load every `.int` of a language directory (e.g. `Localization/INT`).
    pub fn load_dir(dir: &Path) -> Localization {
        let mut loc = Localization::default();
        let Ok(rd) = std::fs::read_dir(dir) else { return loc };
        for e in rd.flatten() {
            let p = e.path();
            if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("int")) {
                continue;
            }
            let pkg = p.file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            let Ok(bytes) = std::fs::read(&p) else { continue };
            loc.parse(&pkg, &decode(&bytes));
        }
        loc
    }

    pub fn load(content: &crate::content::Content, lang: &str) -> Localization {
        let dir = content.cooked.parent().map(|d| d.join("Localization").join(lang)).unwrap_or_default();
        Self::load_dir(&dir)
    }

    fn parse(&mut self, pkg: &str, text: &str) {
        let mut section = String::new();
        for line in text.lines() {
            let l = line.trim();
            if l.starts_with('[') && l.ends_with(']') {
                section = l[1..l.len() - 1].to_ascii_lowercase();
                continue;
            }
            let Some((k, v)) = l.split_once('=') else { continue };
            let v = v.trim();
            let v = v.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(v);
            self.entries.entry((pkg.to_string(), section.clone())).or_default().insert(k.trim().to_ascii_lowercase(), v.to_string());
        }
    }

    /// Value for an object path (`Package.Group.Object`) and key, looking in the object's own
    /// section and its subobject sections (`Group.Object:Sub Class`).
    pub fn get(&self, object_path: &str, key: &str) -> Option<&str> {
        let (pkg, rest) = object_path.split_once('.')?;
        let pkg = pkg.to_ascii_lowercase();
        let rest = rest.to_ascii_lowercase();
        let key = key.to_ascii_lowercase();
        let mut best: Option<&str> = None;
        for ((p, s), kv) in &self.entries {
            if *p != pkg {
                continue;
            }
            let head = s.split([' ', ':']).next().unwrap_or("");
            if head == rest {
                if let Some(v) = kv.get(&key) {
                    if !v.is_empty() {
                        best = Some(v.as_str());
                    }
                }
            }
        }
        best
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// `.int` files are UTF-16LE with BOM or plain text.
fn decode(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        let u: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&u)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// Replace UE3 key placeholders (`` `k `` = the use key) for display.
pub fn with_keys(text: &str, use_key: &str) -> String {
    text.replace("`k", use_key)
}
