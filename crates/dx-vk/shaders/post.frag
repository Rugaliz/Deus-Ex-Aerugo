#version 450
layout(push_constant) uniform Push {
    float gamma_exp;  // out = in ^ gamma_exp (the engine's Brightness setting: 0.5 / Brightness)
    float crosshair;  // 1 = draw the crosshair
    vec2 size;        // target size in pixels
} pc;
layout(set = 0, binding = 0) uniform sampler2D scene;
layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;
void main() {
    vec3 c = texture(scene, v_uv).rgb;
    c = pow(c, vec3(pc.gamma_exp));
    if (pc.crosshair > 0.5) {
        vec2 d = abs(gl_FragCoord.xy - 0.5 * pc.size);
        if ((d.x <= 4.0 && d.y < 1.0) || (d.y <= 4.0 && d.x < 1.0)) c = vec3(1.0);
    }
    out_color = vec4(c, 1.0);
}
