#version 450
layout(push_constant) uniform Push {
    vec4 rect;
    vec4 uv;
    vec4 color;
    vec4 viewport; // width, height of the target in pixels; z: 1 = crisp scaling
} pc;
layout(set = 0, binding = 0) uniform sampler2D tex;
layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;
void main() {
    vec2 uv = v_uv;
    if (pc.viewport.z > 0.5) {
        // crisp scaling (sharp bilinear): sample texel centres, blending only across the one screen pixel where two
        // texels meet, so scaled art keeps sharp texels without uneven, jagged ones
        vec2 size = vec2(textureSize(tex, 0));
        vec2 t = uv * size;
        vec2 w = max(fwidth(t), vec2(1e-4));
        uv = (floor(t) + 0.5 + clamp((fract(t) - 0.5) / w, -0.5, 0.5)) / size;
    }
    vec4 t = texture(tex, uv);
    // transparent texels of masked textures are never drawn, whatever the style (the drivers alpha-test them)
    if (t.a * 255.0 <= 127.0) discard; // D3D ALPHAFUNC GREATER, ALPHAREF 127
    out_color = vec4(t.rgb * pc.color.rgb, 1.0);
}
