//! Bodies burnt to ash (Shadow Kill, walls of light): the dead state's shadow-kill sequence
//! (`StateNPCMasterDead_Limp` update 0x00A9CFC0, `m_ShadowKillState`) as `Twk_Pawn_DefaultNPC`
//! (`DisTweaks_NPCPawn`) times it, and its fiery material.
//!
//! - `m_fShadowKillDelay` after the kill the ash systems start (`m_ShadowKillFXs`);
//! - for `m_fShadowKillFadeToBlackDuration` the mesh's materials' `Sk_Fade_To_Black` runs from 1 to
//!   0 (0x00BC82E0);
//! - then the mesh takes `m_pFieryMaterial` (0x00BC84F0) and for `m_fShadowKillFieryDuration` its
//!   `F_Firy_Amount` runs from 1 to 0;
//! - then the body and what is attached to it go (pawn slot 493, 0x00652CF0 on the attachments).
//!
//! The fiery material (`Shadow_kill_inst`, a `Firy_Fading_PMAT` instance compiled with
//! `Use_for_Shadow_Kill`) is masked; its GPU-skin base pass pixel shader (RefShaderCache shader
//! 37508) and the map's uniform expressions give, with `A` = `F_Firy_Amount`, `T` =
//! `F_Firy_Tilling` and `E` = `F_Firy_Edge_Size`: the mask `m` = `F_Firy_Mask`.r at
//! `((u + 1) T / 2, (v + 1) T / 4)`, `c = clamp(A - 0.52, 0.1, 0.52)`, and the pixel is kept
//! while `saturate((m - (1 - 2c)) c E) >= 1/3`, i.e. `m >= (1 - 2c) + 1 / (3 c E)`. Its colour is
//! `F_Firy_texture x (that) x F_Firy_Intensity` blended toward `D_Diffuse x D_Diffuse_Brightness`,
//! with no specular or emissive in that permutation: the instance sets both factors to 0, so the
//! body is black. (The shader scales the UV by the first interpolant's w, taken as 1.)

use crate::content::Content;
use crate::tweaks::{TweakDb, get_f32, get_obj};
use anyhow::{Context, Result};
use ue3_package::ObjRef;

#[derive(Debug, Clone, PartialEq)]
pub struct AshBodyDef {
    pub delay: f32,
    pub fade_to_black: f32,
    pub fiery: f32,
    /// `F_Firy_Tilling` and `F_Firy_Edge_Size` of the fiery material.
    pub tiling: f32,
    pub edge_size: f32,
    /// The `F_Firy_Mask` texture: (package, object path).
    pub mask: (String, String),
}

/// The alpha-test threshold of the fiery material's mask at `F_Firy_Amount` = `amount`.
pub fn fiery_cutoff(amount: f32, edge_size: f32) -> f32 {
    let c = (amount - 0.52).clamp(0.1, 0.52);
    (1.0 - 2.0 * c) + 1.0 / (3.0 * c * edge_size.max(1e-3))
}

/// `Twk_Pawn_DefaultNPC`'s shadow-kill sequence and its fiery material's parameters.
pub fn ash_body_def(content: &Content) -> Result<AshBodyDef> {
    let db = TweakDb::load(content)?;
    let t = db.resolve("Twk_Pawn_DefaultNPC.Twk_Pawn_DefaultNPC")?;
    let f = |k: &str| get_f32(&t, k).unwrap_or(0.0);
    let material = get_obj(&t, "m_pFieryMaterial").filter(|m| !m.is_empty() && *m != "None").context("Twk_Pawn_DefaultNPC: no m_pFieryMaterial")?.to_string();
    // The material's package first, then Engine (shared textures such as the default mask).
    let names: Vec<&str> = crate::level::LIBRARY_PACKAGES.iter().copied().chain(["Engine"]).collect();
    let pkgs: Vec<_> = names.iter().filter_map(|n| content.package(n).ok().map(|p| (n.to_string(), p))).collect();
    let Some((slot, start)) = pkgs.iter().enumerate().find_map(|(s, (_, p))| (0..p.exports.len()).find(|&i| p.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(&material)).map(|i| (s, i))) else {
        anyhow::bail!("fiery material {material} not found")
    };
    let arcs: Vec<_> = pkgs.iter().map(|p| p.1.clone()).collect();
    let chain = crate::material_chain::walk(&arcs, (slot, start), &mut |s, r| match r {
        ObjRef::Export(i) => Some((s, i)),
        ObjRef::Import(_) => {
            let path = arcs[s].obj_path(r);
            arcs.iter().enumerate().find_map(|(k, p)| (0..p.exports.len()).find(|&i| p.export_class(i) != "Package" && p.obj_path(ObjRef::Export(i)).eq_ignore_ascii_case(&path)).map(|i| (k, i)))
        }
        ObjRef::Null => None,
    });
    let scalar = |n: &str| chain.scalars.iter().find(|s| s.0.eq_ignore_ascii_case(n)).map(|s| s.1);
    let (ms, mi) = chain.textures.iter().chain(chain.default_textures.iter()).find(|t| t.0.eq_ignore_ascii_case("F_Firy_Mask")).map(|t| t.1).context("fiery material: no F_Firy_Mask")?;
    Ok(AshBodyDef {
        delay: f("m_fShadowKillDelay"),
        fade_to_black: f("m_fShadowKillFadeToBlackDuration"),
        fiery: f("m_fShadowKillFieryDuration"),
        tiling: scalar("F_Firy_Tilling").unwrap_or(1.0),
        edge_size: scalar("F_Firy_Edge_Size").unwrap_or(1.0),
        mask: (pkgs[ms].0.clone(), arcs[ms].obj_path(ObjRef::Export(mi))),
    })
}
