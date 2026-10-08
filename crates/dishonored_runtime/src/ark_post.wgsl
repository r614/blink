// Arkane full-screen passes (see ark_post.rs), in the original's order: `fog` (DisFog layers,
// then the blurred bloom parts added), and after the DOF downsample (ark_downsample.wgsl)
// `fragment` (ArkPp far DOF towards the quarter-size copy, the colour LUT maths, film grain).

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput
#import bevy_render::view::View

struct ArkPost {
    layer_min: vec4<f32>,
    layer_max: vec4<f32>,
    layer_near: vec4<f32>,
    layer_inv_range: vec4<f32>,
    layer_no_fog: vec4<f32>,
    layer_opacity: vec4<f32>,
    layer_density: vec4<f32>,
    layer_lut: vec4<f32>,
    layer_color: array<vec4<f32>, 4>,
    sun: vec4<f32>,
    lut: array<vec4<f32>, 128>,
    cb_shadows_opacity: vec4<f32>,
    cb_midtones: vec4<f32>,
    cb_highlights: vec4<f32>,
    overlay: vec4<f32>,
    exposure_gamma_desat: vec4<f32>,
    brightness_contrast: vec4<f32>,
    dof: vec4<f32>,
    grain: vec4<f32>,
    misc: vec4<f32>,
    // gBloomTint: (m_Tint.rgb x m_Scale, -m_Threshold x m_Scale); used by the bloom downsample.
    bloom: vec4<f32>,
    // Bend Time (power_pp.rs): (intensity, vector scale, 0, 0) and the two noise pans.
    power_vectors: vec4<f32>,
    power_pans: vec4<f32>,
    // Dark Vision's eyelid pass: (on, Alpha, 0, 0) and EyeLidColor.
    dv_eyelid: vec4<f32>,
    dv_lid_color: vec4<f32>,
    // Blink's lens: (x distortion, y blur, on, 0).
    blink_lens: vec4<f32>,
    // Possession's field: (R, G, on, material Time).
    possess: vec4<f32>,
    // Light shaft: (Bevy world position, on), LightShaftParameters, BloomTintAndThreshold,
    // (BloomScreenBlendThreshold, 0, 0, 0).
    shaft_pos: vec4<f32>,
    shaft_params: vec4<f32>,
    shaft_bloom: vec4<f32>,
    shaft_misc: vec4<f32>,
}

@group(0) @binding(0) var<uniform> view: View;
#ifdef MULTISAMPLED
@group(0) @binding(1) var depth_texture: texture_depth_multisampled_2d;
#else
@group(0) @binding(1) var depth_texture: texture_depth_2d;
#endif
@group(0) @binding(2) var source: texture_2d<f32>;
@group(0) @binding(3) var source_sampler: sampler;
@group(0) @binding(4) var<uniform> ark: ArkPost;
// `fog`: the blurred bloom parts (quarter size); `fragment`: the DOF's quarter-size copy.
@group(0) @binding(5) var aux: texture_2d<f32>;
// Bend Time's vector field: `PPG_BendTimeVectors` `Tex#0` = water2 (noise), `Tex#1` =
// fxt_eyelid_gradient (mask); the noise wraps.
@group(0) @binding(6) var field_noise: texture_2d<f32>;
@group(0) @binding(7) var field_mask: texture_2d<f32>;
@group(0) @binding(8) var repeat_sampler: sampler;
// Possession's field noise: `PPG_PossessionVectors` `Texture2D_0`.
@group(0) @binding(9) var possess_noise: texture_2d<f32>;

