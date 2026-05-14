use anyhow::{anyhow, bail, Context, Result};
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

const DEFAULT_ROOT: &str = "game";
const CONFIG_FILE: &str = "launcher-manifest-url.txt";

#[derive(Debug)]
struct Options {
    manifest_url: String,
    install_root: PathBuf,
    launch: bool,
    jobs: usize,
    game_args: Vec<String>,
}

#[derive(Debug)]
struct Manifest {
    version: String,
    entrypoint: PathBuf,
    files: Vec<ManifestFile>,
}

#[derive(Debug)]
struct ManifestFile {
    path: PathBuf,
    size: u64,
    sha256: String,
    url: String,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("rift-launcher: {err:#}");
        wait_for_enter();
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let options = parse_args()?;
    println!("Rift launcher");
    println!("manifest: {}", options.manifest_url);
    println!("install:  {}", options.install_root.display());
    println!("jobs:     {}", options.jobs);

    let manifest_text = http_get_text(&options.manifest_url).context("download manifest")?;
    let manifest = parse_manifest(&manifest_text).context("parse manifest")?;
    println!("version:  {}", manifest.version);

    fs::create_dir_all(&options.install_root).context("create install directory")?;
    let state_dir = options.install_root.join(".rift-launcher");
    fs::create_dir_all(&state_dir).context("create launcher state directory")?;

    let updates = collect_changed_files(&options.install_root, &manifest.files, options.jobs)?;
    let current = manifest.files.len() - updates.len();
    println!("check   {current} current, {} changed", updates.len());

    let updated = updates.len();
    if updated > 0 {
        println!("fetch   {updated} changed file(s)");
        download_updates_parallel(
            &options.install_root,
            &state_dir,
            &manifest.version,
            &updates,
            options.jobs,
        )?;
    }

    write_installed_version(&state_dir, &manifest.version)?;
    if updated == 0 {
        println!("up to date");
    } else {
        println!("updated {updated} file(s)");
    }

    if options.launch {
        launch_game(
            &options.install_root,
            &manifest.entrypoint,
            &options.game_args,
        )?;
    }

    Ok(())
}

fn parse_args() -> Result<Options> {
    let mut manifest_url = env::var("RIFT_LAUNCHER_MANIFEST_URL").ok();
    let mut install_root = PathBuf::from(DEFAULT_ROOT);
    let mut launch = true;
    let mut jobs = default_jobs();
    let mut game_args = Vec::new();

    let mut args = env::args().skip(1).peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--manifest" => {
                manifest_url = Some(
                    args.next()
                        .ok_or_else(|| anyhow!("--manifest needs a URL"))?,
                );
            }
            "--root" => {
                install_root =
                    PathBuf::from(args.next().ok_or_else(|| anyhow!("--root needs a path"))?);
            }
            "--jobs" => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow!("--jobs needs a number"))?;
                jobs = value
                    .parse::<usize>()
                    .with_context(|| format!("invalid --jobs value: {value}"))?;
                if jobs == 0 {
                    bail!("--jobs must be at least 1");
                }
            }
            "--no-launch" => launch = false,
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            "--" => {
                game_args.extend(args);
                break;
            }
            other => bail!("unknown argument: {other}"),
        }
    }

    let manifest_url = manifest_url
        .or_else(default_manifest_from_compile_env)
        .or_else(default_manifest_from_config_file)
        .ok_or_else(|| anyhow!("no manifest URL set; pass --manifest URL or place {CONFIG_FILE} next to the launcher"))?;

    Ok(Options {
        manifest_url,
        install_root,
        launch,
        jobs,
        game_args,
    })
}

fn print_help() {
    println!(
        "rift-launcher\n\n\
Usage:\n\
  rift-launcher.exe --manifest https://example.com/rift/manifest.txt\n\
  rift-launcher.exe --root game --manifest URL -- --connect HOST:PORT\n\n\
Options:\n\
  --manifest URL   Update manifest URL.\n\
  --root PATH      Install directory. Default: game\n\
    --jobs N         Parallel downloads. Default: min(8, available CPUs).\n\
  --no-launch      Update only; do not start the game.\n\
  --               Remaining args are passed to the game.\n\n\
If --manifest is omitted, the launcher reads RIFT_LAUNCHER_MANIFEST_URL,\n\
then a launcher-manifest-url.txt file next to the launcher."
    );
}

fn default_jobs() -> usize {
    std::thread::available_parallelism()
        .map(|value| value.get().clamp(1, 8))
        .unwrap_or(4)
}

fn default_manifest_from_compile_env() -> Option<String> {
    option_env!("RIFT_LAUNCHER_MANIFEST_URL")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn default_manifest_from_config_file() -> Option<String> {
    let exe = env::current_exe().ok()?;
    let path = exe.parent()?.join(CONFIG_FILE);
    fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn parse_manifest(text: &str) -> Result<Manifest> {
    let mut lines = text.lines().enumerate();
    let Some((_, header)) = lines.next() else {
        bail!("manifest is empty");
    };
    if header.trim() != "rift-launcher-manifest-v1" {
        bail!("unsupported manifest header: {header}");
    }

    let mut version = None;
    let mut entrypoint = None;
    let mut files = Vec::new();

    for (line_no, raw) in lines {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        match fields.as_slice() {
            ["version", value] => version = Some((*value).to_owned()),
            ["entrypoint", value] => entrypoint = Some(safe_relative_path(value)?),
            ["file", path, size, sha256, url] => {
                let size = size
                    .parse::<u64>()
                    .with_context(|| format!("line {} has invalid size", line_no + 1))?;
                let sha256 = sha256.to_ascii_lowercase();
                if sha256.len() != 64 || !sha256.chars().all(|c| c.is_ascii_hexdigit()) {
                    bail!("line {} has invalid sha256", line_no + 1);
                }
                files.push(ManifestFile {
                    path: safe_relative_path(path)?,
                    size,
                    sha256,
                    url: (*url).to_owned(),
                });
            }
            _ => bail!("line {} has invalid manifest syntax", line_no + 1),
        }
    }

    let version = version.ok_or_else(|| anyhow!("manifest missing version"))?;
    let entrypoint = entrypoint.ok_or_else(|| anyhow!("manifest missing entrypoint"))?;
    if files.is_empty() {
        bail!("manifest contains no files");
    }

    Ok(Manifest {
        version,
        entrypoint,
        files,
    })
}

fn safe_relative_path(value: &str) -> Result<PathBuf> {
    let path = Path::new(value);
    if path.as_os_str().is_empty() {
        bail!("empty path in manifest");
    }

    let mut cleaned = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => cleaned.push(part),
            Component::CurDir => {}
            _ => bail!("unsafe path in manifest: {value}"),
        }
    }
    Ok(cleaned)
}

