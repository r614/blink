// Procedural volumetric glow of the untextured particle mesh materials (Volume_Light_PMAT /
// Glow_PMAT instances): the cooker strips their expression graphs and keeps only the
// parameters (Color, LightIntensity, Fresnel_exponent, SphereMask_Power, StartOffset, ...), so
// the graph is rebuilt from those: additive colour x intensity, faded toward the mesh's
// silhouette by |N.V|^Fresnel_exponent and near the camera over StartOffset (APPROXIMATION,
// EFFECTS.md "Material graphs").

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    mesh_view_bindings::view,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> base_color: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var<uniform> glow_params: vec4<f32>;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    let n = normalize(in.world_normal);
    let to_cam = view.world_position.xyz - in.world_position.xyz;
    let v = normalize(to_cam);
    let facing = pow(clamp(abs(dot(n, v)), 0.0, 1.0), max(glow_params.x, 0.01));
    // Camera fade: nothing at the camera, full beyond StartOffset (Bevy units).
    let fade = clamp(length(to_cam) / max(glow_params.y, 1e-3), 0.0, 1.0);
    var out: FragmentOutput;
    out.color = vec4<f32>(base_color.rgb * facing * fade, 1.0);
    return out;
}
