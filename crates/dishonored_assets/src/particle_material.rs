//! Particle sprite materials: what each Arkane particle base material's compiled base pass
//! pixel shader does with its textures (RefShaderCache-PC-D3D-SM3, `shader_maps` +
//! `fxc /dumpbin`; EFFECTS.md "Particle materials"). The cooked materials keep only their
//! parameter expressions, so the colour and opacity terms come from the shaders.
//!
//! Every recipe reduces the shader to: colour = colour source x constant tint x vertex colour,
//! opacity = opacity source (one channel) x constant x vertex alpha (or, for the height-map
//! materials, `saturate((H - vertex alpha) x Mask_Multiply)`). The importer bakes the colour
//! source with the opacity channel in alpha into the material's diffuse slot; the runtime
//! applies the constants ([`dishonored_ir::ParticleShading`]).

/// Where the sprite's colour comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum Colour {
    /// A texture parameter, all channels or one channel as grey.
    Texture { param: &'static str, channel: Option<usize> },
    /// The base material's unnamed texture `Tex#i` (cooked `UniformExpressionTextures`).
    Expression(usize),
    /// No texture: the constant tint alone.
    White,
}

/// Where the sprite's opacity comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum Opacity {
    /// One channel (0..3) of a texture parameter.
    Texture { param: &'static str, channel: usize },
    /// The shader writes the vertex alpha only (the texture's alpha is ignored).
    VertexOnly,
    /// Procedural radial falloff (`Glow_PMAT`).
    Radial,
}

