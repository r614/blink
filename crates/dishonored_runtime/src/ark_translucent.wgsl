// Translucent decals and props of `Materials_ref.main_materials.generic_translucent_PMAT`
// (ground puddles, splotches, graffiti, confetti), from the material's compiled shaders in
// RefShaderCache-PC-D3D-SM3.upk (research/dishonored/RENDERING.md "Translucent decals"):
//
//   diffuse = D x [D_Diffuse_color.rgb x a] + Scene(R) x mask[ch] x [C_Cubemap_color.rgb x a]
//             (R = reflect(-V, N) with the normal map; Scene = the level's reflection cube)
//   F_Enable_Fresnel (inverted): f = (1 - saturate(N.V))^F_Fresnel_falloff_exponent,
//             diffuse = lerp(diffuse, F_Fresnel_Color.rgb x a, f), alpha += f
//   colour  = diffuse x (AmbientColorAndSkyFactor + sum of lights saturate(N.L) x light) + E
//   alpha   = clamp(O[ch], O_Minimum_Opacity, O_Maximum_Opacity)  (in the base texture's alpha)
//
// No lightmap (the shader maps have no lightmapped variant for static meshes), no specular, no
// shadows (the light passes use FNoStaticShadowingPolicy). Bevy's lighting gives the direct
// term with specular, environment and ambient light switched off; the base pass's ambient is
// added here.

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    forward_io::{VertexOutput, FragmentOutput},
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
    mesh_view_bindings as view_bindings,
}

struct DecalUniform {
    // rgb: AmbientColorAndSkyFactor (in Bevy light units), w: 1 = scene reflection on.
    ambient: vec4<f32>,
    // rgb: C_Cubemap_color.rgb x a (1 when the switch is off), w: 1 = mask texture on.
    cube: vec4<f32>,
    // One-hot channel of C_Cubemap_Mask_Channel.
    mask: vec4<f32>,
    // rgb: F_Fresnel_Color.rgb x a, w: F_Fresnel_falloff_exponent (0 = off).
    fresnel: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> decal: DecalUniform;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var decal_mask: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var decal_mask_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var decal_scene: texture_cube<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(104) var decal_scene_sampler: sampler;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);
    var out: FragmentOutput;
    if (pbr_input.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) != 0u {
        out.color = main_pass_post_lighting_processing(pbr_input, pbr_input.material.base_color);
        return out;
    }
    let n = normalize(pbr_input.N);
    let v = normalize(pbr_input.V);
    var diffuse = pbr_input.material.base_color.rgb;
    var alpha = pbr_input.material.base_color.a;
    if (decal.ambient.w > 0.0) {
        let r = reflect(-v, n);
        var m = 1.0;
#ifdef VERTEX_UVS_A
        if (decal.cube.w > 0.0) {
            m = dot(textureSample(decal_mask, decal_mask_sampler, in.uv), decal.mask);
        }
#endif
        diffuse = diffuse + textureSample(decal_scene, decal_scene_sampler, r).rgb * m * decal.cube.rgb;
    }
    if (decal.fresnel.w > 0.0) {
        let x = 1.0 - saturate(dot(n, v));
        let f = select(0.0, pow(max(x, 1e-6), decal.fresnel.w), x > 1e-6);
        diffuse = mix(diffuse, decal.fresnel.rgb, f);
        alpha = saturate(alpha + f);
    }
    pbr_input.material.base_color = vec4<f32>(diffuse, alpha);
    // Direct light only: no specular (F0 = 0 also zeroes Bevy's f90), no environment or ambient.
    pbr_input.material.reflectance = vec3<f32>(0.0);
    pbr_input.material.metallic = 0.0;
    pbr_input.diffuse_occlusion = vec3<f32>(0.0);
    pbr_input.specular_occlusion = 0.0;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = vec4<f32>(out.color.rgb + diffuse * decal.ambient.rgb * view_bindings::view.exposure, alpha);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
