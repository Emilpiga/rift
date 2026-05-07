#version 450

// Fullscreen-triangle sky vertex shader.
//
// No vertex buffer is bound — we generate three NDC vertices from
// `gl_VertexIndex` so the shader covers the whole framebuffer with
// one draw call of three vertices. Indices map to:
//   0 -> (-1, -1)
//   1 -> ( 3, -1)
//   2 -> (-1,  3)
// which is the standard "oversized triangle clipped to the viewport"
// trick — same coverage as a quad with one fewer triangle and no
// edge artefacts at the diagonal seam.
//
// `z = 1.0` parks the triangle on the far plane so any subsequent
// scene draws (with depth test enabled) sit in front of the sky.

layout(location = 0) out vec2 v_ndc;

void main() {
    vec2 pos = vec2(
        float((gl_VertexIndex << 1) & 2) * 2.0 - 1.0,
        float(gl_VertexIndex & 2) * 2.0 - 1.0
    );
    v_ndc = pos;
    gl_Position = vec4(pos, 1.0, 1.0);
}
