#version 450
// A mesh triangle as `DrawLodMesh` (Render 10b0ea80) hands it to the device: world position, texture coordinates (in
// units of the texture) and the vertex colour (`FLightManager::Light`, the glow colour of unlit faces, white for
// modulated ones) and fog (`FLightManager::Fog`, D3D's specular colour), all computed on the CPU (`lodmesh::build`).
layout(push_constant) uniform Push {
    mat4 vp;
    uvec4 params; // x: alpha test (PF_Masked)
    vec4 clip;    // a mirror frame's plane: points with dot(xyz, p) < w are not drawn; all zero = off
} pc;
layout(location = 0) in vec3 in_pos;
layout(location = 1) in vec2 in_uv;
layout(location = 2) in vec3 in_color;
layout(location = 3) in vec3 in_fog;
layout(location = 4) in vec3 in_normal;
layout(location = 5) in uint in_light;
layout(location = 0) out vec2 v_uv;
layout(location = 1) out vec3 v_color;
layout(location = 2) out vec3 v_fog;
layout(location = 3) out vec3 v_pos;
layout(location = 4) out vec3 v_normal;
layout(location = 5) flat out uint v_light;
void main() {
    v_uv = in_uv;
    v_color = in_color;
    v_fog = in_fog;
    v_pos = in_pos;
    v_normal = in_normal;
    v_light = in_light;
    gl_Position = pc.vp * vec4(in_pos, 1.0);
}
