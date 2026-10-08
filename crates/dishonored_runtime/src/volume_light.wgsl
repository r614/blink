// Light beams of `Vfx_Materials.main_materials.Volume_Light_PMAT` (window shafts, lamp cones, the
// outro's god rays), from the material's compiled base passes in RefShaderCache-PC-D3D-SM3.upk
// (FLocalVertexFactory, unlit, additive; research/dishonored/RENDERING.md "Light beams"):
//
//   6627 (no switch)            a = S x F x M.a x Db x Cf x M.a
//   6629 (Disable_CameraFade)   a = S x F x M.a x Db x M.a
//   6631 (Use_Vertex_Color)     a = F x V.r x M.a x Db x Cf x M.a           (0 when |N.V| < 1e-6)
//   6633 (+ Use_rays)           a = R x V.r x F x M.a x Db x Cf x M.a       (0 when |N.V| < 1e-6)
//   colour = (lerp(Color, ColorAlert, Alert) x LightIntensity x M.rgb + SelectionColor) x a,
//   written with alpha 0 (BLEND_Additive)
//
//   S  = m^SphereMask_Power, m = max(1 - 2 |uv0 - 0.5|, 0) (0 when m < 1e-6)
//   F  = |N.V|^Fresnel_exponent (the tangent-space camera vector's z; 0 below 1e-6)
//   V  = the mesh's vertex colour, M = MeshEmitterVertexColor ((1,1,1,1) on static meshes, the
//        particle colour on mesh emitters)
//   Db = saturate((scene depth - pixel depth) x k), k = 1 / ((1 - Db_Depth_Biased_Alpha_Intensity)
//        x 50), or 1000 when that is below 0.001
//   Cf = saturate((pixel depth - StartOffset) x 0.0005 / Distance)
//   R  = Rays(uv0 + (frac(0.5 Ray_Speed t), 0)).r + Rays(uv0 x (-1.5, 1) + (frac(0.25 Ray_Speed t), 0)).r
//
// Depths are view depths in Unreal units. The scene depth comes from the depth prepass when the
// view has one (DEPTH_PREPASS); without it the scene behind is taken as far (Db = 1).

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    mesh_view_bindings as view_bindings,
    view_transformations::position_world_to_view,
}
#ifdef DEPTH_PREPASS
#import bevy_pbr::{prepass_utils, view_transformations::depth_ndc_to_view_z}
#endif

struct VolumeLightUniform {
    color: vec4<f32>,
    mesh_color: vec4<f32>,
    terms: vec4<f32>,
    flags: vec4<f32>,
    sides: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> vl: VolumeLightUniform;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var vl_rays: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var vl_sampler: sampler;

// Bevy units per Unreal unit (coords::UU_TO_BEVY).
const UU_TO_BEVY: f32 = 0.02;

// pow with the shaders' guard: 0 when the base is below 1e-6.
fn guarded_pow(x: f32, e: f32) -> f32 {
    return select(0.0, pow(max(x, 1e-6), e), x - 1e-6 >= 0.0);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var uv = vec2<f32>(0.0);
#ifdef VERTEX_UVS_A
    uv = in.uv;
#endif
    var vc = vec4<f32>(1.0);
#ifdef VERTEX_COLORS
    vc = in.color;
#endif
    let n = normalize(in.world_normal);
    let v = normalize(view_bindings::view.world_position.xyz - in.world_position.xyz);
    let ndv = abs(dot(n, v));
    let f = guarded_pow(ndv, vl.terms.x);
    let m_a = vl.mesh_color.a;
    // Mask: the vertex colour's red (Use_Vertex_Color, x the rays with Use_rays) or the UV sphere mask.
    var mask = 0.0;
    if (vl.flags.y > 0.0) {
        mask = vc.r;
        if (vl.flags.z > 0.0) {
            let s = vl.flags.w * view_bindings::globals.time;
            let r1 = textureSample(vl_rays, vl_sampler, uv + vec2<f32>(fract(0.5 * s), 0.0)).r;
            let r2 = textureSample(vl_rays, vl_sampler, uv * vec2<f32>(-1.5, 1.0) + vec2<f32>(fract(0.25 * s), 0.0)).r;
            mask = mask * (r1 + r2);
        }
        if (ndv - 1e-6 < 0.0) {
            mask = 0.0;
        }
    } else {
        let d = length(uv - vec2<f32>(0.5));
        mask = guarded_pow(max(1.0 - 2.0 * d, 0.0), vl.terms.y);
    }
    // Pixel and scene view depths (Unreal units).
    let pixel_z = -position_world_to_view(in.world_position.xyz).z / UU_TO_BEVY;
    var scene_z = 1.0e9;
#ifdef DEPTH_PREPASS
    let ndc = prepass_utils::prepass_depth(in.position, 0u);
    if (ndc > 0.0) {
        scene_z = -depth_ndc_to_view_z(ndc) / UU_TO_BEVY;
    }
#endif
    let db = saturate((scene_z - pixel_z) * vl.terms.z);
    // Camera fade (flags.x = 0.0005 / Distance; < 0 with Disable_CameraFade).
    var cf = 1.0;
    if (vl.flags.x >= 0.0) {
        cf = saturate((pixel_z - vl.terms.w) * vl.flags.x);
    }
    var a = mask * f * m_a * db * cf * m_a;
    // Single-sided materials: UE3 culls the back faces.
    if (vl.sides.x <= 0.0 && !is_front) {
        a = 0.0;
    }
    var out: FragmentOutput;
    // Additive: Bevy blends AlphaMode::Add as premultiplied alpha, so alpha 0 adds the colour.
    // color.w converts UE3 radiance to Bevy light units (map_render::light_unit), then the view's
    // exposure applies as to the lightmaps.
    let c = vl.color.rgb * vl.mesh_color.rgb * a * vl.color.w * view_bindings::view.exposure;
    out.color = vec4<f32>(c, 0.0);
    return out;
}
