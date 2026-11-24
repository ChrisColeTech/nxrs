#version 450
layout(location = 0) out vec4 varying_value;
void main() {
    vec2 p = vec2((gl_VertexIndex << 1) & 2, gl_VertexIndex & 2);
    gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
    varying_value.x = 0.25;
    varying_value.y = 0.5;
}
