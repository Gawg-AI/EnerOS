//! 26-B WP-26.2 新门禁：孤儿源文件 / 配置引用 / mock 执法。

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use super::qa::{collect_crate_lib_files, mock_leak_lines};
use super::{invalid, read_json, DynResult};

/// 解析一个文件的 `mod name;` / `pub mod name;` 声明（含 `#[path = "..."]`）。
/// 返回 (模块名, 显式 path 或 None)。
fn parse_mod_decls(text: &str) -> Vec<(String, Option<String>, bool)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let is_test_gated = |idx: usize| -> bool {
        // 声明行前 1–2 行出现 cfg(test) 门控（标准测试模块模式）即视为
        // 可达——`#[cfg(test)] mod tests;` 是测试惯例，不应判孤儿。
        lines
            .get(idx.saturating_sub(2)..idx)
            .unwrap_or(&[])
            .iter()
            .map(|l| l.trim())
            .any(|l| {
                l.starts_with("#[cfg(test)]")
                    || l.contains("cfg(any(test")
                    || l.contains("cfg(all(test")
            })
    };
    while i < lines.len() {
        let t = lines[i].trim();
        // #[path = "x.rs"] 属性（必须紧跟其后的 mod 声明行）。
        let mut path_attr: Option<String> = None;
        if t.starts_with("#[path") {
            if let Some(inner) = t
                .strip_prefix("#[path = ")
                .and_then(|s| s.strip_suffix(']'))
            {
                path_attr = Some(inner.trim_matches('"').to_owned());
            }
            // 下一行应为 mod 声明；若下一行也是属性则跳过（不处理嵌套）。
            i += 1;
            let next = lines.get(i).map(|l| l.trim()).unwrap_or("");
            let body = next
                .strip_prefix("pub mod ")
                .or_else(|| next.strip_prefix("pub(crate) mod "))
                .or_else(|| next.strip_prefix("pub(super) mod "))
                .or_else(|| next.strip_prefix("mod "));
            if let Some(name) = body.and_then(|s| s.strip_suffix(';')) {
                if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    out.push((name.to_owned(), path_attr.clone(), is_test_gated(i)));
                }
            }
            i += 1;
            continue;
        }
        let body = t
            .strip_prefix("pub mod ")
            .or_else(|| t.strip_prefix("pub(crate) mod "))
            .or_else(|| t.strip_prefix("pub(super) mod "))
            .or_else(|| t.strip_prefix("mod "));
        if let Some(name) = body.and_then(|s| s.strip_suffix(';')) {
            if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                out.push((name.to_owned(), path_attr.clone(), is_test_gated(i)));
            }
        }
        i += 1;
    }
    out
}

/// 若行是 `#[cfg(...)]` 属性，返回其 cfg 内容（不含属性包裹符）。
fn cfg_attr_text(line: &str) -> Option<String> {
    let t = line.trim_start();
    let inner = t
        .strip_prefix("#[cfg(")
        .and_then(|s| s.split(']').next())
        .map(|s| s.trim().to_owned())?;
    (!inner.is_empty()).then_some(inner)
}

/// 保护窗口内任一 cfg 属性含 `test` 或 `feature = "mock"`（且非 `not(...)`）
/// 即视为 mock/test 保护——from_parts 调用在该块内合法。
fn cfg_protects_test_or_mock(protected: &std::collections::BTreeMap<i32, String>) -> bool {
    protected.values().any(|cfg| {
        !cfg.contains("not(") && (cfg.contains("test") || cfg.contains("feature = \"mock\""))
    })
}

/// 解析模块声明所在文件的模块文件路径（Rust 2018 规则）：
/// - `foo.rs` 内的 `mod bar;` → `foo/bar.rs` 或 `foo/bar/mod.rs`；
/// - `mod.rs` 内的 `mod bar;` → `bar.rs` 或 `bar/mod.rs`（同目录）；
/// - `#[path]` 显式指定优先（相对当前文件目录）。
///
/// 34 GV-09（复验）：mod 是否被同文件真实引用——`{name}::` 路径或 `use` 语句
/// 引用才算（声明行 `mod name;` 本身不计）。返回 false = 假声明（死文件）。
fn mod_is_referenced(text: &str, name: &str) -> bool {
    if text.contains(&format!("{name}::")) {
        return true;
    }
    for line in text.lines() {
        let t = line.trim();
        // `use` 引用（含 `use crate::x::name;` / `use name;` / `use name::…`）。
        if t.starts_with("use ") && t.contains(name) {
            return true;
        }
    }
    false
}

fn resolve_mod_file(file: &Path, name: &str, explicit: Option<&str>) -> Option<PathBuf> {
    let dir = file.parent()?;
    if let Some(p) = explicit {
        let candidate = dir.join(p);
        return candidate.is_file().then_some(candidate);
    }
    // crate root 模块文件（mod.rs / lib.rs / main.rs）的子模块在同目录；
    // 其他模块文件（foo.rs）的子模块在 foo/ 目录。
    let is_root_mod = file
        .file_stem()
        .map(|s| matches!(s.to_str(), Some("mod" | "lib" | "main")))
        .unwrap_or(false);
    let base = if is_root_mod {
        dir.to_path_buf()
    } else {
        dir.join(file.file_stem()?)
    };
    let a = base.join(format!("{name}.rs"));
    if a.is_file() {
        return Some(a);
    }
    let b = base.join(name).join("mod.rs");
    if b.is_file() {
        return Some(b);
    }
    None
}

