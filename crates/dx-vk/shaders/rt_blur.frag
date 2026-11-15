#version 460
// The traced factor blurred at half size along the surfaces: a 5x5 neighbourhood, each weighed by its distance from the
// eye against this one's (no blur across depth edges). x: the factor, y: the distance (kept).
layout(push_constant) uniform Push { vec4 size; } pc;
layout(set = 0, binding = 0) uniform sampler2D traced;
layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec2 out_v;
void main() {
    ivec2 c = ivec2(gl_FragCoord.xy);
    vec2 here = texelFetch(traced, c, 0).rg;
    if (here.y > 1e5) {
        out_v = here;
        return;
    }
    ivec2 hi = ivec2(pc.size.xy) - 1;
    float sum = 0.0, wsum = 0.0;
    for (int y = -2; y <= 2; y++) {
        for (int x = -2; x <= 2; x++) {
            vec2 s = texelFetch(traced, clamp(c + ivec2(x, y), ivec2(0), hi), 0).rg;
            float dz = (s.y - here.y) / (0.02 * here.y + 1.0);
            float w = exp(-0.18 * float(x * x + y * y)) * exp(-dz * dz);
            sum += s.x * w;
            wsum += w;
        }
    }
    out_v = vec2(sum / max(wsum, 1e-5), here.y);
}
