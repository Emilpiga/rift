#!/bin/bash
# Compile GLSL shaders to SPIR-V
# Requires glslc (from Vulkan SDK) or you can use the shaderc crate at build time

SHADER_DIR="assets/shaders"

if command -v glslc &> /dev/null; then
    echo "Using glslc..."
    glslc "$SHADER_DIR/triangle.vert" -o "$SHADER_DIR/triangle.vert.spv"
    glslc "$SHADER_DIR/triangle.frag" -o "$SHADER_DIR/triangle.frag.spv"
    echo "Shaders compiled successfully."
else
    echo "glslc not found. Install the Vulkan SDK or use 'cargo build' (shaderc build script handles compilation)."
    exit 1
fi
