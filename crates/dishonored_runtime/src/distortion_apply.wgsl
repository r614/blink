// UE3 distortion apply pass (global cache shader 237), see distortion.rs.

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

@group(0) @binding(0) var scene_color: texture_2d<f32>;
@group(0) @binding(1) var offsets: texture_2d<f32>;
@group(0) @binding(2) var apply_sampler: sampler;

@fragment
fn apply(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    // offset = (RG - BA) x (0.25, -0.25); outside SceneColorRect the undistorted pixel. The
    // original's RGBA8 target saturates the sum at 1.
    let a = min(textureSampleLevel(offsets, apply_sampler, in.uv, 0.0), vec4<f32>(1.0));
    var uv = in.uv + (a.xy - a.zw) * vec2<f32>(0.25, -0.25);
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) {
        uv = in.uv;
    }
#ifdef DISTORTION_DEBUG
    let base = textureSampleLevel(scene_color, apply_sampler, in.uv, 0.0);
    return vec4<f32>(base.rgb * 0.3 + vec3<f32>(a.x + a.z, a.y + a.w, 0.0) * 4.0, base.a);
#else
    return textureSampleLevel(scene_color, apply_sampler, uv, 0.0);
#endif
}
