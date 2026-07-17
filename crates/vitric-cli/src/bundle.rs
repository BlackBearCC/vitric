//! `vitric bundle` — bundle the project + engine into a single distributable file (the "standalone" in "standalone single-player").
//!
//! Stance: **release must also pass the gate**. bundle runs `vitric gate` first; no PASS, no
//! package — no certificate, no release; the gate report is printed to stdout as-is, so what
//! failed is clear at a glance. Clear-rate recordings (qa/ recordings referenced by gates) go
//! into the package: they are the certificate itself — the release package carries its own
//! replayable proof of delivery.
//!
//! ## Release package file format (self-unpacking)
//!
//! ```text
//! [engine binary raw bytes]
//! [blob = zlib(archive)]
//! [footer 16 bytes = MAGIC "VITRICPK"(8) + blob length u64 LE]
//! ```
//!
//! The archive is a minimal length-prefixed binary (serde_json+base64 is too wasteful for binary
//! assets; not introducing a new dependency family — flate2 is already in png/ureq's dependency
//! tree, miniz_oxide is a pure Rust backend):
//!
//! ```text
//! u32 LE file count
//! each file: u32 LE path length + UTF-8 relative path ('/' separated) + u64 LE content length + content bytes
//! ```
//!
//! Paths are sorted by BTreeMap and the zlib compression level is fixed — same input, same output,
//! the release package itself is reproducible.
//!
//! ## Startup detection (wired in main.rs)
//!
//! On startup vitric checks its own exe's last 16 bytes for a footer:
//! - **no arguments** → player double-click: unpack to `temp/vitric-<hash>/` then open a window
//!   and run (CPU render, runs anywhere);
//! - **`run-embedded [run options]`** → same as above but options are passed through — `--ticks 5`
//!   headless smoke test, `--renderer gpu` when the player wants GPU, both go through here;
//! - **other arguments** → normal CLI (the release package is also a complete engine).
//!
//! The unpack directory is unique by blob hash: the same package always unpacks to the same place;
//! project files are overwritten on every startup to guarantee consistency with the package, while
//! player saves in saves/ (not in the package) stay in place and persist with the package.
//!
//! ## Cross-platform
//!
//! Build a windows package on linux: `--engine <cross-compiled windows engine.exe>` — the footer
//! format is platform-agnostic; whichever engine it is appended to is whichever platform's release
//! package.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use serde_json::json;

/// Footer magic (release package = engine bytes + blob + MAGIC + blob length u64 LE).
pub const MAGIC: &[u8; 8] = b"VITRICPK";
/// Total footer length: magic 8 bytes + blob length 8 bytes.
const FOOTER_LEN: usize = 16;

/// Project top-level directories that do not go into the release package: player saves
/// (live in the unpack directory at runtime) and original asset backups.
const EXCLUDED_TOP: &[&str] = &["saves", "assets_original"];

/// File set in the package: relative path ('/' separated) → content bytes.
/// BTreeMap guarantees deterministic packing order.
pub type Files = BTreeMap<String, Vec<u8>>;

