#version 450
layout(push_constant) uniform Push {
    mat4 view_proj;
    vec4 cam_pos;
    uvec4 misc;
    vec4 clip;  // points with dot(xyz, p) < w are not drawn (mirror reflections); all zero = off
    uvec4 vis;  // x, y: zones that may be drawn (bit per zone); z: 1 = use the mask
} pc;
layout(set = 0, binding = 0) uniform sampler2D tex;
layout(set = 1, binding = 0) uniform sampler2D lightmap;
layout(set = 2, binding = 0) uniform sampler2D fogmap;  // fog atlas, laid out as the lightmap atlas
layout(location = 0) in vec3 v_pos;
layout(location = 1) in vec3 v_normal;
layout(location = 2) in vec2 v_uv;
layout(location = 3) flat in uint v_flags;
layout(location = 4) in vec2 v_lm;
layout(location = 5) flat in vec3 v_ambient;
layout(location = 0) out vec4 out_color;
void main() {
    if (pc.vis.z != 0u) {
        uint z0 = (v_flags >> 8) & 255u;
        uint z1 = (v_flags >> 16) & 255u;
        bool s0 = ((z0 < 32u ? (pc.vis.x >> z0) : (pc.vis.y >> (z0 - 32u))) & 1u) != 0u;
        bool s1 = ((z1 < 32u ? (pc.vis.x >> z1) : (pc.vis.y >> (z1 - 32u))) & 1u) != 0u;
        if (!s0 && !s1) discard;
    }
    if (pc.clip.xyz != vec3(0.0) && dot(pc.clip.xyz, v_pos) < pc.clip.w) discard;
    // One-sided surfaces are only visible from the side their normal points to.
    if ((v_flags & 1u) == 0u && dot(v_normal, pc.cam_pos.xyz - v_pos) <= 0.0) discard;
    vec4 c = texture(tex, v_uv / vec2(textureSize(tex, 0)));
    if (pc.misc.x != 0u && c.a * 255.0 <= 127.0) discard; // D3D ALPHAFUNC GREATER, ALPHAREF 127
    vec3 light = vec3(1.0);
    if ((v_flags & 2u) != 0u) {
        // Lightmap texels hold at most 127/255 per channel; cam_pos.w is the modulation factor (2: the texture is doubled).
        vec3 lm = texture(lightmap, v_lm / vec2(textureSize(lightmap, 0))).rgb;
        light = pc.misc.y == 1u ? min(vec3(1.0), pc.cam_pos.w * (lm + v_ambient)) : vec3(1.0);
    }
    if (pc.misc.y == 0u) {
        // Without lightmaps: orientation-based shading, same as the CPU renderer.
        light = vec3(0.55 + 0.45 * abs(dot(v_normal, vec3(0.3, 0.5, 0.8))) / (length(v_normal) * 0.99));
    }
    vec3 col = min(c.rgb * light, vec3(1.0));
    // (a surface with a fog map gets no detail pass: DrawComplexSurface drops the detail texture then)
    if ((v_flags & 2u) != 0u && pc.misc.y == 1u && pc.misc.w != 0u) {
        // The device's fog pass over the surface just drawn (D3DDrv DrawComplexSurface: blend ONE, INVSRCALPHA, fog
        // texels doubled on upload, vertex colour white): the surface as stored in the 8-bit frame buffer, then
        // fog + surface * (1 - fog alpha). Zero texels (no fog) leave it as it is.
        vec4 f = texture(fogmap, v_lm / vec2(textureSize(fogmap, 0)));
        if (f != vec4(0.0)) {
            col = min(f.rgb + round(col * 255.0) / 255.0 * (1.0 - f.a), vec3(1.0));
        }
    }
    out_color = vec4(col, 1.0);
}
