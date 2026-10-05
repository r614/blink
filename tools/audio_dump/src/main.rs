//! Inspect Dishonored's Wwise audio straight from the install.
//!
//! audio_dump survey                 codec / section / HIRC statistics over every package
//! audio_dump list <Bank>            media and HIRC objects in one package (e.g. Bank_Player)
//! audio_dump wem <Bank> <id> [out]  decode one media id to a WAV (default under harness_out/audio/)

use anyhow::{Context, Result};
use dishonored_audio::bank::Bank;
use dishonored_audio::pck::Pck;
use dishonored_audio::wem::Riff;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn install() -> PathBuf {
    std::env::var_os("DISHONORED_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files\GOG Galaxy\Games\Dishonored"))
}

fn all_pcks() -> Vec<PathBuf> {
    let root = install().join("DishonoredGame");
    let mut dirs = vec![root.join("CookedPCConsole")];
    for d in ["DLC05", "DLC06", "DLC07"] {
        dirs.push(root.join("DLC").join("PCConsole").join(d));
    }
    let mut out = Vec::new();
    for d in dirs {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pck")) {
                    out.push(p);
                }
            }
        }
    }
    out.sort();
    out
}

fn find_pck(name: &str) -> Result<PathBuf> {
    let want = name.trim_end_matches(".pck").to_ascii_lowercase();
    all_pcks()
        .into_iter()
        .find(|p| p.file_stem().unwrap().to_string_lossy().to_ascii_lowercase() == want)
        .with_context(|| format!("no package {name}"))
}

fn describe(b: &[u8]) -> String {
    match Riff::parse(b) {
        Ok(r) => {
            let tags: Vec<String> = r
                .chunks
                .iter()
                .map(|(t, c)| format!("{}:{}", String::from_utf8_lossy(t), c.len()))
                .collect();
            format!(
                "{} ch{} {}Hz fmt{} [{}]",
                r.codec_name(),
                r.channels,
                r.sample_rate,
                r.fmt.len(),
                tags.join(" ")
            )
        }
        Err(e) => format!("ERR {e}"),
    }
}

fn survey() -> Result<()> {
    let mut codec: BTreeMap<String, usize> = BTreeMap::new();
    let mut layout: BTreeMap<String, usize> = BTreeMap::new();
    let mut hirc: BTreeMap<u8, usize> = BTreeMap::new();
    let mut versions: BTreeMap<u32, usize> = BTreeMap::new();
    let (mut npck, mut nbank, mut nmedia, mut nstream, mut bytes) = (0, 0, 0, 0, 0u64);
    let mut stid = 0;
    let tally = |b: &[u8], codec: &mut BTreeMap<String, usize>, layout: &mut BTreeMap<String, usize>| {
        match Riff::parse(b) {
            Ok(r) => {
                *codec.entry(format!("{} ({} ch)", r.codec_name(), r.channels)).or_default() += 1;
                let tags: Vec<String> = r
                    .chunks
                    .iter()
                    .filter(|(t, _)| t != b"data")
                    .map(|(t, c)| {
                        let t = String::from_utf8_lossy(t).into_owned();
                        if t == "fmt " || t == "vorb" { format!("{t}:{}", c.len()) } else { t }
                    })
                    .collect();
                *layout.entry(format!("{} {}", r.codec_name(), tags.join(","))).or_default() += 1;
            }
            Err(_) => *codec.entry("unparsed".into()).or_default() += 1,
        }
    };
    for p in all_pcks() {
        let pck = match Pck::open(&p) {
            Ok(x) => x,
            Err(e) => {
                println!("{}: {e}", p.display());
                continue;
            }
        };
        npck += 1;
        for be in &pck.banks {
            let raw = pck.read(be)?;
            let bank = Bank::parse(&raw).with_context(|| p.display().to_string())?;
            nbank += 1;
            *versions.entry(bank.version).or_default() += 1;
            if !bank.bank_names.is_empty() {
                stid += 1;
            }
            for o in &bank.hirc {
                *hirc.entry(o.kind).or_default() += 1;
            }
            for m in &bank.media {
                nmedia += 1;
                if let Some(b) = bank.media_bytes(m) {
                    tally(b, &mut codec, &mut layout);
                }
            }
        }
        for se in &pck.streams {
            nstream += 1;
            bytes += se.size as u64;
            let mut head = pck.read(&PckEntryHead::head(se))?;
            head.truncate(se.size as usize);
            tally(&head, &mut codec, &mut layout);
        }
    }
    println!("packages {npck}, banks {nbank} (with STID {stid}), embedded media {nmedia}, streamed files {nstream} ({} MB)", bytes >> 20);
    println!("bank versions: {versions:?}");
    println!("codecs:");
    for (k, v) in &codec {
        println!("  {v:6}  {k}");
    }
    println!("layouts:");
    for (k, v) in &layout {
        println!("  {v:6}  {k}");
    }
    println!("HIRC kinds: {hirc:?}");
    let init = install().join("DishonoredGame/CookedPCConsole/Init.bnk");
    if let Ok(raw) = std::fs::read(&init) {
        let b = Bank::parse(&raw)?;
        println!("Init.bnk: version {} id {:08x} sections {:?}", b.version, b.id, b.sections.iter().map(|(t, s)| (String::from_utf8_lossy(t).into_owned(), *s)).collect::<Vec<_>>());
    }
    Ok(())
}

