#version 450
layout(location = 0) in vec3 a_pos;
layout(location = 1) in vec4 a_color;
layout(location = 0) out vec4 v_color;
layout(set = 0, binding = 1) buffer Rec { uint count; uint rest[]; } rec;
void main() {
    // an append counter: the indices this draw adds to a list, counted in an indirect record
    if (gl_VertexIndex == 0) atomicAdd(rec.count, 6u);
    gl_Position = vec4(a_pos, 1.0);
    v_color = a_color;
}