// `PPG_PossessionVectors` (captured shader 0x9312eee076ac74a4, its disassembly translated
// instruction by instruction; EFFECTS.md "Power screen effects"): a swirl field around the screen
// centre. d = uv - 0.5; theta = atan2-like angle of d (the shader's polynomial atan with its
// quadrant fix-ups), rho = |d|; m = 20 |d.x| + 10 |d.y|; s = sin(frac((5 T + m) / 2 pi + 0.5) 2 pi
// - pi) + 0.1 sin(frac((20 T + m) ...)); the angle becomes theta + 0.03 R s and the field is
// rg = k x rho (sin, -cos) with k = G (120 noise(0.6 uv + 0.2).r + 60 s), ba = rho (cos, sin) + 0.5
// - uv - rg / (3 x (1280, 720)). R, G = `UniformPixelVector_1.xy`, T = `UniformPixelScalars_0.x`.
// The first sampler (the previous field) is black here, so its 0.6 weight adds nothing.
fn possession_field(uv: vec2<f32>) -> vec4<f32> {
    let c2 = vec4<f32>(0.600000024, 2.0, -2.0, 0.180141002);
    let c3 = vec4<f32>(0.0208350997, -0.0851330012, 0.999866009, 0.0);
    let c4 = vec4<f32>(0.199999794, -1.0, 1.0, -0.330299497);
    let c5 = vec4<f32>(0.0, 1.0, -2.0, 1.57079637);
    let pi = 3.14159274;
    let r0 = uv.xyxy * c2.xxyz + c4.xxyz;
    // ScreenPositionScaleBias of a full-buffer pass: (0.5, -0.5, 0.5, 0.5).
    let r1 = r0.zw * vec2<f32>(0.5, -0.5);
    let ax = abs(r1.x);
    let ay = abs(r1.y);
    let z1 = -ax + ay;
    let w1 = -ay + ax;
    let q = select(vec2<f32>(ay, ax), vec2<f32>(ax, ay), z1 >= 0.0);
    let swap = select(1.0, 0.0, w1 >= 0.0);
    let t = (1.0 / q.y) * q.x;
    let t2 = t * t;
    var p = t2 * c3.x + c3.y;
    p = t2 * p + c2.w;
    p = t2 * p + c4.w;
    let a = t * (t2 * p + c3.z);
    let k0 = a * c5.z + c5.w;
    var th = k0 * swap + a;
    th = th + select(-pi, 0.0, r1.x >= 0.0);
    let th2 = th + th;
    let dyx = r0.w * -0.5 - r1.x;
    let sel = dyx >= 0.0;
    let n2x = select(r1.y, r1.x, sel);
    let n2y = select(r1.x, r1.y, sel);
    let fy = select(0.0, 1.0, n2y >= 0.0);
    let fx = select(fy, 0.0, n2x >= 0.0);
    th = fx * -th2 + th;
    let m = ay * 10.0 + ax * 20.0;
    let rho = sqrt(r1.x * r1.x + r1.y * r1.y);
    let tt = ark.possess.w;
    var w = vec2<f32>(tt * 5.0 + m, tt * 20.0 + m);
    w = fract(w * 0.159154937 + 0.5) * 6.28318548 - pi;
    let s = sin(w.x) + sin(w.y) * 0.1;
    var ang = th + s * ark.possess.x * 0.03;
    ang = fract(ang * 0.159154937 + 0.5) * 6.28318548 - pi;
    let cs = vec2<f32>(cos(ang), sin(ang));
    let pos = cs * rho + vec2<f32>(0.5, 0.5);
    let n = textureSampleLevel(possess_noise, repeat_sampler, r0.xy, 0.0).x;
    let kk = (n * 120.0 + s * 60.0) * ark.possess.y;
    let tang = kk * vec2<f32>(cs.y * rho, -cs.x * rho);
    let sample_uv = tang * vec2<f32>(-0.00026041668, -0.000462962955) + pos;
    return vec4<f32>(tang, sample_uv - uv);
}

// `PPG_BendTimeVectors_Std_INST` (captured shader 0xbbe665d07e36986b): the vector field in
// sixteenths of a pixel. Two pans of the noise (rg x 2 - 1) summed, x 0.5, faded over the
// outer tenth of the screen, x 0.3, minus the centred position (2 uv - 1): a pull towards the
// screen centre, wobbled; x (1 - mask.r) x vBendTimeVectorScales.x x vBendTimeOpacity.
fn bend_field(uv: vec2<f32>) -> vec2<f32> {
    let n1 = textureSampleLevel(field_noise, repeat_sampler, uv + ark.power_pans.xy, 0.0).rg * 2.0 - 1.0;
    let n2 = textureSampleLevel(field_noise, repeat_sampler, uv + ark.power_pans.zw, 0.0).rg * 2.0 - 1.0;
    let c = uv * 2.0 - 1.0;
    let edge = clamp(10.0 - abs(c * 10.0), vec2<f32>(0.0), vec2<f32>(1.0));
    let v = (n1 + n2) * 0.5 * edge * 0.3 - c;
    let m = textureSampleLevel(field_mask, source_sampler, uv, 0.0).r;
    return v * (1.0 - m) * ark.power_vectors.y * ark.power_vectors.x;
}

