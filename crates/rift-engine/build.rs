use std::fs;
use std::path::Path;

fn main() {
    let shader_dir = Path::new("../../assets/shaders");
    let out_dir = shader_dir; // compile in-place next to source

    let compiler = shaderc::Compiler::new().expect("Failed to create shader compiler");
    let mut options = shaderc::CompileOptions::new().expect("Failed to create compile options");
    options.set_optimization_level(shaderc::OptimizationLevel::Performance);

    compile_shader(
        &compiler,
        &options,
        shader_dir.join("triangle.vert"),
        out_dir.join("triangle.vert.spv"),
        shaderc::ShaderKind::Vertex,
    );

    compile_shader(
        &compiler,
        &options,
        shader_dir.join("triangle.frag"),
        out_dir.join("triangle.frag.spv"),
        shaderc::ShaderKind::Fragment,
    );

    compile_shader(
        &compiler,
        &options,
        shader_dir.join("blood_splat.vert"),
        out_dir.join("blood_splat.vert.spv"),
        shaderc::ShaderKind::Vertex,
    );

    compile_shader(
        &compiler,
        &options,
        shader_dir.join("blood_splat.frag"),
        out_dir.join("blood_splat.frag.spv"),
        shaderc::ShaderKind::Fragment,
    );

    println!("cargo::rerun-if-changed=../../assets/shaders/");
}

fn compile_shader(
    compiler: &shaderc::Compiler,
    options: &shaderc::CompileOptions,
    input: std::path::PathBuf,
    output: std::path::PathBuf,
    kind: shaderc::ShaderKind,
) {
    let source = fs::read_to_string(&input)
        .unwrap_or_else(|e| panic!("Failed to read shader {:?}: {}", input, e));

    let file_name = input.file_name().unwrap().to_str().unwrap();

    let artifact = compiler
        .compile_into_spirv(&source, kind, file_name, "main", Some(options))
        .unwrap_or_else(|e| panic!("Failed to compile shader {:?}: {}", input, e));

    if artifact.get_num_warnings() > 0 {
        println!("cargo::warning=Shader warnings in {:?}: {}", input, artifact.get_warning_messages());
    }

    fs::write(&output, artifact.as_binary_u8())
        .unwrap_or_else(|e| panic!("Failed to write SPIR-V {:?}: {}", output, e));
}
