// Arkane's bloom parts (see bloom.rs): the materials' bloom colour drawn over the scene's depth
// (TBloomPartMeshPixelShader), the tinted downsample to quarter size (FBloomDownSamplePixelShader,
// global shader 84) and the separable 5-tap Gaussian (TBloomBlurPixelShader 82 / vertex 83).

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput
#import bevy_render::view::View

#ifdef PARTS
@group(0) @binding(0) var<uniform> view: View;
#ifdef MULTISAMPLED
@group(0) @binding(1) var depth_texture: texture_depth_multisampled_2d;
#else
@group(0) @binding(1) var depth_texture: texture_depth_2d;
#endif
@group(1) @binding(0) var part_texture: texture_2d<f32>;
@group(1) @binding(1) var part_sampler: sampler;
@group(1) @binding(2) var<uniform> part_color: vec4<f32>;

struct PartIn {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
}

struct PartOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn part_vertex(v: PartIn) -> PartOut {
    var o: PartOut;
    o.clip = view.clip_from_world * vec4<f32>(v.position, 1.0);
    o.uv = v.uv;
    return o;
}

// BloomColor = E_Emissive (masked channel) [x E_Emissive_Color.rgb x a] x B_Bloom_Intensity, kept
// where the part is not behind the scene (reverse-Z: a larger scene depth is nearer).
@fragment
fn part_fragment(in: PartOut) -> @location(0) vec4<f32> {
    let c = textureSample(part_texture, part_sampler, in.uv).rgb * part_color.rgb;
    let scene = textureLoad(depth_texture, vec2<i32>(in.clip.xy), 0);
    if (scene > in.clip.z * 1.0002 + 1e-7) {
        discard;
    }
    return vec4<f32>(c, 1.0);
}
#endif

#ifdef DOWNSAMPLE
@group(0) @binding(0) var parts: texture_2d<f32>;
// gBloomTint: (m_Tint.rgb x m_Scale, -m_Threshold x m_Scale)
@group(0) @binding(1) var<uniform> tint: vec4<f32>;

// Four taps at +-1/4 of a quarter-size pixel (each between 2x2 parts texels): the 4x4 block's
// average, x tint + bias, at least 0.
@fragment
fn downsample(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(parts));
    let p = vec2<i32>(in.position.xy) * 4;
    var acc = vec3<f32>(0.0);
    for (var y = 0; y < 4; y = y + 1) {
        for (var x = 0; x < 4; x = x + 1) {
            acc = acc + max(textureLoad(parts, min(p + vec2<i32>(x, y), size - 1), 0).rgb, vec3<f32>(0.0));
        }
    }
    return vec4<f32>(max(acc / 16.0 * tint.rgb + tint.w, vec3<f32>(0.0)), 1.0);
}
#endif

#ifdef BLUR
@group(0) @binding(0) var blur_source: texture_2d<f32>;
@group(0) @binding(1) var blur_sampler: sampler;

// Taps at 0, +-1.3973, +-3.3973 texels along the pass's axis, weights 0.2416, 0.2643, 0.1149
// (a 9-tap Gaussian folded into bilinear taps).
@fragment
fn blur(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
#ifdef HORIZONTAL
    let d = vec2<f32>(1.0 / f32(textureDimensions(blur_source).x), 0.0);
#else
    let d = vec2<f32>(0.0, 1.0 / f32(textureDimensions(blur_source).y));
#endif
    var acc = textureSampleLevel(blur_source, blur_sampler, in.uv - d * 3.39731479, 0.0).rgb * 0.114869513;
    acc = acc + textureSampleLevel(blur_source, blur_sampler, in.uv - d * 1.39731467, 0.0).rgb * 0.264311969;
    acc = acc + textureSampleLevel(blur_source, blur_sampler, in.uv, 0.0).rgb * 0.241637036;
    acc = acc + textureSampleLevel(blur_source, blur_sampler, in.uv + d * 1.39731467, 0.0).rgb * 0.264311969;
    acc = acc + textureSampleLevel(blur_source, blur_sampler, in.uv + d * 3.39731479, 0.0).rgb * 0.114869513;
    return vec4<f32>(acc, 1.0);
}
#endif
