#version 450

// Fullscreen-triangle vertex shader shared by every post-process
// pass (bright extract, blur, composite). One draw call of three
// vertices, no vertex buffer. See `sky.vert` for the same trick.

layout(location = 0) out vec2 v_uv;

void main() {
    vec2 pos = vec2(
        float((gl_VertexIndex << 1) & 2) * 2.0 - 1.0,
        float(gl_VertexIndex & 2) * 2.0 - 1.0
    );
    // Map NDC straight to UV. Vulkan's NDC has Y pointing
    // down (y=-1 is the top of the screen) and texture UV
    // (0,0) is the top-left, so a direct `*0.5 + 0.5` mapping
    // already lines up — no Y flip needed. Flipping here would
    // sample the offscreen scene/bloom textures upside-down
    // and invert the whole composite.
    v_uv = pos * 0.5 + 0.5;
    gl_Position = vec4(pos, 0.0, 1.0);
}
