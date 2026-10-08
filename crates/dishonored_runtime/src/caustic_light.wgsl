// The caustic spot lights' contribution (caustics.rs): the original's light pass for a spot light with
// the `Casutics_LightFunction_PMAT` function, on a static mesh section the light reaches, added to the
// frame. The module is this file followed by the function material's pixel shader (RefShaderCache
// 36479, `FLightFunctionPixelShader`) translated by `ue3_shader` (`ue3prog::build_fragment`:
// `ue_ps_call`), assembled in caustics.rs.
//
//   light  = LightColor x Brightness x atten x cone x f x (albedo x saturate(N.L) + spec)
//   atten  = max(1 - d^2 / R^2, 0)^FalloffExponent (0 below 1e-6)
//   cone   = saturate((cos a - cos outer) / (cos inner - cos outer))^2
//   spec   = S x saturate(R.L)^SP x (SP + 8) / (8 pi), S = 2 sqrt(F0 / 0.16), SP from the roughness
//            (the same light-pass terms as `lighting::ue3_light`)
//   f      = the function shader's grey value
//
// The function shader is a screen-space pass: it reads the scene depth (`SceneColorTexture` alpha,
// uu / 256; here the depth prepass), rebuilds (x w, y w, w, 1) from the pixel and multiplies it by
// `ScreenToLight` (c6..c9, rows; the shader uses the columns x, y and w). The engine's matrix is
// the light function's world-to-light transform, scaled by the function's `Scale`; here its rows
// are the images of the camera's pixel basis, so the product is the scene point's offset from the
// light along the light's Y and Z axes over `Scale`, with w = 1 (LIKELY: which light axes the
// function's xy are, see caustics.rs). `GreyscaleFactor` is 1 (LIKELY: the pass's result is the
// light's attenuation, a grey value).

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, calculate_diffuse_color, calculate_F0_dielectric},
    forward_io::{VertexOutput, FragmentOutput},
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
    mesh_view_bindings as view_bindings,
    lighting,
    prepass_utils,
    view_transformations,
}

struct CausticLightData {
    pos_radius: vec4<f32>,
    dir_cos_outer: vec4<f32>,
    color_cos_inner: vec4<f32>,
    shape: vec4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
}

struct Caustics {
    head: vec4<f32>,
    color: vec4<f32>,
    lights: array<CausticLightData, 4>,
}

// The function material's parameter slots (`param_nodes` order: Speed, CausticColor).
struct UeMaterial { p: array<vec4<f32>, 64> };

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> caustics: Caustics;
// Texture2D_0: T_EV_DustPanner_01, Texture2D_1: T_Caustics_2
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var ue_t0: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var ue_s0: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var ue_t1: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(104) var ue_s1: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(105) var<uniform> ue_mat: UeMaterial;

// What the translated shader reads.
var<private> ue_screen_to_light: array<vec4<f32>, 4>;
var<private> frag_depth_uu: f32;
fn ue_time() -> f32 { return view_bindings::globals.time; }
fn ue_greyscale() -> vec4<f32> { return vec4<f32>(1.0); }
// The scene depth in the alpha channel (uu / 256), from the depth prepass.
fn ue_scene_color(uv: vec2<f32>) -> vec4<f32> {
#ifdef DEPTH_PREPASS
    let d = prepass_utils::prepass_depth(vec4<f32>(uv * view_bindings::view.viewport.zw, 0.0, 0.0), 0u);
    let z = -view_transformations::depth_ndc_to_view_z(max(d, 1e-7)) * 50.0;
    return vec4<f32>(0.0, 0.0, 0.0, z / 256.0);
#else
    return vec4<f32>(0.0, 0.0, 0.0, frag_depth_uu / 256.0);
#endif
}

