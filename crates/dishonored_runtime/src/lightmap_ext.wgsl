// Standard PBR forward fragment plus a baked lightmap shared by every batch of a material:
// UV1 already holds atlas coordinates (the instance's lightmap region is baked into the
// merged vertices), so no per-entity lightmap slot is needed. The sample feeds
// `pbr_input.lightmap_light`, the same input Bevy's own lightmap path fills.

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    forward_io::{VertexOutput, FragmentOutput},
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> baked_params: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var baked_lightmap: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var baked_sampler: sampler;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);
#ifdef VERTEX_UVS_B
    // Mip 0 only: lightmap atlases bleed across charts when mipmapped.
    pbr_input.lightmap_light = textureSampleLevel(baked_lightmap, baked_sampler, in.uv_b, 0.0).rgb * baked_params.x;
#endif
    var out: FragmentOutput;
    if (pbr_input.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        out.color = apply_pbr_lighting(pbr_input);
    } else {
        out.color = pbr_input.material.base_color;
    }
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
