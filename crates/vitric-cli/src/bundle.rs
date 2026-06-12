//! `vitric bundle` — 把项目 + 引擎打成一个可分发的单文件（"独立单机"的独立）。
//!
//! 立场：**发行也要过门禁**。bundle 先跑 `vitric gate`，不 PASS 不出包——无证书不发行；
//! 门禁报告原样打到 stdout，差在哪一目了然。通关录像（gates 引用的 qa/ 录像）会进包：
//! 它们就是证书本体，发行包自带可重放的交付证明。
//!
//! ## 发行包文件格式（自解包式）
//!
//! ```text
//! [引擎二进制原字节]
//! [blob = zlib(档案)]
//! [尾标 16 字节 = MAGIC "VITRICPK"(8) + blob 长度 u64 LE]
//! ```
//!
//! 档案是极简长度前缀二进制（serde_json+base64 对二进制素材太浪费；不引新依赖家族，
//! flate2 早已在 png/ureq 的依赖树里，miniz_oxide 纯 Rust 后端）：
//!
//! ```text
//! u32 LE 文件数
//! 每个文件: u32 LE 路径长 + UTF-8 相对路径('/' 分隔) + u64 LE 内容长 + 内容字节
//! ```
//!
//! 路径按 BTreeMap 排序、zlib 压缩级固定——同输入同输出，发行包本身可复现。
//!
//! ## 启动检测（main.rs 接线）
//!
//! vitric 启动时看自己 exe 末尾 16 字节有没有尾标：
//! - **无参数** → 玩家双击：解包到 `temp/vitric-<哈希>/` 后开窗运行（CPU 渲染，处处能跑）；
//! - **`run-embedded [run 选项]`** → 同上但选项透传——`--ticks 5` 无头冒烟、
//!   `--renderer gpu` 玩家要 GPU，都从这进；
//! - **其他参数** → 正常 CLI（发行包同时也是完整引擎）。
//!
//! 解包目录按 blob 哈希唯一：同一个包永远解到同一处；项目文件每次启动覆写保证和包一致，
//! 玩家存档 saves/（不进包）留在原地随包持久。
//!
//! ## 跨平台
//!
//! 在 linux 上给 windows 出包：`--engine <交叉编译好的 windows 引擎.exe>`——
//! 尾标格式与平台无关，附在哪个引擎上就是哪个平台的发行包。

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use serde_json::json;

/// 尾标魔数（发行包 = 引擎字节 + blob + MAGIC + blob 长度 u64 LE）。
pub const MAGIC: &[u8; 8] = b"VITRICPK";
/// 尾标总长：魔数 8 字节 + blob 长度 8 字节。
const FOOTER_LEN: usize = 16;

/// 不进发行包的项目顶层目录：玩家存档（运行时长在解包目录里）和原始素材备份。
const EXCLUDED_TOP: &[&str] = &["saves", "assets_original"];

/// 包里的文件集：相对路径（'/' 分隔）→ 内容字节。BTreeMap 保证打包顺序确定。
pub type Files = BTreeMap<String, Vec<u8>>;

/// `vitric bundle <项目目录> [--out <文件>] [--engine <引擎二进制>]`
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

    // 门禁先行：不 PASS 不发行。报告打到 stdout（同 vitric gate），拒绝时差在哪看得见
    let (report, pass) = crate::gate::run(&dir)?;
    if !pass {
        println!("{}", serde_json::to_string_pretty(&report).expect("报告可序列化"));
        return Err(
            "交付门禁未通过，拒绝打包——无证书不发行（fail 项详见上方 JSON 报告）".to_string()
        );
    }
    let project_name =
        report["project"].as_str().expect("gate 报告必有项目名").to_string();

    // 引擎二进制：缺省就是正在跑的这个 vitric；跨平台出包用 --engine 指交叉编译产物
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

    // 缺省输出名跟引擎的目标平台走：windows 引擎（.exe）出 <名>-windows.exe，
    // 否则按宿主平台。要别的名字用 --out
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
    // 发行包要能直接跑：给所有人可执行位（unix；windows 看扩展名不看权限位）
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

