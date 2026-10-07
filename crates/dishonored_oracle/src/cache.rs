//! Cache of original-game answers, keyed by (exe SHA-256, subject, probe, query-set hash).
//!
//! A subject is a map (`L_Boyle_Ext_P`) or a scenario (`movement/jump_standing`). The inner
//! loop reads these files and never runs the game; a capture (phase 2, the engine hook)
//! writes them once per exe build and query set.
//!
//! File layout (little endian), `<root>/<exe sha256 first 16 hex>/<subject>/<probe>-<query hash>.orc`:
//!
//! | bytes | field |
//! |---|---|
//! | 8 | magic `BLKORACL` |
//! | 2 | format version ([`FORMAT_VERSION`]) |
//! | 2 | probe ([`Probe`] tag) |
//! | 4 | key length K |
//! | 8 | payload length P |
//! | 8 | FNV-1a 64 of key + payload bytes |
//! | K | [`OracleKey`], bincode (standard config) |
//! | P | payload, bincode (standard config); its type is fixed by the probe |
//!
//! A reader rejects another magic, version, probe or checksum, so a format change only needs a
//! version bump; stale files are re-captured, never misread.

use anyhow::{Context, Result, bail, ensure};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const MAGIC: [u8; 8] = *b"BLKORACL";
pub const FORMAT_VERSION: u16 = 1;
const HEADER: usize = 8 + 2 + 2 + 4 + 8 + 8;

/// What kind of answers a file holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Probe {
    /// `UWorld::SingleLineCheck` lines and boxes ([`crate::query::CollisionSet`]).
    Collision = 1,
    /// Path / reachability queries on the navigation mesh.
    Navigation = 2,
    /// Light level at points (`DishonoredVisibilityComponent`).
    Light = 3,
    /// Line of sight / NPC vision.
    Visibility = 4,
    /// Reflection dump of live objects (phase 2).
    State = 5,
    /// Per-tick behaviour trace of a scenario ([`crate::trace::Trace`]).
    Trace = 6,
}

