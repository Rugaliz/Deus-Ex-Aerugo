#version 450
layout(push_constant) uniform Push {
    float gamma_exp;  // out = in ^ gamma_exp (the engine's Brightness setting: 0.5 / Brightness)
    float crosshair;  // 1 = draw the crosshair
    vec2 size;        // target size in pixels
    float dither;     // one step of the output (16-bit scene), 0 = none
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
    if (pc.dither > 0.0) {
        // triangular noise of one output step (two hashes of the pixel): no banding when the 16-bit scene is quantised
        vec2 p = gl_FragCoord.xy;
        float n1 = fract(sin(dot(p, vec2(12.9898, 78.233))) * 43758.5453);
        float n2 = fract(sin(dot(p, vec2(39.3468, 11.1353))) * 24634.6345);
        c = clamp(c + (n1 + n2 - 1.0) * pc.dither, 0.0, 1.0);
    }
    out_color = vec4(c, 1.0);
}
