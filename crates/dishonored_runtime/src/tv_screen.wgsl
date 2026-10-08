// The Tower Return lens TV (`saili_FT_LensTV.screen_MAT`), from the material's compiled unlit base
// pass (tv_screen.rs has the derivation).

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    mesh_view_bindings as view_bindings,
}

struct TvUniform {
    // (Switch_ON_OFF, EffectBaseSpeed, RGB effect offset strength, radiance to Bevy light units).
    params: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> tv: TvUniform;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var tv_target: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var tv_sampler: sampler;

const PI: f32 = 3.14159274;
const TAU: f32 = 6.2831855;

// frac(sin(a x 7.953399) + sin(a x 57.11987)) of the colour offsets.
fn noise_offset(a: f32) -> f32 {
    return fract(sin(a * 7.953399) + sin(a * 57.11987));
}

// sin(2 pi frac(x + 0.5) - pi) of the scan lines and the flicker.
fn wave(x: f32) -> f32 {
    return sin(fract(x + 0.5) * TAU - PI);
}

@fragment
fn fragment(in: VertexOutput) -> FragmentOutput {
    var uv = vec2<f32>(0.0);
#ifdef VERTEX_UVS_A
    uv = in.uv;
#endif
    let t = view_bindings::globals.time;
    let speed = tv.params.y;
    let strength = tv.params.z;
    let t14 = t * speed * 0.14;
    let t1 = t * speed * 0.1;
    let t27 = t * speed * 0.27;
    let o_r = fract(sin(t14 * 20.26834) + sin(t14 * TAU)) * 0.01 * strength;
    let o_g = noise_offset(t1) * 0.013 * strength;
    let o_b = noise_offset(t27) * 0.017 * strength;
    let r = textureSample(tv_target, tv_sampler, uv + vec2<f32>(o_r)).r;
    let g = textureSample(tv_target, tv_sampler, uv + vec2<f32>(0.0, o_g)).g;
    let b = textureSample(tv_target, tv_sampler, uv + vec2<f32>(o_b, -2.0 * o_b)).b;
    var c = vec3<f32>(r, g, b) * tv.params.x;
    // Scan lines.
    let scan = wave((fract(-0.02 * t) + uv.y) * 142.857147);
    c = c * ((scan * 0.5 - 0.5) * 0.7 + 1.0);
    // Vignette.
    let e = max(abs(uv * 2.0 - 1.0), vec2<f32>(1e-6));
    let s = saturate(pow(e.x, 3.5) + pow(e.y, 3.5));
    c = c * select(1.0, 1.0 - pow(s, 0.25), s - 1e-6 >= 0.0);
    // Flicker.
    let p = wave(uv.y * 2.7 + fract(-0.23 * t)) * wave(uv.y * 1.1 + fract(-0.37 * t));
    let q = select(0.0, pow(p * p, 2.0), abs(p) - 1e-6 >= 0.0);
    c = c * select(0.65, (q - 1.0) * 0.7 + 1.0, q - 0.5 >= 0.0);
    var out: FragmentOutput;
    out.color = vec4<f32>(c * 10.0 * tv.params.w * view_bindings::view.exposure, 1.0);
    return out;
}
