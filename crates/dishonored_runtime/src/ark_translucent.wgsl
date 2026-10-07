// Translucent decals and props of `Materials_ref.main_materials.generic_translucent_PMAT`
// (ground puddles, splotches, graffiti, confetti), from the material's compiled shaders in
// RefShaderCache-PC-D3D-SM3.upk (research/dishonored/RENDERING.md "Translucent decals"):
//
//   diffuse = D x [D_Diffuse_color.rgb x a] + Scene(R) x mask[ch] x [C_Cubemap_color.rgb x a]
//             (R = reflect(-V, N) with the normal map; Scene = the level's reflection cube)
//   F_Enable_Fresnel (inverted): f = (1 - saturate(N.V))^F_Fresnel_falloff_exponent,
//             diffuse = lerp(diffuse, F_Fresnel_Color.rgb x a, f), alpha += f
//   base    = diffuse x (lightmap colour x sum_i dir_i max(saturate(n.B_i)^2, 1e-6)
//             + AmbientColorAndSkyFactor) + E   (FDirectionalLightMapTexturePolicy, e.g. 9347;
//             n = the normal map's tangent-space normal, B_i the HL2 basis; the lightmap specular
//             is multiplied by SpecularOverrideParameter.xyz = 0)
//   lights  = diffuse x saturate(N.L) x light for the dominant / toggleable lights, the dominant
//             directional light shadowed by the component's distance field
//   alpha   = clamp(O[ch], O_Minimum_Opacity, O_Maximum_Opacity)  (in the base texture's alpha)
//
// Bevy's lighting gives the direct term with specular, environment and ambient light switched
// off; the lightmap and the base pass's ambient are added here.

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    pbr_functions::{calculate_diffuse_color, calculate_F0_dielectric},
    forward_io::{VertexOutput, FragmentOutput},
    pbr_types::{STANDARD_MATERIAL_FLAGS_UNLIT_BIT, PbrInput},
    mesh_view_bindings as view_bindings,
    lighting,
    lighting::LAYER_BASE,
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

