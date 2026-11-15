#version 450
layout(push_constant) uniform Push {
    vec4 rect;
    vec4 uv;
    vec4 color;
    vec4 viewport;
} pc;
layout(set = 0, binding = 0) uniform sampler2D tex;
layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;
void main() {
    vec4 t = texture(tex, v_uv);
    // transparent texels of masked textures are never drawn, whatever the style (the drivers alpha-test them)
    if (t.a * 255.0 <= 127.0) discard; // D3D ALPHAFUNC GREATER, ALPHAREF 127
    out_color = vec4(t.rgb * pc.color.rgb, 1.0);
}
