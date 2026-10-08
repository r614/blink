// Lens flare quads (lens_flare.rs): `FLensFlareVertexFactory`'s vertex shader (RefShaderCache shader
// 1332) and the base passes of `Materials_ref.FX_materials.LensFlare_PMAT` (additive, unlit):
//
//   2643 (no switch)                 c = rf x V.rgb x m^Power x LFColor.rgb x LFColor.w
//   2645 (Lg_Enable_Glowing_LensFlare)  c x |Range sin(2 pi t Speed) + Visibility|
//   2649 (L_Enable_LensFlare_Mask)   c = rf x V.rgb x Mask.r x LFColor.rgb x LFColor.w
//   2651 (+ Du_Enable_Dust_panner)   alpha = DustIntensity x Dust(uv x Tiling + frac(Speed t (1, 0.5))).r + V.a
//   2647 (+ Db_Enable_Depth_Biased_Alpha) alpha x saturate((scene depth - pixel depth) k)
//   colour = min(max(c, Minimum_Opacity), Maximum_Opacity) x alpha, written with alpha 0
//
//   m  = max(1 - 2 |uv - 0.5|, 0); the power variants are 0 where m < 1e-6
//   rf = 1 - radial distance factor x source distance (0 for a source element: 1)
//   V  = the vertex colour: Color x DistMap_Color (rgb), Alpha x DistMap_Alpha (a)
//
// Vertex shader 1332: the quad is centred on the source and spans (uv - 0.5) x size along the
// camera's right and up, turned by the element's rotation:
//   a = cos Right - sin Up, b = cos Up + sin Right, p = centre + x a + y b
//
// Depths are view depths in Unreal units. The scene depth comes from the depth prepass when the
// view has one (DEPTH_PREPASS); without it the scene behind is taken as far (fade 1).

#import bevy_pbr::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput, FragmentOutput},
    mesh_view_bindings as view_bindings,
    view_transformations::{position_world_to_clip, position_world_to_view, direction_view_to_world},
}
#ifdef DEPTH_PREPASS
#import bevy_pbr::{prepass_utils, view_transformations::depth_ndc_to_view_z}
#endif

struct LensFlare {
    terms: vec4<f32>,
    glow: vec4<f32>,
    dust: vec4<f32>,
    color: vec4<f32>,
    quad: vec4<f32>,
    base: vec4<f32>,
    dm_scale: array<vec4<f32>, 4>,
    dm_color: array<vec4<f32>, 4>,
    dm_alpha: array<vec4<f32>, 4>,
    dm_meta: vec4<f32>,
    dm_time: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> lf: LensFlare;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var lf_mask: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var lf_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var lf_dust: texture_2d<f32>;

// Bevy units per Unreal unit (coords::UU_TO_BEVY).
const UU_TO_BEVY: f32 = 0.02;
const TAU: f32 = 6.2831855;

// A cooked distribution's samples, linear between them: index = t x time scale (a time scale of 0
// is the first sample, a constant). Counts of 0 read as 1.
fn lookup(samples: array<vec4<f32>, 4>, count: f32, time_scale: f32, t: f32) -> vec4<f32> {
    var s = samples;
    let n = i32(count);
    if (n <= 0) {
        return vec4<f32>(1.0);
    }
    let x = max(t * time_scale, 0.0);
    let i = min(i32(floor(x)), n - 1);
    let j = min(i + 1, n - 1);
    var a = 0.0;
    if (i != j) {
        a = x - f32(i);
    }
    return mix(s[i], s[j], a);
}

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    let centre = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(0.0, 0.0, 0.0, 1.0)).xyz;
    // Camera distance to the source in uu: the `DistMap_` distributions' lookup.
    let d = distance(view_bindings::view.world_position.xyz, centre) / UU_TO_BEVY;
    let scale = lookup(lf.dm_scale, lf.dm_meta.x, lf.dm_time.x, d);
    var uv = vec2<f32>(0.0);
#ifdef VERTEX_UVS_A
    uv = vertex.uv;
