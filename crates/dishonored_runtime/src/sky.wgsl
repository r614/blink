// Skies_PMAT, the sky dome material, as its compiled base pass computes it (unlit emissive;
// RefShaderCache `TBasePassPixelShaderFNoLightMapPolicyNoSkyLightFALSEFALSE` of the
// Skies_PMAT shader maps and their uniform expressions, research/LOG.md "Sky dome"). See
// sky.rs for the terms.

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    mesh_view_bindings::globals,
}

struct Sky {
    // (gradient, clouds, clouds 2, storm) switches, 1 = on
    flags: vec4<f32>,
    // D_Diffuse_color.rgb x a
    diffuse_color: vec4<f32>,
    grad_top: vec4<f32>,
    grad_bottom: vec4<f32>,
    // C_Clouds_Color.rgb x a, C2_Clouds_2_Color.rgb x a
    clouds_color: vec4<f32>,
    clouds2_color: vec4<f32>,
    // Storm_Color.rgb, w = Storm_Intensity + Storm_Min_Intensity
    storm_color: vec4<f32>,
    // (C_Clouds_Speed, C2_Clouds_2_Speed, Cloud_Distortion, G_Gradiant_Power)
    speeds: vec4<f32>,
    // (C_Clouds_Visibility, C2_Clouds_2_Visibility, sky_contrast, U_Offset)
    misc: vec4<f32>,
    // (Storm_Offset, Use_UV_Channel_2, H_HorizonIntensity, Storm_mask channel or -1 for RGB)
    misc2: vec4<f32>,
    // Horizon_Color.rgb x a, w = H_Enable_Horizon
    horizon: vec4<f32>,
    // (H_Horizon_Speed, H_Horizon_UV_Distortion, H_HorizonOffset, H_HorizonExponent)
    horizon2: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> sky: Sky;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var sky_any: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var sky_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var sky_diffuse: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var sky_clouds: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(5) var sky_clouds2: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(6) var sky_storm: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(7) var sky_noise: texture_2d<f32>;

// The cloud layers turn about the texture centre by Speed x Time x 0.1 radians.
fn rotated(uv: vec2<f32>, speed: f32) -> vec2<f32> {
    let a = speed * globals.time * 0.1;
    let c = cos(a);
    let s = sin(a);
    let d = uv - 0.5;
    return vec2<f32>(dot(vec2<f32>(c, -s), d), dot(vec2<f32>(s, c), d)) + 0.5;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var uv0 = vec2<f32>(0.0);
    var uv1 = vec2<f32>(0.0);
#ifdef VERTEX_UVS_A
    uv0 = in.uv;
#endif
#ifdef VERTEX_UVS_B
    uv1 = in.uv_b;
#else
    uv1 = uv0;
#endif
    var clouds = vec3<f32>(0.0);
    if (sky.flags.y > 0.5) {
        clouds = textureSample(sky_clouds, sky_sampler, rotated(uv0, sky.speeds.x)).rgb;
    }
    var colour: vec3<f32>;
    if (sky.flags.x > 0.5) {
        // Gradient from G_Gradiant_Top to G_Gradiant_Bottom over |v + clouds.r x distortion|^power.
        let v = abs(clouds.r * sky.speeds.z + uv0.y);
        let w = select(0.0, pow(v, sky.speeds.w), v - 1e-6 >= 0.0);
        colour = sky.grad_top.rgb + (sky.grad_bottom.rgb - sky.grad_top.rgb) * w;
    } else {
        // With Use_UV_Channel_2 at (U_Offset - UV1.x, UV1.y), else at (-UV0.x, UV0.y) (6046 / 6044).
        var duv = vec2<f32>(-uv0.x, uv0.y);
        if (sky.misc2.y > 0.5) {
            duv = vec2<f32>(sky.misc.w - uv1.x, uv1.y);
        }
        duv = duv + clouds.r * sky.speeds.z;
        colour = textureSample(sky_diffuse, sky_sampler, duv).rgb;
    }
    colour = colour * sky.diffuse_color.rgb;
    if (sky.flags.y > 0.5) {
        colour = colour + clouds * sky.misc.x * (sky.clouds_color.rgb * clouds - colour);
    }
    if (sky.flags.z > 0.5) {
        let c2 = textureSample(sky_clouds2, sky_sampler, rotated(uv0, sky.speeds.y)).rgb;
        colour = colour + c2 * sky.misc.y * (sky.clouds2_color.rgb * c2 - colour);
    }
    colour = pow(max(abs(colour), vec3<f32>(1e-6)), vec3<f32>(sky.misc.z));
    if (sky.horizon.w > 0.5) {
        // H_Enable_Horizon (6044, 6059): the contrast result saturated, then lerped to the horizon
        // colour by the horizon band over UV1.y, distorted by the noise Tex#1.
        colour = saturate(colour);
        let pan = fract(sky.horizon2.x * globals.time * vec2<f32>(0.01, 0.025));
        let noise = textureSample(sky_noise, sky_sampler, vec2<f32>(2.0 * uv1.x, 0.25 * uv1.y) + pan).r;
        let h = saturate((noise * sky.horizon2.y + uv1.y + sky.horizon2.z) * uv1.y * 0.5 + sky.horizon2.z);
        let k = select(0.0, pow(max(h, 1e-6), sky.horizon2.w) * sky.misc2.z, h - 1e-6 >= 0.0);
        colour = colour + k * (sky.horizon.rgb - colour);
    }
    if (sky.flags.w > 0.5) {
        let st = textureSample(sky_storm, sky_sampler, vec2<f32>(sky.misc2.x + uv1.x, uv1.y));
        // Storm_mask: one channel for all three (6059: R), or RGB.
        var storm = st.rgb;
        if (sky.misc2.w >= 0.0) {
            storm = vec3<f32>(st[i32(sky.misc2.w)]);
        }
        colour = colour + storm * sky.storm_color.rgb * sky.storm_color.w;
    }
    var out: FragmentOutput;
    out.color = vec4<f32>(colour, 1.0);
    return out;
}