// `PPG_BlinkVectors` (captured shader 0x0b36c71ce1512549, `Tex#0` = fxt_eyelid_gradient), with
// d = uv - 0.5 and rho = |(1.975 d.x, 2 d.y)|: rg = 240 y (1 - saturate(gradient.r + 0.5 - y)) d
// (a radial blur), ba = 0.15 x (1 - rho^3) d (a lens bulge, applied as an offset by the blur's
// second pass); x / y = `UniformPixelVector_1.xy` (`power_pp::BlinkLens`).
fn blink_field(uv: vec2<f32>) -> vec4<f32> {
    let x = ark.blink_lens.x;
    let y = ark.blink_lens.y;
    let g = textureSampleLevel(field_mask, source_sampler, uv, 0.0).r;
    let a = 120.0 * (1.0 - clamp(g + 0.5 - y, 0.0, 1.0));
    let d = uv - 0.5;
    let rho = length(vec2<f32>(1.97530866 * d.x, 2.0 * d.y));
    let lens = 0.15 * x * (1.0 - rho * rho * rho) * d;
    return vec4<f32>(a * 2.0 * y * d, lens);
}

// The active power's field: rg = blur vector (sixteenths of a pixel), ba = uv offset of the
// second pass. Blink's switch comes before Bend Time's in `Test_PPG`.
fn power_field(uv: vec2<f32>) -> vec4<f32> {
    if (ark.blink_lens.z > 0.5) {
        return blink_field(uv);
    }
    if (ark.possess.z > 0.5) {
        return possession_field(uv);
    }
    return vec4<f32>(bend_field(uv), 0.0, 0.0);
}

// `ArkPpNodeBlur` along the field (captured shaders 0xbd9a42b61794aabb then 0x6095a5daa9109b1d):
// four taps at uv + k x field x scale (k = 0..3) averaged, first with gVectorFieldScales =
// (1 / (16 x size), 0, 0) on the scene, then with (1 / (4 x size), 1, 1) on that result: the
// second pass starts at uv + field.ba and reads the field at its own pixel.
fn power_pass1(p: vec2<f32>, s1: vec2<f32>) -> vec3<f32> {
    let f = power_field(p).xy * s1;
    var sum = vec3<f32>(0.0);
    for (var k = 0; k < 4; k = k + 1) {
        sum = sum + textureSampleLevel(source, source_sampler, p + f * f32(k), 0.0).rgb;
    }
    return sum * 0.25;
}

fn power_blur(uv: vec2<f32>) -> vec3<f32> {
    let size = vec2<f32>(textureDimensions(source));
    let s1 = 1.0 / (16.0 * size);
    let s2 = 1.0 / (4.0 * size);
    let field = power_field(uv);
    let base = uv + field.zw;
    let f = field.xy * s2;
    var sum = vec3<f32>(0.0);
    for (var k = 0; k < 4; k = k + 1) {
        sum = sum + power_pass1(base + f * f32(k), s1);
    }
    return sum * 0.25;
}

const LUMA = vec3<f32>(0.2126, 0.7152, 0.0722);

fn lut_entry(layer: u32, i: u32) -> f32 {
    let v = ark.lut[layer * 32u + i / 4u];
    return v[i % 4u];
}

// The LUT texture is 128 x 1, linearly filtered and clamped, sampled at (x, 0.5).
fn lut_sample(layer: u32, x: f32) -> f32 {
    let t = clamp(x, 0.0, 1.0) * 128.0 - 0.5;
    let i0 = u32(clamp(floor(t), 0.0, 127.0));
    let i1 = min(i0 + 1u, 127u);
    let f = clamp(t - floor(t), 0.0, 1.0);
    if (t < 0.0) {
        return lut_entry(layer, 0u);
    }
    return mix(lut_entry(layer, i0), lut_entry(layer, i1), f);
}