impl Probe {
    pub fn tag(self) -> &'static str {
        match self {
            Probe::Collision => "collision",
            Probe::Navigation => "nav",
            Probe::Light => "light",
            Probe::Visibility => "los",
            Probe::State => "state",
            Probe::Trace => "trace",
        }
    }
    pub fn from_u16(v: u16) -> Option<Self> {
        Some(match v {
            1 => Probe::Collision,
            2 => Probe::Navigation,
            3 => Probe::Light,
            4 => Probe::Visibility,
            5 => Probe::State,
            6 => Probe::Trace,
            _ => return None,
        })
    }
    pub fn parse(s: &str) -> Option<Self> {
        [Probe::Collision, Probe::Navigation, Probe::Light, Probe::Visibility, Probe::State, Probe::Trace].into_iter().find(|p| p.tag() == s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OracleKey {
    pub exe_sha256: [u8; 32],
    /// Map or scenario name.
    pub subject: String,
    pub probe: Probe,
    /// Hash of the query set (or scenario spec) the answers belong to.
    pub query_hash: u64,
}

impl OracleKey {
    pub fn exe_hex(&self) -> String {
        hex(&self.exe_sha256)
    }
    /// The cache file of this key under `root`.
    pub fn path(&self, root: &Path) -> PathBuf {
        let subject: String = self.subject.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '/' { c } else { '_' }).collect();
        root.join(&self.exe_hex()[..16]).join(subject).join(format!("{}-{:016x}.orc", self.probe.tag(), self.query_hash))
    }
}

/// Provenance stored with every answer set.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    /// Seconds since the Unix epoch when the answers were captured.
    pub captured_unix: u64,
    /// How: tool, command, input files.
    pub source: String,
    /// Original-side capture time in seconds (for the speed table).
    pub capture_seconds: f64,
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn fnv64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// FNV-1a 64 continued over more bytes.
pub fn fnv64_more(mut h: u64, data: &[u8]) -> u64 {
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn cfg() -> bincode::config::Configuration {
    bincode::config::standard()
}

pub fn encode<T: Serialize>(key: &OracleKey, payload: &T) -> Result<Vec<u8>> {
    let k = bincode::serde::encode_to_vec(key, cfg())?;
    let p = bincode::serde::encode_to_vec(payload, cfg())?;
    let mut out = Vec::with_capacity(HEADER + k.len() + p.len());
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&(key.probe as u16).to_le_bytes());
    out.extend_from_slice(&(k.len() as u32).to_le_bytes());
    out.extend_from_slice(&(p.len() as u64).to_le_bytes());
    out.extend_from_slice(&fnv64_more(fnv64(&k), &p).to_le_bytes());
    out.extend_from_slice(&k);
    out.extend_from_slice(&p);
    Ok(out)
}

/// Header + key of a file (cheap: the payload is not decoded).
pub fn decode_key(bytes: &[u8]) -> Result<OracleKey> {
    ensure!(bytes.len() >= HEADER && bytes[..8] == MAGIC, "not an oracle cache file");
    let version = u16::from_le_bytes([bytes[8], bytes[9]]);
    ensure!(version == FORMAT_VERSION, "oracle cache format {version}, expected {FORMAT_VERSION}");
    let probe = u16::from_le_bytes([bytes[10], bytes[11]]);
    let Some(probe) = Probe::from_u16(probe) else { bail!("unknown probe {probe}") };
    let klen = u32::from_le_bytes(bytes[12..16].try_into()?) as usize;
    ensure!(bytes.len() >= HEADER + klen, "truncated key");
    let (key, _): (OracleKey, _) = bincode::serde::decode_from_slice(&bytes[HEADER..HEADER + klen], cfg())?;
    ensure!(key.probe == probe, "probe tag mismatch");
    Ok(key)
}

pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<(OracleKey, T)> {
    let key = decode_key(bytes)?;
    let klen = u32::from_le_bytes(bytes[12..16].try_into()?) as usize;
    let plen = u64::from_le_bytes(bytes[16..24].try_into()?) as usize;
    let sum = u64::from_le_bytes(bytes[24..32].try_into()?);
    ensure!(bytes.len() == HEADER + klen + plen, "length {} != {}", bytes.len(), HEADER + klen + plen);
    let body = &bytes[HEADER..];
    ensure!(fnv64_more(fnv64(&body[..klen]), &body[klen..]) == sum, "checksum mismatch");
    let (payload, used): (T, usize) = bincode::serde::decode_from_slice(&body[klen..], cfg())?;
    ensure!(used == plen, "payload decoded {used} of {plen} bytes");
    Ok((key, payload))
}

/// Root of the oracle cache: `$DISHONORED_ORACLE_CACHE`, else `$DISHONORED_CACHE/oracle`, else
/// `cache/oracle` (gitignored). Never inside the repository's tracked files.
pub fn root() -> PathBuf {
    if let Some(p) = std::env::var_os("DISHONORED_ORACLE_CACHE") {
        return PathBuf::from(p);
    }
    std::env::var_os("DISHONORED_CACHE").map(|p| PathBuf::from(p).join("oracle")).unwrap_or_else(|| PathBuf::from("cache").join("oracle"))
}

pub fn store<T: Serialize>(root: &Path, key: &OracleKey, payload: &T) -> Result<PathBuf> {
    let p = key.path(root);
    std::fs::create_dir_all(p.parent().unwrap())?;
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, encode(key, payload)?)?;
    std::fs::rename(&tmp, &p)?;
    Ok(p)
}

pub fn load<T: DeserializeOwned>(root: &Path, key: &OracleKey) -> Result<T> {
    let p = key.path(root);
    let bytes = std::fs::read(&p).with_context(|| format!("{}", p.display()))?;
    let (k, v) = decode(&bytes).with_context(|| format!("{}", p.display()))?;
    ensure!(&k == key, "{}: key mismatch", p.display());
    Ok(v)
}

