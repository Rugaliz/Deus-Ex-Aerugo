#version 460
#ifdef RAYTRACE
#extension GL_EXT_ray_query : require
#endif
// multisampled opaque pipelines: masked texels by alpha-to-coverage (`dx_vk::A2C_SPEC`)
layout(constant_id = 0) const bool A2C = false;
// meshes lit per pixel from the actor's light list (the picture option); otherwise the vertex colour
layout(constant_id = 3) const bool PIXEL_LIGHT = false;
layout(std430, set = 1, binding = 0) readonly buffer Slots { vec4 data[]; } slots;
// D3DDrv's `DrawGouraudPolygon` (100056b0): stage 0 MODULATE, texture times the vertex colour, plus the specular
// fog colour; `SetBlending`
// (10008270) alpha-tests masked faces only (translucent ones lose `PF_Masked`).
layout(push_constant) uniform Push {
    mat4 vp;
    uvec4 params;
    vec4 clip; // see lodmesh.vert
} pc;
#ifdef RAYTRACE
// the ray-traced picture options (`dx_vk::SCENE_SPEC`); `rt.rs` for the data (set 2)
layout(constant_id = 4) const bool RT_SHADOWS = false;
layout(constant_id = 5) const bool RT_AO = false;
layout(set = 2, binding = 0) uniform accelerationStructureEXT tlas;

// Whether anything lies on the segment from `o` along unit `d` up to `tmax`; the distance to it in `t`.
bool blocked(vec3 o, vec3 d, float tmax, out float t) {
    rayQueryEXT q;
    rayQueryInitializeEXT(q, tlas, gl_RayFlagsTerminateOnFirstHitEXT, 0xFFu, o, 0.0, d, tmax);
    // meshes are not opaque geometries: a hit counts unless it is on this mesh itself (`params.w`: its geometry + 1)
    while (rayQueryProceedEXT(q)) {
        if (rayQueryGetIntersectionTypeEXT(q, false) == gl_RayQueryCandidateIntersectionTriangleEXT
            && uint(rayQueryGetIntersectionGeometryIndexEXT(q, false)) + 1u != pc.params.w) {
            rayQueryConfirmIntersectionEXT(q);
        }
    }
    bool hit = rayQueryGetIntersectionTypeEXT(q, true) != gl_RayQueryCommittedIntersectionNoneEXT;
    t = hit ? rayQueryGetIntersectionTEXT(q, true) : tmax;
    return hit;
}

float ign() {
    return fract(52.9829189 * fract(dot(gl_FragCoord.xy, vec2(0.06711056, 0.00583715))));
}

// How much of a light at `light` reaches `p` past the world and every object (the mesh itself too): the light as a
// disc of radius 12, two rays, the segments stopping short of the light (a lamp's own mesh does not hide it).
float light_visible(vec3 p, vec3 n, vec3 light) {
    vec3 to0 = light - p;
    float dist = length(to0);
    vec3 dir = to0 / dist;
    vec3 o = p + n * (dot(n, dir) < 0.0 ? -1.0 : 1.0);
    vec3 a = abs(dir.z) < 0.9 ? vec3(0.0, 0.0, 1.0) : vec3(1.0, 0.0, 0.0);
    vec3 tx = normalize(cross(a, dir));
    vec3 ty = cross(dir, tx);
    float noise = ign();
    float seen = 0.0;
    for (int k = 0; k < 2; k++) {
        float ang = 6.2831853 * (float(k) * 0.5 + noise);
        float rad = 12.0 * sqrt((float(k) + 0.5) / 2.0);
        vec3 to = light + (tx * cos(ang) + ty * sin(ang)) * rad - o;
        float d = length(to);
        float t;
        if (d <= 25.0 || !blocked(o, to / d, d - 24.0, t)) seen += 0.5;
    }
    return seen;
}