struct DecalLightmap {
    // x: exposure (UE3 lightmap unit to Bevy), y: 1 = lightmapped, z: 1 = distance-field shadow.
    params: vec4<f32>,
    // (uv_min, uv_max - uv_min) of the component's atlas region.
    rect: vec4<f32>,
    // Distance-field shadow (bias, scale, exponent, 0).
    df: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> decal: DecalUniform;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var decal_mask: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var decal_mask_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var decal_scene: texture_cube<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(104) var decal_scene_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(105) var<uniform> decal_lm: DecalLightmap;
@group(#{MATERIAL_BIND_GROUP}) @binding(106) var decal_lightmap: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(107) var decal_lightmap_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(108) var decal_directional: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(109) var decal_shadow: texture_2d<f32>;

const B0 = vec3<f32>(0.0, 0.81649661, 0.57735026);
const B1 = vec3<f32>(-0.70710677, -0.40824830, 0.57735026);
const B2 = vec3<f32>(0.70710677, -0.40824830, 0.57735026);

// The dominant directional light (index 0) as Bevy's PBR applies it, unshadowed.
fn dominant_light(pbr_input: PbrInput) -> vec3<f32> {
    let m = pbr_input.material;
    var li: lighting::LightingInput;
    li.layers[LAYER_BASE].NdotV = max(dot(pbr_input.N, pbr_input.V), 0.0001);
    li.layers[LAYER_BASE].N = pbr_input.N;
    li.layers[LAYER_BASE].R = reflect(-pbr_input.V, pbr_input.N);
    li.layers[LAYER_BASE].perceptual_roughness = m.perceptual_roughness;
    li.layers[LAYER_BASE].roughness = lighting::perceptualRoughnessToRoughness(m.perceptual_roughness);
    li.P = pbr_input.world_position.xyz;
    li.V = pbr_input.V;
    li.diffuse_color = calculate_diffuse_color(m.base_color.rgb, m.metallic, m.specular_transmission, m.diffuse_transmission);
    li.metallic = m.metallic;
    li.F0_dielectric = calculate_F0_dielectric(m.reflectance);
    li.F0_metallic = m.base_color.rgb;
    li.F_ab = lighting::F_AB(m.perceptual_roughness, li.layers[LAYER_BASE].NdotV);
    return lighting::directional_light(0u, &li, true);
}

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
    // The base pass's directional lightmap (in Bevy's light unit, before the view exposure).
    var lightmap = vec3<f32>(0.0);
    var shadow = 1.0;
#ifdef VERTEX_UVS_A
#ifdef VERTEX_UVS_B
    if (decal_lm.params.y > 0.0) {
        let luv = decal_lm.rect.xy + in.uv_b * decal_lm.rect.zw;
        // Mip 0 only: lightmap atlases bleed across charts when mipmapped.
        let color = textureSampleLevel(decal_lightmap, decal_lightmap_sampler, luv, 0.0).rgb * decal_lm.params.x;
        let dir = textureSampleLevel(decal_directional, decal_lightmap_sampler, luv, 0.0).rgb;
        // The normal in UE3 tangent space (X = dP/du, Y = dP/dv, Z = normal), from the geometric
        // normal and the UV derivatives as in lightmap_ext.wgsl.
        let ng = normalize(pbr_input.world_normal);
        let dp1 = dpdx(in.world_position.xyz);
        let dp2 = dpdy(in.world_position.xyz);
        let duv1 = dpdx(in.uv);
        let duv2 = dpdy(in.uv);
        let dp2perp = cross(dp2, ng);
        let dp1perp = cross(ng, dp1);
        var t = dp2perp * duv1.x + dp1perp * duv2.x;
        var b = dp2perp * duv1.y + dp1perp * duv2.y;
        t = t - ng * dot(ng, t);
        b = b - ng * dot(ng, b);
        var n_ts = vec3<f32>(0.0, 0.0, 1.0);
        if (length(t) > 1e-12 && length(b) > 1e-12) {
            n_ts = normalize(vec3<f32>(dot(n, normalize(t)), dot(n, normalize(b)), dot(n, ng)));
        }
        let w = vec3<f32>(saturate(dot(n_ts, B0)), saturate(dot(n_ts, B1)), saturate(dot(n_ts, B2)));
        lightmap = color * dot(dir, max(w * w, vec3<f32>(1e-6)));
        if (decal_lm.params.z > 0.0) {
            let d = textureSampleLevel(decal_shadow, decal_lightmap_sampler, luv, 0.0).r;
            let s = saturate((d + decal_lm.df.x) * decal_lm.df.y);
            shadow = select(0.0, pow(s, decal_lm.df.z), s - 1e-6 >= 0.0);
        }
    }
#endif
#endif
    pbr_input.material.base_color = vec4<f32>(diffuse, alpha);
    // Direct light only: no specular (F0 = 0 also zeroes Bevy's f90), no environment or ambient.
    pbr_input.material.reflectance = vec3<f32>(0.0);
    pbr_input.material.metallic = 0.0;
    pbr_input.diffuse_occlusion = vec3<f32>(0.0);
    pbr_input.specular_occlusion = 0.0;
    out.color = apply_pbr_lighting(pbr_input);
    // Decals do not receive Bevy's shadows: the dominant directional light takes the component's
    // distance field instead.
    if (shadow < 1.0 && view_bindings::lights.n_directional_lights > 0u) {
        out.color = vec4<f32>(max(out.color.rgb + (shadow - 1.0) * dominant_light(pbr_input) * view_bindings::view.exposure, vec3<f32>(0.0)), out.color.a);
    }
    out.color = vec4<f32>(out.color.rgb + diffuse * (lightmap + decal.ambient.rgb) * view_bindings::view.exposure, alpha);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
