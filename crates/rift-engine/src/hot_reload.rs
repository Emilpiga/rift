use anyhow::Result;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct HotReloader {
    _watcher: RecommendedWatcher,
    rx: mpsc::Receiver<notify::Result<Event>>,
    pub pipeline_dirty: Arc<AtomicBool>,
    shader_dir: PathBuf,
}

impl HotReloader {
    pub fn new(shader_dir: &Path) -> Result<Self> {
        let (tx, rx) = mpsc::channel();
        let pipeline_dirty = Arc::new(AtomicBool::new(false));

        let dirty_flag = pipeline_dirty.clone();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            if let Ok(event) = &res {
                match event.kind {
                    EventKind::Modify(_) | EventKind::Create(_) => {
                        // Only trigger for .vert/.frag files
                        let is_shader = event.paths.iter().any(|p| {
                            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
                            matches!(ext, "vert" | "frag" | "comp" | "geom")
                        });
                        if is_shader {
                            dirty_flag.store(true, Ordering::Relaxed);
                            log::info!("Shader change detected: {:?}", event.paths);
                        }
                    }
                    _ => {}
                }
            }
            tx.send(res).ok();
        })?;

        watcher.watch(shader_dir, RecursiveMode::Recursive)?;
        log::info!("Hot-reload: watching {:?} for shader changes", shader_dir);

        Ok(Self {
            _watcher: watcher,
            rx,
            pipeline_dirty,
            shader_dir: shader_dir.to_path_buf(),
        })
    }

    /// Returns true if shaders changed since last check.
    pub fn check_and_reset(&self) -> bool {
        // Drain events
        while self.rx.try_recv().is_ok() {}
        self.pipeline_dirty.swap(false, Ordering::Relaxed)
    }

    pub fn shader_dir(&self) -> &Path {
        &self.shader_dir
    }
}

/// Compile a GLSL shader from source at runtime.
pub fn compile_glsl(source: &str, filename: &str, kind: shaderc::ShaderKind) -> Result<Vec<u8>> {
    let compiler = shaderc::Compiler::new()?;
    let options = shaderc::CompileOptions::new()?;

    let result = compiler.compile_into_spirv(source, kind, filename, "main", Some(&options))?;

    if result.get_num_warnings() > 0 {
        log::warn!("Shader warnings in {}: {}", filename, result.get_warning_messages());
    }

    Ok(result.as_binary_u8().to_vec())
}

/// Determine shader kind from file extension.
pub fn shader_kind_from_path(path: &Path) -> Option<shaderc::ShaderKind> {
    match path.extension()?.to_str()? {
        "vert" => Some(shaderc::ShaderKind::Vertex),
        "frag" => Some(shaderc::ShaderKind::Fragment),
        "comp" => Some(shaderc::ShaderKind::Compute),
        "geom" => Some(shaderc::ShaderKind::Geometry),
        _ => None,
    }
}