/// `vitric bundle <project directory> [--out <file>] [--engine <engine binary>]`
pub fn run(args: &[String]) -> Result<(), String> {
    let dir = PathBuf::from(args.first().ok_or("bundle 缺少项目目录参数")?);
    let mut out_arg: Option<PathBuf> = None;
    let mut engine_arg: Option<PathBuf> = None;
    let mut i = 1;
    while i < args.len() {
        let need = |key: &str| format!("{key} 缺少参数值");
        match args[i].as_str() {
            "--out" => {
                out_arg = Some(PathBuf::from(args.get(i + 1).ok_or(need("--out"))?));
                i += 2;
            }
            "--engine" => {
                engine_arg = Some(PathBuf::from(args.get(i + 1).ok_or(need("--engine"))?));
                i += 2;
            }
            other => return Err(format!("未知选项 {other:?}。可用: --out --engine")),
        }
    }

    // Gate first: no PASS, no release. Report goes to stdout (same as vitric gate);
    // on rejection, what failed is visible
    let (report, pass) = crate::gate::run(&dir)?;
    if !pass {
        println!("{}", serde_json::to_string_pretty(&report).expect("报告可序列化"));
        return Err(
            "交付门禁未通过，拒绝打包——无证书不发行（fail 项详见上方 JSON 报告）".to_string()
        );
    }
    let project_name =
        report["project"].as_str().expect("gate 报告必有项目名").to_string();

    // Engine binary: defaults to the currently running vitric; for cross-platform packaging
    // use --engine to point at a cross-compiled artifact
    let engine = match engine_arg {
        Some(p) => p,
        None => std::env::current_exe().map_err(|e| format!("定位当前引擎二进制失败: {e}"))?,
    };
    let engine_bytes =
        fs::read(&engine).map_err(|e| format!("读取引擎二进制 {} 失败: {e}", engine.display()))?;
    if embedded_range(&engine_bytes)?.is_some() {
        return Err(format!(
            "引擎 {} 自己已是发行包（尾部有内嵌项目），套娃打包会让玩家模式解错项目。\
             提示：用干净的引擎二进制，或加 --engine 指一个",
            engine.display()
        ));
    }

    // Default output name follows the engine's target platform: a windows engine (.exe) produces
    // <name>-windows.exe, otherwise follows the host platform. Use --out for a different name
    let windows_target = engine.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"));
    let (platform, suffix) = if windows_target {
        ("windows", ".exe")
    } else {
        (std::env::consts::OS, std::env::consts::EXE_SUFFIX)
    };
    let out = out_arg.unwrap_or_else(|| PathBuf::from(format!("{project_name}-{platform}{suffix}")));

    let files = collect_project_files(&dir, &out)?;
    let bundle = seal(engine_bytes, &files)?;
    fs::write(&out, &bundle).map_err(|e| format!("写发行包 {} 失败: {e}", out.display()))?;
    // The release package must be directly runnable: give everyone execute permission
    // (unix; windows looks at extension, not permission bits)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&out, fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("设置 {} 可执行位失败: {e}", out.display()))?;
    }

    println!(
        "{}",
        json!({
            "bundled": true,
            "out": out.display().to_string(),
            "bytes": bundle.len(),
            "project": project_name,
            "files": files.len(),
            "engine": engine.display().to_string(),
        })
    );
    Ok(())
}

/// Assemble release package bytes: engine + zlib(archive) + footer. Inverse of [`open`].
pub fn seal(engine_bytes: Vec<u8>, files: &Files) -> Result<Vec<u8>, String> {
    let blob = compress(&pack_archive(files)?)?;
    let mut out = engine_bytes;
    out.extend_from_slice(&blob);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(blob.len() as u64).to_le_bytes());
    Ok(out)
}

/// Extract the embedded project from release package bytes. `Ok(None)` = plain engine (no footer).
/// Returns (blob hash, path→content): the hash is the source of the unpack directory name;
/// same package, same directory.
pub fn open(bundle_bytes: &[u8]) -> Result<Option<(u64, Files)>, String> {
    let Some(range) = embedded_range(bundle_bytes)? else { return Ok(None) };
    let blob = &bundle_bytes[range];
    let hash = fnv1a64(blob);
    let archive = decompress(blob)?;
    Ok(Some((hash, unpack_archive(&archive)?)))
}

