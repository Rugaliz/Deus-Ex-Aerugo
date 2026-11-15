#version 460
#extension GL_GOOGLE_include_directive : require
// Each pixel's distance from the eye, from the depth buffer while the scene's rendering is paused: what `rt_apply.frag`
// weighs the half-size factor by (the depth is an attachment again by then). 1e6 where nothing is drawn and where the
// stencil marks an opening to another frame (an opaque mirror, the sky): those are left as they are.
#include "rt_common.glsl"
layout(set = 0, binding = 0) uniform sampler2D depth_tex;
layout(set = 0, binding = 1) uniform usampler2D stencil_tex;
layout(location = 0) in vec2 v_uv;
layout(location = 0) out float out_dist;
void main() {
    ivec2 px = ivec2(gl_FragCoord.xy);
    float depth = texelFetch(depth_tex, px, 0).r;
    bool opening = texelFetch(stencil_tex, px, 0).r != 0u;
    out_dist = depth >= 1.0 || opening ? 1e6 : length(world_at(px, depth) - pc.eye.xyz);
}