/// Every cache file under `root` for an exe (all exes when `None`): (key, path).
pub fn list(root: &Path, exe: Option<&[u8; 32]>) -> Vec<(OracleKey, PathBuf)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.filter_map(|e| e.ok()) {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "orc") {
                let Ok(mut f) = std::fs::File::open(&p) else { continue };
                // The key sits right after the fixed header: read just enough of the file.
                let mut head = vec![0u8; 4096];
                use std::io::Read;
                let n = f.read(&mut head).unwrap_or(0);
                head.truncate(n);
                if let Ok(k) = decode_key(&head) {
                    if exe.is_none_or(|e| &k.exe_sha256 == e) {
                        out.push((k, p));
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| (&a.0.subject, a.0.probe, a.0.query_hash).cmp(&(&b.0.subject, b.0.probe, b.0.query_hash)));
    out
}

/// SHA-256 of a file, remembered per (path, size, mtime) in `<root>/exe_hashes.json`.
pub fn file_sha256(root: &Path, path: &Path) -> Result<[u8; 32]> {
    use sha2::{Digest, Sha256};
    let m = std::fs::metadata(path).with_context(|| format!("{}", path.display()))?;
    let mtime = m.modified()?.duration_since(std::time::UNIX_EPOCH)?.as_secs();
    let id = format!("{}|{}|{mtime}", path.display(), m.len());
    let memo = root.join("exe_hashes.json");
    let mut known: std::collections::BTreeMap<String, String> = std::fs::read_to_string(&memo).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    if let Some(h) = known.get(&id) {
        if let Some(b) = unhex32(h) {
            return Ok(b);
        }
    }
    let data = std::fs::read(path)?;
    let d: [u8; 32] = Sha256::digest(&data).into();
    known.insert(id, hex(&d));
    std::fs::create_dir_all(root).ok();
    std::fs::write(&memo, serde_json::to_string_pretty(&known)?).ok();
    Ok(d)
}

pub fn unhex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

/// The original executable of an install.
pub fn exe_path(install: &Path) -> PathBuf {
    install.join("Binaries").join("Win32").join("Dishonored.exe")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> OracleKey {
        OracleKey { exe_sha256: [7; 32], subject: "L_Test_P".into(), probe: Probe::Collision, query_hash: 0xabcdef }
    }

    #[test]
    fn roundtrip_and_rejections() {
        let payload: Vec<(u32, f32)> = vec![(1, 0.5), (2, 0.25)];
        let bytes = encode(&key(), &payload).unwrap();
        let (k, v): (OracleKey, Vec<(u32, f32)>) = decode(&bytes).unwrap();
        assert_eq!(k, key());
        assert_eq!(v, payload);
        assert_eq!(decode_key(&bytes).unwrap(), key());
        // Corrupt payload byte: checksum.
        let mut bad = bytes.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(decode::<Vec<(u32, f32)>>(&bad).is_err());
        // Other format version.
        let mut old = bytes.clone();
        old[8] = 99;
        assert!(decode_key(&old).unwrap_err().to_string().contains("format"));
        // Truncated.
        assert!(decode::<Vec<(u32, f32)>>(&bytes[..bytes.len() - 1]).is_err());
    }

    #[test]
    fn store_load_list() {
        let root = std::env::temp_dir().join(format!("oracle_cache_test_{}", std::process::id()));
        let k = key();
        let p = store(&root, &k, &vec![1u8, 2, 3]).unwrap();
        assert!(p.ends_with("collision-0000000000abcdef.orc"));
        let v: Vec<u8> = load(&root, &k).unwrap();
        assert_eq!(v, vec![1, 2, 3]);
        let other = OracleKey { query_hash: 1, ..k.clone() };
        assert!(load::<Vec<u8>>(&root, &other).is_err());
        let l = list(&root, Some(&[7; 32]));
        assert_eq!(l.len(), 1);
        assert!(list(&root, Some(&[8; 32])).is_empty());
        std::fs::remove_dir_all(&root).ok();
    }
}
