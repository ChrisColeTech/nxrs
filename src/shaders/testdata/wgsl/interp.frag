#version 450
// WEB-W10b: a fragment stage reading input 0 without perspective (Vulkan uses this stage's interpolation)
layout(location = 0) noperspective in vec4 o;
layout(location = 1) flat in uvec4 ko;
layout(location = 0) out vec4 c;
void main() {
    c = o + vec4(ko);
}