/// Read only the first 4 KB of a streamed file (enough for the RIFF headers).
struct PckEntryHead;
impl PckEntryHead {
    fn head(e: &dishonored_audio::pck::PckEntry) -> dishonored_audio::pck::PckEntry {
        dishonored_audio::pck::PckEntry { size: e.size.min(4096), ..*e }
    }
}

fn list(name: &str) -> Result<()> {
    let pck = Pck::open(find_pck(name)?)?;
    println!("{} languages {:?}", pck.path.display(), pck.languages);
    for be in &pck.banks {
        let raw = pck.read(be)?;
        let bank = Bank::parse(&raw)?;
        println!(
            "bank {:08x} v{} sections {:?}",
            bank.id,
            bank.version,
            bank.sections.iter().map(|(t, s)| (String::from_utf8_lossy(t).into_owned(), *s)).collect::<Vec<_>>()
        );
        for m in &bank.media {
            println!("  media {:10} {:8}B {}", m.id, m.size, bank.media_bytes(m).map(describe).unwrap_or_default());
        }
        for o in &bank.hirc {
            println!("  hirc type {:2} id {:10} len {:4} {:02x?}", o.kind, o.id, o.body.len(), &o.body[..o.body.len().min(std::env::var("HEXLEN").ok().and_then(|s| s.parse().ok()).unwrap_or(48))]);
        }
        for (id, n) in &bank.bank_names {
            println!("  stid {id:08x} {n}");
        }
    }
    for se in &pck.streams {
        let b = pck.read(se)?;
        println!("  stream {:10} {:8}B {}", se.id, se.size, describe(&b));
    }
    Ok(())
}

fn find_media(pck: &Pck, id: u32) -> Result<Vec<u8>> {
    for be in &pck.banks {
        let bank = Bank::parse(&pck.read(be)?)?;
        if let Some(b) = bank.media_by_id(id) {
            return Ok(b.to_vec());
        }
    }
    let se = pck.streams.iter().find(|s| s.id == id).context("media id not in package")?;
    pck.read(se)
}

fn wem(name: &str, id: u32, out: Option<&str>) -> Result<()> {
    let pck = Pck::open(find_pck(name)?)?;
    let b = find_media(&pck, id)?;
    println!("{}", describe(&b));
    if let Ok(r) = Riff::parse(&b) {
        println!("fmt  {:02x?}", r.fmt);
        if let Ok(d) = r.data() {
            println!("data {:02x?}", &d[..d.len().min(96)]);
        }
    }
    let lib = dishonored_audio::vorbis::CodebookLibrary::from_install(&install())?;
    println!("codebook library: {} codebooks", lib.len());
    let pcm = dishonored_audio::wem::decode(&b, &lib)?;
    println!(
        "decoded {} ch {} Hz {:.3}s peak {:.3} rms {:.4}",
        pcm.channels,
        pcm.sample_rate,
        pcm.duration(),
        pcm.peak(),
        pcm.rms()
    );
    let out = out.map(PathBuf::from).unwrap_or_else(|| {
        Path::new("harness_out/audio").join(format!("{}_{id}.wav", name.trim_end_matches(".pck")))
    });
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&out, pcm.to_wav())?;
    println!("wrote {}", out.display());
    Ok(())
}

