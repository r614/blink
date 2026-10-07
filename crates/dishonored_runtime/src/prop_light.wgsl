// Doors and usable objects lit by the original's light environment (prop_light.rs): the base
// pass TBasePassPixelShader<FDirectionalLightLightMapPolicy, SkyLight> of generic_PMAT on the
// skinned vertex factory (e.g. 15153):
//
//   colour = (saturate(N.L) x diffuse + S x pow(saturate(R.L), SP) x (SP + 8) x 0.0397887) x LightColor
//          + diffuse x (UpperSkyColor x ((1 + N.Z) / 2)^2 + LowerSkyColor x ((1 - N.Z) / 2)^2)
//
// (0.0397887 = 1 / (8 pi); each lobe 0 below 1e-6). Bevy adds the dominant / toggleable lights;
// its ambient and environment light are removed.

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing, calculate_diffuse_color},
    forward_io::{VertexOutput, FragmentOutput},
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
    mesh_view_bindings as view_bindings,
}

struct PropLightUniform {
    direction: vec4<f32>,
    color: vec4<f32>,
    upper_sky: vec4<f32>,
    lower_sky: vec4<f32>,
    spec: vec4<f32>,
    spec_color: vec4<f32>,
    spec_mask: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> env: PropLightUniform;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var env_specular: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var env_specular_sampler: sampler;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);
    var out: FragmentOutput;
    if (pbr_input.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) != 0u {
        out.color = main_pass_post_lighting_processing(pbr_input, pbr_input.material.base_color);
        return out;
    }
    let m = pbr_input.material;
    let diffuse = calculate_diffuse_color(m.base_color.rgb, m.metallic, m.specular_transmission, m.diffuse_transmission);
    var base = vec3<f32>(0.0);
    if (env.color.w > 0.0) {
        let n = normalize(pbr_input.N);
        let v = normalize(pbr_input.V);
        let l = env.direction.xyz;
        let ndotl = saturate(dot(n, l));
        let rdotl = saturate(dot(reflect(-v, n), l));
        var s = vec3<f32>(0.0);
#ifdef VERTEX_UVS_A
        if (env.spec.y > 0.0) {
            let t = textureSample(env_specular, env_specular_sampler, in.uv);
            // The masked channel (or rgb), times the colour when S_Enable_Specular_color is on.
            let masked = select(t.rgb, vec3<f32>(dot(t, env.spec_mask)), env.spec.z > 0.5);
            s = masked * env.spec_color.rgb;
        }
#endif
        let sp = env.spec.x;
        let spec = select(vec3<f32>(0.0), s * pow(rdotl, sp) * (sp + 8.0) * 0.0397887342, rdotl - 1e-6 >= 0.0);
        let dif = select(vec3<f32>(0.0), ndotl * diffuse, ndotl - 1e-6 >= 0.0);
        // Bevy world +Y is UE3 +Z.
        let w = dot(n, vec3<f32>(0.0, 1.0, 0.0));
        let up = 0.5 + 0.5 * w;
        let lo = 0.5 - 0.5 * w;
        let sky = diffuse * (env.upper_sky.rgb * up * up + env.lower_sky.rgb * lo * lo);
        base = (dif + spec) * env.color.rgb + sky;
    }
    // The original has no ambient or environment light on these props.
    pbr_input.diffuse_occlusion = vec3<f32>(0.0);
    pbr_input.specular_occlusion = 0.0;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = vec4<f32>(out.color.rgb + base * view_bindings::view.exposure, out.color.a);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
