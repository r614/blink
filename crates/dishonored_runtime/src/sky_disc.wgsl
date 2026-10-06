// Moon_PMAT / Sun_PMAT as their compiled base passes compute them (see sky_disc.rs).

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    mesh_view_bindings::globals,
}

struct Disc {
    // moon: Moon_Color.rgb x a; sun: Color.rgb x SunIntensity
    color: vec4<f32>,
    // (kind 0 moon / 1 sun, Cloud_Speed, SphereMask_Power, 0)
    params: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> disc: Disc;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var disc_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var disc_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var disc_clouds: texture_2d<f32>;

@fragment
fn fragment(in: VertexOutput) -> FragmentOutput {
    var uv = vec2<f32>(0.0);
#ifdef VERTEX_UVS_A
    uv = in.uv;
#endif
    var out: FragmentOutput;
    if (disc.params.x > 0.5) {
        // Sun: a cone mask about the UV centre.
        let m = max(1.0 - 2.0 * length(uv - 0.5), 0.0);
        let c = select(vec3<f32>(0.0), pow(m, disc.params.z) * disc.color.rgb, m - 1e-6 >= 0.0);
        out.color = vec4<f32>(c, 1.0);
        return out;
    }
    // Moon: the disc texture's red is the colour, its green the shape; the clouds scroll in u.
    let t = textureSample(disc_tex, disc_sampler, uv);
    let cuv = vec2<f32>(uv.x * 0.15 + disc.params.y * globals.time, uv.y);
    let cloud = saturate(textureSample(disc_clouds, disc_sampler, cuv).r);
    out.color = vec4<f32>(t.r * disc.color.rgb, cloud * t.g);
    return out;
}
