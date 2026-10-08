// Dark Vision souls: DarkVision_Souls_INST's soul pass (RefShaderCache 35003, dark_vision.rs).
// Two passes share this shader: in front of the scene (Full_Color, alpha 0.8) and hidden by it
// (Behind_color + noise, rim alpha); both fade out between vNearFarParams.x and .y.

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    mesh_view_bindings::{view, globals},
}

struct Soul {
    full: vec4<f32>,
    behind: vec4<f32>,
    // (fade start, fade end) Bevy units, behind pass flag, 0
    params: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> soul: Soul;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var tex0: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var tex_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var tex1: texture_2d<f32>;

@fragment
fn fragment(in: VertexOutput) -> FragmentOutput {
    var uv = vec2<f32>(0.0);
#ifdef VERTEX_UVS_A
    uv = in.uv;
#endif
    let view_pos = view.view_from_world * vec4<f32>(in.world_position.xyz, 1.0);
    let depth = -view_pos.z;
    // 1 - smoothstep(near, far, depth)
    let x = clamp((depth - soul.params.x) / max(soul.params.y - soul.params.x, 1e-3), 0.0, 1.0);
    let fade = 1.0 - (3.0 - 2.0 * x) * x * x;
    var out: FragmentOutput;
    if (soul.params.z < 0.5) {
        // In front of the scene: Full_Color, rim alpha. UE3 view space looks along +z (Bevy's
        // -z): the normal's view z is -1 facing the camera.
        let nz = -(view.view_from_world * vec4<f32>(normalize(in.world_normal), 0.0)).z;
        let a = 0.7 * (0.5 + 0.5 * (nz + 1.0) * (nz + 1.0));
        out.color = vec4<f32>(soul.full.rgb * fade, a * fade);
        return out;
    }
    // Hidden by the scene: Behind_color plus the panning noise, alpha 0.8.
    let t = globals.time;
    let d = textureSample(tex0, tex_sampler, uv + fract(vec2<f32>(0.05 * t))).rg;
    let n = textureSample(tex1, tex_sampler, uv + fract(vec2<f32>(-0.05 * t)) + 0.4 * d).g;
    out.color = vec4<f32>((soul.behind.rgb + 0.3 * n) * fade, 0.8 * fade);
    return out;
}
