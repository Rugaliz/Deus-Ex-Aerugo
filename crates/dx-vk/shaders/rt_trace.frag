#version 460
#extension GL_EXT_ray_query : require
#extension GL_GOOGLE_include_directive : require
// Ray-traced shadows of characters and objects, and ambient occlusion, at half the view's size (one fragment per 2x2
// pixels): the factor the picture is to be multiplied by, and the distance from the eye (for the blur). From the depth
// buffer, so each visible point is traced once. `rt_post.rs` for the casters.
#include "rt_common.glsl"
layout(set = 0, binding = 0) uniform sampler2D depth_tex;
// 0x40: a pixel of an object (`rt_post::OBJECT_STENCIL`)
layout(set = 0, binding = 3) uniform usampler2D stencil_tex;
layout(set = 0, binding = 1) uniform accelerationStructureEXT tlas;
// a caster: one object with one of the (at most four) lights that light it most
struct Caster {
    vec4 light;  // the light's position, its size (the disc the rays spread over)
    vec4 centre; // the object's middle and radius
    vec4 params; // x: how dark (the light's share of the object's light), y: how far past the object its shadow reaches,
                 // z: 1 for a corpse
};
layout(std430, set = 0, binding = 2) readonly buffer Casters { Caster casters[]; };
// the alpha-tested faces of meshes (cut-out leaves): the mesh structure's geometry 2 m + 1 is mesh m's, its faces from
// `mesh_first[m]` on; each face's texture coordinates and alpha mask (`[first word, width, height]`, a bit a texel)
struct MaskedFace {
    vec4 uv01;
    vec2 uv2;
    uint mask;
    uint pad;
};
layout(std430, set = 0, binding = 4) readonly buffer Faces { MaskedFace faces[]; };
layout(std430, set = 0, binding = 5) readonly buffer MeshFirst { uint mesh_first[]; };
layout(std430, set = 0, binding = 6) readonly buffer MaskInfo { uvec4 mask_info[]; };
layout(std430, set = 0, binding = 7) readonly buffer MaskBits { uint mask_bits[]; };
layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec2 out_v;

const uint MASK_WORLD = 1u;
const uint MASK_OBJECT = 2u;

// A ray query must get finite numbers, a direction that is not zero and an end beyond its start (else the GPU can hang).
bool ray_ok(vec3 o, vec3 d, float tmax) {
    return !any(isnan(o)) && !any(isinf(o)) && !any(isnan(d)) && dot(d, d) > 1e-12 && tmax > 0.5 && !isinf(tmax);
}

// Whether an alpha-tested face is solid where it was hit (`geometry`, `prim` of the mesh structure, barycentrics `b`):
// the texel of its mask, the texture repeating.
bool solid(int geometry, int prim, vec2 b) {
    MaskedFace f = faces[mesh_first[geometry >> 1] + uint(prim)];
    vec2 uv = f.uv01.xy * (1.0 - b.x - b.y) + f.uv01.zw * b.x + f.uv2 * b.y;
    uvec4 m = mask_info[f.mask];
    uvec2 size = max(m.yz, uvec2(1u));
    uvec2 texel = min(uvec2(fract(uv) * vec2(size)), size - 1u);
    uint i = texel.y * size.x + texel.x;
    return (mask_bits[m.x + (i >> 5)] & (1u << (i & 31u))) != 0u;
}

// Whether anything of `mask` lies on the segment from `o` along `d` between `tmin` and `tmax`; how far it is in `t`.
// Opaque geometry counts at once; an alpha-tested face only where it is solid.
bool blocked_in(vec3 o, vec3 d, float tmin, float tmax, uint mask, out float t) {
    t = tmax;
    if (!ray_ok(o, d, tmax) || tmax <= tmin) return false;
    rayQueryEXT q;
    rayQueryInitializeEXT(q, tlas, gl_RayFlagsTerminateOnFirstHitEXT, mask, o, tmin, d, tmax);
    while (rayQueryProceedEXT(q)) {
        if (rayQueryGetIntersectionTypeEXT(q, false) == gl_RayQueryCandidateIntersectionTriangleEXT
            && solid(rayQueryGetIntersectionGeometryIndexEXT(q, false), rayQueryGetIntersectionPrimitiveIndexEXT(q, false), rayQueryGetIntersectionBarycentricsEXT(q, false))) {
            rayQueryConfirmIntersectionEXT(q);
        }
    }
    bool hit = rayQueryGetIntersectionTypeEXT(q, true) != gl_RayQueryCommittedIntersectionNoneEXT;
    if (hit) t = rayQueryGetIntersectionTEXT(q, true);
    return hit;
}

