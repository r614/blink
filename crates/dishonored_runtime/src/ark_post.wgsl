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
