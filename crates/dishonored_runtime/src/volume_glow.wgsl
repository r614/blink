// Procedural volumetric glow of the untextured particle mesh materials (Volume_Light_PMAT /
// Glow_PMAT instances): the cooker strips their expression graphs and keeps only the
// parameters (Color, LightIntensity, Fresnel_exponent, SphereMask_Power, StartOffset, ...), so
// the graph is rebuilt from those: additive colour x intensity, faded toward the mesh's
// silhouette by |N.V|^Fresnel_exponent, near the camera over StartOffset and away from the
// particle by the sphere mask (SphereMask_Power over Distance) (APPROXIMATION,
// EFFECTS.md "Material graphs").

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    mesh_view_bindings::view,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> base_color: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var<uniform> glow_params: vec4<f32>;
// xyz = sphere mask centre (the particle, world), w = radius (Distance x mesh radius x size).
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var<uniform> glow_sphere: vec4<f32>;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    let n = normalize(in.world_normal);
    let to_cam = view.world_position.xyz - in.world_position.xyz;
    let v = normalize(to_cam);
    let facing = pow(clamp(abs(dot(n, v)), 0.0, 1.0), max(glow_params.x, 0.01));
    // Camera fade: nothing at the camera, full beyond StartOffset (Bevy units).
    let fade = clamp(length(to_cam) / max(glow_params.y, 1e-3), 0.0, 1.0);
    // Sphere mask (SphereMask_Power in glow_params.z): fades out away from the particle.
    let r = clamp(1.0 - length(in.world_position.xyz - glow_sphere.xyz) / max(glow_sphere.w, 1e-3), 0.0, 1.0);
    let mask = pow(r, max(glow_params.z, 0.01));
    var out: FragmentOutput;
    // Additive: Bevy blends AlphaMode::Add as premultiplied alpha, so alpha 0 adds the colour.
    out.color = vec4<f32>(base_color.rgb * facing * fade * mask, 0.0);
    return out;
}
