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
    pbr_types::{STANDARD_MATERIAL_FLAGS_UNLIT_BIT, PbrInput},
    pbr_functions::{calculate_diffuse_color, calculate_F0_dielectric},
    mesh_view_bindings as view_bindings,
    mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT,
    mesh_types::MESH_FLAGS_SHADOW_RECEIVER_BIT,
    lighting,
    lighting::LAYER_BASE,
    shadows,
}

// x = exposure (UE3 lightmap unit to Bevy), y = specular colour / Bevy reflectance (0 = no
// specular), z = lightmap specular exponent (SpecularPower + 1).
@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> baked_params: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var baked_lightmap: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var baked_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var baked_directional: texture_2d<f32>;
// Dominant directional light distance-field shadow (`FSignedDistanceFieldShadowTexturePolicy`,
// RefShaderCache light pass, e.g. 1445): s = saturate((d + x) * y), 0 below 1e-6, else s^z.
// baked_shadow_params = (on, bias, scale, exponent).
@group(#{MATERIAL_BIND_GROUP}) @binding(104) var baked_shadow: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(105) var<uniform> baked_shadow_params: vec4<f32>;

// The dominant directional light (index 0) as Bevy's PBR applies it, unshadowed.
fn dominant_light(pbr_input: PbrInput) -> vec3<f32> {
    let m = pbr_input.material;
    var li: lighting::LightingInput;
    li.layers[LAYER_BASE].NdotV = max(dot(pbr_input.N, pbr_input.V), 0.0001);
    li.layers[LAYER_BASE].N = pbr_input.N;
    li.layers[LAYER_BASE].R = reflect(-pbr_input.V, pbr_input.N);
    li.layers[LAYER_BASE].perceptual_roughness = m.perceptual_roughness;
    li.layers[LAYER_BASE].roughness = lighting::perceptualRoughnessToRoughness(m.perceptual_roughness);
    li.P = pbr_input.world_position.xyz;
    li.V = pbr_input.V;
    li.diffuse_color = calculate_diffuse_color(m.base_color.rgb, m.metallic, m.specular_transmission, m.diffuse_transmission);
    li.metallic = m.metallic;
    li.F0_dielectric = calculate_F0_dielectric(m.reflectance);
    li.F0_metallic = m.base_color.rgb;
    li.F_ab = lighting::F_AB(m.perceptual_roughness, li.layers[LAYER_BASE].NdotV);
    return lighting::directional_light(0u, &li, true);
}

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
    if (baked_params.w >= 3.0 && baked_params.w < 8.0) {
        // Debug views (DISHONORED_LIGHTMAP_DEBUG): 3 lobe weights, 4 tangent-space view
        // vector, 5 tangent-space normal, 6 diffuse irradiance, 7 basis intensities (8, the
        // distance-field shadow, is drawn after the lighting).
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
#ifdef VERTEX_UVS_B
        // Where the original shadows the dominant light with the component's distance field,
        // swap the cascade factor Bevy applied for the distance-field one.
        if (baked_shadow_params.x > 0.0 && view_bindings::lights.n_directional_lights > 0u) {
            let d = textureSampleLevel(baked_shadow, baked_sampler, in.uv_b, 0.0).r;
            var s = saturate((d + baked_shadow_params.y) * baked_shadow_params.z);
            s = select(0.0, pow(s, baked_shadow_params.w), s - 1e-6 >= 0.0);
            var cascade = 1.0;
            if ((pbr_input.flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u
                    && (view_bindings::lights.directional_lights[0].flags & DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u) {
                let view_z = dot(vec4<f32>(
                    view_bindings::view.view_from_world[0].z,
                    view_bindings::view.view_from_world[1].z,
                    view_bindings::view.view_from_world[2].z,
                    view_bindings::view.view_from_world[3].z
                ), pbr_input.world_position);
                cascade = shadows::fetch_directional_shadow(0u, pbr_input.world_position, pbr_input.world_normal, view_z, pbr_input.frag_coord.xy);
            }
            let light = dominant_light(pbr_input) * view_bindings::view.exposure;
            out.color = vec4<f32>(max(out.color.rgb + (s - cascade) * light, vec3<f32>(0.0)), out.color.a);
            if (baked_params.w == 8.0) {
                out.color = vec4<f32>(s, d, cascade, 1.0);
            }
        }
#endif
    } else {
        out.color = pbr_input.material.base_color;
    }
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
