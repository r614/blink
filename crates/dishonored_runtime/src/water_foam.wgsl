// The sea of the lighthouse island (`Lighthouse_FX.Materials.water_Foam_MAT`), from the material's
// compiled base passes in RefShaderCache-PC-D3D-SM3.upk (unlit, BLEND_Translucent; research/
// dishonored/RENDERING.md "Island sea"): 37072 (no switch), 37073 / 37074 (`Enable_Foam`, foam
// mask channel G / R), 37075 / 37076 (`Enable_Foam` + `Enable_Rain`).
//
//   P    = the pixel's world position, O = ObjectWorldPositionAndRadius.xyz (the bounds origin)
//   ang  = 2 pi frac(((P.yx - O.yx) x 0.01 + 6 t) x 0.1 + 0.5) - pi;  wavy = (sin ang.x, cos ang.y)
//   n    = 2 Waves((3 W1 u, W1 v) + Wavy_UV wavy + pan1).xy - 1
//          + 2 Waves((3.3 W2 u, 1.1 W2 v) + Wavy_UV wavy + pan2).xy - 1   (+ rain x fade, below)
//          pan1 = frac(W_Waves_Speed t (-0.13, -0.075)), pan2 = frac(W_Waves_Speed t (0.075, -0.13))
//   R    = C_Realtime_reflection_Texture at the screen position + W_Distorsion_intensity n +
//          Wavy_Cube wavy (a planar reflection render target), x CubeMap_intensity
//   m    = F_Foam_Mask(uv0)[F_Foam_Mask_Chanel] (R without Enable_Foam)
//   rgb  = (1 - m) R + Water_color.rgb x a  [+ F_Foam_Texture(F_Foam_Tilling uv0 + F_Foam_Distortion
//          (2 F_Foam_Distortion((FDT u, 0.5 FDT v) + (0, frac(FDS t))).xy - 1) + Wavy_UV wavy +
//          (0, frac(F_Foam_Speed t))).r x Foam_Brightness.rgb x a x m]
//   a    = saturate((scene depth - pixel depth) / Opacity_Depth) x Translucency
//   rain = (W_Waves_normalrain(A).r |sin| + W_Waves_normalrain(B).g |cos|) x Rain_Normal_Intensity,
//          sin / cos of 2 pi Rain_Speed t; A: (sgn(sin) 3 Rt u - 0.5, 3 Rt v - 0.5) turned by
//          2 pi frac(sgn(sin) x 15.915494 + 0.5) - pi, + 0.5; B: (sgn(cos) (3 Rt v + 2.5), 3 Rt u);
//          fade = saturate(1 - pixel depth / 4096), added to both n components
//
// Missing input (U-764, as the river water): the port has no planar reflection capture, so R is
// the level's reflection cube at the camera vector reflected about the flat surface, offset by
// the same distortion terms in tangent space. Without a depth prepass the scene behind is taken
// as far (a = Translucency).

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    mesh_view_bindings as view_bindings,
    view_transformations::position_world_to_view,
}
#ifdef DEPTH_PREPASS
#import bevy_pbr::{prepass_utils, view_transformations::depth_ndc_to_view_z}
#endif

