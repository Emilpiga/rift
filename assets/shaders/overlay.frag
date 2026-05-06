#version 450

layout(binding = 0) uniform sampler2D fontAtlas;

layout(location = 0) in vec4 fragColor;
layout(location = 1) in vec2 fragUV;

layout(location = 0) out vec4 outColor;

void main() {
    // RGBA atlas: solid-colour rects sample the white pixel at
    // UV(0,0) -> (1,1,1,1); font glyphs are stored as
    // (1,1,1, mask); icons store full RGBA. Multiplying by the
    // vertex colour lets callers tint glyphs/icons or fill rects
    // with arbitrary colours through the same path.
    vec4 tex = texture(fontAtlas, fragUV);
    outColor = fragColor * tex;
}