struct Fog {
    transmittance: f32,
    inscatter: vec3<f32>,
}

// FDisFogPixelShader: per layer, the length of the camera->surface segment inside the
// layer's height slab drives a NearPlane..FarPlane ramp (or the LUT) scaled by Opacity;
// layers beyond their NoFogPlane (scene depth) add nothing. Layers combine as
// colour * prod(1 - f_i) + sum(f_i * colour_i).
fn dis_fog(offset_uu: vec3<f32>, depth_uu: f32) -> Fog {
    var out: Fog;
    out.transmittance = 1.0;
    out.inscatter = vec3<f32>(0.0);
    let n = u32(ark.misc.y);
    if (n == 0u) {
        return out;
    }
    let dist = length(offset_uu);
    // UE3 Z (up) is Bevy Y.
    let dz = offset_uu.y;
    let inv_dz = 1.0 / select(dz, 1e-6, abs(dz) < 1e-6);
    let cam_z = view.world_position.y / ark.misc.x;
    for (var i = 0u; i < n; i = i + 1u) {
        let t0 = clamp((ark.layer_min[i] - cam_z) * inv_dz * ark.layer_density[i], 0.0, 1.0);
        let t1 = clamp((ark.layer_max[i] - cam_z) * inv_dz * ark.layer_density[i], 0.0, 1.0);
        let x = (abs(t1 - t0) * dist - ark.layer_near[i]) * ark.layer_inv_range[i];
        var f = select(clamp(x, 0.0, 1.0), lut_sample(i, x), ark.layer_lut[i] > 0.5) * ark.layer_opacity[i];
        if (depth_uu > ark.layer_no_fog[i]) {
            f = 0.0;
        }
        if (i == 0u && ark.sun.w > 0.1) {
            let c = clamp(dot(normalize(offset_uu.xyz), ark.sun.xyz), 0.0, 1.0);
            f = select(0.0, pow(c, ark.sun.w) * f, c > 1e-6);
        }
        out.transmittance = out.transmittance * (1.0 - f);
        out.inscatter = out.inscatter + f * ark.layer_color[i].rgb;
    }
    return out;
}

fn balance_side(c: vec3<f32>) -> vec3<f32> {
    return (1.075 - 1.0 / (c * 15.9375 + 1.0)) * 0.392156869;
}

// FArkPpDofLutBlenderPS evaluated directly (the original bakes it into a 16^3 LUT indexed
// by sqrt(colour), so its input is clamped to [0, 1]).
fn colour_lut(input: vec3<f32>) -> vec3<f32> {
    let x = clamp(input, vec3<f32>(0.0), vec3<f32>(1.0));
    let e = ark.exposure_gamma_desat;
    let r1 = x * e.x;
    let pre = mix(r1, vec3<f32>(dot(r1, LUMA)), e.z);
    // Shadows: positive amounts weight dark values, negative ones bright values.
    let s = ark.cb_shadows_opacity;
    let ws = select(balance_side(1.0 - pre), balance_side(pre), s.xyz >= vec3<f32>(0.0));
    var c = clamp(ws * s.xyz + pre, vec3<f32>(0.0), vec3<f32>(1.0));
    // Midtones: a parabola centred on 127/255.
    let m = (c * 255.0 - 127.0) * 0.00787401572;
    c = clamp((1.0 - m * m) * ark.cb_midtones.xyz * 0.261568636 + c, vec3<f32>(0.0), vec3<f32>(1.0));
    // Highlights: the mirror of shadows.
    let h = ark.cb_highlights.xyz;
    let wh = select(balance_side(c), balance_side(1.0 - c), h >= vec3<f32>(0.0));
    c = clamp(wh * h + c, vec3<f32>(0.0), vec3<f32>(1.0));
    var r = mix(pre, c, s.w);
    r = mix(r, ark.overlay.rgb, ark.overlay.w);
    r = mix(r, vec3<f32>(dot(r, LUMA)), e.w);
    r = pow(max(r, vec3<f32>(1e-5)), vec3<f32>(e.y));
    let b = ark.brightness_contrast.x;
    r = select(r * (1.0 - b), r + (1.0 - r) * b, b >= 0.0);
    return (r - 0.5) * ark.brightness_contrast.y + 0.5;
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

fn depth_at(p: vec2<i32>) -> f32 {
#ifdef MULTISAMPLED
    return textureLoad(depth_texture, p, 0);
#else
    return textureLoad(depth_texture, p, 0);
#endif
}

struct SceneDepth {
    // Scene depth along the view axis (uu); 1e30 for the cleared background.
    depth_uu: f32,
    // Camera -> surface (uu, Bevy axes).
    offset_uu: vec3<f32>,
    hit: bool,
}

fn scene_depth(uv: vec2<f32>, pix: vec2<i32>) -> SceneDepth {
    var s: SceneDepth;
    s.depth_uu = 1e30;
    s.offset_uu = vec3<f32>(0.0);
    s.hit = false;
    let raw = depth_at(pix);
    // Reverse-Z: 0 is the far plane / cleared background (no geometry, no fog).
    if (raw > 0.0) {
        let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, raw, 1.0);
        let wp = view.world_from_clip * ndc;
        let world = wp.xyz / wp.w;
        let vp = view.view_from_world * vec4<f32>(world, 1.0);
        s.depth_uu = -vp.z / ark.misc.x;
        s.offset_uu = (world - view.world_position) / ark.misc.x;
        s.hit = true;
    }
    return s;
}

