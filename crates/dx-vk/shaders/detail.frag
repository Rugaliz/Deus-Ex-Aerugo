#version 450
// D3DDrv's detail texture pass (DrawComplexSurface 10003900): stage 0 BLENDDIFFUSEALPHA, the detail texel over the
// vertex colour (grey 0x80) by the vertex alpha (`v_color.r`, the near fade), drawn modulated (DESTCOLOR, SRCCOLOR).
layout(push_constant) uniform Push {
    mat4 vp;
    uvec4 params;
} pc;
layout(set = 0, binding = 0) uniform sampler2D tex;
layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec3 v_color;
// the mesh vertex shader's other outputs (unused here)
layout(location = 2) in vec3 v_fog;
layout(location = 3) in vec3 v_pos;
layout(location = 4) in vec3 v_normal;
layout(location = 5) flat in uint v_light;
layout(location = 0) out vec4 out_color;
void main() {
    vec3 t = texture(tex, v_uv).rgb;
    float a = v_color.r;
    out_color = vec4(t * a + vec3(128.0 / 255.0) * (1.0 - a), 1.0);
}
