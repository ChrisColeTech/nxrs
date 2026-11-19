#version 450
// lane C's convention: the support buffer at set 0 binding 0 for every stage, the fragment stage's own resources in
// set 1 from binding 1 (described by the reflection json the test writes beside it)
layout(set = 0, binding = 0) uniform Support {
    uvec4 alpha_test;
    uvec4 is_bgra[8];
    vec4 viewport_inverse;
    vec4 viewport_size;
    ivec4 frag_scale_count;
    vec4 render_scale[73];
} sb;
layout(set = 1, binding = 1) uniform sampler2D tex0;
layout(set = 1, binding = 2) uniform C3 { vec4 tint; } c3;
layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 o_color;
void main() {
    o_color = texture(tex0, v_uv) * c3.tint * sb.render_scale[0].x;
}