/// Unpack the embedded project from this exe to `temp/vitric-<hash>/`, returning the unpack directory.
/// `Ok(None)` = this exe is not a release package. Project files are overwritten every time
/// (kept consistent with the package); saves/ is not in the package, so player saves stay in the
/// unpack directory and persist with the package.
pub fn extract_self() -> Result<Option<PathBuf>, String> {
    let exe = std::env::current_exe().map_err(|e| format!("定位自身 exe 失败: {e}"))?;
    let bytes = fs::read(&exe).map_err(|e| format!("读取自身 exe {} 失败: {e}", exe.display()))?;
    let Some((hash, files)) = open(&bytes)? else { return Ok(None) };
    let dir = std::env::temp_dir().join(format!("vitric-{hash:016x}"));
    for (rel, data) in &files {
        let path = dir.join(rel); // rel was already verified safe in unpack_archive (relative, no ..)
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("解包建目录 {} 失败: {e}", parent.display()))?;
        }
        fs::write(&path, data).map_err(|e| format!("解包写 {} 失败: {e}", path.display()))?;
    }
    Ok(Some(dir))
}

/// Find the footer at the end of the byte stream, returning the blob's range in the stream.
/// `Ok(None)` = no footer (plain engine). Magic present but length mismatched = corrupt package;
/// explicitly error, don't silently treat as a plain engine.
pub fn embedded_range(bytes: &[u8]) -> Result<Option<std::ops::Range<usize>>, String> {
    if bytes.len() < FOOTER_LEN {
        return Ok(None);
    }
    let tail = &bytes[bytes.len() - FOOTER_LEN..];
    if &tail[..8] != MAGIC {
        return Ok(None);
    }
    let blob_len = u64::from_le_bytes(tail[8..16].try_into().expect("8 字节"));
    let end = bytes.len() - FOOTER_LEN;
    let blob_len = usize::try_from(blob_len)
        .ok()
        .filter(|n| *n <= end)
        .ok_or_else(|| {
            format!(
                "尾标声称内嵌包长 {blob_len} 字节，但尾标之前只有 {end} 字节——\
                 发行包损坏（截断或拼接出错），重新 vitric bundle 出一份"
            )
        })?;
    Ok(Some(end - blob_len..end))
}

/// Collect project files (relative path '/' separated → content).
/// Excludes: top-level saves/ and assets_original/, hidden entries across the whole tree
/// (.git/.DS_Store), and the output file itself (to prevent re-bundling a previously built
/// package into itself).
fn collect_project_files(root: &Path, out: &Path) -> Result<Files, String> {
    // The output file may not exist yet; canonicalize the parent directory + append the file name
    // to get a comparable absolute path
    let out_abs = out.parent().filter(|p| !p.as_os_str().is_empty()).map_or_else(
        || std::env::current_dir().ok().map(|d| d.join(out)),
        |p| p.canonicalize().ok().map(|p| p.join(out.file_name().unwrap_or_default())),
    );
    let mut files = BTreeMap::new();
    walk(root, root, out_abs.as_deref(), &mut files)?;
    Ok(files)
}

