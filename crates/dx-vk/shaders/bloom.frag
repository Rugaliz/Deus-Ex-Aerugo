#version 450
// Bloom (`Quality::bloom`, an Aerugo option): the bright parts of the main view spread into a soft glow.
//   mode 0: the picture at half size, 13 taps, only what is above the threshold (soft knee)
//   mode 1: one level down (13 taps)
//   mode 2: one level up (3x3 tent), added onto the bigger level
//   mode 3: the glow at full size, times the strength, added onto the picture (`texel`: one pixel of the target)
layout(push_constant) uniform Push {
    vec2 texel;       // one texel of the source
    float threshold;  // brightness (largest channel) where the glow starts
    float knee;       // width of the soft start
    float strength;   // mode 3: how much glow is added
    int mode;
} pc;
layout(set = 0, binding = 0) uniform sampler2D src;
layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;

vec3 tap(vec2 o) {
    return texture(src, v_uv + o * pc.texel).rgb;
}

// 13 taps (a 4x4 box of four 2x2 boxes and a centre one, weighted): no flicker when a bright pixel moves
vec3 down13() {
    vec3 a = tap(vec2(-2, -2)), b = tap(vec2(0, -2)), c = tap(vec2(2, -2));
    vec3 d = tap(vec2(-2, 0)), e = tap(vec2(0, 0)), f = tap(vec2(2, 0));
    vec3 g = tap(vec2(-2, 2)), h = tap(vec2(0, 2)), i = tap(vec2(2, 2));
    vec3 j = tap(vec2(-1, -1)), k = tap(vec2(1, -1)), l = tap(vec2(-1, 1)), m = tap(vec2(1, 1));
    return e * 0.125 + (a + c + g + i) * 0.03125 + (b + d + f + h) * 0.0625 + (j + k + l + m) * 0.125;
}

void main() {
    if (pc.mode == 0) {
        vec3 c = down13();
        float br = max(c.r, max(c.g, c.b));
        float soft = clamp(br - pc.threshold + pc.knee, 0.0, 2.0 * pc.knee);
        soft = soft * soft / (4.0 * pc.knee + 1e-4);
        float w = max(soft, br - pc.threshold) / max(br, 1e-4);
        out_color = vec4(c * w, 1.0);
    } else if (pc.mode == 1) {
        out_color = vec4(down13(), 1.0);
    } else if (pc.mode == 2) {
        vec3 s = tap(vec2(-1, -1)) + tap(vec2(1, -1)) + tap(vec2(-1, 1)) + tap(vec2(1, 1));
        s += 2.0 * (tap(vec2(0, -1)) + tap(vec2(-1, 0)) + tap(vec2(1, 0)) + tap(vec2(0, 1)));
        s += 4.0 * tap(vec2(0, 0));
        out_color = vec4(s / 16.0, 1.0);
    } else {
        // by the pixel's place (the scene's viewport is flipped and may be a part of the target): texel = 1 / target
        out_color = vec4(texture(src, gl_FragCoord.xy * pc.texel).rgb * pc.strength, 0.0);
    }
}
