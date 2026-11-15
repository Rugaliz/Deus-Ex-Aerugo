#version 460
#ifdef RAYTRACE
#extension GL_EXT_ray_query : require
#endif
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
layout(location = 6) flat in uint v_surf;
layout(location = 0) out vec4 out_color;
#ifdef RAYTRACE
// the ray-traced picture options (`dx_vk::SCENE_SPEC`); `rt.rs` for the data
layout(constant_id = 4) const bool RT_SHADOWS = false;
layout(constant_id = 5) const bool RT_AO = false;
layout(constant_id = 6) const bool RT_DEBUG = false;
layout(set = 3, binding = 0) uniform accelerationStructureEXT tlas;
struct RtLight { vec4 pos_inv_r; vec4 colour_kind; vec4 axis_cone; };
layout(std430, set = 3, binding = 1) readonly buffer Lights { RtLight lights[]; };
layout(std430, set = 3, binding = 2) readonly buffer Lists { uint lists[]; };
const uint MASK_WORLD = 1u;
const uint MASK_OBJECT = 2u;

// Whether anything of `mask` lies on the segment from `o` along unit `d` up to `tmax`; the distance to it in `t`.
bool blocked(vec3 o, vec3 d, float tmax, uint mask, out float t) {
    rayQueryEXT q;
    rayQueryInitializeEXT(q, tlas, gl_RayFlagsTerminateOnFirstHitEXT, mask, o, 0.0, d, tmax);
    // meshes are not opaque geometries (their own shaders skip themselves): every hit on them counts here
    while (rayQueryProceedEXT(q)) {
        if (rayQueryGetIntersectionTypeEXT(q, false) == gl_RayQueryCandidateIntersectionTriangleEXT) rayQueryConfirmIntersectionEXT(q);
    }
    bool hit = rayQueryGetIntersectionTypeEXT(q, true) != gl_RayQueryCommittedIntersectionNoneEXT;
    t = hit ? rayQueryGetIntersectionTEXT(q, true) : tmax;
    return hit;
}

// A light's share of the lightmap at `p` (normal `n`), in lightmap units: FLightManager's spatial functions without the
// shadow mask (omni: (2t^3 - 3t^2 + 1) cos(incidence), t = distance / radius; spot: times ((cos - c) / (1 - c))^2 inside
// the cone; non-incidence: 1.02 - t), times the light's colour, at most 127/255 (the 7-bit lightmap).
vec3 light_share(RtLight l, vec3 p, vec3 n) {
    vec3 d = p - l.pos_inv_r.xyz;
    float dist = length(d);
    float t = dist * l.pos_inv_r.w;
    if (t >= 1.0 || dist < 1e-3) return vec3(0.0);
    float kind = l.colour_kind.w;
    float f;
    if (kind > 1.5) {
        f = max(1.02 - t, 0.0);
    } else {
        f = (2.0 * t * t * t - 3.0 * t * t + 1.0) * abs(dot(n, d)) / dist;
        if (kind > 0.5) {
            float c = l.axis_cone.w;
            float cphi = dot(l.axis_cone.xyz, d) / dist;
            if (cphi <= c) return vec3(0.0);
            float tt = (cphi - c) / (1.0 - c);
            f *= tt * tt;
        }
    }
    return min(f * l.colour_kind.rgb, vec3(127.0 / 255.0));
}

// How much of a light objects hide from `o`: the light taken as a disc of radius 12 facing `o`, four rays to points of
// it (turned per pixel), the fraction blocked: soft shadow edges, wider the farther the object is from the surface.
float object_cover(vec3 o, vec3 light, float dist) {
    vec3 dir = (light - o) / dist;
    vec3 a = abs(dir.z) < 0.9 ? vec3(0.0, 0.0, 1.0) : vec3(1.0, 0.0, 0.0);
    vec3 tx = normalize(cross(a, dir));
    vec3 ty = cross(dir, tx);
    float noise = fract(52.9829189 * fract(dot(gl_FragCoord.xy, vec2(0.06711056, 0.00583715))));
    float hidden = 0.0;
    for (int k = 0; k < 4; k++) {
        float ang = 6.2831853 * (float(k) * 0.25 + noise);
        float rad = 12.0 * sqrt((float(k) + 0.5) / 4.0);
        vec3 to = light + (tx * cos(ang) + ty * sin(ang)) * rad - o;
        float d = length(to);
        float t;
        if (blocked(o, to / d, d - 24.0, MASK_OBJECT, t)) hidden += 0.25;
    }
    return hidden;
}