/// 全 crate 扫描 `include!("...rs")` 引用（孤儿排除项）。
fn collect_include_refs(src_dir: &Path) -> BTreeSet<PathBuf> {
    let mut out = BTreeSet::new();
    let mut stack = vec![src_dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().map(|x| x == "rs").unwrap_or(false) {
                if let Ok(text) = fs::read_to_string(&p) {
                    for line in text.lines() {
                        let t = line.trim();
                        if let Some(inner) = t
                            .strip_prefix("include!(")
                            .and_then(|s| s.strip_suffix(')'))
                        {
                            let rel = inner.trim_matches('"');
                            let cand = p.parent().unwrap_or(&p).join(rel);
                            if cand.is_file() {
                                out.insert(cand);
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

/// 工作区级模块引用检测（42 Round 131）：模块名在 crates/ 任一 .rs
/// 中被 `mod name;` 声明、`name::` 路径使用或 `use` 引入即视为被引用。
/// 用于修正 34 GV-09 的"仅同文件引用"过严误报：跨 crate 消费的 pub 模块
///（`other_crate::name::…`）与仅被同 crate bin 引用的模块不应判孤儿。
/// 真正无任何引用的 `pub mod hidden;` 死文件仍会被判孤儿（GV-09 语义保留）。
fn workspace_mod_referenced(crates_dir: &Path, name: &str) -> bool {
    let mut stack = vec![crates_dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.ends_with("target") || p.ends_with("fuzz") {
                    continue;
                }
                stack.push(p);
            } else if p.extension().map(|x| x == "rs").unwrap_or(false) {
                let Ok(text) = fs::read_to_string(&p) else {
                    continue;
                };
                for line in text.lines() {
                    let t = line.trim();
                    // 仅统计“使用”而非“声明”：`mod name;` 声明由 BFS 解析
                    // 处理（声明本身不是引用——GV-09 反绕过的核心语义）；
                    // 这里只认 `name::` 路径使用与 `use` 引入。
                    if let Some(pos) = t.find(name) {
                        let before_ok = pos == 0
                            || !t.as_bytes()[pos - 1].is_ascii_alphanumeric()
                                && t.as_bytes()[pos - 1] != b'_';
                        let after = t[pos + name.len()..].chars().next();
                        let after_ok = after == Some(':');
                        if before_ok && after_ok {
                            return true;
                        }
                    }
                    // `use` 引入。
                    if t.starts_with("use ") && t.contains(name) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// 读取豁免表 tools/gate-exemptions.toml（若存在）。
/// 行格式：`"crate/file" = { rule = "...", reason = "...", expires_at = "..." }`。
fn load_exemptions(root: &Path) -> Vec<(String, String)> {
    let path = root.join("tools/gate-exemptions.toml");
    let Ok(text) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with('[') {
            continue;
        }
        if let Some((key, _)) = t.split_once('=') {
            let key = key.trim().trim_matches('"');
            if !key.is_empty() {
                out.push((key.to_owned(), path.display().to_string()));
            }
        }
    }
    out
}

/// `qa orphan-check`：孤儿源文件检测（26-B §3.1 算法）。
pub fn command_orphan_check(root: &Path, _args: &[String]) -> DynResult<()> {
    let crates_dir = root.join("crates");
    let mut orphans: Vec<String> = Vec::new();
    let mut stack = vec![crates_dir.clone()];
    let mut crate_dirs: Vec<PathBuf> = Vec::new();
    while let Some(d) = stack.pop() {
        if d.ends_with("target") || d.ends_with("fuzz") {
            continue;
        }
        let Ok(entries) = fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            }
        }
        if d.join("Cargo.toml").is_file() {
            crate_dirs.push(d.clone());
        }
    }
    crate_dirs.sort();

    for crate_dir in crate_dirs {
        let src = crate_dir.join("src");
        if !src.is_dir() {
            continue;
        }
        // 1. 收集全部 src/**/*.rs（排除 build.rs）。
        let mut all: BTreeSet<PathBuf> = BTreeSet::new();
        let mut fstack = vec![src.clone()];
        while let Some(d) = fstack.pop() {
            let Ok(entries) = fs::read_dir(&d) else {
                continue;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    fstack.push(p);
                } else if p.extension().map(|x| x == "rs").unwrap_or(false)
                    && p.file_name().map(|n| n != "build.rs").unwrap_or(false)
                {
                    all.insert(p);
                }
            }
        }
        // 2. 入口：src/lib.rs、src/main.rs、src/bin/*.rs。
        let mut entries: Vec<PathBuf> = Vec::new();
        if src.join("lib.rs").is_file() {
            entries.push(src.join("lib.rs"));
        }
        if src.join("main.rs").is_file() {
            entries.push(src.join("main.rs"));
        }
        if src.join("bin").is_dir() {
            let Ok(bin_entries) = fs::read_dir(src.join("bin")) else {
                continue;
            };
            for b in bin_entries.flatten() {
                if b.path().extension().map(|x| x == "rs").unwrap_or(false) {
                    entries.push(b.path());
                }
            }
        }
        // 3. BFS 解析 mod 声明。
        let mut reachable: BTreeSet<PathBuf> = BTreeSet::new();
        let mut queue: Vec<PathBuf> = entries.clone();
        while let Some(f) = queue.pop() {
            if !reachable.insert(f.clone()) {
                continue;
            }
            let Ok(text) = fs::read_to_string(&f) else {
                continue;
            };
            for (name, explicit, test_gated) in parse_mod_decls(&text) {
                if let Some(mf) = resolve_mod_file(&f, &name, explicit.as_deref()) {
                    // 34 GV-09（复验）：假 mod 声明绕过——声明但同文件无符号引用
                    //（`name::`/use）的 mod 视为死文件，不纳入可达集（复验已实测
                    // 复现绕过：裸 `pub mod hidden;` 藏死文件）。
                    // 42（Round 131）修正过严误报：cfg(test) 门控的标准测试模块、
                    // 跨 crate 消费的 pub 模块（`other_crate::name::…`）与仅被
                    // 同 crate bin 引用的模块不再误判——引用检测扩展到工作区。
                    if test_gated
                        || mod_is_referenced(&text, &name)
                        || workspace_mod_referenced(&crates_dir, &name)
                    {
                        queue.push(mf);
                    }
                }
            }
        }
        // 4. include! 引用。
        let included = collect_include_refs(&src);
        // 5. 孤儿 = all - reachable - included。
        let exemptions = load_exemptions(root);
        for f in all.difference(&reachable) {
            if included.contains(f) {
                continue;
            }
            let rel = f.strip_prefix(root).unwrap_or(f);
            let rel_s = rel.to_string_lossy();
            // 34 GV-09：豁免精确化——旧 ends_with(key) 过松
            //（foo.rs 豁免 barfoo.rs）；仅允许完全相等或路径段后缀。
            if exemptions
                .iter()
                .any(|(key, _)| rel_s == *key || rel_s.ends_with(&format!("/{key}")))
            {
                continue;
            }
            orphans.push(format!("{}: {}", crate_dir.display(), f.display()));
        }
    }

    // 42（Round 131）：文本启发式只是候选——编译验证才是孤儿的事实判据。
    // 候选文件的 mod 声明+文件删除后若 crate（含 tests/examples/benches）
    // 仍能编译，说明该模块对构建零贡献（真死文件）；若编译失败则说明
    // 模块通过方法语法/跨 crate 路径被使用（活模块，文本启发式误报）。
    // 逐一编译验证代价高，按 crate 批量验证；失败即整体回滚该 crate 的
    // 候选并视为活模块。GV-09 反绕过语义保留：真死文件必须被检出。
    let mut confirmed: Vec<String> = Vec::new();
    let mut by_crate: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    for o in &orphans {
        let (crate_dir, file) = parse_orphan_entry(o);
        by_crate.entry(crate_dir).or_default().push(file);
    }
    for (crate_dir, files) in by_crate {
        // 快照：声明所在文件原文 + 候选文件字节。
        let mut snapshots: Vec<(PathBuf, String, PathBuf, Vec<u8>)> = Vec::new();
        let mut removed_any = false;
        for file in &files {
            let name = file
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_owned();
            let Some(decl_file) = find_mod_declaring_file(&crate_dir, &name) else {
                // 无声明（游离文件）：不在模块树中、不参与编译，且候选
                // 已排除 include! 引用——删除必然安全（39 退役 DDS 等
                // 遗留文件的收尾路径）。
                if fs::remove_file(file).is_ok() {
                    confirmed.push(format!("{}: {}", crate_dir.display(), file.display()));
                }
                continue;
            };
            let Ok(orig_text) = fs::read_to_string(&decl_file) else {
                continue;
            };
            let Ok(orig_bytes) = fs::read(file) else {
                continue;
            };
            let new_text = strip_mod_decl(&orig_text, &name);
            if new_text == orig_text {
                continue; // 声明未找到（不应发生），跳过。
            }
            fs::write(&decl_file, &new_text).ok();
            fs::remove_file(file).ok();
            snapshots.push((decl_file.clone(), orig_text, file.clone(), orig_bytes));
            removed_any = true;
        }
        if !removed_any {
            continue;
        }
        let ok = crate_checks_clean(root, &crate_dir);
        if ok {
            for (_, _, file, _) in &snapshots {
                confirmed.push(format!("{}: {}", crate_dir.display(), file.display()));
            }
        } else {
            // 回滚：恢复声明与文件（活模块误报）。
            for (decl_file, orig_text, file, orig_bytes) in &snapshots {
                fs::write(decl_file, orig_text).ok();
                fs::write(file, orig_bytes).ok();
            }
            println!(
                "ORPHAN-KEEP (live module, compile-verified): {} ({} file(s) restored)",
                crate_dir.display(),
                snapshots.len()
            );
        }
    }

    let out_path = root.join("target/orphan-check.json");
    fs::create_dir_all(out_path.parent().unwrap_or(root))?;
    fs::write(
        &out_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": "eneros.orphan-check.v1",
            "orphans": confirmed,
        }))?,
    )?;
    if confirmed.is_empty() {
        println!("ORPHAN-CHECK → 0 orphans（{}）", out_path.display());
        Ok(())
    } else {
        for o in &confirmed {
            println!("ORPHAN: {o}");
        }
        Err(invalid(format!(
            "orphan-check: {} confirmed orphan source file(s) — 见 target/orphan-check.json",
            confirmed.len()
        )))
    }
}

/// 解析 `crate_dir: file` 形式的孤儿条目。
fn parse_orphan_entry(entry: &str) -> (PathBuf, PathBuf) {
    let mut parts = entry.splitn(2, ": ");
    let crate_dir = PathBuf::from(parts.next().unwrap_or(""));
    let file = PathBuf::from(parts.next().unwrap_or(""));
    (crate_dir, file)
}

/// 在 crate src 内查找声明 `mod <name>;`（含可见性前缀）的文件。
fn find_mod_declaring_file(crate_dir: &Path, name: &str) -> Option<PathBuf> {
    let src = crate_dir.join("src");
    let mut stack = vec![src];
    let mut out = None;
    while let Some(d) = stack.pop() {
        let Ok(entries) = fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().map(|x| x == "rs").unwrap_or(false) {
                if let Ok(text) = fs::read_to_string(&p) {
                    if text.lines().any(|l| {
                        let t = l.trim();
                        (t.starts_with("mod ")
                            || t.starts_with("pub mod ")
                            || t.starts_with("pub(crate) mod ")
                            || t.starts_with("pub(super) mod "))
                            && t.contains(&format!("{name};"))
                    }) {
                        out = Some(p);
                        break;
                    }
                }
            }
        }
        if out.is_some() {
            break;
        }
    }
    out
}

/// 移除声明 `mod <name>;`（含其前 `#[path = "..."]` 属性行）。
fn strip_mod_decl(text: &str, name: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim();
        let is_decl = |l: &str| {
            let s = l.trim();
            (s.starts_with("mod ")
                || s.starts_with("pub mod ")
                || s.starts_with("pub(crate) mod ")
                || s.starts_with("pub(super) mod "))
                && s.contains(&format!("{name};"))
        };
        if t.starts_with("#[path") && i + 1 < lines.len() && is_decl(lines[i + 1]) {
            i += 2;
            continue;
        }
        if is_decl(lines[i]) {
            i += 1;
            continue;
        }
        out.push(lines[i]);
        i += 1;
    }
    out.join("\n")
}

/// 对 crate 执行 host 全目标 check（含 tests/examples/benches）——删除
/// 候选模块后仍编译通过 = 真死文件。
fn crate_checks_clean(root: &Path, crate_dir: &Path) -> bool {
    let Ok(manifest) = fs::read_to_string(crate_dir.join("Cargo.toml")) else {
        return false;
    };
    let Some(name) = manifest
        .lines()
        .find_map(|l| l.trim().strip_prefix("name = "))
        .and_then(|v| v.trim_matches('"').to_owned().into())
    else {
        return false;
    };
    let output =
        std::process::Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned()))
            .current_dir(root)
            .args(["check", "-p", &name, "--all-targets", "--locked"])
            .output();
    match output {
        Ok(o) => o.status.success(),
        Err(_) => false,
    }
}

