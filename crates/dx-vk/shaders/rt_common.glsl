// Shared by the ray-traced screen passes (`rt_post.rs`): the main view's camera, and positions from the depth buffer.
layout(push_constant) uniform Push {
    vec4 eye;     // xyz: the eye; w: q * near (the projection's depth terms, `world::view_proj`)
    vec4 fwd;     // xyz: forward; w: q
    vec4 right;   // xyz: right / sx
    vec4 up;      // xyz: up / sy
    vec4 rect;    // the main view in target pixels: x, y, width, height
    uvec4 params; // x: casters, y: 1 shadows + 2 ambient occlusion, z, w: the target's size
} pc;

// The world position of full-size pixel `px` whose depth is `depth` (the view is flipped: +y is up).
vec3 world_at(ivec2 px, float depth) {
    vec2 ndc = vec2(((float(px.x) + 0.5 - pc.rect.x) / pc.rect.z) * 2.0 - 1.0, 1.0 - ((float(px.y) + 0.5 - pc.rect.y) / pc.rect.w) * 2.0);
    float z = pc.eye.w / (pc.fwd.w - depth);
    return pc.eye.xyz + pc.fwd.xyz * z + pc.right.xyz * (ndc.x * z) + pc.up.xyz * (ndc.y * z);
}

float noise_at(vec2 px) {
    return fract(52.9829189 * fract(dot(px, vec2(0.06711056, 0.00583715))));
}