/// 组装发行包字节：引擎 + zlib(档案) + 尾标。与 [`open`] 互逆。
pub fn seal(engine_bytes: Vec<u8>, files: &Files) -> Result<Vec<u8>, String> {
    let blob = compress(&pack_archive(files)?)?;
    let mut out = engine_bytes;
    out.extend_from_slice(&blob);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(blob.len() as u64).to_le_bytes());
    Ok(out)
}

/// 从发行包字节里取出内嵌项目。`Ok(None)` = 普通引擎（无尾标）。
/// 返回 (blob 哈希, 路径→内容)：哈希就是解包目录名的来源，同包同目录。
pub fn open(bundle_bytes: &[u8]) -> Result<Option<(u64, Files)>, String> {
    let Some(range) = embedded_range(bundle_bytes)? else { return Ok(None) };
    let blob = &bundle_bytes[range];
    let hash = fnv1a64(blob);
    let archive = decompress(blob)?;
    Ok(Some((hash, unpack_archive(&archive)?)))
}

/// 把自己 exe 里的内嵌项目解到 `temp/vitric-<哈希>/`，返回解包目录。
/// `Ok(None)` = 本 exe 不是发行包。项目文件每次覆写（和包保持一致）；
/// saves/ 不在包里，玩家存档留在解包目录里随包持久。
pub fn extract_self() -> Result<Option<PathBuf>, String> {
    let exe = std::env::current_exe().map_err(|e| format!("定位自身 exe 失败: {e}"))?;
    let bytes = fs::read(&exe).map_err(|e| format!("读取自身 exe {} 失败: {e}", exe.display()))?;
    let Some((hash, files)) = open(&bytes)? else { return Ok(None) };
    let dir = std::env::temp_dir().join(format!("vitric-{hash:016x}"));
    for (rel, data) in &files {
        let path = dir.join(rel); // rel 已在 unpack_archive 里验证过安全（相对、无 ..）
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("解包建目录 {} 失败: {e}", parent.display()))?;
        }
        fs::write(&path, data).map_err(|e| format!("解包写 {} 失败: {e}", path.display()))?;
    }
    Ok(Some(dir))
}

/// 字节流末尾找尾标，返回 blob 在流里的范围。`Ok(None)` = 没有尾标（普通引擎）。
/// 有魔数但长度对不上 = 包损坏，显式报错不静默当普通引擎。
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

/// 收集项目文件（相对路径 '/' 分隔 → 内容）。
/// 排除：顶层 saves/ 与 assets_original/、全树隐藏项（.git/.DS_Store）、
/// 以及输出文件自身（防把上一次打的包再打进包）。
fn collect_project_files(root: &Path, out: &Path) -> Result<Files, String> {
    // 输出文件可能还不存在，canonicalize 父目录 + 拼文件名得到可比对的绝对路径
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
            continue; // 隐藏项（.git 等）不进发行包
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
                continue; // 输出文件自己
            }
        }
        let rel = rel_path(root, &path)?;
        let bytes = fs::read(&path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
        out.insert(rel, bytes);
    }
    Ok(())
}

/// root 下的相对路径，统一 '/' 分隔（跨平台一份档案格式）。
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

/// 序列化档案：`u32 文件数；每个文件 u32 路径长 + 路径 + u64 内容长 + 内容`（全 LE）。
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

/// 反序列化档案。路径必须是安全相对路径（解包就是按它写文件），
/// 越界（`..`/绝对路径/`\`）、重复、尾部多余字节都显式报错——损坏的包不能半解。
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

/// 解包路径白名单：非空、相对、'/' 分隔、不含 `..`/空段/反斜杠。
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

/// FNV-1a 64：给解包目录起唯一名。只求"同包同目录"，不是密码学哈希。
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}