// Ambient occlusion: two short rays over the hemisphere of `n`, closer hits darker (as the world's).
float occlusion(vec3 p, vec3 n) {
    const float R = 32.0;
    vec3 a = abs(n.z) < 0.9 ? vec3(0.0, 0.0, 1.0) : vec3(1.0, 0.0, 0.0);
    vec3 tx = normalize(cross(a, n));
    vec3 ty = cross(n, tx);
    float noise = ign();
    vec3 o = p + n * 1.0;
    float occ = 0.0;
    for (int i = 0; i < 2; i++) {
        float u = (float(i) + 0.5) / 2.0;
        float phi = 6.2831853 * (float(i) * 0.618034 + noise);
        float r = sqrt(u);
        vec3 d = normalize(tx * (r * cos(phi)) + ty * (r * sin(phi)) + n * sqrt(1.0 - u));
        float t;
        if (blocked(o, d, R, t)) occ += 1.0 - t / R;
    }
    return 1.0 - 0.6 * occ / 2.0;
}
#endif
layout(set = 0, binding = 0) uniform sampler2D tex;
layout(location = 0) in vec2 v_uv;
layout(location = 1) in vec3 v_color;
layout(location = 2) in vec3 v_fog;
layout(location = 3) in vec3 v_pos;
layout(location = 4) in vec3 v_normal;
layout(location = 5) flat in uint v_light;
layout(location = 0) out vec4 out_color;
void main() {
    // a mirror frame's `NearClip`: `DrawLodMesh` clips each polygon at it (Render 10b0bab0), keeping distance >= 0
    if (pc.clip.xyz != vec3(0.0) && dot(pc.clip.xyz, v_pos) - pc.clip.w < 0.0) discard;
    vec4 c = texture(tex, v_uv);
    float alpha = 1.0;
    if (pc.params.x != 0u) {
        if (A2C) {
            // multisampled: the texel's alpha sharpened to about one pixel's width, as sample coverage (smooth cut-outs)
            alpha = clamp((c.a - 0.5) / max(fwidth(c.a), 1e-4) + 0.5, 0.0, 1.0);
            if (alpha <= 0.0) discard;
        } else if (c.a * 255.0 <= 127.0) discard; // D3D ALPHAFUNC GREATER, ALPHAREF 127
    }
    // D3D fixed function: texture * diffuse, then the specular colour (the vertex fog; `SPECULARENABLE` only for
    // fogged faces, zero elsewhere) added and saturated
    vec3 color = v_color;
    if (PIXEL_LIGHT && v_light < 0xFFFFFFFEu) {
        // `lodmesh::vertex_light` (FLightManager's mesh lighting) at this pixel, from the interpolated position and normal
        vec4 h0 = slots.data[v_light];
        vec4 h1 = slots.data[v_light + 1u];
        vec3 n = normalize(v_normal);
        vec3 p = v_pos;
        vec3 e = p - h1.xyz;
        float e2 = dot(e, e);
        float w = dot(n, p);
        vec3 acc = vec3(0.0);
        uint count = uint(h1.w);
        for (uint i = 0u; i < count; i++) {
            vec4 l = slots.data[v_light + 2u + 2u * i];
            vec3 lc = slots.data[v_light + 3u + 2u * i].rgb;
            vec3 d = l.xyz - p;
            float d2 = dot(d, d);
            float dist = sqrt(d2);
            float k = dot(d, n) / dist + 1.0;
            float v = max(k * k - 1.5, 0.0);
            // the light mirrored in the tangent plane, seen in front: the specular term
            float pd = dot(n, l.xyz) - w;
            vec3 rv = (l.xyz - n * (pd + pd)) - h1.xyz;
            float s = dot(rv, e) - e2;
            if (s > 0.0) v += s * s * 6.0 / (d2 * e2);
            float f = v * (1.0 - dist * l.w);
#ifdef RAYTRACE
            // only lights in front of the surface are tested: the mesh formula's mirrored-light term also lights
            // surfaces from behind, which a shadow ray through the mesh itself would wrongly remove
            if (RT_SHADOWS && f > 0.0 && dot(d, n) > 0.0) f *= light_visible(p, n, l.xyz);
#endif
            if (f > 0.0) acc += f * lc;
        }
        color = min(acc * h0.w + h0.rgb, vec3(1.0));
    }
#ifdef RAYTRACE
    if (RT_AO && v_light != 0xFFFFFFFFu) {
        vec3 n = normalize(v_normal);
        color *= occlusion(v_pos, n);
    }
#endif
    out_color = vec4(min(c.rgb * color + v_fog, vec3(1.0)), alpha);
}