// Ray-traced shadows of objects (meshes, movers) on a level surface: the share of every light on the surface that an
// object hides and the world does not (the world's own shadows are in the lightmap already). The segment stops short of
// the light, so a lamp's own mesh does not hide it.
vec3 object_shadow(vec3 p, vec3 n) {
    if (v_surf == 0xffffffffu) return vec3(0.0);
    uint at = lists[v_surf];
    if (at == 0u) return vec3(0.0);
    uint count = lists[at];
    vec3 lost = vec3(0.0);
    for (uint i = 0u; i < count; i++) {
        RtLight l = lights[lists[at + 1u + i]];
        if (l.colour_kind.w >= 7.5) continue;  // no object within its reach
        vec3 share = light_share(l, p, n);
        if (share == vec3(0.0)) continue;
        vec3 to = l.pos_inv_r.xyz - p;
        float dist = length(to);
        vec3 dir = to / dist;
        vec3 o = p + n * (dot(n, dir) < 0.0 ? -0.5 : 0.5);
        float tmax = dist - 24.0;
        if (tmax <= 1.0) continue;
        // the object rays first (they only walk the few object structures); the world ray, which walks the level,
        // only where an object hides the light
        float cover = object_cover(o, l.pos_inv_r.xyz, dist);
        float t;
        if (cover > 0.0 && !blocked(o, dir, tmax, MASK_WORLD, t)) lost += share * cover;
    }
    return lost;
}

// Ambient occlusion: two short rays over the hemisphere of `n` (cosine weighted, turned per pixel), closer hits darker.
float occlusion(vec3 p, vec3 n) {
    const float R = 48.0;
    vec3 a = abs(n.z) < 0.9 ? vec3(0.0, 0.0, 1.0) : vec3(1.0, 0.0, 0.0);
    vec3 tx = normalize(cross(a, n));
    vec3 ty = cross(n, tx);
    float noise = fract(52.9829189 * fract(dot(gl_FragCoord.xy, vec2(0.06711056, 0.00583715))));
    vec3 o = p + n * 0.5;
    float occ = 0.0;
    for (int i = 0; i < 2; i++) {
        float u = (float(i) + 0.5) / 2.0;
        float phi = 6.2831853 * (float(i) * 0.618034 + noise);
        float r = sqrt(u);
        vec3 d = normalize(tx * (r * cos(phi)) + ty * (r * sin(phi)) + n * sqrt(1.0 - u));
        float t;
        if (blocked(o, d, R, MASK_WORLD | MASK_OBJECT, t)) occ += 1.0 - t / R;
    }
    return 1.0 - 0.6 * occ / 2.0;
}
#endif

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
#ifdef RAYTRACE
        if (RT_SHADOWS) {
            lm = max(lm - object_shadow(v_pos, normalize(v_normal)), vec3(0.0));
        }
#endif
        light = pc.misc.y == 1u ? min(vec3(1.0), pc.cam_pos.w * (lm + v_ambient)) : vec3(1.0);
    }
    if (pc.misc.y == 0u) {
        // Without lightmaps: orientation-based shading, same as the CPU renderer.
        light = vec3(0.55 + 0.45 * abs(dot(v_normal, vec3(0.3, 0.5, 0.8))) / (length(v_normal) * 0.99));
    }
#ifdef RAYTRACE
    // ambient occlusion near the eye only (it fades out by 1800 units, where it is too small to see)
    float eye_dist = length(pc.cam_pos.xyz - v_pos);
    if (RT_AO && (v_flags & 2u) != 0u && eye_dist < 1800.0) {
        vec3 n = normalize(v_normal);
        if (dot(n, pc.cam_pos.xyz - v_pos) < 0.0) n = -n;
        light *= mix(occlusion(v_pos, n), 1.0, smoothstep(1200.0, 1800.0, eye_dist));
    }
#endif
    vec3 col = min(c.rgb * light, vec3(1.0));
#ifdef RAYTRACE
    if (RT_DEBUG) {
        // a ray from the eye to this fragment must find no world geometry before it
        vec3 to = v_pos - pc.cam_pos.xyz;
        float dist = length(to);
        float t;
        if (dist > 8.0 && blocked(pc.cam_pos.xyz, to / dist, dist - 4.0, MASK_WORLD, t)) col = vec3(1.0, 0.0, 0.0);
    }
#endif
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
