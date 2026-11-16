#version 450
// the same position for the same vertex in every pipeline that draws it (depth-equal multi-pass drawing)
invariant gl_Position;
layout(push_constant) uniform Push { mat4 view_proj; } pc;
layout(location = 0) in vec3 in_pos;
layout(location = 1) in vec3 in_color;
layout(location = 0) out vec3 v_color;
void main() {
    gl_Position = pc.view_proj * vec4(in_pos, 1.0);
    v_color = in_color;
}
