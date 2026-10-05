//! UE3 localization files (`DishonoredGame/Localization/<LANG>/<name>.int`): INI syntax,
//! usually UTF-16LE with a BOM, values optionally quoted. Read at runtime only; the text is
//! retail data and never compiled into the port.

use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Default, Clone)]
pub struct Localization {
    /// (section, key) -> text, first definition wins (later duplicates are DLC repeats).
    pub entries: HashMap<(String, String), String>,
}

/// Decode a localization file's bytes (UTF-16LE/BE with BOM, UTF-8 with or without BOM).
pub fn decode(bytes: &[u8]) -> String {
    match bytes {
        [0xFF, 0xFE, rest @ ..] => String::from_utf16_lossy(&rest.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>()),
        [0xFE, 0xFF, rest @ ..] => String::from_utf16_lossy(&rest.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect::<Vec<_>>()),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

impl Localization {
    pub fn parse_into(&mut self, text: &str) {
        let mut section = String::new();
        for line in text.lines() {
            let l = line.trim();
            if l.is_empty() || l.starts_with(';') {
                continue;
            }
            if l.starts_with('[') && l.ends_with(']') {
                section = l[1..l.len() - 1].to_string();
                continue;
            }
            let Some((k, v)) = l.split_once('=') else { continue };
            let v = v.trim();
            let v = v.strip_prefix('"').and_then(|x| x.strip_suffix('"')).unwrap_or(v);
            self.entries.entry((section.clone(), k.trim().to_string())).or_insert_with(|| v.to_string());
        }
    }

    /// Load `<root>/DishonoredGame/Localization/<lang>/<name>.int` for each name.
    pub fn load(root: &Path, lang: &str, names: &[&str]) -> Self {
        let mut l = Self::default();
        let dir = root.join("DishonoredGame").join("Localization").join(lang);
        for n in names {
            if let Ok(b) = std::fs::read(dir.join(format!("{n}.int"))) {
                l.parse_into(&decode(&b));
            }
        }
        l
    }

    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.entries.get(&(section.to_string(), key.to_string())).map(String::as_str)
    }

    /// First match of `key` in any of `sections`.
    pub fn find(&self, sections: &[&str], key: &str) -> Option<&str> {
        sections.iter().find_map(|s| self.get(s, key))
    }
}

/// Plain text of a Scaleform/UE3 string: `<br>` becomes a newline, `§C_BLUE§`-style colour
/// tags and other `§...§` markers are dropped.
pub fn plain(s: &str) -> String {
    let s = s.replace("<br>", "\n").replace("<br/>", "\n").replace("<br />", "\n");
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        if c == '§' {
            in_tag = !in_tag;
            continue;
        }
        if !in_tag {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_quoted_utf16() {
        let text = "[S]\nt_A=\"Hello\"\nt_B=World\n; c\n[T]\nt_A=x\n";
        let mut b = vec![0xFF, 0xFE];
        for u in text.encode_utf16() {
            b.extend_from_slice(&u.to_le_bytes());
        }
        let mut l = Localization::default();
        l.parse_into(&decode(&b));
        assert_eq!(l.get("S", "t_A"), Some("Hello"));
        assert_eq!(l.get("S", "t_B"), Some("World"));
        assert_eq!(l.find(&["X", "T"], "t_A"), Some("x"));
        assert_eq!(plain("a<br>§C_BLUE§b§C§"), "a\nb");
    }
}
