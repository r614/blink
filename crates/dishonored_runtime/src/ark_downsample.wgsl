// FArkPpDofDownsamplePS (global shader 63, with FArkPpDofDownsampleVS 62): one bilinear tap at
// the shared corner of a 2x2 block of the source, i.e. the block's average; the ArkPp DOF runs
// it twice (full -> half -> quarter size, 0x00922210). Averaged exactly here.

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

@group(0) @binding(0) var source: texture_2d<f32>;

@fragment
fn downsample(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(source));
    let p = vec2<i32>(in.position.xy) * 2;
    var acc = vec4<f32>(0.0);
    for (var y = 0; y < 2; y = y + 1) {
        for (var x = 0; x < 2; x = x + 1) {
            acc = acc + textureLoad(source, min(p + vec2<i32>(x, y), size - 1), 0);
        }
    }
    return acc * 0.25;
}
