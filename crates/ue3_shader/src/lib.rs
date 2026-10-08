// Adapted from dishonored-bevy by Eamon (github.com/Eamo5/dishonored-bevy), used with permission.
//! The original's compiled D3D9 SM3 shaders (RefShaderCache / GlobalShaderCache) translated to
//! WGSL: `sm3` parses and translates the bytecode, `shadercache` reads the shader cache and its
//! material shader maps, `ue3prog` assembles a renderable program for a material.
pub mod shadercache;
pub mod sm3;
pub mod ue3prog;
