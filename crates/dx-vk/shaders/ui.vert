#version 450
layout(push_constant) uniform Push {
    vec4 rect;      // x, y, w, h in pixels
    vec4 uv;        // u0, v0, u1, v1, normalised (may exceed 1: the sampler repeats)
    vec4 color;     // rgb tint, w: 1 = masked (discard transparent texels)
    vec4 viewport;  // width, height of the target in pixels
} pc;
layout(location = 0) out vec2 v_uv;
void main() {
    // two triangles of a unit quad
    const vec2 corners[6] = vec2[6](vec2(0, 0), vec2(1, 0), vec2(0, 1), vec2(1, 0), vec2(1, 1), vec2(0, 1));
    vec2 c = corners[gl_VertexIndex];
    vec2 px = pc.rect.xy + c * pc.rect.zw;
    v_uv = mix(pc.uv.xy, pc.uv.zw, c);
    gl_Position = vec4(px / pc.viewport.xy * 2.0 - 1.0, 0.0, 1.0);
}
