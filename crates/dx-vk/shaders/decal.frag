#version 450
// Decals are drawn modulated (`DrawFrame` passes PF_Modulated; the D3D device then ignores the vertex colour): the
// texture alone, multiplied into the scene by the blend.
layout(set = 0, binding = 0) uniform sampler2D tex;
layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;
void main() {
    vec4 c = texture(tex, v_uv);
    if (c.a * 255.0 <= 127.0) discard; // D3D ALPHAFUNC GREATER, ALPHAREF 127
    out_color = vec4(c.rgb, 1.0);
}