/// Decode every embedded media file (and with `streams`, every streamed file too).
fn decode_all(streams: bool) -> Result<()> {
    let lib = dishonored_audio::vorbis::CodebookLibrary::from_install(&install())?;
    let t0 = std::time::Instant::now();
    let (mut ok, mut fail, mut secs, mut silent, mut short) = (0usize, 0usize, 0f64, 0usize, 0usize);
    let mut errors: BTreeMap<String, usize> = BTreeMap::new();
    let mut check = |b: &[u8], what: String| match dishonored_audio::wem::decode(b, &lib) {
        Ok(p) => {
            ok += 1;
            secs += p.duration() as f64;
            let want = Riff::parse(b).ok().and_then(|r| dishonored_audio::vorbis::vorb_info(&r).ok()).map(|v| v.sample_count as usize).unwrap_or(0);
            if want != p.frames() {
                short += 1;
                println!("LENGTH {what}: {} frames, header {want}", p.frames());
            }
            if p.peak() < 1e-4 || p.samples.iter().any(|s| !s.is_finite()) {
                silent += 1;
            }
        }
        Err(e) => {
            fail += 1;
            let k = e.to_string();
            if !errors.contains_key(&k) {
                println!("FAIL {what}: {k}");
            }
            *errors.entry(k).or_default() += 1;
        }
    };
    let mut prefetch_total = 0usize;
    for p in all_pcks() {
        let pck = Pck::open(&p)?;
        let name = p.file_stem().unwrap().to_string_lossy().into_owned();
        for be in &pck.banks {
            let bank = Bank::parse(&pck.read(be)?)?;
            for m in &bank.media {
                if pck.streams.iter().any(|s| s.id == m.id) {
                    prefetch_total += 1;
                    continue;
                }
                if let Some(b) = bank.media_bytes(m) {
                    check(b, format!("{name} media {}", m.id));
                }
            }
        }
        if streams {
            for se in &pck.streams {
                check(&pck.read(se)?, format!("{name} stream {}", se.id));
            }
        }
    }
    println!(
        "decoded {ok}, failed {fail}, silent/non-finite {silent}, length mismatches {short}, prefetch heads skipped {prefetch_total}, total {:.1} min audio in {:.1}s",
        secs / 60.0,
        t0.elapsed().as_secs_f32()
    );
    for (k, v) in errors {
        println!("  {v:5} {k}");
    }
    Ok(())
}

fn print_cue(lib: &dishonored_audio::library::SoundLibrary, c: &dishonored_audio::library::Cue, depth: usize) {
    use dishonored_audio::library::Cue;
    let pad = "  ".repeat(depth);
    match c {
        Cue::Leaf(l) => println!(
            "{pad}leaf {} media {} {} vol {:+.1} dB pitch {:+.0} loop {} att {:?}",
            lib.banks[l.media.bank].name,
            l.media.id,
            if l.media.streamed { "stream" } else { "embedded" },
            l.volume_db,
            l.pitch_cents,
            l.looping,
            l.max_distance
        ),
        Cue::All(v) => {
            println!("{pad}all ({})", v.len());
            v.iter().for_each(|c| print_cue(lib, c, depth + 1));
        }
        Cue::Random(v) => {
            println!("{pad}random ({})", v.len());
            v.iter().take(12).for_each(|c| print_cue(lib, &c.0, depth + 1));
            if v.len() > 12 {
                println!("{pad}  ... {} more", v.len() - 12);
            }
        }
    }
}