/// How a vector parameter's alpha is used.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AlphaUse {
    /// Multiplies the opacity (`D_DiffuseColorTweak`).
    Opacity,
    /// Multiplies the colour (`Color.rgb x Color.a`).
    Colour,
    Ignored,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Recipe {
    pub colour: Colour,
    pub opacity: Opacity,
    /// Constant colour vector (name, default) and the use of its alpha.
    pub tint: Option<(&'static str, [f32; 4], AlphaUse)>,
    /// Constant colour scalar (name, default).
    pub brightness: Option<(&'static str, f32)>,
    /// Constant opacity scalar (name, default); with `erode`, the erosion's multiplier.
    pub opacity_scale: Option<(&'static str, f32)>,
    /// The vertex alpha erodes the opacity map (height-map blood, water splash).
    pub erode: bool,
}

const WHITE: [f32; 4] = [1.0; 4];

/// The recipe of a particle base material (`base`: its path or name) for one compiled
/// permutation. `switch` reads a static switch (false when absent), `mask` the single channel of
/// a static component mask. None for materials that are not particle bases (or not decoded).
pub fn recipe(base: &str, switch: &dyn Fn(&str) -> bool, mask: &dyn Fn(&str) -> Option<usize>) -> Option<Recipe> {
    let name = base.rsplit('.').next().unwrap_or(base);
    let r = match name {
        // Shaders 2815 / 35485 / 35475 (translucent), 35915 / 36015 / 35919 (additive),
        // DefaultTranslu2sided 0x6a3280b: rgb = D_Diffuse x D_DiffuseColorTweak.rgb x vertex,
        // alpha = O_Opacity[O_Opacity_Channel] x tweak.a x vertex alpha (additive: rgb x alpha).
        "DefaultTranslu_PMAT" | "DefaultTranslu2sided_PMAT" | "DefaultTransluFOG_PMAT" | "DefaultAdditive_PMAT" | "DefaultAdditive_2Sided_PMAT" | "DefaultAdditive_UV_PMAT" | "DefaultAdditive_OutBendTime_PMAT" => {
            let colour = if switch("Mo_Enable_Diffuse_Motion") {
                Colour::Texture { param: "Motion_Mask", channel: None }
            } else if switch("D_DisableDiffuse") || switch("D_Disable_Diffuse") {
                Colour::White
            } else if switch("D_Enable_Diffuse_Channel") {
                // `mul r0.xyz, r0.x, c6`: the selected channel as grey.
                Colour::Texture { param: "D_Diffuse", channel: Some(mask("D_Diffuse_Channel").unwrap_or(0)) }
            } else {
                Colour::Texture { param: "D_Diffuse", channel: None }
            };
            let opacity = if switch("Mo_Enable_Opacity_Motion") {
                Opacity::Texture { param: "Motion_Mask", channel: 0 }
            } else if switch("O_EnableOpacityAlpha") {
                Opacity::Texture { param: "O_Opacity", channel: 3 }
            } else {
                Opacity::Texture { param: "O_Opacity", channel: mask("O_Opacity_Channel").unwrap_or(0) }
            };
            Recipe { colour, opacity, tint: Some(("D_DiffuseColorTweak", WHITE, AlphaUse::Opacity)), brightness: None, opacity_scale: None, erode: false }
        }
        // 37612..37620: rgb = D_Diffuse x vertex (D_DisableDiffuse: D_Diffuse_color.rgb x .a),
        // alpha = vertex alpha, x MA_MaskTexture[MA_Mask_Channel] with MA_Use_Masked_Alpha.
        "Default_Simple_Translucent_PMAT" => {
            let disable = switch("D_DisableDiffuse");
            Recipe {
                colour: if disable { Colour::White } else { Colour::Texture { param: "D_Diffuse", channel: None } },
                opacity: if switch("MA_Use_Masked_Alpha") { Opacity::Texture { param: "MA_MaskTexture", channel: mask("MA_Mask_Channel").unwrap_or(0) } } else { Opacity::VertexOnly },
                tint: disable.then_some(("D_Diffuse_color", [0.0, 0.0, 0.0, 1.0], AlphaUse::Colour)),
                brightness: None,
                opacity_scale: None,
                erode: false,
            }
        }
        // 6086..6105: rgb = D_Diffuse x D_Diffuse_Brightness (B_Enable_Blood: x DiffuseColor.rgb
        // x DiffuseColor.a), alpha = saturate((O_OpacityRGB_R_is_default[R | G Use_Green |
        // B Use_Blue] - vertex alpha) x Mask_Multiply). The Smoke / Ink variants share the
        // parameters (LIKELY).
        "HeightMapAnim_PMAT" | "HeightMapAnim_Smoke_PMAT" | "HeightMapAnim_Ink_PMAT" => {
            let blood = switch("B_Enable_Blood");
            Recipe {
                colour: if switch("D_Disable_Diffuse") { Colour::White } else { Colour::Texture { param: "D_Diffuse", channel: None } },
                opacity: Opacity::Texture { param: "O_OpacityRGB_R_is_default", channel: height_channel(switch) },
                tint: blood.then_some(("DiffuseColor", WHITE, AlphaUse::Colour)),
                brightness: (!blood).then_some(("D_Diffuse_Brightness", 1.0)),
                opacity_scale: Some(("Mask_Multiply", 20.0)),
                erode: true,
            }
        }
        // 5807 / 5820 / 5872 (BLEND_Masked): rgb = lerp of two sub-images of D_diffuse_texture x
        // D_Diffuse_Brightness (+ cubemap), clip where saturate((H - vertex alpha) x
        // Mask_Multiply) < 1/3, H = H_HightMap_RGB_R_is_default[R | G Use_Green | B Use_Blue].
        "SubUV_HeightMap_PMAT" => Recipe {
            colour: Colour::Texture { param: "D_diffuse_texture", channel: None },
            opacity: Opacity::Texture { param: "H_HightMap_RGB_R_is_default", channel: height_channel(switch) },
            tint: None,
            brightness: Some(("D_Diffuse_Brightness", 1.0)),
            opacity_scale: Some(("Mask_Multiply", 20.0)),
            erode: true,
        },
        // 0x60a95c8: rgb ~ D_DiffuseAlpha x vertex (plus a lit term), alpha = O_Opacity[channel] x
        // vertex alpha.
        "HeavySmoke_PMAT" => Recipe {
            colour: Colour::Texture { param: "D_DiffuseAlpha", channel: None },
            opacity: Opacity::Texture { param: "O_Opacity", channel: mask("O_Opacity_Channel").unwrap_or(0) },
            tint: None,
            brightness: None,
            opacity_scale: None,
            erode: false,
        },
        // 0x69d877a / 0x6a3f7ee: rgb = R_Reflexion2d (normal-distorted) x D_DiffuseColorTweak.rgb x
        // .a x vertex red, alpha = Alpha[Alpha_Mask] x vertex alpha (additive: rgb x alpha).
        "Water_Translu__PMAT" | "Water_Additive__PMAT" => Recipe {
            colour: Colour::Texture { param: "R_Reflexion2d", channel: None },
            opacity: Opacity::Texture { param: "Alpha", channel: mask("Alpha_Mask").unwrap_or(0) },
            tint: Some(("D_DiffuseColorTweak", WHITE, AlphaUse::Colour)),
            brightness: None,
            opacity_scale: None,
            erode: false,
        },
        // 0x62006c7 (additive): rgb = Tex#0 x Tex#0 (two droplet scales) x Brightness, x
        // saturate((Shape.r - vertex alpha) x Max_Opacity).
        "WaterSplash_PMAT" => Recipe {
            colour: Colour::Expression(0),
            opacity: Opacity::Texture { param: "Shape", channel: 0 },
            tint: None,
            brightness: Some(("Brightness", 1.0)),
            opacity_scale: Some(("Max_Opacity", 1.0)),
            erode: true,
        },
        // 0x6201d3c: rgb = F_Fire_Color_Texture x F_Fire_Color.rgb x .a (eroded by D_Diffuse.r
        // with the vertex red) + smoke light, alpha = D_Diffuse.b x vertex alpha.
        "Fire_Explosion_PMAT" | "Fire_Explosion_02_PMAT" | "Fire_Explosion_03_PMAT" | "Fire_Explosion_Oil_PMAT" => Recipe {
            colour: Colour::Texture { param: "F_Fire_Color_Texture", channel: None },
            opacity: Opacity::Texture { param: "D_Diffuse", channel: 2 },
            tint: Some(("F_Fire_Color", WHITE, AlphaUse::Colour)),
            brightness: None,
            opacity_scale: None,
            erode: false,
        },
        // 0x60bcd89: rgb = gradient Tex#1 looked up by Tex#0.g (power vertex green) x
        // lookup_color, alpha = smoothstep of Diffuse.r against the vertex red x vertex alpha.
        // APPROXIMATION: Tex#0 as the colour, Diffuse.r as the opacity.
        "Look_Up_Fire_PMAT" => Recipe {
            colour: Colour::Expression(0),
            opacity: Opacity::Texture { param: "Diffuse", channel: 0 },
            tint: Some(("lookup_color", WHITE, AlphaUse::Colour)),
            brightness: None,
            opacity_scale: None,
            erode: false,
        },
        // 0x6a19303: rgb = Color.rgb x Color.a (lerp to ColorAlert by Alert), alpha =
        // A_Alpha.r x (1 + inferno) x Opacity x vertex alpha.
        "Volume_Light_translu_PMAT" => Recipe {
            colour: Colour::White,
            opacity: Opacity::Texture { param: "A_Alpha", channel: 0 },
            tint: Some(("Color", [0.7, 1.0, 1.3, 1.0], AlphaUse::Colour)),
            brightness: None,
            opacity_scale: Some(("Opacity", 0.25)),
            erode: false,
        },
        // 0x60faee0 (additive): rgb = G_GlowColor.rgb x vertex x (max(1 - 2r, 0)^G_GlowPower +
        // 0.1 r^2) x G_GlowColor.a x vertex alpha; no texture.
        "Glow_PMAT" => Recipe {
            colour: Colour::White,
            opacity: Opacity::Radial,
            tint: Some(("G_GlowColor", WHITE, AlphaUse::Opacity)),
            brightness: None,
            opacity_scale: None,
            erode: false,
        },
        _ => return None,
    };
    Some(r)
}

/// The height-map channel of the HeightMapAnim / SubUV_HeightMap shaders: `Use_Blue` reads B
/// (6101, 5872), `Use_Green` G (6093), else R.
fn height_channel(switch: &dyn Fn(&str) -> bool) -> usize {
    if switch("Use_Blue") {
        2
    } else if switch("Use_Green") {
        1
    } else {
        0
    }
}

/// Opacity texture parameters by convention, for particle materials on bases without a
/// recipe: the first named parameter present wins (with its channel mask, if any).
pub const OPACITY_PARAMS: &[(&str, Option<&str>)] = &[
    ("O_Opacity", Some("O_Opacity_Channel")),
    ("O_OpacityRGB_R_is_default", None),
    ("H_HightMap_RGB_R_is_default", None),
    ("MA_MaskTexture", Some("MA_Mask_Channel")),
    ("A_Alpha", None),
    ("Alpha", Some("Alpha_Mask")),
    ("Shape", None),
    ("M_Mask", Some("M_Mask_Channel")),
];

/// ParticleSystems the gameplay effects spawn: the effect table of EFFECTS.md (tweaks) and every
/// particle the contact system's sub-intersections name (impacts, blood, splashes,
/// explosions). Decoded with every level.
pub const GAMEPLAY_PARTICLES: &[&str] = &[
    "Vfx_Weapon.Effects.pistol.Ps_PlayerPistol_StdMuzzle_01",
    "Vfx_Weapon.Effects.Ps_PistolMuzzle",
    "Vfx_GamePlay.ShadowKill.ps_SK_Leg_Arms",
    "Vfx_GamePlay.Blink.Blink_Mantle_01",
    "Vfx_GamePlay.Blink.Blink_Ground_01",
    "Vfx_GamePlay.Blink.Blink_Fall_01",
    "Vfx_GamePlay.Windblast.Windblast_lvl1",
    "Vfx_GamePlay.Windblast.Windblast_lvl2",
    "Vfx_GamePlay.Windblast.Windblast_UndWater_lvl1",
    "Vfx_GamePlay.Windblast.Windblast_GroundImpact_01",
    "Vfx_GamePlay.Windblast.Windblast_PLayerWater_01",
    "Vfx_GamePlay.Windblast.Windblast_PLayerDirt_01",
    "Vfx_GamePlay.Windblast.WIndBlast_WaterImpact_01",
    "Vfx_GamePlay.RatSwarm.RSwarm_Despawn_01",
    "Vfx_PhysMat.pistol.Ps_Pistol_Blast",
    "Vfx_Weapon.Effects.Ps_SwordSparks_Parry",
    "Vfx_PhysMat.Blood.Pfx_PistolBlood_Blast",
    "Vfx_PhysMat.Blood.Pfx_BloodSProjectile_Default",
    "Vfx_PhysMat.Blood.Pfx_BloodMotion",
    "Vfx_PhysMat.pistol.Ps_PistolImpact_DirtPs_PistolImpact_Dirt",
    "Vfx_PhysMat.pistol.Ps_PistolImpact_MetalPs_PistolImpact_Dirt",
    "Vfx_PhysMat.pistol.Ps_PistolImpact_WoodPs_PistolImpact_Dirt",
    "Vfx_PhysMat.pistol.Ps_PistolImpact_MudPs_PistolImpact_Dirt",
    "Vfx_PhysMat.pistol.Ps_PistolImpact_GravelPs_PistolImpact_Dirt",
    "Vfx_PhysMat.Crossbow.Ps_BowImpact_DirtPs_BowImpact_Dirt",
    "Vfx_PhysMat.Crossbow.Ps_BowImpact_MetalPs_BowImpact_Dirt",
    "Vfx_PhysMat.Crossbow.Ps_BowImpact_WoodPs_BowImpact_Dirt",
    "Vfx_PhysMat.Crossbow.Ps_BowImpact_MudPs_BowImpact_Dirt",
    "Vfx_PhysMat.Crossbow.Ps_BowImpact_GravelPs_BowImpact_Dirt",
    "Vfx_PhysMat.Crossbow.Ps_BowImpact_GrassPs_BowImpact_Dirt",
    "Vfx_PhysMat.Crossbow.Ps_BowImpact_waterPs_BowImpact_Dirt",
    "Vfx_PhysMat.Crossbow.Ps_BowImpact_SleepPs_BowImpact_Dirt",
    "Vfx_Weapon.Effects.FlareBolts.Flare_Bolts_01",
    "Vfx_Weapon.Effects.FlareBolts.Flare_Bolts_02",
    "Vfx_PhysMat.Wood.Pfx_wooden_shard",
    "Vfx_PhysMat.Wood.Ps_Wood_Explo_01",
    "Vfx_PhysMat.Water.Pfx_WaterSplash",
    "Vfx_PhysMat.Water.Ps_Water_Explo_01",
    "Vfx_PhysMat.Water.water_projectile",
    "Vfx_PhysMat.Stone.Ps_Explosion_Stone",
    "Vfx_PhysMat.Dirt.Pfx_dirt_Stone",
    "Vfx_PhysMat.Dirt.Pfx_dirt_gravel",
    "Vfx_PhysMat.Dirt.Ps_Dirt_Explo_Mud_01",
    "Vfx_PhysMat.Dirt.Pfx_SwordImpact_Dirt_Dust",
    "Vfx_PhysMat.Metal.Pfx_SwordImpact_Metal",
    "Vfx_PhysMat.Impact.Impact_generic",
    "Vfx_PhysMat.Impact.Impact_MetalvsMetal",
    // Powers, gadgets and projectiles (tweaks: Possession, Devouring Swarm, Dark Vision
    // cones, grenades, Spring Razor, trails, the crossbow muzzle; most live in Startup).
    "Vfx_GamePlay.Possession.Possession_HandCast_01",
    "Vfx_GamePlay.Possession.PossesTarget_Human_01",
    "Vfx_GamePlay.Possession.PossesTarget_Rats_01",
    "Vfx_GamePlay.RatSwarm.Ps_RSwarm_HandCast_01",
    "Vfx_GamePlay.RatSwarm.RSwarm_Summoning_lvl1",
    "Vfx_GamePlay.RatSwarm.RSwarm_Summoning_lvl2",
    "Vfx_GamePlay.DarkVision.Ps_DVision_VCone_Blue_01",
    "Vfx_GamePlay.DarkVision.Ps_DVision_Sounds_Base01",
    "Vfx_Weapon.Effects.Grenade.ps_grenade",
    "Vfx_Weapon.Effects.Grenade.Ps_Grenade_Trail",
    "Vfx_Weapon.Effects.SpringRazor.Ps_SpringRazor",
    "Vfx_Weapon.Effects.Trails.Ps_Pistol_StdTracer_01",
    "Vfx_Weapon.Effects.Trails.StdBolt_Trail",
    "Vfx_Weapon.Effects.FlareBolts.Flare_Bolt_Trail",
    "Vfx_Weapon.Effects.Ps_CrossBowMuzzle",
    // The left hand's Mark of the Outsider: `AnimNotify_PlayParticleEffect` on the `Tattoo`
    // socket of `Ply_Powers_as` casts, switch and out of mana.
    "Vfx_GamePlay.Powers.Ps_Tattoo_Glow_01",
    "Vfx_GamePlay.Powers.Ps_Tattoo_Glow_02",
    "Vfx_GamePlay.Powers.Ps_Tattoo_Glow_03",
    "Vfx_GamePlay.Powers.Ps_Tattoo_Glow_04",
    "Vfx_GamePlay.Powers.Ps_Tattoo_Glow_WB",
    "Vfx_GamePlay.Powers.Ps_Mana_Empty",
    // The rest of the contact table (footfalls, landings, slides, thrown objects, creatures).
    "Vfx_PhysMat.Dirt.Ps_JumpLand_Big_DirtGravel",
    "Vfx_PhysMat.Dirt.Pfx_Thrown_Rocks",
    "Vfx_PhysMat.FootSteps.Ps_fsteps_Dirt_RunPs_fsteps_Dirt_Run",
    "Vfx_PhysMat.FootSteps.Ps_fsteps_Water_RunPs_fsteps_Dirt_Run",
    "Vfx_PhysMat.FootSteps.Ps_fsteps_Mud_RunPs_fsteps_Dirt_Run",
    "Vfx_PhysMat.FootSteps.Ps_fsteps_Gravel_RunPs_fsteps_Dirt_Run",
    "Vfx_PhysMat.FootSteps.PS_fsteps_Grass_RunPs_fsteps_Dirt_Run",
    "Vfx_PhysMat.FootSteps.fsteps_water_run",
    "Vfx_PhysMat.FootSteps.fsteps_tallboy_stonewet",
    "Vfx_PhysMat.FootSteps.fsteps_tallboy",
    "Vfx_PhysMat.FootSteps.fsteps_mud",
    "Vfx_PhysMat.FootSteps.fsteps_gravel",
    "Vfx_PhysMat.FootSteps.fsteps_grass",
    "Vfx_PhysMat.Slide.PS_Slide_Stone",
    "Vfx_PhysMat.Slide.PS_Slide_Grass",
    "Vfx_PhysMat.Slide.PS_Slide_DeepWater",
    "Vfx_PhysMat.Slide.PS_Slide_StoneWet",
    "Vfx_PhysMat.Slide.PS_Slide_Mud",
    "Vfx_PhysMat.Slide.PS_Slide_Generic",
    "Vfx_PhysMat.Slide.PS_Slide_Dirt",
    "Vfx_GamePlay.RiverKrust.Ps_RiverKrust_Explo",
    "Vfx_GamePlay.RiverKrust.RK_Impact_Default",
    "Vfx_GamePlay.RiverKrust.RK_SwordImpact",
    "vfx_characters.SmallRats.Pfx_SmallRats_Death",
];