bool blocked(vec3 o, vec3 d, float tmax, uint mask, out float t) {
    return blocked_in(o, d, 0.5, tmax, mask, t);
}

// Ray `k` of `n` from `o` to a point of the light's disc (centre `light`, radius `size`, facing `o`), turned by `turn`:
// whether an object hides it where it goes through the caster's sphere (centre `cc`, radius `cr`; what lies elsewhere
// on the way, such as a lamp's own housing, is not this caster's shadow).
float hides(vec3 o, vec3 light, float size, vec3 side, vec3 up, float turn, int k, int n, vec3 cc, float cr) {
    float a = turn + float(k) * 2.3999632;
    float r = sqrt((float(k) + 0.5) / float(n)) * size;
    vec3 to = light + (cos(a) * side + sin(a) * up) * r - o;
    float d = length(to);
    vec3 u = to / d;
    float b = dot(cc - o, u);
    float disc = b * b - (dot(cc - o, cc - o) - cr * cr);
    if (disc <= 0.0) return 0.0;
    float h = sqrt(disc);
    float t;
    return blocked_in(o, u, max(b - h, 0.5), min(b + h, d - 2.0), MASK_OBJECT, t) ? 1.0 : 0.0;
}

void main() {
    ivec2 px = ivec2(gl_FragCoord.xy) * 2;
    if ((pc.params.y & 512u) != 0u) { out_v = vec2(1.0, 1e6); return; }
    float depth = texelFetch(depth_tex, px, 0).r;
    if (depth >= 1.0 || float(px.x) < pc.rect.x || float(px.y) < pc.rect.y || float(px.x) >= pc.rect.x + pc.rect.z || float(px.y) >= pc.rect.y + pc.rect.w) {
        out_v = vec2(1.0, 1e6);
        return;
    }
    vec3 p = world_at(px, depth);
    float dist = length(p - pc.eye.xyz);
    // the normal from the neighbours' positions (the nearer of the two sides, across edges)
    ivec2 size = ivec2(pc.params.zw);
    vec3 dx = vec3(0.0), dy = vec3(0.0);
    for (int s = -1; s <= 1; s += 2) {
        ivec2 qx = clamp(px + ivec2(s, 0), ivec2(0), size - 1), qy = clamp(px + ivec2(0, s), ivec2(0), size - 1);
        vec3 ex = (world_at(qx, texelFetch(depth_tex, qx, 0).r) - p) * float(s);
        vec3 ey = (world_at(qy, texelFetch(depth_tex, qy, 0).r) - p) * float(s);
        if (s < 0 || dot(ex, ex) < dot(dx, dx)) dx = ex;
        if (s < 0 || dot(ey, ey) < dot(dy, dy)) dy = ey;
    }
    vec3 n = cross(dx, dy);
    n = dot(n, n) > 1e-12 ? normalize(n) : vec3(0.0, 0.0, 1.0);
    if (dot(n, pc.eye.xyz - p) < 0.0) n = -n;
    vec3 o = p + n * 1.0;
    float turn = 6.2831853 * noise_at(vec2(px));

    float f = 1.0;
    // objects get no shadows from casters (their own lighting shades them; their shapes for the rays are coarser than
    // drawn, so they would shadow themselves)
    bool on_object = (texelFetch(stencil_tex, px, 0).r & 0x40u) != 0u;
    uint shadow_casters = ((pc.params.y & 1u) != 0u && !on_object) ? pc.params.x : 0u;
    for (uint i = 0u; i < shadow_casters; i++) {
        Caster c = casters[i];
        vec3 L = c.light.xyz, C = c.centre.xyz;
        float r = c.centre.w, lsize = c.light.w;
        // only where the object's shadow can fall: behind it, seen from the light, within the cone of its outline
        // (widened by the light's size) and its reach
        vec3 dC = C - L;
        float lc = length(dC);
        if (lc < 1.0) continue;
        vec3 u = dC / lc;
        vec3 v = p - L;
        float along = dot(v, u);
        if (along < lc - r || along - lc > c.params.y) continue;
        float allowed = (r + lsize) * along / lc + lsize;
        if (length(v - u * along) > allowed) continue;
        float lv = length(v);
        vec3 l = -v / lv;
        float ndl = dot(n, l);
        if (ndl <= 0.0) continue;
        vec3 side = normalize(abs(l.z) < 0.9 ? cross(l, vec3(0.0, 0.0, 1.0)) : cross(l, vec3(1.0, 0.0, 0.0)));
        vec3 up = cross(side, l);
        // the most light the point can lose to this caster (see below)
        float most = c.params.x * ndl * min(lc * lc / (lv * lv), 1.0);
        float hidden = 0.0;
        if (c.params.z > 0.5 && most < 0.05) {
            // a corpse's faint shadow (at most 5% darker): two rays across the light's disc, four each
            for (int k = 0; k < 8; k += 4) hidden += hides(o, L, lsize, side, up, turn, k, 8, C, r);
            hidden *= 4.0;
        } else {
            // four rays over the light's disc; four more between them where those disagree (a shadow's edge)
            for (int k = 0; k < 8; k += 2) hidden += hides(o, L, lsize, side, up, turn, k, 8, C, r);
            if (hidden > 0.0 && hidden < 4.0) {
                for (int k = 1; k < 8; k += 2) hidden += hides(o, L, lsize, side, up, turn, k, 8, C, r);
            } else {
                hidden *= 2.0;
            }
        }
        if (hidden == 0.0) continue;
        // no shadow through a wall: the world between the point and the object
        vec3 toC = C - o;
        float dc = length(toC);
        float t;
        if (blocked(o, toC / dc, max(dc - r, 1.0), MASK_WORLD, t)) continue;
        // the light the point loses: as much as the light gives it, which falls off with the angle it comes in at
        // (a lamp low on the horizon lights the floor little, and its long shadow is faint) and, farther from the
        // light than the object, with the distance
        f *= 1.0 - most * hidden / 8.0;
    }

    // ambient occlusion: four short rays over the hemisphere (cosine weighted), closer hits darker; near the eye only.
    // A point on an object is occluded by the world alone (an object's own shape is coarser than drawn)
    if ((pc.params.y & 2u) != 0u && dist < 2500.0) {
        const float R = 40.0;
        vec3 side = normalize(abs(n.z) < 0.9 ? cross(n, vec3(0.0, 0.0, 1.0)) : cross(n, vec3(1.0, 0.0, 0.0)));
        vec3 up = cross(side, n);
        uint mask = on_object ? MASK_WORLD : (MASK_WORLD | MASK_OBJECT);
        float occ = 0.0;
        for (int k = 0; k < 4; k++) {
            float a = turn + float(k) * 2.3999632;
            float rr = sqrt((float(k) + 0.5) / 4.0);
            vec3 d = normalize((cos(a) * side + sin(a) * up) * rr + n * sqrt(1.0 - rr * rr));
            float t;
            if (blocked(o, d, R, mask, t)) occ += 1.0 - t / R;
        }
        f *= mix(1.0 - 0.7 * occ / 4.0, 1.0, smoothstep(1800.0, 2500.0, dist));
    }
    if ((pc.params.y & 128u) != 0u) f = fract(dist / 256.0);
    if ((pc.params.y & 256u) != 0u) {
        float t;
        vec3 dir = (p - pc.eye.xyz) / dist;
        f = blocked(pc.eye.xyz, dir, dist + 20.0, 0xFFu, t) ? (blocked(pc.eye.xyz, dir, dist + 20.0, MASK_OBJECT, t) ? 0.5 : 0.0) : 1.0;
    }
    out_v = vec2(f, dist);
}
