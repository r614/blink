// Water of `Materials_ref.water_materials.river_main_PMAT` (the Boyle river, canals, the
// Playground's pool), from the material's compiled base pass in RefShaderCache-PC-D3D-SM3.upk
// (11099 / 11093 for Water_boyle_ext_INST03, 10608 for Water_boyle_ext_under_01_INST;
// research/dishonored/RENDERING.md "Water (river_main_PMAT)"):
//
//   waves  n = N(uv1) + N(uv2), N = 2 x W_Waves_normalmap - 1; uv_i = rot_i(tiling_i uv - 0.5)
//          + (frac(speed_i t k_i.x), frac(speed_i t k_i.y)) + 0.5, k_1 = (-0.13, 0.075),
//          k_2 = (0.075, -0.13), rot_i by W*_orientation x -0.0175 rad
//   R      reflection x C_Cubemap_color.rgb x a: the cubemap at R_ts + W1_Distorsion1_intensity1 x n
//          (R_ts: the camera vector reflected about the tangent-space normal, taken to world space;
//          10608). With C_Enable_Realtime_Reflection the original samples the planar reflection
//          render target (`boyle_ext_MS.RealTime_refl`, drawn by SceneCaptureReflectActor_0) at the
//          screen position + intensity x n.xy instead (11099): the port has no planar reflection
//          capture, so those instances use the cubemap term too (U-764).
//   diff   D_diffuse_texture at D_Diffuse_Tilling uv + o, mask = D_diffuse_mask[ch] at
//          D_Diffuse_Mask_Tilling uv + o, o = n.xy x intensity x D_diffuse_Distorsion_strength
//          (D_Enable_Diffuse_Distorsion); m = mask with D_Enable_Diffuse_on_top, else 0
//   D      = lerp(R, diff, m) + W_Water_Color                       (lit: lightmap, lights)
//   E      = lerp(R (1 - m), F_Fog_color, f), f = clamp(depth / F_Fog_StartDistance, 0,
//            F_Fog_opacity)^F_Fog_Attenuation (0 at 0; F_Enable_Fake_Fog), depth = the scene depth
//            behind the surface                                    (unlit)
//   colour = E + D x (lightmap + AmbientColorAndSkyFactor) + lights x D
//   alpha  = saturate(lerp(O_Depth_min, O_Depth_max, saturate(t / O_Translucent_Depth))
//            x saturate(2 t) + O_Opacity_Intensity), t = the water's thickness (scene depth -
//            the surface's depth)
//
// The scene depth comes from the depth prepass when the view has one (DEPTH_PREPASS); without
// it the scene behind is taken as far: alpha = saturate(O_Depth_max + O_Opacity_Intensity) and
// f = F_Fog_opacity^F_Fog_Attenuation (U-764).

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
    forward_io::{VertexOutput, FragmentOutput},
    mesh_view_bindings as view_bindings,
    view_transformations::position_world_to_view,
}
#ifdef DEPTH_PREPASS
#import bevy_pbr::{prepass_utils, view_transformations::depth_ndc_to_view_z}
#endif

struct WaterUniform {
    w1: vec4<f32>,
    w2: vec4<f32>,
    dist: vec4<f32>,
    fog: vec4<f32>,
    opacity: vec4<f32>,
    cube: vec4<f32>,
    fog_color: vec4<f32>,
    water_color: vec4<f32>,
    mask: vec4<f32>,
    ambient: vec4<f32>,
}

