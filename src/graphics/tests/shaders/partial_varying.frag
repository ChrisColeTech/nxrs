#version 450
layout(location = 0) in vec4 varying_value;
layout(location = 0) out vec4 color;
void main() {
    color = vec4(varying_value.xyz / varying_value.w, varying_value.w);
}
