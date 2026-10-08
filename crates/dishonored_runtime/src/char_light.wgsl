// Characters lit by their light environment (char_light.rs): the base pass
// TBasePassPixelShader<FSHLightLightMapPolicy, NoSkyLight> of generic_PMAT on the GPU skin vertex
// factory (shader 2941 of RefShaderCache-PC-D3D-SM3.upk, shader map 0x62ef6e3 and others):
//
//   E_rgb   = pi Y00 c00 + sum_l (A_l) sum_m c_lm Y_lm(N)       (WorldIncidentLighting[7], N in UE3 world space;
//             the constants 0.886227608 / 1.02332771 / 0.858085215 / 0.247708291 / 0.429042608
//             are (pi, 2 pi / 3, pi / 4) x the real SH normalisations, Condon-Shortley phase)
//   colour  = diffuse x (E >= 0 ? E : 0)
//           + (saturate(N.L) x diffuse + S x pow(saturate(R.L), SP) x (SP + 8) / (8 pi)) x LightColor
//
// The light environment (`FDynamicLightEnvironmentState`, 0x004F85C0) synthesizes the directional
// light from the SH: its direction is (-L3, -L1, L2) of the luminance SH (0.3, 0.59, 0.11), its
// colour the projection of the SH on Y(direction) clamped at 0, subtracted from the SH, which is
// then the SH light. The SH the CPU hands over is the environment's current state; the original
// moves it to a new target linearly over its update interval (0x004E18F0:
// alpha = dt / (time left)), so the material carries the previous SH, the target and the two times,
// and the synthesis runs here on the interpolated SH, as it does in the original every tick.
// The dominant and toggleable lights stay Bevy lights; Bevy's ambient and environment light, which the
// original does not have on these pawns, are removed.

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing, calculate_diffuse_color},
    forward_io::{VertexOutput, FragmentOutput},
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
    mesh_view_bindings as view_bindings,
}

struct CharLightUniform {
    // SH coefficient k of the previous / target environment: (r, g, b, 0), Bevy light units.
    sh_a: array<vec4<f32>, 9>,
    sh_b: array<vec4<f32>, 9>,
    // x, y: the interpolation's start and end (Time::elapsed_secs_wrapped), w: 1 = environment on.
    time: vec4<f32>,
    // x: specular power SP, y: 1 = specular texture on, z: 1 = one masked channel (`spec_mask`).
    spec: vec4<f32>,
    spec_color: vec4<f32>,
    spec_mask: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> env: CharLightUniform;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var env_specular: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var env_specular_sampler: sampler;

// SHBasisFunction (0x00450860): the real SH, bands 0-2, with the Condon-Shortley phase.
fn sh_basis(d: vec3<f32>) -> array<f32, 9> {
    let x = d.x;
    let y = d.y;
    let z = d.z;
    var r: array<f32, 9>;
    r[0] = 0.282095;
    r[1] = -0.488603 * y;
    r[2] = 0.488603 * z;
    r[3] = -0.488603 * x;
    r[4] = 1.092548 * x * y;
    r[5] = -1.092548 * y * z;
    r[6] = 0.315392 * (3.0 * z * z - 1.0);
    r[7] = -1.092548 * x * z;
    r[8] = 0.546274 * (x * x - y * y);
    return r;
}

fn luminance(c: vec3<f32>) -> f32 {
    return 0.3 * c.x + 0.59 * c.y + 0.11 * c.z;
}

// The SH light's irradiance at N (UE3 world space): the pixel shader's dp4s over WorldIncidentLighting.
fn sh_irradiance(s: array<vec3<f32>, 9>, n: vec3<f32>) -> vec3<f32> {
    let x = n.x;
    let y = n.y;
    let z = n.z;
    var e = s[0] * 0.886227608;
    e = e + s[1] * (-1.02332771 * y) + s[2] * (1.02332771 * z) + s[3] * (-1.02332771 * x);
    e = e + s[4] * (0.858085215 * x * y) + s[5] * (-0.858085215 * y * z) + s[6] * (0.247708291 * (3.0 * z * z - 1.0));
    e = e + s[7] * (-0.858085215 * x * z) + s[8] * (0.429042608 * (x * x - y * y));
    return e;
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
    let m = pbr_input.material;
    let diffuse = calculate_diffuse_color(m.base_color.rgb, m.metallic, m.specular_transmission, m.diffuse_transmission);
    var base = vec3<f32>(0.0);
    if (env.time.w > 0.5) {
        // The interpolated environment SH.
        let now = view_bindings::globals.time;
        var t = 1.0;
        if (now >= env.time.x) {
            t = saturate((now - env.time.x) / max(env.time.y - env.time.x, 1e-5));
        }
        var s: array<vec3<f32>, 9>;
        for (var k: u32 = 0u; k < 9u; k = k + 1u) {
            s[k] = mix(env.sh_a[k].xyz, env.sh_b[k].xyz, t);
        }
        // The synthesized directional light (UE3 world space, towards the light).
        var dir = vec3<f32>(0.0, 0.0, 1.0);
        var dir_color = vec3<f32>(0.0);
        let dv = vec3<f32>(-luminance(s[3]), -luminance(s[1]), luminance(s[2]));
        if (dot(dv, dv) >= 1e-10) {
            let d = normalize(dv);
            let y = sh_basis(d);
            var yy = 0.0;
            var proj = vec3<f32>(0.0);
            for (var k: u32 = 0u; k < 9u; k = k + 1u) {
                yy = yy + y[k] * y[k];
                proj = proj + s[k] * y[k];
            }
            let c = max(proj / yy, vec3<f32>(0.0));
            if (c.x > 0.0 || c.y > 0.0 || c.z > 0.0) {
                for (var k: u32 = 0u; k < 9u; k = k + 1u) {
                    s[k] = s[k] - c * y[k];
                }
                dir = d;
                dir_color = c;
            }
        }
        let n = normalize(pbr_input.N);
        let v = normalize(pbr_input.V);
        // Bevy world (x, up, z) is UE3 (x, z, up).
        let l = vec3<f32>(dir.x, dir.z, dir.y);
        let ndotl = saturate(dot(n, l));
        let rdotl = saturate(dot(reflect(-v, n), l));
        var sp_col = vec3<f32>(0.0);
#ifdef VERTEX_UVS_A
        if (env.spec.y > 0.0) {
            let tex = textureSample(env_specular, env_specular_sampler, in.uv);
            // The masked channel (or rgb), times the colour when S_Enable_Specular_color is on.
            let masked = select(tex.rgb, vec3<f32>(dot(tex, env.spec_mask)), env.spec.z > 0.5);
            sp_col = masked * env.spec_color.rgb;
        }
#endif
        let sp = env.spec.x;
        let spec = select(vec3<f32>(0.0), sp_col * pow(rdotl, sp) * (sp + 8.0) * 0.0397887342, rdotl - 1e-6 >= 0.0);
        let dif = select(vec3<f32>(0.0), ndotl * diffuse, ndotl - 1e-6 >= 0.0);
        let e = sh_irradiance(s, vec3<f32>(n.x, n.z, n.y));
        let sh_light = select(vec3<f32>(0.0), e * diffuse, e >= vec3<f32>(0.0));
        base = (dif + spec) * dir_color + sh_light;
    }
    // The original has no ambient or environment light on these pawns.
    pbr_input.diffuse_occlusion = vec3<f32>(0.0);
    pbr_input.specular_occlusion = 0.0;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = vec4<f32>(out.color.rgb + base * view_bindings::view.exposure, out.color.a);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