/// `event <Event> <Bank>[,<Bank>...]`: resolve an event and decode one pick per leaf group.
fn event(name: &str, banks: &str) -> Result<()> {
    let mut lib = dishonored_audio::library::SoundLibrary::new(install())?;
    for b in banks.split(',') {
        lib.load_bank(b)?;
    }
    let id = dishonored_audio::fnv1_32(name);
    println!("{name}: id {id}");
    if let Some((b, dishonored_audio::hirc::Node::Event { actions })) = lib.node(id) {
        println!("  in {} with actions:", lib.banks[b].name);
        for a in actions {
            if let Some((_, dishonored_audio::hirc::Node::Action(act))) = lib.node(*a) {
                println!("    action {a} type 0x{:04x} target {} set {:x?}", act.action_type, act.target, act.set_group);
            }
        }
    }
    for s in std::env::var("AK_STATES").unwrap_or_default().split(',').filter(|s| !s.is_empty()) {
        println!("  state event {s}: {}", lib.apply_state_event(s));
    }
    if let Ok(t) = std::env::var("AK_NODE") { if let Some((_, n)) = lib.node(t.parse()?) { println!("  node {t}: {:?}", match n { dishonored_audio::hirc::Node::Container { kind, children, switch_groups, default_switch, group_id, .. } => format!("kind {kind} children {} groups {:x?} default {default_switch:x} group {group_id:x}", children.len(), switch_groups), _ => "other".into() }); } }
    println!("  states {:x?}", lib.states.lock().unwrap());
    let cue = lib.resolve(name).context("event does not resolve to any sound")?;
    print_cue(&lib, &cue, 1);
    let mut seed = 12345u32;
    let mut rnd = move || {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (seed >> 8) as f32 / (1u32 << 24) as f32
    };
    let mut leaves = Vec::new();
    cue.pick(&mut rnd, &mut leaves);
    std::fs::create_dir_all("harness_out/audio")?;
    for (i, l) in leaves.iter().enumerate() {
        let p = lib.decode(l.media)?;
        let out = format!("harness_out/audio/{name}_{i}.wav");
        std::fs::write(&out, p.to_wav())?;
        println!("  pick {i}: media {} {:.2}s peak {:.3} -> {out}", l.media.id, p.duration(), p.peak());
    }
    Ok(())
}

/// `emitters <Map_P>`: placed AkAmbientSound actors, their banks and whether they resolve.
fn emitters(map: &str) -> Result<()> {
    let t0 = std::time::Instant::now();
    let idx = dishonored_audio::index::EventIndex::build(&install())?;
    println!("event index: {} events in {} packages ({:.2}s)", idx.events.len(), idx.packages.len(), t0.elapsed().as_secs_f32());
    let content = dishonored_assets::Content::locate()?;
    let pkgs = dishonored_assets::map_packages(&content, map);
    let mut lib = dishonored_audio::library::SoundLibrary::new(install())?;
    let (mut n, mut ok) = (0, 0);
    for p in &pkgs {
        let pkg = content.package(p)?;
        for e in dishonored_audio::emitters::ambient_emitters(&pkg) {
            n += 1;
            let bank = idx.bank_of(&e.event).map(str::to_string);
            let resolved = bank.as_ref().is_some_and(|b| lib.load_bank(b).is_ok() && lib.resolve(&e.event).is_some());
            ok += resolved as usize;
            println!(
                "{p:24} {:40} {:>9.0},{:>9.0},{:>7.0} auto {} bank {:?} resolves {resolved}",
                e.event, e.position[0], e.position[1], e.position[2], e.auto_play, bank
            );
        }
    }
    println!("{n} emitters, {ok} resolve to sounds ({:.2}s)", t0.elapsed().as_secs_f32());
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("survey") => survey(),
        Some("emitters") => emitters(args.get(1).map(String::as_str).unwrap_or("L_Boyle_Ext_P")),
        Some("event") => event(args.get(1).context("event name")?, args.get(2).context("bank list")?),
        Some("decodeall") => decode_all(args.get(1).is_some_and(|a| a == "--streams")),
        Some("list") => list(args.get(1).context("bank name")?),
        Some("wem") => wem(
            args.get(1).context("bank name")?,
            args.get(2).context("media id")?.parse()?,
            args.get(3).map(String::as_str),
        ),
        _ => {
            eprintln!("usage: audio_dump survey | list <Bank> | wem <Bank> <id> [out.wav]");
            Ok(())
        }
    }
}