fn walk(root: &Path, dir: &Path, skip: Option<&Path>, out: &mut Files) -> Result<(), String> {
    let entries =
        fs::read_dir(dir).map_err(|e| format!("读目录 {} 失败: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("读目录 {} 条目失败: {e}", dir.display()))?;
        let path = entry.path();
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(format!(
                "{} 文件名不是 UTF-8——发行包路径要求 UTF-8，改个名字",
                path.display()
            ));
        };
        if name.starts_with('.') {
            continue; // hidden entries (.git etc.) do not go into the release package
        }
        if path.is_dir() {
            if dir == root && EXCLUDED_TOP.contains(&name) {
                continue;
            }
            walk(root, &path, skip, out)?;
            continue;
        }
        if let (Some(skip), Ok(abs)) = (skip, path.canonicalize()) {
            if abs == skip {
                continue; // the output file itself
            }
        }
        let rel = rel_path(root, &path)?;
        let bytes = fs::read(&path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
        out.insert(rel, bytes);
    }
    Ok(())
}

/// Relative path under root, unified '/' separator (one archive format across platforms).
fn rel_path(root: &Path, path: &Path) -> Result<String, String> {
    let rel = path.strip_prefix(root).expect("walk 只走 root 之下");
    let mut parts = Vec::new();
    for c in rel.components() {
        let s = c
            .as_os_str()
            .to_str()
            .ok_or_else(|| format!("{} 路径不是 UTF-8", path.display()))?;
        parts.push(s);
    }
    Ok(parts.join("/"))
}

/// Serialize archive: `u32 file count; each file u32 path length + path + u64 content length + content` (all LE).
pub fn pack_archive(files: &Files) -> Result<Vec<u8>, String> {
    let count = u32::try_from(files.len()).map_err(|_| "文件数超过 u32".to_string())?;
    let mut out = Vec::new();
    out.extend_from_slice(&count.to_le_bytes());
    for (path, data) in files {
        let p = path.as_bytes();
        let plen = u32::try_from(p.len()).map_err(|_| format!("路径过长: {path}"))?;
        out.extend_from_slice(&plen.to_le_bytes());
        out.extend_from_slice(p);
        out.extend_from_slice(&(data.len() as u64).to_le_bytes());
        out.extend_from_slice(data);
    }
    Ok(out)
}

/// Deserialize archive. Paths must be safe relative paths (unpacking writes files by them);
/// escapes (`..`/absolute paths/`\`), duplicates, and trailing extra bytes all explicitly error —
/// a corrupt package must not be half-unpacked.
pub fn unpack_archive(bytes: &[u8]) -> Result<Files, String> {
    let mut cur = 0usize;
    let take = |cur: &mut usize, n: usize| -> Result<&[u8], String> {
        let end = cur
            .checked_add(n)
            .filter(|e| *e <= bytes.len())
            .ok_or("档案在条目中途截断——发行包损坏")?;
        let s = &bytes[*cur..end];
        *cur = end;
        Ok(s)
    };
    let count = u32::from_le_bytes(take(&mut cur, 4)?.try_into().expect("4 字节"));
    let mut files = BTreeMap::new();
    for i in 0..count {
        let plen = u32::from_le_bytes(take(&mut cur, 4)?.try_into().expect("4 字节")) as usize;
        let path = std::str::from_utf8(take(&mut cur, plen)?)
            .map_err(|e| format!("档案条目 {i} 路径不是 UTF-8: {e}"))?
            .to_string();
        check_safe_rel_path(&path)?;
        let dlen = u64::from_le_bytes(take(&mut cur, 8)?.try_into().expect("8 字节"));
        let dlen = usize::try_from(dlen).map_err(|_| format!("条目 {path} 长度超过本机地址空间"))?;
        let data = take(&mut cur, dlen)?.to_vec();
        if files.insert(path.clone(), data).is_some() {
            return Err(format!("档案里路径 {path} 出现两次——发行包损坏"));
        }
    }
    if cur != bytes.len() {
        return Err(format!("档案尾部有 {} 字节多余数据——发行包损坏", bytes.len() - cur));
    }
    Ok(files)
}

/// Unpack path allowlist: non-empty, relative, '/' separated, no `..`/empty segments/backslashes.
fn check_safe_rel_path(path: &str) -> Result<(), String> {
    let bad = path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path.split('/').any(|seg| seg.is_empty() || seg == "." || seg == "..");
    if bad {
        return Err(format!(
            "档案路径 {path:?} 不是安全相对路径（禁止绝对路径/../反斜杠）——发行包损坏或被篡改"
        ));
    }
    Ok(())
}

fn compress(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
    enc.write_all(bytes).map_err(|e| format!("压缩失败: {e}"))?;
    enc.finish().map_err(|e| format!("压缩失败: {e}"))
}

fn decompress(blob: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    ZlibDecoder::new(blob)
        .read_to_end(&mut out)
        .map_err(|e| format!("内嵌包解压失败: {e}——发行包损坏"))?;
    Ok(out)
}

/// FNV-1a 64: gives the unpack directory a unique name. Only aims for "same package, same
/// directory"; not a cryptographic hash.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}
