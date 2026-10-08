// The caustic spot lights' contribution (caustics.rs): the original's light pass for a spot light with
// the `Casutics_LightFunction_PMAT` function (RefShaderCache pixel shader 36479), on a static mesh
// section the light reaches, added to the frame.
//
//   light  = LightColor x Brightness x atten x cone x f x (albedo x saturate(N.L) + spec)
//   atten  = max(1 - d^2 / R^2, 0)^FalloffExponent (0 below 1e-6)
//   cone   = saturate((cos a - cos outer) / (cos inner - cos outer))^2
//   spec   = S x saturate(R.L)^SP x (SP + 8) / (8 pi), S = 2 sqrt(F0 / 0.16), SP from the roughness
//            (the same light-pass terms as `lighting::ue3_light`)
//
// The function f (36479), with t = Time x Speed and (u, v) the light-function coordinates:
//   n  = Tex0(2 (u, v) + frac(t (0.75, -0.005))).x + Tex0(2 (u, v) + frac(t (0, 0.75))).x
//   cA = |Tex1((u, v) (-1.5, 1.5) + frac(t (0.05, 0.25)) + 0.12 n)|^0.4
//   cB = |Tex1(1.35 (u, v) + frac(t (-0.25, -0.05)) + 0.12 n)|^0.4
//   f  = dot(cA x cB x CausticColor.rgb x CausticColor.w, 0.333)     (greyscaled)
// LIKELY: (u, v) = the offset from the light along the light's Y and Z axes / Scale (orthographic).

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, calculate_diffuse_color, calculate_F0_dielectric},
    forward_io::{VertexOutput, FragmentOutput},
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
    mesh_view_bindings as view_bindings,
    lighting,
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

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> caustics: Caustics;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var caustic_noise: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var caustic_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var caustic_tex: texture_2d<f32>;

// The material function at light-function coordinates `uv` (grey).
fn caustic_function(uv: vec2<f32>) -> f32 {
    let t = view_bindings::globals.time * caustics.head.y;
    let c10 = fract(vec2<f32>(0.05, 0.25) * t);
    let c11 = fract(vec2<f32>(0.75, -0.005) * t);
    let c12 = fract(vec2<f32>(0.0, 0.75) * t);
    let c13 = fract(vec2<f32>(-0.25, -0.05) * t);
    let n = textureSample(caustic_noise, caustic_sampler, uv * 2.0 + c11).x + textureSample(caustic_noise, caustic_sampler, uv * 2.0 + c12).x;
    let a = textureSample(caustic_tex, caustic_sampler, uv * vec2<f32>(-1.5, 1.5) + c10 + vec2<f32>(0.12 * n)).rgb;
    let b = textureSample(caustic_tex, caustic_sampler, uv * 1.35 + c13 + vec2<f32>(0.12 * n)).rgb;
    let pa = pow(max(abs(a), vec3<f32>(1e-6)), vec3<f32>(0.4));
    let pb = pow(max(abs(b), vec3<f32>(1e-6)), vec3<f32>(0.4));
    let col = pa * pb * (caustics.color.rgb * caustics.color.w);
    return dot(col, vec3<f32>(0.333));
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);
    let m = pbr_input.material;
    let n = normalize(pbr_input.N);
    let v = normalize(pbr_input.V);
    let p = in.world_position.xyz;
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
        // Light-function coordinates: the offset from the light along its Y and Z axes, Scale per repeat.
        let off = p - l.pos_radius.xyz;
        let f = caustic_function(vec2<f32>(dot(off, l.right.xyz) / l.shape.y, dot(off, l.up.xyz) / l.shape.z));
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
