#version 450
// the same position for the same vertex in every pipeline that draws it (depth-equal multi-pass drawing)
invariant gl_Position;
// A decal piece (`URender::ClipDecal`): its triangles are in the per-frame slot buffer, two vec4 per vertex (world
// position, texel coordinates); `params.x` holds the first vec4's index (as bits), `params.zw` the texture's size.
layout(push_constant) uniform Push {
    mat4 vp;
    vec4 params;
} pc;
layout(std430, set = 1, binding = 0) readonly buffer Slots { vec4 data[]; } slots;
layout(location = 0) out vec2 v_uv;
void main() {
    uint at = floatBitsToUint(pc.params.x) + uint(gl_VertexIndex) * 2u;
    vec4 p = slots.data[at];
    vec4 t = slots.data[at + 1u];
    v_uv = t.xy / pc.params.zw;
    gl_Position = pc.vp * vec4(p.xyz, 1.0);
}