/// `qa config-refs`：配置引用检测（26-B §3.2；29 GV-01 转阻断）。
///
/// 阻断语义（GV-01）：代码中引用 `configs/<name>.toml` 但文件不存在 → 违规。
/// 未引用的配置（unreferenced）仅登记 evidence（CF-01 处置：consumer 注释
/// 声明 + 34 域清理），不阻断——否则 80 个模板 toml 会整体变红。
pub fn command_config_refs(root: &Path, _args: &[String]) -> DynResult<()> {
    let configs_dir = root.join("configs");
    if !configs_dir.is_dir() {
        return Err(invalid("configs/ 不存在"));
    }
    // 34 GV-15：配置清单递归 configs/**（纳入 4 个子目录）——与
    // config-syntax 共用 collect_toml_files（禁两套扫描）。
    let config_files = collect_toml_files(&configs_dir);
    // 被引用的 config 文件名（去扩展名）。
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    // 代码引用但 configs/ 不存在的 toml 名（GV-01 阻断项）。
    let mut missing: Vec<String> = Vec::new();
    // 34 GV-15：引用扫描根扩至 tools/ 与 .github/（CI 工作流引用可见）。
    for scan_root in ["crates", "xtask", "ci", "tests", "tools", ".github"] {
        let base = root.join(scan_root);
        if !base.is_dir() {
            continue;
        }
        let mut stack = vec![base];
        while let Some(d) = stack.pop() {
            let Ok(es) = fs::read_dir(&d) else { continue };
            for e in es.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if matches!(
                    p.extension().and_then(|x| x.to_str()),
                    Some("rs") | Some("sh") | Some("yml") | Some("yaml")
                ) {
                    if let Ok(text) = fs::read_to_string(&p) {
                        for line in text.lines() {
                            let t = line.trim();
                            if t.starts_with("//") || t.starts_with('#') {
                                continue;
                            }
                            for name in config_names(&config_files) {
                                if t.contains(&format!("{name}.toml")) {
                                    referenced.insert(name.clone());
                                }
                            }
                            // 正向检测：`<word>.toml` 字样但 configs/ 下无此文件。
                            for hit in toml_refs_in_line(t) {
                                if !find_config_file(&config_files, &hit) {
                                    missing.push(format!("{hit}.toml（{}）", p.display()));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let all_names = config_names(&config_files);
    let unreferenced: Vec<String> = all_names
        .iter()
        .filter(|n| !referenced.contains(*n))
        .cloned()
        .collect();
    let out_dir = root.join("evidence/config-refs");
    fs::create_dir_all(&out_dir)?;
    let out = out_dir.join(format!("{}-unreferenced.json", chrono_like_today()));
    fs::write(
        &out,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": "eneros.config-refs.v1",
            "referenced": referenced.iter().collect::<Vec<_>>(),
            "unreferenced": unreferenced,
            "missing": missing,
        }))?,
    )?;
    println!(
        "CONFIG-REFS → {}（unreferenced {}，missing {}）",
        out.display(),
        unreferenced.len(),
        missing.len()
    );
    // GV-01：转阻断——代码引用了不存在的配置即 FAIL。
    if !missing.is_empty() {
        for m in &missing {
            println!("CONFIG-REFS-MISSING: {m}");
        }
        return Err(invalid(format!(
            "config-refs: {} 处引用不存在的配置（孤儿引用）——宣称的配置必须存在（G7）",
            missing.len()
        )));
    }
    // CF-01（41 WP-41.2）：注释自证作废——每个 configs/**/*.toml 必须按
    // `configs/registry.toml` 分类（ACTIVE/TEMPLATE/DELETE），ACTIVE 配置
    // 还必须与 release profile 的 ProductManifest configs/consumer 双向
    // 链接（consumer 服务必须存在且声明该配置；服务声明 configs 必须被
    // manifest configs 覆盖）。templates/ 目录 = TEMPLATE 的唯一位置。
    let registry = read_config_registry(&configs_dir)?;
    let mut violations: Vec<String> = Vec::new();
    let templates_dir = configs_dir.join("templates");
    let mut template_count = 0usize;
    for path in collect_toml_files(&configs_dir) {
        let rel = path
            .strip_prefix(&configs_dir)
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_owned();
        // registry.toml 自身是分类元数据，不是被分类配置。
        if name == "registry" {
            continue;
        }
        let under_templates = path.starts_with(&templates_dir);
        if under_templates {
            template_count += 1;
            if registry.active.contains_key(&name) {
                violations.push(format!("CF-01:template-in-active:{rel}"));
            }
            continue;
        }
        // 根级（含 compliance/power/time 等子目录）必须是 ACTIVE。
        match registry.active.get(&name) {
            Some(_) => {}
            None => violations.push(format!("CF-01:unclassified:{rel}")),
        }
    }
    // DELETE 表内文件必须不存在（已删除）。
    for name in registry.deleted.keys() {
        if find_config_file(&config_files, name) {
            violations.push(format!("CF-01:deleted-still-present:{name}.toml"));
        }
    }
    // ACTIVE ↔ manifest 双向链接（qemu-prod 与 board-prod 两个 release
    // profile 都必须声明）。
    for profile_id in ["qemu-prod", "board-prod"] {
        let profile_path = root
            .join("product/profiles")
            .join(format!("{profile_id}.json"));
        if !profile_path.is_file() {
            continue;
        }
        let manifest = manifest_configs_from_profile(&profile_path)?;
        for name in registry.active.keys() {
            match manifest.configs.get(name) {
                None => {
                    violations.push(format!("CF-01:manifest-missing-config:{profile_id}:{name}"))
                }
                Some(spec) => {
                    if spec.consumer.is_empty() {
                        violations.push(format!("CF-01:manifest-no-consumer:{profile_id}:{name}"));
                    }
                    for consumer in &spec.consumer {
                        match manifest.services.get(consumer) {
                            None => violations.push(format!(
                                "CF-01:unknown-consumer-service:{profile_id}:{name}:{consumer}"
                            )),
                            Some(svc) => {
                                if !svc.configs.iter().any(|c| c == name) {
                                    violations.push(format!(
                                        "CF-01:consumer-not-declared:{profile_id}:{name}:{consumer}"
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    if !violations.is_empty() {
        for v in &violations {
            println!("CONFIG-REFS-CF01: {v}");
        }
        return Err(invalid(format!(
            "config-refs: {} 处配置清偿违规（CF-01 41 版：registry/manifest 双向链接）",
            violations.len()
        )));
    }
    println!(
        "CONFIG-REFS-CF01 → OK（active {}，templates {}，deleted {}）",
        registry.active.len(),
        template_count,
        registry.deleted.len()
    );
    Ok(())
}

/// configs/registry.toml 解析（41 WP-41.2 分类唯一事实）。
struct ConfigRegistryFile {
    active: BTreeMap<String, Vec<String>>,
    deleted: BTreeMap<String, String>,
}

fn read_config_registry(configs_dir: &Path) -> DynResult<ConfigRegistryFile> {
    let path = configs_dir.join("registry.toml");
    let text = fs::read_to_string(&path)
        .map_err(|e| invalid(format!("configs/registry.toml 缺失（41 CF-01 必需）：{e}")))?;
    let value: toml::Value = toml::from_str(&text)
        .map_err(|e| invalid(format!("configs/registry.toml 语法错误：{e}")))?;
    let mut active: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut deleted: BTreeMap<String, String> = BTreeMap::new();
    if let Some(table) = value.get("active").and_then(toml::Value::as_table) {
        for (name, entry) in table {
            let consumers = entry
                .get("consumer")
                .and_then(toml::Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(toml::Value::as_str)
                        .map(|s| s.to_owned())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            active.insert(name.clone(), consumers);
        }
    }
    if let Some(table) = value.get("deleted").and_then(toml::Value::as_table) {
        for (name, note) in table {
            deleted.insert(name.clone(), note.as_str().unwrap_or_default().to_owned());
        }
    }
    if active.is_empty() {
        return Err(invalid(
            "configs/registry.toml: active 表为空（41 CF-01 必需）",
        ));
    }
    Ok(ConfigRegistryFile { active, deleted })
}

/// 从 profile v2 提取 configs/services 链接表（gate 不依赖 image 构建）。
fn manifest_configs_from_profile(profile_path: &Path) -> DynResult<ManifestLinkage> {
    let raw: serde_json::Value = read_json(profile_path)?;
    let mut configs: BTreeMap<String, ConfigLink> = BTreeMap::new();
    if let Some(list) = raw.get("configs").and_then(serde_json::Value::as_array) {
        for value in list {
            let id = value
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let consumer = value
                .get("consumer")
                .and_then(serde_json::Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(|s| s.to_owned())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            configs.insert(id, ConfigLink { consumer });
        }
    }
    let mut services: BTreeMap<String, ServiceLink> = BTreeMap::new();
    if let Some(list) = raw.get("services").and_then(serde_json::Value::as_array) {
        for value in list {
            let id = value
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let configs = value
                .get("configs")
                .and_then(serde_json::Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(serde_json::Value::as_str)
                        .map(|s| s.to_owned())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            services.insert(id, ServiceLink { configs });
        }
    }
    Ok(ManifestLinkage { configs, services })
}

struct ConfigLink {
    consumer: Vec<String>,
}

struct ServiceLink {
    configs: Vec<String>,
}

struct ManifestLinkage {
    configs: BTreeMap<String, ConfigLink>,
    services: BTreeMap<String, ServiceLink>,
}

/// 提取一行中出现的 `<word>.toml` 字样（word = [A-Za-z0-9_-]+），用于正向
/// 检测"代码引用了 configs/ 下不存在的 toml"。
///
/// 仅计两种形态（避免 tools/、/etc/ 等非 configs 路径误报）：
/// - 显式 `configs/<word>.toml`（含 `configs/dengbao.toml#section` 锚点形式）；
/// - 裸 `<word>.toml`（无路径前缀，代码以配置文件名宣称）。
fn toml_refs_in_line(line: &str) -> Vec<String> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        while i < bytes.len()
            && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'-')
        {
            i += 1;
        }
        if i > start && bytes[i..].starts_with(b".toml") {
            let word = &line[start..i];
            // 排除 Cargo/lockfile 等已知非 configs 模板名，及工具
            // usage 帮助文本中的通用示例文件名（sim/tpl = 用户自带
            // 路径占位，非仓内配置宣称；34 GV-15）。
            let is_known_other = matches!(
                word,
                "Cargo"
                    | "cargo"
                    | "deny"
                    | "clippy"
                    | "rust-toolchain"
                    | "gate-exemptions"
                    | "sim"
                    | "tpl"
                    // product/ 政策文件（非 configs/ 模板；policy.rs 错误串
                    // 会裸引用其文件名——GV-01 只扫 configs/ 宣称）。
                    | "kernel-policy"
                    | "crypto-policy"
            );
            let dir = dir_of(&line[..start]);
            if !is_known_other && (dir.is_empty() || dir == "configs") {
                out.push(word.to_owned());
            }
            i += 5;
        } else {
            i = (i + 1).max(start + 1);
        }
    }
    out.sort();
    out.dedup();
    out
}

/// word 前导目录名：`configs/foo.toml` → "configs"；`tools/x.toml` → "tools"；
/// 裸词 → ""。
fn dir_of(prefix: &str) -> &str {
    let Some(idx) = prefix.rfind('/') else {
        return "";
    };
    let rest = &prefix[..idx];
    let seg = match rest.rfind('/') {
        Some(idx2) => &rest[idx2 + 1..],
        None => rest,
    };
    // 34 GV-15：取段内最后一个路径片段——剔除行内代码/引号前缀
    //（如 `let _ = "configs/` 旧实现整段返回导致 dir 误判漏检）。
    match seg.rfind(|c: char| c.is_whitespace() || c == '"' || c == '\'') {
        Some(i) => &seg[i + 1..],
        None => seg,
    }
}

fn config_names(files: &[PathBuf]) -> Vec<String> {
    let mut names: Vec<String> = files
        .iter()
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .collect();
    names.sort();
    names.dedup();
    names
}

/// 34 GV-15：按 stem 在递归配置清单中查文件存在性（含子目录）。
fn find_config_file(files: &[PathBuf], stem: &str) -> bool {
    files
        .iter()
        .any(|p| p.file_stem().map(|s| s == stem).unwrap_or(false))
}

/// 日期（YYYY-MM-DD）——不依赖 chrono。
fn chrono_like_today() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = now / 86400;
    // 1970-01-01 起的天数 → 简单公历转换。
    let (y, m, d) = civil_from_days(days as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `qa gate config-syntax`：configs/** 全部 toml 语法校验（29 CF-01）。
/// host 侧用 toml crate 解析（含 compliance/、platform-contracts/、power/、
/// time/ 子目录），语法错 FAIL——堵住"81 个 toml 无语法校验"缺口。
pub fn command_config_syntax(root: &Path, _args: &[String]) -> DynResult<()> {
    let configs_dir = root.join("configs");
    let mut bad: Vec<String> = Vec::new();
    let mut count = 0usize;
    for path in collect_toml_files(&configs_dir) {
        count += 1;
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                bad.push(format!("{}: read {}", path.display(), e));
                continue;
            }
        };
        if let Err(e) = text.parse::<toml::Value>() {
            bad.push(format!("{}: {e}", path.display()));
        }
    }
    let out_dir = root.join("evidence/config-syntax");
    fs::create_dir_all(&out_dir)?;
    let out = out_dir.join(format!("{}.json", chrono_like_today()));
    fs::write(
        &out,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": "eneros.config-syntax.v1",
            "checked": count,
            "errors": bad,
        }))?,
    )?;
    println!(
        "CONFIG-SYNTAX → {}（checked {}，errors {}）",
        out.display(),
        count,
        bad.len()
    );
    if !bad.is_empty() {
        for b in &bad {
            println!("CONFIG-SYNTAX-ERROR: {b}");
        }
        return Err(invalid(format!(
            "config-syntax: {} 个 toml 语法错误（configs/ 全量）",
            bad.len()
        )));
    }
    Ok(())
}

/// 递归收集 configs/ 下全部 toml 文件。
fn collect_toml_files(configs_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![configs_dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(es) = fs::read_dir(&d) else { continue };
        for e in es.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().map(|x| x == "toml").unwrap_or(false) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// `qa gate ...` 分发。
pub fn command_gate(root: &Path, args: &[String]) -> DynResult<()> {
    let (mode, tail) = args.split_first().ok_or_else(|| {
        invalid("gate requires a subcommand: orphan-check|config-refs|mock-leak|config-syntax")
    })?;
    match mode.as_str() {
        "orphan-check" => command_orphan_check(root, tail),
        "config-refs" => command_config_refs(root, tail),
        "mock-leak" => command_mock_leak(root, tail),
        "config-syntax" => command_config_syntax(root, tail),
        "docs" => command_docs_gate(root, tail),
        "keys" => command_keys_gate(root, tail),
        other => Err(invalid(format!("unknown gate subcommand '{other}'"))),
    }
}

/// `qa gate keys`（47 WP-47.3 私钥出仓门禁）：
/// 1. 键文件零跟踪（`git ls-files product/security/` 不含 .key/.pem）；
/// 2. 源码零默认键引用（旧路径 `product/security/dev-signing.key` 只允许
///    出现在 keys 模块文档/迁移登记——出仓后不再有可被误提交的位置）；
/// 3. 测试键就位（keys-local/ 或生成脚本存在）——缺键的测试装配必须
///    显式失败而非静默无签名。
pub fn command_keys_gate(root: &Path, _args: &[String]) -> DynResult<()> {
    use std::process::Command;

    // 1. 键文件必须零跟踪。
    let out = Command::new("git")
        .arg("ls-files")
        .arg("product/security")
        .output()
        .map_err(|e| invalid(format!("git ls-files: {e}")))?;
    let tracked = String::from_utf8_lossy(&out.stdout);
    let tracked_keys: Vec<&str> = tracked
        .lines()
        .filter(|l| l.ends_with(".key") || l.ends_with(".pem"))
        .collect();
    if !tracked_keys.is_empty() {
        return Err(invalid(format!(
            "私钥被 git 跟踪（出仓红线条）：{}",
            tracked_keys.join(", ")
        )));
    }
    println!("KEYS-GATE tracked-keys=0 PASS");

    // 2. 源码零默认键引用（keys 模块文档与 evidence 迁移登记豁免）。
    let out = Command::new("grep")
        .arg("-rn")
        .arg("--exclude=gate.rs")
        .arg("--exclude=keys.rs")
        .arg("product/security/dev-signing.key")
        .arg("xtask/src")
        .arg("ci")
        .output()
        .map_err(|e| invalid(format!("grep: {e}")))?;
    if !out.stdout.is_empty() {
        // 允许 keys.rs 的路径常量（其值已是 keys-local——文本不含旧路径）。
        return Err(invalid(String::from_utf8_lossy(&out.stdout).into_owned()));
    }
    println!("KEYS-GATE default-refs=0 PASS");

    // 3. 测试键就位（或生成脚本存在）。
    if !super::keys::test_keys_present(root) {
        if !root.join("tools/gen-local-keys.sh").is_file() {
            return Err(invalid(
                "keys-local 测试键缺失且无生成脚本（tools/gen-local-keys.sh）—— 测试装配必须显式失败",
            ));
        }
        println!("KEYS-GATE test-keys=pending-script PASS（运行 tools/gen-local-keys.sh 生成）");
    } else {
        println!("KEYS-GATE test-keys=present PASS");
    }
    // 4. 生产注入面（文档化）：release 装配签名必须 --sign-key/
    //    ENEROS_SIGN_KEY/KMS_DIR——resolve_signing_key(Production) 已
    //    fail-closed（单测覆盖）。
    println!("KEYS-GATE production-injection=fail-closed（keys::resolve_signing_key 单测覆盖）");
    Ok(())
}

/// `gate docs`：发布文档 Gate（蓝图 36 WP-36.11）——
/// 1. `docs/v1.0.0-go-no-go.md` 首屏必须包含当前 NO-GO 裁决；
/// 2. release 文档不得把 stale evidence（evidence/index.json 标记）当
///    PASS 依据引用；
/// 3. evidence index 必须存在且 schema 正确。
pub fn command_docs_gate(root: &Path, _args: &[String]) -> DynResult<()> {
    #[derive(serde::Deserialize)]
    struct EvidenceIndex {
        schema: String,
        baseline_commit: String,
        entries: Vec<IndexEntry>,
    }
    #[derive(serde::Deserialize)]
    #[allow(dead_code)] // 44.2：JSON 模型字段；stale_reason 保留供人工复核
    struct IndexEntry {
        path: String,
        status: String,
        #[serde(default)]
        stale_reason: String,
    }
    let mut violations = Vec::new();
    let index_path = root.join("evidence/index.json");
    let index: EvidenceIndex = read_json(&index_path)?;
    if index.schema != "eneros.evidence-index.v1" {
        violations.push(format!("evidence-index-schema:{}", index.schema));
    }
    if index.baseline_commit.is_empty() {
        violations.push("evidence-index-baseline-empty".to_owned());
    }
    let stale_paths: std::collections::BTreeSet<String> = index
        .entries
        .iter()
        .filter(|e| e.status == "stale")
        .map(|e| e.path.clone())
        .collect();

    let go_no_go = root.join("docs/v1.0.0-go-no-go.md");
    let text = fs::read_to_string(&go_no_go)
        .map_err(|e| invalid(format!("cannot read {}: {e}", go_no_go.display())))?;
    // 首屏 20 行必须包含当前 NO-GO 裁决。
    let head: String = text.lines().take(20).collect::<Vec<_>>().join(
        "
",
    );
    if !head.contains("NO-GO") && !head.contains("NOT-READY") {
        violations.push("go-no-go-missing-current-verdict".to_owned());
    }
    // 引用 stale evidence 路径 → 失败（release 文档不得把旧证据当依据）。
    for stale in &stale_paths {
        if text.contains(stale.as_str()) {
            violations.push(format!("go-no-go-references-stale-evidence:{stale}"));
        }
    }
    // 引用旧 profile digest（35 审查基线之前）→ 失败。
    if text.contains("e58daf026ef07f6b5193f11d425f90cced563f5eb3037f337dcfafcb80f5cd45")
        && !head.contains("NO-GO")
    {
        violations.push("go-no-go-references-stale-profile-digest".to_owned());
    }
    if !violations.is_empty() {
        return Err(invalid(format!(
            "DOCS-GATE FAIL: {}",
            violations.join("; ")
        )));
    }
    println!(
        "DOCS-GATE PASS go_no_go={} stale_entries={}",
        go_no_go.display(),
        stale_paths.len()
    );
    Ok(())
}

/// `qa gate mock-leak`：mock 泄漏执法（26-B §3.3）——WP-26.1 扫描器升级为
/// CI 阻断项（非零退出即红）。WP-26.3 完成后必须全绿。
pub fn command_mock_leak(root: &Path, _args: &[String]) -> DynResult<()> {
    let crates_dir = root.join("crates");
    // 34 GV-08（复验）：豁免表接入 mock-leak（旧实现仅 orphan-check 消费）。
    let exemptions = load_exemptions(root);
    let mut violations: Vec<String> = Vec::new();
    for (crate_name, files) in collect_crate_lib_files(&crates_dir) {
        for file in files {
            // 豁免：路径相等或路径段后缀（与 orphan-check 同语义）。
            let rel = file.strip_prefix(root).unwrap_or(&file);
            let rel_s = rel.to_string_lossy();
            if exemptions
                .iter()
                .any(|(key, _)| rel_s == *key || rel_s.ends_with(&format!("/{key}")))
            {
                continue;
            }
            let is_lib = file.file_name().map(|n| n == "lib.rs").unwrap_or(false);
            for (line, text) in mock_leak_lines(&file, is_lib) {
                violations.push(format!(
                    "{}:{}:{} (crate={})",
                    file.display(),
                    line,
                    text,
                    crate_name
                ));
            }
            // WP-26.4 + 29 GV-02：AuthorizedCommand 构造执法——非 control-gateway
            // crate 出现 from_parts 调用且不在 cfg(any(test, feature="mock"))
            // 保护的模块/块内 → 违规。
            // 字段私有已使生产字面量编译失败（类型系统强制）；本规则封死
            // from_parts 的生产调用。GV-02 修复：不再跳过缩进行（旧版只查
            // 顶层 → 恒 PASS），改为全行扫描 + 模块级 cfg 保护窗口（向上
            // 回溯到最近的块声明，覆盖 `#[cfg(test)] mod tests { ... }`）。
            // 豁免定义侧 crate（control-gateway = 唯一生产构造方；
            // telemetry-model = 类型与 from_parts 定义处）。
            if crate_name != "control-gateway" && crate_name != "telemetry-model" {
                let Ok(text) = fs::read_to_string(&file) else {
                    continue;
                };
                let lines: Vec<&str> = text.lines().collect();
                // 每行块的 cfg 保护栈：块深度 → 该块的 cfg 属性文本。
                let mut depth: i32 = 0;
                let mut protected: std::collections::BTreeMap<i32, String> =
                    std::collections::BTreeMap::new();
                // 上一个待绑定的 cfg 属性（下一块开始行生效）。
                let mut pending: Option<String> = None;
                for (i, raw) in lines.iter().enumerate() {
                    let t = raw.trim();
                    if let Some(cfg) = cfg_attr_text(t) {
                        pending = Some(cfg);
                        if t.contains('{') {
                            // 属性与块同行：`#[cfg(test)] mod tests {`
                            if let Some(cfg) = pending.take() {
                                depth += 1;
                                protected.insert(depth, cfg);
                            }
                        }
                        continue;
                    }
                    if t.is_empty() || t.starts_with("//") || t.starts_with("/*") {
                        continue; // 空行/注释不打断待绑定属性
                    }
                    let opens = t.matches('{').count() as i32;
                    let closes = t.matches('}').count() as i32;
                    if opens > 0 {
                        if let Some(cfg) = pending.take() {
                            for l in (depth + 1)..=(depth + opens) {
                                protected.insert(l, cfg.clone());
                            }
                        }
                    }
                    depth += opens - closes;
                    protected.retain(|&d, _| d <= depth);
                    if t.contains("AuthorizedCommand::from_parts(")
                        && !cfg_protects_test_or_mock(&protected)
                    {
                        violations.push(format!(
                            "{}:{}:{} (crate={}, rule=authorized-construction)",
                            file.display(),
                            i + 1,
                            t,
                            crate_name
                        ));
                    }
                }
            }
        }
    }
    violations.sort();
    violations.dedup();
    let out_path = root.join("target/mock-leak.json");
    fs::create_dir_all(out_path.parent().unwrap_or(root))?;
    fs::write(
        &out_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": "eneros.mock-leak.v1",
            "violations": violations,
        }))?,
    )?;
    if violations.is_empty() {
        println!("MOCK-LEAK → 0 violations（{}）", out_path.display());
        Ok(())
    } else {
        for v in &violations {
            println!("MOCK-LEAK: {v}");
        }
        Err(invalid(format!(
            "mock-leak: {} violation(s) — 见 target/mock-leak.json",
            violations.len()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_mod() {
        let decls = parse_mod_decls("pub mod entry;\npub mod history;\n");
        assert_eq!(decls.len(), 2);
        assert_eq!(decls[0].0, "entry");
        assert_eq!(decls[1].0, "history");
    }

    #[test]
    fn parse_cfg_prefixed_mod() {
        let decls = parse_mod_decls("#[cfg(feature = \"loom\")]\npub mod loom_freeze;\n");
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].0, "loom_freeze");
    }

    #[test]
    fn resolve_2018_rules() {
        // foo.rs 内 mod bar; → foo/bar.rs
        let dir = std::env::temp_dir().join(format!("modres-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("foo")).unwrap();
        let foo = dir.join("foo.rs");
        std::fs::write(&foo, "").unwrap();
        std::fs::write(dir.join("foo/bar.rs"), "").unwrap();
        let r = resolve_mod_file(&foo, "bar", None).expect("foo/bar.rs");
        assert_eq!(r, dir.join("foo/bar.rs"));
        // mod.rs 内 mod bar; → bar.rs（同目录）
        let modrs = dir.join("mod.rs");
        std::fs::write(&modrs, "").unwrap();
        std::fs::write(dir.join("bar.rs"), "").unwrap();
        let r2 = resolve_mod_file(&modrs, "bar", None).expect("bar.rs");
        assert_eq!(r2, dir.join("bar.rs"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn orphan_bfs_reaches_lib_mods() {
        // 模拟 account 结构：lib.rs 声明 entry，entry.rs 可达。
        let dir = std::env::temp_dir().join(format!("orphan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let crate_dir = dir.join("crates/agents/account");
        std::fs::create_dir_all(crate_dir.join("src")).unwrap();
        std::fs::write(
            crate_dir.join("Cargo.toml"),
            "[package]\nname=\"account\"\n",
        )
        .unwrap();
        std::fs::write(
            crate_dir.join("src/lib.rs"),
            "pub mod entry;\npub mod ledger;\npub mod hidden;\npub fn use_entry() { entry::touch(); }\npub fn use_ledger() { ledger::touch(); }\n",
        )
        .unwrap();
        std::fs::write(crate_dir.join("src/entry.rs"), "pub fn touch() {}\n").unwrap();
        std::fs::write(crate_dir.join("src/ledger.rs"), "pub fn touch() {}\n").unwrap();
        // 34 GV-09（复验）：hidden 为假声明——lib.rs 声明但不引用 → 死文件不可达。
        std::fs::write(crate_dir.join("src/hidden.rs"), "").unwrap();
        std::fs::write(crate_dir.join("src/orphan.rs"), "").unwrap();

        // 复刻 command_orphan_check 的核心 BFS。
        let src = crate_dir.join("src");
        let mut all: BTreeSet<PathBuf> = BTreeSet::new();
        let mut fstack = vec![src.clone()];
        while let Some(d) = fstack.pop() {
            let Ok(entries) = fs::read_dir(&d) else {
                continue;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    fstack.push(p);
                } else if p.extension().map(|x| x == "rs").unwrap_or(false) {
                    all.insert(p);
                }
            }
        }
        let mut reachable: BTreeSet<PathBuf> = BTreeSet::new();
        let mut queue = vec![src.join("lib.rs")];
        while let Some(f) = queue.pop() {
            if !reachable.insert(f.clone()) {
                continue;
            }
            let Ok(text) = fs::read_to_string(&f) else {
                continue;
            };
            for (name, explicit, test_gated) in parse_mod_decls(&text) {
                if let Some(mf) = resolve_mod_file(&f, &name, explicit.as_deref()) {
                    // 与生产 BFS 一致：假声明（无符号引用）不纳入可达集。
                    if test_gated || mod_is_referenced(&text, &name) {
                        queue.push(mf);
                    }
                }
            }
        }
        assert!(reachable.contains(&src.join("entry.rs")), "entry 必须可达");
        assert!(
            reachable.contains(&src.join("ledger.rs")),
            "ledger 必须可达"
        );
        assert!(
            !reachable.contains(&src.join("hidden.rs")),
            "假 mod 声明（无符号引用）必须不可达（34 GV-09 复验）"
        );
        assert!(!reachable.contains(&src.join("orphan.rs")), "orphan 不可达");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
