#version 450
layout(push_constant) uniform Push {
    mat4 view_proj;
    vec4 cam_pos;
    uvec4 misc; // x: 1 = masked batch, y: 1 = use lightmaps (0: shaded by orientation, 2: plain textures), z: level time (float bits), w: 1 = draw fog
    vec4 clip;  // points with dot(xyz, p) < w are not drawn (mirror reflections); all zero = off
    uvec4 vis;  // x, y: zones that may be drawn (bit per zone); z: 1 = use the mask
} pc;
layout(location = 0) in vec3 in_pos;
layout(location = 1) in vec3 in_normal;
layout(location = 2) in vec2 in_uv;
layout(location = 3) in uint in_flags;
layout(location = 4) in vec2 in_lm;      // lightmap position in atlas texels
layout(location = 5) in vec3 in_ambient; // zone ambient (0..1 of a byte)
layout(location = 6) in vec2 in_pan;     // auto-pan speed (texels: speed * 35 per second)
layout(location = 7) in uint in_surf;    // level surface (ray-traced shadows' light list), 0xffffffff for movers
layout(location = 0) out vec3 v_pos;
layout(location = 1) out vec3 v_normal;
layout(location = 2) out vec2 v_uv;
layout(location = 3) flat out uint v_flags;
layout(location = 4) out vec2 v_lm;
layout(location = 5) flat out vec3 v_ambient;
layout(location = 6) flat out uint v_surf;
void main() {
    gl_Position = pc.view_proj * vec4(in_pos, 1.0);
    v_pos = in_pos;
    v_normal = in_normal;
    // auto-panning surfaces (the original: (int(speed * time * 35 * 256) & 0x3ffff) / 256 added to the pan)
    float t = uintBitsToFloat(pc.misc.z);
    vec2 auto_pan = mod(floor(in_pan * t * 8960.0), 262144.0) / 256.0;
    v_uv = in_uv + auto_pan * vec2(notEqual(in_pan, vec2(0.0)));
    // PF_SmallWavy (Render.dll 10b1a72a, flag bit 2): U += 4 cos(2.3 t) + 8 sin(t), V += 4 sin(2.3 t) + 8 cos(t)
    // texels (the engine in double; no map surface has the flag)
    if ((in_flags & 4u) != 0u) {
        v_uv += vec2(4.0 * cos(2.3 * t) + 8.0 * sin(t), 4.0 * sin(2.3 * t) + 8.0 * cos(t));
    }
    v_flags = in_flags;
    v_lm = in_lm;
    v_ambient = in_ambient;
    v_surf = in_surf;
}