fn file_is_current(path: &Path, expected_hash: &str, expected_size: u64) -> Result<bool> {
    let Ok(meta) = fs::metadata(path) else {
        return Ok(false);
    };
    if !meta.is_file() || meta.len() != expected_size {
        return Ok(false);
    }
    Ok(sha256_file(path)? == expected_hash)
}

fn collect_changed_files<'a>(
    install_root: &Path,
    files: &'a [ManifestFile],
    jobs: usize,
) -> Result<Vec<&'a ManifestFile>> {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs)
        .thread_name(|index| format!("rift-launcher-check-{index}"))
        .build()
        .context("create file check worker pool")?;

    pool.install(|| {
        files
            .par_iter()
            .map(|file| {
                let path = install_root.join(&file.path);
                if file_is_current(&path, &file.sha256, file.size)
                    .with_context(|| format!("check {}", display_path(&file.path)))?
                {
                    Ok(None)
                } else {
                    Ok(Some(file))
                }
            })
            .collect::<Result<Vec<_>>>()
            .map(|entries| entries.into_iter().flatten().collect())
    })
}

fn download_and_install(
    install_root: &Path,
    state_dir: &Path,
    version: &str,
    file: &ManifestFile,
) -> Result<()> {
    let bytes = http_get_bytes(&file.url).context("download file")?;
    if bytes.len() as u64 != file.size {
        bail!(
            "downloaded size mismatch: got {}, expected {}",
            bytes.len(),
            file.size
        );
    }
    let hash = sha256_bytes(&bytes);
    if hash != file.sha256 {
        bail!(
            "downloaded hash mismatch: got {hash}, expected {}",
            file.sha256
        );
    }

    let target = install_root.join(&file.path);
    let tmp = state_dir.join("tmp").join(&file.path);
    if let Some(parent) = tmp.parent() {
        fs::create_dir_all(parent)?;
    }

    {
        let mut out = fs::File::create(&tmp)?;
        out.write_all(&bytes)?;
        out.sync_all().ok();
    }

    if target.exists() {
        let backup = state_dir.join("backup").join(version).join(&file.path);
        if let Some(parent) = backup.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&target, &backup).with_context(|| format!("backup {}", target.display()))?;
        fs::remove_file(&target).with_context(|| format!("replace {}", target.display()))?;
    }

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(&tmp, &target).or_else(|_| {
        fs::copy(&tmp, &target)?;
        fs::remove_file(&tmp)?;
        Ok::<_, io::Error>(())
    })?;

    Ok(())
}

fn download_updates_parallel(
    install_root: &Path,
    state_dir: &Path,
    version: &str,
    files: &[&ManifestFile],
    jobs: usize,
) -> Result<()> {
    let completed = AtomicUsize::new(0);
    let total = files.len();
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs)
        .thread_name(|index| format!("rift-launcher-download-{index}"))
        .build()
        .context("create download worker pool")?;

    pool.install(|| {
        files.par_iter().try_for_each(|file| {
            download_and_install(install_root, state_dir, version, file)
                .with_context(|| format!("update {}", display_path(&file.path)))?;

            let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
            println!("fetch   {done}/{total} {}", display_path(&file.path));
            Ok::<_, anyhow::Error>(())
        })
    })
}

fn http_get_text(url: &str) -> Result<String> {
    let response = ureq::get(url)
        .call()
        .map_err(|err| anyhow!(err.to_string()))?;
    response.into_string().map_err(Into::into)
}

fn http_get_bytes(url: &str) -> Result<Vec<u8>> {
    let response = ureq::get(url)
        .call()
        .map_err(|err| anyhow!(err.to_string()))?;
    let mut bytes = Vec::new();
    response.into_reader().read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

fn write_installed_version(state_dir: &Path, version: &str) -> Result<()> {
    fs::create_dir_all(state_dir)?;
    fs::write(state_dir.join("installed-version.txt"), version)?;
    Ok(())
}

fn launch_game(root: &Path, entrypoint: &Path, args: &[String]) -> Result<()> {
    let root = root
        .canonicalize()
        .with_context(|| format!("resolve install root {}", root.display()))?;
    let exe = root.join(entrypoint);
    if !exe.exists() {
        bail!("entrypoint missing after update: {}", exe.display());
    }

    println!("launch  {}", exe.display());
    Command::new(&exe)
        .args(args)
        .current_dir(&root)
        .spawn()
        .with_context(|| format!("launch {}", exe.display()))?;
    Ok(())
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn wait_for_enter() {
    if env::var_os("RIFT_LAUNCHER_NO_PAUSE").is_some() {
        return;
    }
    eprintln!("Press Enter to close...");
    let mut line = String::new();
    let _ = io::stdin().read_line(&mut line);
}
