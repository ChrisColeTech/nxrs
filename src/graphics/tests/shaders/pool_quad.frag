#version 450
#extension GL_EXT_nonuniform_qualifier : require
layout(set = 6, binding = 0) uniform texture2D textures[];
layout(set = 6, binding = 1) uniform sampler samplers[];
layout(push_constant) uniform Pools { uint texture_base; uint sampler_base; } pools;
layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 o_color;
void main() {
    o_color = texture(sampler2D(textures[pools.texture_base + 5], samplers[pools.sampler_base + 3]), v_uv);
}
