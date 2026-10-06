// UE3 distortion pass (see distortion.rs): the material distortion-accumulate pixel shader as
// decoded from the original's compiled shaders (the apply pass is distortion_apply.wgsl).

#import bevy_render::view::View

@group(0) @binding(0) var<uniform> view: View;
#ifdef MULTISAMPLED
@group(0) @binding(1) var depth_texture: texture_depth_multisampled_2d;
#else
@group(0) @binding(1) var depth_texture: texture_depth_2d;
#endif

@group(1) @binding(0) var dist_texture: texture_2d<f32>;
@group(1) @binding(1) var opacity_texture: texture_2d<f32>;
@group(1) @binding(2) var uv_dist_texture: texture_2d<f32>;
@group(1) @binding(3) var material_sampler: sampler;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) dist_offset: vec2<f32>,
    @location(3) uv_dist_offset: vec2<f32>,
    @location(4) opacity_offset: vec2<f32>,
    // (Dist_Distortion_Intensity x tweak alpha x vertex alpha, opacity texture on, UV_Distortion_Intensity, 0)
    @location(5) k: vec4<f32>,
}

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) dist_offset: vec2<f32>,
    @location(2) uv_dist_offset: vec2<f32>,
    @location(3) opacity_offset: vec2<f32>,
    @location(4) k: vec4<f32>,
}

@vertex
fn vertex(v: VertexIn) -> VertexOut {
    var o: VertexOut;
    o.clip = view.clip_from_world * vec4<f32>(v.position, 1.0);
    o.uv = v.uv;
    o.dist_offset = v.dist_offset;
    o.uv_dist_offset = v.uv_dist_offset;
    o.opacity_offset = v.opacity_offset;
    o.k = v.k;
    return o;
}

fn scene_depth(uv: vec2<f32>) -> f32 {
    let size = vec2<f32>(textureDimensions(depth_texture));
    let p = vec2<i32>(clamp(uv * size, vec2<f32>(0.0), size - 1.0));
#ifdef MULTISAMPLED
    return textureLoad(depth_texture, p, 0);
#else
    return textureLoad(depth_texture, p, 0);
#endif
}

@fragment
fn accumulate(in: VertexOut) -> @location(0) vec4<f32> {
    // Material Distortion input (DefaultAdditive_PMAT family, e.g. RefShaderCache 36094 and its
    // uniform expressions): Dist_Distortion.r at the (UV-distorted, panned) uv x intensity x
    // tweak alpha x vertex alpha x O_Opacity.r at the scrolled uv.
    var uv = in.uv;
    if (in.k.z != 0.0) {
        uv = uv + textureSample(uv_dist_texture, material_sampler, in.uv + in.uv_dist_offset).rg * in.k.z;
    }
    var d = textureSample(dist_texture, material_sampler, uv + in.dist_offset).r * in.k.x;
    if (in.k.y > 0.5) {
        d = d * textureSample(opacity_texture, material_sampler, in.uv + in.opacity_offset).r;
    }
    // The Distortion input is a scalar here, so both offset components are equal.
    let dd = vec2<f32>(d, d);
    // dp2add(D, D, -0.1); texkill
    if (dot(dd, dd) - 0.1 < 0.0) {
        discard;
    }
    // Scene depth at the displaced pixel; zero where the scene is in front of the primitive
    // (reverse-Z: a larger depth value is nearer).
    let screen_uv = (in.clip.xy - view.viewport.xy) / view.viewport.zw;
    let displaced = screen_uv + dd * vec2<f32>(1.0 / 255.0, -1.0 / 255.0);
    var e = clamp(dd * (4.0 / 255.0), vec2<f32>(-1.0), vec2<f32>(1.0));
    if (scene_depth(displaced) > in.clip.z) {
        e = vec2<f32>(0.0);
    }
    return vec4<f32>(max(e, vec2<f32>(0.0)), max(-e, vec2<f32>(0.0)));
}