// The material function for a light: `ScreenToLight` as the images of the pixel basis.
fn caustic_function(frag: vec4<f32>, l: CausticLightData) -> f32 {
    let v = view_bindings::view;
    let tan_x = 1.0 / v.clip_from_view[0][0];
    let tan_y = 1.0 / v.clip_from_view[1][1];
    // Bevy units per uu along the pixel basis (x w, y w, w): the camera's right, up and forward.
    let bx = v.world_from_view[0].xyz * tan_x * 0.02;
    let by = v.world_from_view[1].xyz * tan_y * 0.02;
    let bz = -v.world_from_view[2].xyz * 0.02;
    let o = v.world_position - l.pos_radius.xyz;
    let r = l.right.xyz;
    let u = l.up.xyz;
    let su = l.shape.y;
    let sv = l.shape.z;
    ue_screen_to_light[0] = vec4<f32>(dot(bx, r) / su, dot(bx, u) / sv, 0.0, 0.0);
    ue_screen_to_light[1] = vec4<f32>(dot(by, r) / su, dot(by, u) / sv, 0.0, 0.0);
    ue_screen_to_light[2] = vec4<f32>(dot(bz, r) / su, dot(bz, u) / sv, 0.0, 0.0);
    ue_screen_to_light[3] = vec4<f32>(dot(o, r) / su, dot(o, u) / sv, 0.0, 1.0);
    let size = v.viewport.zw;
    let ndc = vec2<f32>(frag.x / size.x * 2.0 - 1.0, 1.0 - frag.y / size.y * 2.0);
    var tc: array<vec4<f32>, 8>;
    tc[0] = vec4<f32>(ndc, 0.0, 1.0);
    return ue_ps_call(tc, frag).x;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);
    let m = pbr_input.material;
    let n = normalize(pbr_input.N);
    let v = normalize(pbr_input.V);
    let p = in.world_position.xyz;
    frag_depth_uu = -view_transformations::position_world_to_view(p).z * 50.0;
    let diffuse_color = calculate_diffuse_color(m.base_color.rgb, m.metallic, m.specular_transmission, m.diffuse_transmission);
    let a = lighting::perceptualRoughnessToRoughness(m.perceptual_roughness);
    let sp = max(2.0 / max(a * a, 1e-8) - 2.0, 0.0);
    let s = 2.0 * sqrt(max(calculate_F0_dielectric(m.reflectance), vec3<f32>(0.0)) / 0.16);
    let r = reflect(-v, n);
    var total = vec3<f32>(0.0);
    let count = i32(caustics.head.x);
    for (var i: i32 = 0; i < count; i = i + 1) {
        let l = caustics.lights[i];
        let to_light = l.pos_radius.xyz - p;
        let dist2 = dot(to_light, to_light);
        let x = max(1.0 - dist2 / (l.pos_radius.w * l.pos_radius.w), 0.0);
        let atten = select(0.0, pow(max(x, 1e-6), l.shape.x), x - 1e-6 >= 0.0);
        let ldir = to_light * inverseSqrt(max(dist2, 1e-8));
        var cone = 1.0;
        if (l.shape.w > 0.5) {
            let c = saturate((dot(-ldir, l.dir_cos_outer.xyz) - l.dir_cos_outer.w) / max(l.color_cos_inner.w - l.dir_cos_outer.w, 1e-3));
            cone = c * c;
        }
        let ndotl = saturate(dot(n, ldir));
        let rdotl = saturate(dot(r, ldir));
        var d = vec3<f32>(0.0);
        if (ndotl - 1e-6 >= 0.0) {
            d = diffuse_color * ndotl;
        }
        var spec = vec3<f32>(0.0);
        if (rdotl - 1e-6 >= 0.0) {
            spec = s * pow(rdotl, sp) * (sp + 8.0) * 0.0397887342;
        }
        let f = caustic_function(in.position, l);
        total = total + (d + spec) * l.color_cos_inner.rgb * (atten * cone * f);
    }
    var out: FragmentOutput;
    if ((m.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) != 0u) {
        total = vec3<f32>(0.0);
    }
    // Additive: Bevy blends AlphaMode::Add as premultiplied alpha, so alpha 0 adds the colour. The
    // colour is in Bevy light units (caustics.rs), then the view's exposure applies as to the lightmaps.
    out.color = vec4<f32>(total * view_bindings::view.exposure, 0.0);
    return out;
}

// ---- the function material's pixel shader (translated), appended by caustics.rs ----
