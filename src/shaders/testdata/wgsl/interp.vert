#version 450
// WEB-W10b: a vertex stage whose output 0 is perspective (the default)
layout(location = 0) in vec4 a;
layout(location = 1) in vec4 n;
layout(location = 2) in uvec4 k;
layout(location = 0) out vec4 o;
layout(location = 1) flat out uvec4 ko;
void main() {
    gl_Position = a;
    o = n * 2.0;
    ko = k;
}
