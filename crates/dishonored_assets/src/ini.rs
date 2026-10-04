//! UE3 INI reader. Layers `Engine/Config/Base*.ini` under `DishonoredGame/Config/Default*.ini`
//! the way the engine builds its effective config. Supports `+Key=` / `-Key=` / `.Key=`
//! array syntax by keeping every value for repeated keys.

use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Default, Clone)]
pub struct Ini {
    /// section -> key -> values (last wins for scalars)
    pub sections: BTreeMap<String, BTreeMap<String, Vec<String>>>,
}

impl Ini {
    pub fn parse_into(&mut self, text: &str) {
        let mut section = String::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(';') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                section = line[1..line.len() - 1].to_string();
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let (op, k) = match k.chars().next() {
                Some(c @ ('+' | '-' | '.' | '!')) => (Some(c), &k[1..]),
                _ => (None, k),
            };
            let vals = self
                .sections
                .entry(section.clone())
                .or_default()
                .entry(k.trim().to_string())
                .or_default();
            match op {
                Some('-') => vals.retain(|x| x != v.trim()),
                Some('!') => vals.clear(),
                Some('+') if vals.iter().any(|x| x == v.trim()) => {}
                Some('+') | Some('.') => vals.push(v.trim().to_string()),
                _ => {
                    // Repeated plain keys form arrays in UE3 INIs (e.g. m_Powers=...).
                    vals.push(v.trim().to_string());
                }
            }
        }
    }

    /// Load `<root>/Engine/Config/Base<name>.ini` then `<root>/DishonoredGame/Config/Default<name>.ini`.
    /// Default-layer values for a key replace base-layer values.
    pub fn load_layered(root: &Path, name: &str) -> Self {
        let mut base = Ini::default();
        if let Ok(t) = std::fs::read_to_string(root.join("Engine/Config").join(format!("Base{name}.ini"))) {
            base.parse_into(&t);
        }
        let mut def = Ini::default();
        if let Ok(t) = std::fs::read_to_string(root.join("DishonoredGame/Config").join(format!("Default{name}.ini"))) {
            def.parse_into(&t);
        }
        for (s, kv) in def.sections {
            let sec = base.sections.entry(s).or_default();
            for (k, v) in kv {
                sec.insert(k, v);
            }
        }
        base
    }

    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.sections
            .get(section)?
            .get(key)?
            .last()
            .map(String::as_str)
    }

    pub fn get_f32(&self, section: &str, key: &str) -> Option<f32> {
        self.get(section, key)?.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arrays_and_overrides() {
        let mut i = Ini::default();
        i.parse_into("[A]\nx=1\n+y=a\n+y=b\n-y=a\n; c\n[B]\nz=2.5\n");
        assert_eq!(i.get("A", "x"), Some("1"));
        assert_eq!(i.sections["A"]["y"], vec!["b".to_string()]);
        assert_eq!(i.get_f32("B", "z"), Some(2.5));
    }
}
