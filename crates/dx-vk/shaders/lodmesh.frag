#version 450
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
layout(location = 0) out vec4 out_color;
void main() {
    // a mirror frame's `NearClip`: `DrawLodMesh` clips each polygon at it (Render 10b0bab0), keeping distance >= 0
    if (pc.clip.xyz != vec3(0.0) && dot(pc.clip.xyz, v_pos) - pc.clip.w < 0.0) discard;
    vec4 c = texture(tex, v_uv);
    if (pc.params.x != 0u && c.a * 255.0 <= 127.0) discard; // D3D ALPHAFUNC GREATER, ALPHAREF 127
    // D3D fixed function: texture * diffuse, then the specular colour (the vertex fog; `SPECULARENABLE` only for
    // fogged faces, zero elsewhere) added and saturated
    out_color = vec4(min(c.rgb * v_color + v_fog, vec3(1.0)), 1.0);
}
