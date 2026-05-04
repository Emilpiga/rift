#version 450

layout(binding = 0) uniform sampler2D fontAtlas;

layout(location = 0) in vec4 fragColor;
layout(location = 1) in vec2 fragUV;

layout(location = 0) out vec4 outColor;

void main() {
    float alpha = texture(fontAtlas, fragUV).r;
    // UV (0,0) = top-left of white pixel region (solid quads use that)
    outColor = vec4(fragColor.rgb, fragColor.a * alpha);
}