// ---- UE3 light shafts (research/dishonored/systems/render_post.md "Light shafts") ----
// The pass 0x00837480 per light: `Downsample` (global shader 158, the directional variant),
// `RadialBlur` (157) and `Apply` (155), their disassembly translated. The targets are the scene
// divided by SHAFT_FACTOR (keep in step with ark_post.rs); with the view filling them the
// aspect terms are AspectRatioAndInvAspectRatio = (1, w / h, 1, h / w).
const SHAFT_FACTOR: f32 = 4.0;

fn shaft_origin(rt: vec2<f32>) -> vec2<f32> {
    // TextureSpaceBlurOrigin: the light's position through the view projection, in uv, with the
    // y axis in aspect-corrected units (0x00820300).
    let clip = view.clip_from_world * vec4<f32>(ark.shaft_pos.xyz, 1.0);
    let uv = (clip.xy / clip.w) * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    return vec2<f32>(uv.x, uv.y * rt.y / rt.x);
}

fn shaft_depth(uv: vec2<f32>) -> f32 {
    let sz = vec2<f32>(textureDimensions(source));
    let p = clamp(vec2<i32>(uv * sz), vec2<i32>(0), vec2<i32>(sz) - vec2<i32>(1));
    return scene_depth(uv, p).depth_uu;
}