struct WaterFoamUniform {
    water: vec4<f32>,
    foam_color: vec4<f32>,
    waves: vec4<f32>,
    refl: vec4<f32>,
    foam: vec4<f32>,
    foam2: vec4<f32>,
    rain: vec4<f32>,
    mask: vec4<f32>,
    object: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> wf: WaterFoamUniform;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var wf_waves: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var wf_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var wf_rain: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var wf_mask: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(5) var wf_foam_dist: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(6) var wf_foam_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(7) var wf_scene: texture_cube<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(8) var wf_scene_sampler: sampler;

// Bevy units per Unreal unit (coords::UU_TO_BEVY).
const UU_TO_BEVY: f32 = 0.02;
const PI: f32 = 3.14159274;

fn sgn(x: f32) -> f32 {
    return select(select(0.0, -1.0, x < 0.0), 1.0, x > 0.0);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var uv = vec2<f32>(0.0);
#ifdef VERTEX_UVS_A
    uv = in.uv;
#endif
    let t = view_bindings::globals.time;
    // UE3 world position (Bevy (x, y, z) = UE (x, z, y) x UU_TO_BEVY).
    let p = vec3<f32>(in.world_position.x, in.world_position.z, in.world_position.y) / UU_TO_BEVY;
    let dxy = vec2<f32>(p.y - wf.object.y, p.x - wf.object.x);
    let ang = 2.0 * PI * fract((dxy * 0.01 + 6.0 * t) * 0.1 + 0.5) - PI;
    let wavy = vec2<f32>(sin(ang.x), cos(ang.y));
    let w = wavy * wf.waves.w;
    let pan1 = fract(wf.waves.x * t * vec2<f32>(-0.13, -0.075));
    let pan2 = fract(wf.waves.x * t * vec2<f32>(0.075, -0.13));
    let n1 = textureSample(wf_waves, wf_sampler, vec2<f32>(3.0 * wf.waves.y * uv.x, wf.waves.y * uv.y) + w + pan1).xy * 2.0 - 1.0;
    let n2 = textureSample(wf_waves, wf_sampler, vec2<f32>(3.3 * wf.waves.z * uv.x, 1.1 * wf.waves.z * uv.y) + w + pan2).xy * 2.0;
    let pixel_z = -position_world_to_view(in.world_position.xyz).z / UU_TO_BEVY;
    var rain = 0.0;
    if (wf.foam2.z > 0.0) {
        let ph = 2.0 * PI * wf.rain.x * t;
        let s = sin(ph);
        let c = cos(ph);
        let ss = sgn(s);
        let sc = sgn(c);
        let th = 2.0 * PI * fract(ss * 15.915494 + 0.5) - PI;
        let q = vec2<f32>(ss * 3.0 * wf.rain.y * uv.x - 0.5, 3.0 * wf.rain.y * uv.y - 0.5);
        let uv_a = vec2<f32>(dot(vec2<f32>(cos(th), -sin(th)), q), dot(vec2<f32>(sin(th), cos(th)), q)) + 0.5;
        let uv_b = vec2<f32>(sc * 3.0 * wf.rain.y * uv.y + 2.5 * sc, 3.0 * wf.rain.y * uv.x);
        let r1 = textureSample(wf_rain, wf_sampler, uv_a).r;
        let r2 = textureSample(wf_rain, wf_sampler, uv_b).g;
        rain = (r1 * abs(s) + r2 * abs(c)) * wf.rain.z * saturate(1.0 - pixel_z / 4096.0);
    }
    let n = n1 + n2 + vec2<f32>(rain) - 1.0;
    // Reflection (substitute for the planar reflection render target, see above).
    let ng = normalize(in.world_normal);
    var tg = vec3<f32>(1.0, 0.0, 0.0);
    var bt = vec3<f32>(0.0, 0.0, 1.0);
#ifdef VERTEX_TANGENTS
    tg = normalize(in.world_tangent.xyz - ng * dot(ng, in.world_tangent.xyz));
    bt = cross(ng, tg) * in.world_tangent.w;
#endif
    let v = normalize(view_bindings::view.world_position.xyz - in.world_position.xyz);
    let v_ts = vec3<f32>(dot(v, tg), -dot(v, bt), dot(v, ng));
    let off = wf.refl.x * n + wf.refl.y * wavy;
    let r_ts = vec3<f32>(-v_ts.x + off.x, -v_ts.y + off.y, v_ts.z);
    let r_ws = r_ts.x * tg - r_ts.y * bt + r_ts.z * ng;
    let refl = textureSample(wf_scene, wf_scene_sampler, r_ws).rgb * wf.refl.z;
    let m = dot(textureSample(wf_mask, wf_sampler, uv), wf.mask);
    var rgb = (1.0 - m) * refl + wf.water.rgb;
    if (wf.foam_color.w > 0.0) {
        let fd = textureSample(wf_foam_dist, wf_sampler, vec2<f32>(wf.foam.x * uv.x, 0.5 * wf.foam.x * uv.y) + vec2<f32>(0.0, fract(wf.foam2.x * t))).xy * 2.0 - 1.0;
        let fuv = wf.foam.z * uv + wf.foam.y * fd + wf.waves.w * wavy + vec2<f32>(0.0, fract(wf.foam.w * t));
        let foam = textureSample(wf_foam_tex, wf_sampler, fuv).r;
        rgb = rgb + foam * wf.foam_color.rgb * m;
    }
    var scene_z = 1.0e9;
#ifdef DEPTH_PREPASS
    let ndc = prepass_utils::prepass_depth(in.position, 0u);
    if (ndc > 0.0) {
        scene_z = -depth_ndc_to_view_z(ndc) / UU_TO_BEVY;
    }
#endif
    let a = saturate((scene_z - pixel_z) / max(wf.refl.w, 1e-6)) * wf.foam2.y;
    var out: FragmentOutput;
    out.color = vec4<f32>(rgb * wf.water.w * view_bindings::view.exposure, a);
    return out;
}
