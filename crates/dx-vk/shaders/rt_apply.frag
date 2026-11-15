#version 460
#extension GL_GOOGLE_include_directive : require
// The blurred factor, brought to full size (the four nearest half-size texels, weighed by their distance from the eye
// against this pixel's, `rt_dist.frag`), multiplied into the picture (blend DST_COLOR, ZERO). Mirror areas are left out by the stencil.
#include "rt_common.glsl"
layout(set = 0, binding = 0) uniform sampler2D blurred;
layout(set = 0, binding = 1) uniform sampler2D dist_tex;
layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;
void main() {
    ivec2 px = ivec2(gl_FragCoord.xy);
    if ((pc.params.y & 16u) != 0u) {
        out_color = vec4(0.5, 0.5, 0.5, 1.0);
        return;
    }
    float dist = texelFetch(dist_tex, px, 0).r;
    if ((pc.params.y & 64u) != 0u) {
        float f = texelFetch(blurred, px / 2, 0).r;
        out_color = vec4(f, f, f, 1.0);
        return;
    }
    if (dist >= 1e5) {
        out_color = vec4(1.0);
        return;
    }
    vec2 h = (vec2(px) + 0.5) * 0.5 - 0.5;
    ivec2 b = ivec2(floor(h));
    vec2 fr = h - vec2(b);
    ivec2 hi = ivec2((pc.params.zw + 1u) / 2u) - 1;
    float sum = 0.0, wsum = 0.0;
    for (int y = 0; y <= 1; y++) {
        for (int x = 0; x <= 1; x++) {
            vec2 s = texelFetch(blurred, clamp(b + ivec2(x, y), ivec2(0), hi), 0).rg;
            float bw = (x == 0 ? 1.0 - fr.x : fr.x) * (y == 0 ? 1.0 - fr.y : fr.y);
            float dz = (s.y - dist) / (0.02 * dist + 1.0);
            float w = (bw + 1e-3) * exp(-dz * dz);
            sum += s.x * w;
            wsum += w;
        }
    }
    float f = wsum > 1e-4 ? sum / wsum : 1.0;
    out_color = vec4(f, f, f, 1.0);
}
