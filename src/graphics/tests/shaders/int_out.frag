#version 450
layout(location = 0) out vec4 o_color;
void main() {
    // the bits of an integer target written through a float output, as translated pixel shaders do
    o_color = vec4(uintBitsToFloat(0x12345678u), 0.0, 0.0, 0.0);
}
