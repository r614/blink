// Falling water sheets of `Vfx_Materials.main_materials.water_spillway_translu_PMAT` (sewer
// outflows, the Pub's spillways, the outro's whisky), from the material's compiled base passes in
// RefShaderCache-PC-D3D-SM3.upk (unlit, BLEND_Translucent; research/dishonored/RENDERING.md
// "Water sheets"): 1585 (1584 adds the vertex fog), 1596 with `River` (1595 with fog).
//
//   n    = 2 Normal(SpillWay_Tilling (u, 2 v) + (0, frac(-0.7 SpillWay_Distortion_Speed t))) - 1
//   s    = SpillWay_Texture(uv0 + SpillWay_Distortion n.xy)          (n not normalised here)
//   R    = reflect the tangent-space camera vector about normalize(n), then multiplied by the
//          instance's LocalToWorld (the shader treats the tangent-space vector as a local one)
//   c    = TextureCube(R) x C_Cubemap_color.rgb x a
//   d    = Tex#2(0.5 u + 0.25, 10 v + frac(-3 SpillWay_Droplets_Speed t)).b x SpillWay_Droplets_Intensity
//   m    = Tex#3(uv0).b
//   k    = saturate(s.r x ((1 - m) d + s.r)) + C_Cube_Map_Min_Value
//   rgb  = c x k x D_DiffuseColorTweak.rgb x a (+ SelectionColor 0)
//   a    = saturate(s.g x (d + m)) x SpillWay_Translucency        (River: saturate(s.g) x Translucency)

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    mesh_view_bindings as view_bindings,
}

struct SpillwayUniform {
    tweak: vec4<f32>,
    cube: vec4<f32>,
    params: vec4<f32>,
    params2: vec4<f32>,
    l2w_x: vec4<f32>,
    l2w_y: vec4<f32>,
    l2w_z: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> sw: SpillwayUniform;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var sw_normal: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var sw_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var sw_spill: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var sw_droplets: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(5) var sw_mask: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(6) var sw_cube: texture_cube<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(7) var sw_cube_sampler: sampler;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var uv = vec2<f32>(0.0);
#ifdef VERTEX_UVS_A
    uv = in.uv;
#endif
    let t_now = view_bindings::globals.time;
    let tiling = sw.params.x;
    let nuv = vec2<f32>(tiling * uv.x, 2.0 * tiling * uv.y) + vec2<f32>(0.0, fract(-0.7 * sw.params.z * t_now));
    let n_raw = textureSample(sw_normal, sw_sampler, nuv).xyz * 2.0 - 1.0;
    let n = normalize(n_raw);
    let s = textureSample(sw_spill, sw_sampler, uv + sw.params.y * n_raw.xy);
    // The tangent-space camera vector (UE3's tangent frame: X = dP/du, Y = dP/dv, Z = normal;
    // the normal map is DirectX-style, as in ark_water.wgsl).
    let ng = normalize(in.world_normal);
    var t = vec3<f32>(1.0, 0.0, 0.0);
    var b = vec3<f32>(0.0, 0.0, 1.0);
#ifdef VERTEX_TANGENTS
    t = normalize(in.world_tangent.xyz - ng * dot(ng, in.world_tangent.xyz));
    b = cross(ng, t) * in.world_tangent.w;
#endif
    let v = normalize(view_bindings::view.world_position.xyz - in.world_position.xyz);
    let v_ts = vec3<f32>(dot(v, t), -dot(v, b), dot(v, ng));
    let r_ts = 2.0 * dot(n, v_ts) * n - v_ts;
    // The shader multiplies the tangent-space vector by LocalToWorld: as a UE3 local vector
    // (x, y, z) it is the Bevy local vector (x, z, y), taken to the world by the instance's
    // rotation and scale.
    let r_ws = sw.l2w_x.xyz * r_ts.x + sw.l2w_y.xyz * r_ts.z + sw.l2w_z.xyz * r_ts.y;
    let c = textureSample(sw_cube, sw_cube_sampler, r_ws).rgb * sw.cube.rgb;
    let d = textureSample(sw_droplets, sw_sampler, vec2<f32>(0.5 * uv.x + 0.25, 10.0 * uv.y + fract(-3.0 * sw.params.w * t_now))).b * sw.params2.x;
    let m = textureSample(sw_mask, sw_sampler, uv).b;
    let k = saturate(s.r * ((1.0 - m) * d + s.r)) + sw.cube.w;
    var a = saturate(s.g * (d + m)) * sw.params2.y;
    if (sw.params2.z > 0.0) {
        a = saturate(s.g) * sw.params2.y;
    }
    // Single-sided materials: UE3 culls the back faces.
    if (sw.params2.w <= 0.0 && !is_front) {
        a = 0.0;
    }
    var out: FragmentOutput;
    // Unlit: UE3 radiance to Bevy light units (tweak.w = map_render::light_unit) and the view's
    // exposure, as for the lightmaps.
    out.color = vec4<f32>(c * k * sw.tweak.rgb * sw.tweak.w * view_bindings::view.exposure, a);
    return out;
}
