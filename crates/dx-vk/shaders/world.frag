#version 450
// multisampled opaque pipelines: masked texels by alpha-to-coverage (`dx_vk::A2C_SPEC`)
layout(constant_id = 0) const bool A2C = false;
// 16-bit scene: no rounding to the original's 8-bit frame buffer
layout(constant_id = 1) const bool DEEP = false;
// lightmaps and fog maps sampled bicubic (B-spline) instead of bilinear
layout(constant_id = 2) const bool SMOOTH_LM = false;
layout(push_constant) uniform Push {
    mat4 view_proj;
    vec4 cam_pos;
    uvec4 misc;
    vec4 clip;  // points with dot(xyz, p) < w are not drawn (mirror reflections); all zero = off
    uvec4 vis;  // x, y: zones that may be drawn (bit per zone); z: 1 = use the mask; w: replacement scale (f32 bits, 0 = none)
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
// An atlas map at `pos` (texels, centres at +0.5): bilinear as the original, or a cubic B-spline from four bilinear taps
// (the atlas keeps two replicated texels around every rectangle, so the wider footprint stays on its own surface).
vec4 atlas(sampler2D t, vec2 pos) {
    vec2 size = vec2(textureSize(t, 0));
    if (!SMOOTH_LM) return texture(t, pos / size);
    vec2 p = pos - 0.5;
    vec2 i = floor(p);
    vec2 f = p - i;
    vec2 f2 = f * f, f3 = f2 * f;
    vec2 w0 = (-f3 + 3.0 * f2 - 3.0 * f + 1.0) / 6.0;
    vec2 w1 = (3.0 * f3 - 6.0 * f2 + 4.0) / 6.0;
    vec2 w2 = (-3.0 * f3 + 3.0 * f2 + 3.0 * f + 1.0) / 6.0;
    vec2 w3 = f3 / 6.0;
    vec2 g0 = w0 + w1, g1 = w2 + w3;
    vec2 h0 = (i - 0.5 + w1 / g0) / size;
    vec2 h1 = (i + 1.5 + w3 / g1) / size;
    return g0.y * (g0.x * texture(t, vec2(h0.x, h0.y)) + g1.x * texture(t, vec2(h1.x, h0.y)))
         + g1.y * (g0.x * texture(t, vec2(h0.x, h1.y)) + g1.x * texture(t, vec2(h1.x, h1.y)));
}

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
    // texture coordinates count in the game's texels; a mod's bigger picture (vis.w: its scale) maps the same way
    vec2 tex_size = vec2(textureSize(tex, 0));
    if (pc.vis.w != 0u) tex_size /= uintBitsToFloat(pc.vis.w);
    vec4 c = texture(tex, v_uv / tex_size);
    float alpha = 1.0;
    if (pc.misc.x != 0u) {
        if (A2C) {
            // multisampled: the texel's alpha sharpened to about one pixel's width, as sample coverage (smooth cut-outs)
            alpha = clamp((c.a - 0.5) / max(fwidth(c.a), 1e-4) + 0.5, 0.0, 1.0);
            if (alpha <= 0.0) discard;
        } else if (c.a * 255.0 <= 127.0) discard; // D3D ALPHAFUNC GREATER, ALPHAREF 127
    }
    vec3 light = vec3(1.0);
    if ((v_flags & 2u) != 0u) {
        // Lightmap texels hold at most 127/255 per channel; cam_pos.w is the modulation factor (2: the texture is doubled).
        vec3 lm = atlas(lightmap, v_lm).rgb;
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
        vec4 f = atlas(fogmap, v_lm);
        if (f != vec4(0.0)) {
            vec3 stored = DEEP ? col : round(col * 255.0) / 255.0;
            col = min(f.rgb + stored * (1.0 - f.a), vec3(1.0));
        }
    }
    out_color = vec4(col, alpha);
}