struct DecalLightmap {
    params: vec4<f32>,
    rect: vec4<f32>,
    df: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> water: WaterUniform;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var water_waves: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var water_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var water_mask: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(104) var water_diffuse: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(105) var water_scene: texture_cube<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(106) var water_scene_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(107) var<uniform> water_lm: DecalLightmap;
@group(#{MATERIAL_BIND_GROUP}) @binding(108) var water_lightmap: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(109) var water_lightmap_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(110) var water_directional: texture_2d<f32>;

// Bevy units per Unreal unit (coords::UU_TO_BEVY).
const UU_TO_BEVY: f32 = 0.02;

// One wave layer's UV: rotate (tiling x uv - 0.5), pan, + 0.5.
fn wave_uv(uv: vec2<f32>, w: vec4<f32>, k: vec2<f32>) -> vec2<f32> {
    let p = w.x * uv - vec2<f32>(0.5);
    let c = cos(w.y);
    let s = sin(w.y);
    let r = vec2<f32>(dot(vec2<f32>(c, -s), p), dot(vec2<f32>(s, c), p));
    let t = view_bindings::globals.time;
    return r + fract(w.z * t * k) + vec2<f32>(0.5);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    var out: FragmentOutput;
    let ng = normalize(pbr_input.world_normal);
    let v = normalize(pbr_input.V);
    var uv = vec2<f32>(0.0);
#ifdef VERTEX_UVS_A
    uv = in.uv;
#endif
    // Waves (Texture2D_0): n = N1 + N2, each 2 x texel - 1.
    let n1 = textureSample(water_waves, water_sampler, wave_uv(uv, water.w1, vec2<f32>(-0.13, 0.075))).xyz * 2.0 - 1.0;
    let n2 = textureSample(water_waves, water_sampler, wave_uv(uv, water.w2, vec2<f32>(0.075, -0.13))).xyz * 2.0 - 1.0;
    let n = n1 + n2;
    // Tangent frame: UE3's tangent space (X = dP/du, Y = dP/dv, Z = normal); the normal map is
    // DirectX-style, so its green runs against Bevy's bitangent (as `flip_normal_map_y`).
    var t = vec3<f32>(1.0, 0.0, 0.0);
    var b = vec3<f32>(0.0, 0.0, 1.0);
#ifdef VERTEX_TANGENTS
    t = normalize(in.world_tangent.xyz - ng * dot(ng, in.world_tangent.xyz));
    b = cross(ng, t) * in.world_tangent.w;
#endif
    // Reflection: the camera vector reflected about the tangent-space normal, offset by the
    // waves, back to world space (10608).
    let v_ts = vec3<f32>(dot(v, t), -dot(v, b), dot(v, ng));
    let r_ts = vec3<f32>(-v_ts.x, -v_ts.y, v_ts.z) + water.dist.x * n;
    let r_ws = r_ts.x * t - r_ts.y * b + r_ts.z * ng;
    let refl = textureSample(water_scene, water_scene_sampler, r_ws).rgb * water.cube.rgb;
    // Diffuse on top, through its mask.
    let o = select(vec2<f32>(0.0), n.xy * water.dist.x * water.dist.y, water.water_color.w > 0.0);
    let diff = textureSample(water_diffuse, water_sampler, water.dist.z * uv + o).rgb;
    let m = select(0.0, dot(textureSample(water_mask, water_sampler, water.dist.w * uv + o), water.mask), water.cube.w > 0.0);
    let d = mix(refl, diff, m) + water.water_color.rgb;
    // Scene depth behind the surface and the water's thickness (Unreal units).
    let surface_z = -position_world_to_view(in.world_position.xyz).z / UU_TO_BEVY;
    var scene_z = 1.0e9;
#ifdef DEPTH_PREPASS
    let ndc = prepass_utils::prepass_depth(in.position, 0u);
    if (ndc > 0.0) {
        scene_z = -depth_ndc_to_view_z(ndc) / UU_TO_BEVY;
    }
#endif
    let thick = max(scene_z - surface_z, 0.0);
    var e = refl * (1.0 - m);
    if (water.fog.w > 0.0) {
        let x = clamp(scene_z / water.fog.x, 0.0, water.fog.y);
        let f = select(0.0, pow(x, water.fog.z), x - 1e-6 >= 0.0);
        e = mix(e, water.fog_color.rgb, f);
    }
    let a0 = saturate(thick / water.opacity.x);
    let alpha = saturate(mix(water.opacity.z, water.opacity.y, a0) * saturate(2.0 * thick) + water.opacity.w);
    // The base pass's directional lightmap with the flat normal (the material outputs none).
    var lightmap = vec3<f32>(0.0);
#ifdef VERTEX_UVS_B
    if (water_lm.params.y > 0.0) {
        let luv = water_lm.rect.xy + in.uv_b * water_lm.rect.zw;
        let color = textureSampleLevel(water_lightmap, water_lightmap_sampler, luv, 0.0).rgb * water_lm.params.x;
        let dir = textureSampleLevel(water_directional, water_lightmap_sampler, luv, 0.0).rgb;
        lightmap = color * dot(dir, vec3<f32>(1.0 / 3.0));
    }
#endif
    // The light passes light D with the geometric normal; no specular, environment or ambient.
    pbr_input.material.base_color = vec4<f32>(d, alpha);
    pbr_input.N = ng;
    pbr_input.material.reflectance = vec3<f32>(0.0);
    pbr_input.material.metallic = 0.0;
    pbr_input.diffuse_occlusion = vec3<f32>(0.0);
    pbr_input.specular_occlusion = 0.0;
    out.color = apply_pbr_lighting(pbr_input);
    let ex = view_bindings::view.exposure;
    out.color = vec4<f32>(out.color.rgb + (d * (lightmap + water.ambient.rgb) + e * water.ambient.w) * ex, alpha);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
