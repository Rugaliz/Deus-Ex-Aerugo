#version 450
layout(push_constant) uniform Push {
    mat4 vp;
    vec4 center;
    vec4 right;   // half-width axis in the world
    vec4 up;      // half-height axis in the world
    vec4 params;  // x: ScaleGlow, y: 0 opaque, 1 translucent, 2 modulated, z: 1 = alpha test
} pc;
layout(location = 0) out vec2 v_uv;
const vec2 corners[6] = vec2[6](vec2(-1, -1), vec2(1, -1), vec2(1, 1), vec2(-1, -1), vec2(1, 1), vec2(-1, 1));
void main() {
    vec2 q = corners[gl_VertexIndex];
    v_uv = vec2(q.x * 0.5 + 0.5, 0.5 - q.y * 0.5);
    gl_Position = pc.vp * vec4(pc.center.xyz + pc.right.xyz * q.x + pc.up.xyz * q.y, 1.0);
}
