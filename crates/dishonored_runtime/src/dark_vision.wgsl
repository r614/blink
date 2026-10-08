// Dark Vision souls (dark_vision.rs): a revealed pawn drawn through walls with the tweak's
// colour ramp, m_LowerColor at the feet to m_UpperColor at the head, brighter towards the
// silhouette (APPROXIMATION of DarkVision_Souls_INST).

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    mesh_view_bindings::view,
}

struct Soul {
    lower: vec4<f32>,
    upper: vec4<f32>,
    // (feet y, head y, strength, 0), Bevy world units
    params: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> soul: Soul;

@fragment
fn fragment(in: VertexOutput) -> FragmentOutput {
    let h = clamp((in.world_position.y - soul.params.x) / max(soul.params.y - soul.params.x, 1e-3), 0.0, 1.0);
    let n = normalize(in.world_normal);
    let v = normalize(view.world_position.xyz - in.world_position.xyz);
    let rim = 1.0 - clamp(abs(dot(n, v)), 0.0, 1.0);
    let k = (0.25 + 0.75 * rim * rim) * soul.params.z;
    var out: FragmentOutput;
    out.color = vec4<f32>(mix(soul.lower.rgb, soul.upper.rgb, h) * k, 1.0);
    return out;
}
