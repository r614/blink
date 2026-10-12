// Soft (depth-biased alpha) particles: the `Db_Enable_Depth_Biased_Alpha` permutations of the
// Arkane particle bases (DefaultAdditive_PMAT / _2Sided / _OutBendTime, DefaultTranslu_PMAT /
// Translu2sided / TransluFOG, Glow_PMAT, LensFlare_PMAT, WindSwirls_01_PMAT, ...), from their
// compiled base passes (RefShaderCache-PC-D3D-SM3.upk; e.g. 36033 / 36034 / 35519 / 35911,
// research/dishonored/systems/render_depth.md). The part of each shader that differs from the
// switch-off permutation is
//
//   Db = saturate((SceneColor(screen).a x 256 - view depth) x k),
//   k  = 1 / ((1 - Db_Depth_Biased_Alpha_Intensity) x 250), or 1000 when that product is below 0.001
//
// and the output alpha (additive: the colour's scale, as BLEND_Additive writes alpha 0) is
// multiplied by Db. Depths are Unreal units (the shader's x 256 of the scene colour alpha is the
// D3D9 depth channel); the scene depth comes from the depth prepass (DEPTH_PREPASS) and is taken
// as far without one. The rest of the shading is Bevy's unlit StandardMaterial of the sprite.

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
    forward_io::{VertexOutput, FragmentOutput},
    view_transformations::position_world_to_view,
}
#ifdef DEPTH_PREPASS
#import bevy_pbr::{prepass_utils, view_transformations::depth_ndc_to_view_z}
#endif

// x: k (0 = the material has no depth-biased alpha); y: the height-map erosion's multiplier
// (HeightMapAnim / SubUV_HeightMap / WaterSplash bases: alpha = saturate((H - vertex alpha) x
// Mask_Multiply), 0 = none), or -1 for the look-up smoke's erosion (Look_Up_Smoke_translu_PMAT,
// 36275 / 37106: alpha = smoothstep(saturate((H - (1 - vertex alpha)) / vertex alpha)) x vertex
// alpha x z, z = diffuse_color2.a); w = 1 drops the vertex alpha factor (test_look_up, 36313 /
// 36315: the additive glow is smoothstep(...) x vertex red x its lookup colour, nothing else).
// Eroding sprites keep their vertex alpha at 1 (the material alpha is the baked height H) and
// carry the particle's alpha in the second UV set.
@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> soft: vec4<f32>;

// Bevy units per Unreal unit (coords::UU_TO_BEVY).
const UU_TO_BEVY: f32 = 0.02;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
#ifdef VERTEX_UVS_B
    let va = in.uv_b.x;
#else
    let va = 0.0;
#endif
    if (soft.y > 0.0) {
        // Before the mask's discard (the masked SubUV_HeightMap clips below 1/3).
        pbr_input.material.base_color.a = saturate((pbr_input.material.base_color.a - va) * soft.y);
    } else if (soft.y < 0.0) {
        let t = saturate((pbr_input.material.base_color.a - (1.0 - va)) / max(va, 1.0e-6));
        pbr_input.material.base_color.a = t * t * (3.0 - 2.0 * t) * select(va, 1.0, soft.w > 0.0) * soft.z;
    }
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);
    if (soft.x > 0.0) {
        var scene_z = 1.0e9;
#ifdef DEPTH_PREPASS
        let ndc = prepass_utils::prepass_depth(in.position, 0u);
        if (ndc > 0.0) {
            scene_z = -depth_ndc_to_view_z(ndc) / UU_TO_BEVY;
        }
#endif
        let pixel_z = -position_world_to_view(in.world_position.xyz).z / UU_TO_BEVY;
        pbr_input.material.base_color.a = pbr_input.material.base_color.a * saturate((scene_z - pixel_z) * soft.x);
    }
    var out: FragmentOutput;
    if ((pbr_input.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u) {
        out.color = apply_pbr_lighting(pbr_input);
    } else {
        out.color = pbr_input.material.base_color;
    }
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
