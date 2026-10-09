// The interactables' highlight (`Materials_ref.FX_materials.golden_focus_PMAT`, the
// `vfx_interactivity.interactivity_INST` permutation without switches), from its compiled unlit
// translucent base pass (pixel shader 36348; highlight.rs has the derivation).

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    mesh_view_bindings as view_bindings,
}

struct HighlightUniform {
    // F_Fresnel_Color.
    color: vec4<f32>,
    // (F_Fresnel_Power, F_Fresnel_Visibility, radiance to Bevy light units, 0).
    params: vec4<f32>,
    // SelectionColor.
    selection: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> hl: HighlightUniform;

@fragment
fn fragment(in: VertexOutput) -> FragmentOutput {
    // dp3 / rsq / mul_sat: saturate(dot(normalize(V), N)), the tangent-space camera vector's z.
    let v = normalize(view_bindings::view.world_position.xyz - in.world_position.xyz);
    let n_dot_v = saturate(dot(normalize(in.world_normal), v));
    // add / pow / cmp: (1 - NdotV)^Power, 0 when 1 - 0.999999 - NdotV < 0.
    let f = select(0.0, pow(1.0 - n_dot_v, hl.params.x), 0.999998987 - n_dot_v >= 0.0);
    let vis = hl.params.y;
    var out: FragmentOutput;
    let rgb = f * vis * hl.color.rgb + hl.selection.rgb;
    out.color = vec4<f32>(rgb * hl.params.z * view_bindings::view.exposure, saturate(f * saturate(vis)));
    return out;
}
