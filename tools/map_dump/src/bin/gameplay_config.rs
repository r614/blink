//! Extract the gameplay configuration from the install and write it as JSON.
//! `gameplay_config [out.json]` (default: fixtures/config/gameplay_config.json)
use anyhow::Result;
use dishonored_assets::Content;
use dishonored_assets::gameplay::load_gameplay_config;

fn main() -> Result<()> {
    let out = std::env::args().nth(1).unwrap_or_else(|| "fixtures/config/gameplay_config.json".into());
    let content = Content::locate()?;
    let cfg = load_gameplay_config(&content)?;
    if let Some(dir) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&out, serde_json::to_string(&cfg)?)?;
    println!("wrote {out}");
    println!("{:#?}", cfg.movement);
    Ok(())
}