// Shader 158: the 4-tap downsample (taps at 2 scene texels), alpha = the occlusion mask
// (mean of saturate(depth / OcclusionDepthRange), at least the border term), rgb = the bloom of
// the pixels beyond half the range above BloomThreshold, x 0.25, confined near the light.
@fragment
fn shaft_down(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let sz = vec2<f32>(textureDimensions(source));
    let rt = floor(sz / SHAFT_FACTOR);
    let origin = shaft_origin(rt);
    let inv = vec2<f32>(1.0, rt.y / rt.x);
    let uvmax = (rt - vec2<f32>(1.0)) / rt;
    let t = uv / uvmax;
    let s = vec2<f32>(1.0) - t;
    let edge = s.x * t.x * t.y * s.y;
    let q = 1.0 - 8.0 * edge;
    let q2 = q * q;
    let q4 = q2 * q2;
    let e_fade = 1.0 - q4;
    let o = vec2<f32>(2.0) / sz;
    let uva = clamp(uv - vec2<f32>(o.x, 0.0), vec2<f32>(0.0), vec2<f32>(1.0));
    let uvc = clamp(uv - vec2<f32>(0.0, o.y), vec2<f32>(0.0), vec2<f32>(1.0));
    let uvd = clamp(uv - o, vec2<f32>(0.0), vec2<f32>(1.0));
    let ca = textureSampleLevel(source, source_sampler, uva, 0.0).rgb;
    let c0 = textureSampleLevel(source, source_sampler, uv, 0.0).rgb;
    let cc = textureSampleLevel(source, source_sampler, uvc, 0.0).rgb;
    let cd = textureSampleLevel(source, source_sampler, uvd, 0.0).rgb;
    let dep = vec4<f32>(shaft_depth(uv), shaft_depth(uva), shaft_depth(uvc), shaft_depth(uvd));
    let k = ark.shaft_params.x;
    let mask = dot(clamp(dep * k, vec4<f32>(0.0), vec4<f32>(1.0)), vec4<f32>(0.25));
    let bloom_mask = dot(clamp((dep - vec4<f32>(0.5 / k)) * k, vec4<f32>(0.0), vec4<f32>(1.0)), vec4<f32>(0.25));
    let mean = (ca + c0 + cc + cd) * 0.25;
    let c = mean * ark.shaft_params.y;
    let lum = max(dot(mean, vec3<f32>(0.3, 0.59, 0.11)), 6.10351999e-05);
    let over = lum - ark.shaft_bloom.w;
    var bloom = vec3<f32>(0.0);
    if (over >= 0.0) {
        bloom = (over * (1.0 / lum) * c) * 2.0 * bloom_mask;
    }
    bloom = bloom * e_fade;
    let d = origin - uv * inv;
    let near = 1.0 - clamp(length(d) * 5.0, 0.0, 1.0);
    bloom = bloom * (near * near);
    return vec4<f32>(bloom * 0.25, max(mask, q4));
}

// Shader 157: 32 iterations of two taps along the segment towards the light, step
// min(|d|, sqrt(|d| / 2)) in aspect space, the weights a (b) for a >= 0.25 else 4 a^2 on rgb and
// a (b) on alpha, a from 2 and b from 1.96875 in steps of 0.0625, all / 64.
@fragment
fn shaft_blur(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let rt = vec2<f32>(textureDimensions(source));
    let origin = shaft_origin(rt);
    let inv = vec2<f32>(1.0, rt.y / rt.x);
    let asp = vec2<f32>(1.0, rt.x / rt.y);
    let d = origin - uv * inv;
    let len = length(d);
    let half_len = 0.5 * len;
    let m = min(half_len, 0.5 * sqrt(half_len));
    let step_asp = d * (m / max(half_len, 1e-20));
    let step_uv = step_asp * asp;
    let umin = vec2<f32>(0.0);
    let umax = (rt - vec2<f32>(1.0)) / rt;
    var acc = vec4<f32>(0.0);
    var a = 2.0;
    var b = 1.96875;
    var p1 = uv;
    var p2 = uv + step_uv * (1.0 / 64.0);
    for (var i = 0; i < 32; i = i + 1) {
        let w1 = select(4.0 * a * a, a, 4.0 * a * a - a >= 0.0);
        let w2 = select(4.0 * b * b, b, 4.0 * b * b - b >= 0.0);
        let t1 = textureSampleLevel(source, source_sampler, clamp(p1, umin, umax), 0.0);
        let t2 = textureSampleLevel(source, source_sampler, clamp(p2, umin, umax), 0.0);
        acc = acc + vec4<f32>(t1.rgb * w1, t1.a * a) + vec4<f32>(t2.rgb * w2, t2.a * b);
        a = a - 0.0625;
        b = b - 0.0625;
        p1 = p1 + step_uv * 0.03125;
        p2 = p2 + step_uv * 0.03125;
    }
    return acc * (1.0 / 64.0);
}

