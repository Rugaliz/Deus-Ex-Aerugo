#version 450
layout(push_constant) uniform Push {
    mat4 vp;
    vec4 center;
    vec4 right;
    vec4 up;
    vec4 params;
} pc;
layout(set = 0, binding = 0) uniform sampler2D tex;
layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;
void main() {
    vec4 c = texture(tex, v_uv);
    // transparent texels of masked textures are never drawn, whatever the style (the drivers alpha-test them)
    if (c.a * 255.0 <= 127.0) discard; // D3D ALPHAFUNC GREATER, ALPHAREF 127
    // `DrawActorSprite`: the tile's colour is ScaleGlow on every channel, capped at 1 (a glow above 1 does not brighten)
    float g = min(pc.params.x, 1.0);
    vec3 col = pc.params.y > 1.5 ? mix(vec3(0.5), c.rgb, clamp(g, 0.0, 1.0)) : c.rgb * g;
    out_color = vec4(min(col, vec3(1.0)), 1.0);
}
