#version 450
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
    if (PIXEL_LIGHT && v_light != 0xFFFFFFFFu) {
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
            if (f > 0.0) acc += f * lc;
        }
        color = min(acc * h0.w + h0.rgb, vec3(1.0));
    }
    out_color = vec4(min(c.rgb * color + v_fog, vec3(1.0)), alpha);
}