#endif
    let half_size = (uv - vec2<f32>(0.5)) * vec2<f32>(lf.quad.x * scale.x, lf.quad.y * scale.y) * UU_TO_BEVY;
    let right = direction_view_to_world(vec3<f32>(1.0, 0.0, 0.0));
    let up = direction_view_to_world(vec3<f32>(0.0, 1.0, 0.0));
    let s = sin(lf.quad.z);
    let c = cos(lf.quad.z);
    let a = c * right - s * up;
    let b = c * up + s * right;
    let world = centre + half_size.x * a + half_size.y * b;
    out.world_position = vec4<f32>(world, 1.0);
    out.position = position_world_to_clip(world);
    out.world_normal = vec3<f32>(0.0, 1.0, 0.0);
#ifdef VERTEX_UVS_A
    out.uv = uv;
#endif
#ifdef VERTEX_COLORS
    let dm_c = lookup(lf.dm_color, lf.dm_meta.y, lf.dm_time.y, d);
    let dm_a = lookup(lf.dm_alpha, lf.dm_meta.z, lf.dm_time.z, d);
    out.color = vec4<f32>(lf.base.rgb * dm_c.rgb, lf.base.a * dm_a.x);
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> FragmentOutput {
    var uv = vec2<f32>(0.0);
#ifdef VERTEX_UVS_A
    uv = in.uv;
#endif
    var vc = vec4<f32>(1.0);
#ifdef VERTEX_COLORS
    vc = in.color;
#endif
    let flags = u32(lf.glow.w + 0.5);
    let has_glow = (flags & 1u) != 0u;
    let has_mask = (flags & 2u) != 0u;
    let has_dust = (flags & 4u) != 0u;
    let has_db = (flags & 8u) != 0u;
    let t = view_bindings::globals.time;

    let m = max(1.0 - 2.0 * length(uv - vec2<f32>(0.5)), 0.0);
    var profile = m;
    if (has_mask) {
        profile = textureSample(lf_mask, lf_sampler, uv).r;
    } else {
        profile = pow(max(m, 1e-6), lf.terms.x);
    }
    // 1 - radial distance factor x the source distance (0 for the source element).
    let rf = 1.0;
    var col = rf * vc.rgb * profile * (lf.color.rgb * lf.color.w);
    if (has_glow) {
        col = col * abs(lf.glow.y * sin(TAU * t * lf.glow.x) + lf.glow.z);
    }
    if (!has_mask && m - 1e-6 < 0.0) {
        col = vec3<f32>(0.0);
    }
    col = min(max(col, vec3<f32>(lf.terms.w)), vec3<f32>(lf.terms.z));
    var alpha = vc.a;
    if (has_dust) {
        // Periodic(Du_Dust_Speed x t x (1, 0.5)).
        let pan = fract(lf.dust.z * t * vec2<f32>(1.0, 0.5));
        alpha = lf.dust.x * textureSample(lf_dust, lf_sampler, uv * lf.dust.y + pan).r + vc.a;
    }
    if (has_db) {
        let pixel_z = -position_world_to_view(in.world_position.xyz).z / UU_TO_BEVY;
        var scene_z = 1.0e9;
#ifdef DEPTH_PREPASS
        let ndc = prepass_utils::prepass_depth(in.position, 0u);
        if (ndc > 0.0) {
            scene_z = -depth_ndc_to_view_z(ndc) / UU_TO_BEVY;
        }
#endif
        // 1 / ((1 - Db_Depth_Biased_Alpha_Intensity) x 250), or 1000 when that is below 0.001.
        let b = (1.0 - lf.dust.w) * 250.0;
        var k = 1000.0;
        if (b - 0.001 >= 0.0) {
            k = 1.0 / b;
        }
        alpha = alpha * saturate((scene_z - pixel_z) * k);
    }
    var out: FragmentOutput;
    // Additive: Bevy blends AlphaMode::Add as premultiplied alpha, so alpha 0 adds the colour.
    // quad.w converts UE3 radiance to Bevy light units (map_render::light_unit), then the view's
    // exposure applies as to the lightmaps.
    out.color = vec4<f32>(col * alpha * lf.quad.w * view_bindings::view.exposure, 0.0);
    return out;
}
