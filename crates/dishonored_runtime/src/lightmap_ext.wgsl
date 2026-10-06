// Standard PBR forward fragment plus Dishonored's two-coefficient directional lightmap,
// reconstructed per pixel like the original base pass (`TBasePassPixelShader
// FDirectionalLightMapTexturePolicy*` in RefShaderCache-PC-D3D-SM3.upk, research/LOG.md
// "Directional lightmaps"):
//   color = NormalizedAverageColor * scale0, dir = DirectionalMaxComponent * scale1
//   diffuse  = albedo * color * sum_i dir_i * max(saturate(N.B_i)^2, 1e-6)
//   specular = spec   * color * sum_i dir_i * max(saturate(R.B_i), 1e-6)^(SpecularPower + 1),
//              R = reflect(-V, N)
// with N, V in UE3 tangent space (X = dP/du, Y = dP/dv, Z = normal) and B_i the HL2 basis.
// UV1 already holds atlas coordinates (the instance's region is baked into the merged
// vertices), so one bind group serves every batch of a material.

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    forward_io::{VertexOutput, FragmentOutput},
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
}

// x = exposure (UE3 lightmap unit to Bevy), y = specular colour / Bevy reflectance (0 = no
// specular), z = lightmap specular exponent (SpecularPower + 1).
@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> baked_params: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var baked_lightmap: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var baked_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var baked_directional: texture_2d<f32>;

const B0 = vec3<f32>(0.0, 0.81649661, 0.57735026);
const B1 = vec3<f32>(-0.70710677, -0.40824830, 0.57735026);
const B2 = vec3<f32>(0.70710677, -0.40824830, 0.57735026);

fn basis_weights(n: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(saturate(dot(n, B0)), saturate(dot(n, B1)), saturate(dot(n, B2)));
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);
    var lightmap_specular = vec3<f32>(0.0);
#ifdef VERTEX_UVS_B
    // Mip 0 only: lightmap atlases bleed across charts when mipmapped.
    let color = textureSampleLevel(baked_lightmap, baked_sampler, in.uv_b, 0.0).rgb * baked_params.x;
    let dir = textureSampleLevel(baked_directional, baked_sampler, in.uv_b, 0.0).rgb;
    // Tangent frame from the geometric normal and the UV derivatives (dP/du, dP/dv); these
    // directions survive the UE3 -> Bevy mirroring, unlike a handedness sign.
    let ng = normalize(pbr_input.world_normal);
    let dp1 = dpdx(in.world_position.xyz);
    let dp2 = dpdy(in.world_position.xyz);
    let duv1 = dpdx(in.uv);
    let duv2 = dpdy(in.uv);
    let dp2perp = cross(dp2, ng);
    let dp1perp = cross(ng, dp1);
    var t = dp2perp * duv1.x + dp1perp * duv2.x;
    var b = dp2perp * duv1.y + dp1perp * duv2.y;
    t = t - ng * dot(ng, t);
    b = b - ng * dot(ng, b);
    let tl = length(t);
    let bl = length(b);
    var n_ts = vec3<f32>(0.0, 0.0, 1.0);
    var v_ts = vec3<f32>(0.0, 0.0, 1.0);
    if (tl > 1e-12 && bl > 1e-12 && baked_params.w != 2.0) {
        t = t / tl;
        b = b / bl;
        let n = normalize(pbr_input.N);
        n_ts = normalize(vec3<f32>(dot(n, t), dot(n, b), dot(n, ng)));
        v_ts = normalize(vec3<f32>(dot(pbr_input.V, t), dot(pbr_input.V, b), dot(pbr_input.V, ng)));
    }
    let wd = basis_weights(n_ts);
    pbr_input.lightmap_light = color * dot(dir, max(wd * wd, vec3<f32>(1e-6)));
    let r = 2.0 * n_ts * dot(n_ts, v_ts) - v_ts;
    // Lobe exponent = material SpecularPower + 1 (the base pass computes pow(x, SP + 1);
    // a constant SP 15 folds into four squarings).
    let ws = pow(max(basis_weights(r), vec3<f32>(1e-6)), vec3<f32>(baked_params.z));
    // UE3 specular colour = S_ texture (masked channel) [* S_Specular_color.rgb]; Bevy's
    // reflectance holds 0.5 * a * colour * texture, so y = 2 / a (0 without specular).
    lightmap_specular = color * dot(dir, ws) * pbr_input.material.reflectance * baked_params.y;
    if (baked_params.w >= 3.0) {
        // Debug views (DISHONORED_LIGHTMAP_DEBUG): 3 lobe weights, 4 tangent-space view
        // vector, 5 tangent-space normal, 6 diffuse irradiance, 7 basis intensities.
        var dbg = ws;
        if (baked_params.w == 4.0) { dbg = v_ts * 0.5 + 0.5; }
        if (baked_params.w == 5.0) { dbg = n_ts * 0.5 + 0.5; }
        if (baked_params.w == 6.0) { dbg = pbr_input.lightmap_light / baked_params.x; }
        if (baked_params.w == 7.0) { dbg = dir; }
        var o: FragmentOutput;
        o.color = vec4<f32>(dbg, 1.0);
        return o;
    }
#endif
    var out: FragmentOutput;
    if (pbr_input.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        out.color = apply_pbr_lighting(pbr_input);
        out.color = vec4<f32>(out.color.rgb + lightmap_specular, out.color.a);
    } else {
        out.color = pbr_input.material.base_color;
    }
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