// Shader 155: scene x the occlusion factor (OcclusionMaskDarkness where the mask is 0,
// 1.5 - OcclusionMaskDarkness / 2 where it is 1, 1 far from the light) plus the tinted blurred
// bloom, attenuated by the scene's own brightness.
@fragment
fn shaft_apply(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let rt = vec2<f32>(textureDimensions(aux));
    let origin = shaft_origin(rt);
    let inv = vec2<f32>(1.0, rt.y / rt.x);
    let d = origin - uv * inv;
    let g = clamp(length(d) * 0.5, 0.0, 1.0);
    let blurred = textureSampleLevel(aux, source_sampler, uv, 0.0);
    let m = blurred.a * blurred.a;
    let tint = blurred.rgb * ark.shaft_bloom.xyz;
    let f0 = m * ark.shaft_params.z + (1.0 - m) * ark.shaft_params.w;
    let f = g + (1.0 - g) * f0;
    let scene = textureSampleLevel(source, source_sampler, uv, 0.0);
    let lum = dot(scene.rgb, vec3<f32>(0.3, 0.59, 0.11));
    let k = clamp(exp2(lum * -3.0) * ark.shaft_misc.x, 0.0, 1.0);
    return vec4<f32>(tint * k * 4.0 + scene.rgb * f, scene.a);
}

// FDisFogPixelShader: the fogged scene plus the blurred bloom parts (`sBloomParts`, added after
// the fog: `colour x prod(1 - f_i) + sum f_i C_i + bloom`).
@fragment
fn fog(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let pix = vec2<i32>(in.position.xy);
    let src = textureSampleLevel(source, source_sampler, in.uv, 0.0);
    let d = scene_depth(in.uv, pix);
    var f: Fog;
    f.transmittance = 1.0;
    f.inscatter = vec3<f32>(0.0);
    if (d.hit) {
        f = dis_fog(d.offset_uu, d.depth_uu);
    }
    let bloom = textureSampleLevel(aux, source_sampler, in.uv, 0.0).rgb;
    return vec4<f32>(src.rgb * f.transmittance + f.inscatter + bloom, src.a);
}

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let pix = vec2<i32>(in.position.xy);
    let src = textureSampleLevel(source, source_sampler, in.uv, 0.0);
    var colour = src.rgb;
    // Blink's / Bend Time's blur (Test_PPG `BlinkSwitch` / `BendTimeSwitch` -> `ArkPpNodeBlur`),
    // on the fogged scene.
    if (ark.power_vectors.x > 0.0 || ark.blink_lens.z > 0.5 || ark.possess.z > 0.5) {
        colour = power_blur(in.uv);
    }
    // Dark Vision's eyelid pass (`PPG_EyeLidFinal_Mat` / `PPG_DarkVisionFinal_Mat`, captured shader
    // 0x3c1c89ab574a7626): the scene (souls already blended in) x fxt_eyelid_gradient.g, faded
    // towards EyeLidColor by saturate(gradient.r - (1 - 2 Alpha)); Alpha 0 = lids shut.
    if (ark.dv_eyelid.x > 0.5) {
        let g = textureSampleLevel(field_mask, source_sampler, in.uv, 0.0);
        let k = clamp(g.r - (1.0 - 2.0 * ark.dv_eyelid.y), 0.0, 1.0);
        colour = mix(ark.dv_lid_color.rgb, g.g * colour, k);
    }
    if (ark.misc.z < 0.5) {
        return vec4<f32>(colour, src.a);
    }
    // FArkPpDofUber_*PS far blur: towards the quarter-size copy of the fogged scene (two 2x2
    // box downsamples, FArkPpDofDownsamplePS), sampled bilinearly at the pixel's own uv, by
    // saturate((depth - FocusDistance) / InFocusRadius) x FarBlurAmount.
    let d = scene_depth(in.uv, pix);
    let blur = clamp((d.depth_uu - ark.dof.x) * ark.dof.y, 0.0, 1.0) * ark.dof.z;
    if (blur > 0.0) {
        let low = textureSampleLevel(aux, source_sampler, in.uv, 0.0).rgb;
        colour = mix(colour, low, blur);
    }
    var display = colour_lut(colour);
    // Film grain: a per-pixel, per-frame hash in [0, 1) scaled to +- grain / 255.
    let g = fract(sin(dot(in.position.xy + vec2<f32>(ark.grain.z * 17.0, ark.grain.z * 59.0), vec2<f32>(12.9898, 78.233))) * 43758.5453);
    display = display + (g * ark.grain.x + ark.grain.y);
    return vec4<f32>(srgb_to_linear(clamp(display, vec3<f32>(0.0), vec3<f32>(1.0))), src.a);
}
