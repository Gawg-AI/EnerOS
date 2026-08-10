//! 26-B WP-26.2 新门禁：孤儿源文件 / 配置引用 / mock 执法。

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use super::qa::{collect_crate_lib_files, mock_leak_lines};
use super::{invalid, DynResult};

/// 解析一个文件的 `mod name;` / `pub mod name;` 声明（含 `#[path = "..."]`）。
/// 返回 (模块名, 显式 path 或 None)。
fn parse_mod_decls(text: &str) -> Vec<(String, Option<String>)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
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
                    out.push((name.to_owned(), path_attr.clone()));
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
                out.push((name.to_owned(), path_attr.clone()));
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
        !cfg.contains("not(")
            && (cfg.contains("test") || cfg.contains("feature = \"mock\""))
    })
}

/// 解析模块声明所在文件的模块文件路径（Rust 2018 规则）：
/// - `foo.rs` 内的 `mod bar;` → `foo/bar.rs` 或 `foo/bar/mod.rs`；
/// - `mod.rs` 内的 `mod bar;` → `bar.rs` 或 `bar/mod.rs`（同目录）；
/// - `#[path]` 显式指定优先（相对当前文件目录）。
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
            for (name, explicit) in parse_mod_decls(&text) {
                if let Some(mf) = resolve_mod_file(&f, &name, explicit.as_deref()) {
                    queue.push(mf);
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
            if exemptions
                .iter()
                .any(|(key, _)| rel.to_string_lossy().ends_with(key))
            {
                continue;
            }
            orphans.push(format!("{}: {}", crate_dir.display(), f.display()));
        }
    }

    let out_path = root.join("target/orphan-check.json");
    fs::create_dir_all(out_path.parent().unwrap_or(root))?;
    fs::write(
        &out_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": "eneros.orphan-check.v1",
            "orphans": orphans,
        }))?,
    )?;
    if orphans.is_empty() {
        println!("ORPHAN-CHECK → 0 orphans（{}）", out_path.display());
        Ok(())
    } else {
        for o in &orphans {
            println!("ORPHAN: {o}");
        }
        Err(invalid(format!(
            "orphan-check: {} orphan source file(s) — 见 target/orphan-check.json",
            orphans.len()
        )))
    }
}

/// `qa config-refs`：配置引用检测（26-B §3.2；29 GV-01 转阻断）。
///
/// 阻断语义（GV-01）：代码中引用 `configs/<name>.toml` 但文件不存在 → 违规。
/// 未引用的配置（unreferenced）仅登记 evidence（CF-01 处置：consumer 注释
/// 声明 + 34 域清理），不阻断——否则 80 个模板 toml 会整体变红。
pub fn command_config_refs(root: &Path, _args: &[String]) -> DynResult<()> {
    let configs_dir = root.join("configs");
    let Ok(entries) = fs::read_dir(&configs_dir) else {
        return Err(invalid("configs/ 不存在"));
    };
    let entry_list: Vec<fs::DirEntry> = entries.flatten().collect();
    // 被引用的 config 文件名（去扩展名）。
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    // 代码引用但 configs/ 不存在的 toml 名（GV-01 阻断项）。
    let mut missing: Vec<String> = Vec::new();
    for scan_root in ["crates", "xtask", "ci", "tests"] {
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
                    Some("rs") | Some("sh")
                ) {
                    if let Ok(text) = fs::read_to_string(&p) {
                        for line in text.lines() {
                            let t = line.trim();
                            if t.starts_with("//") || t.starts_with('#') {
                                continue;
                            }
                            for name in config_names(&entry_list) {
                                if t.contains(&format!("{name}.toml")) {
                                    referenced.insert(name.clone());
                                }
                            }
                            // 正向检测：`<word>.toml` 字样但 configs/ 下无此文件。
                            for hit in toml_refs_in_line(t) {
                                if !configs_dir.join(format!("{hit}.toml")).is_file() {
                                    missing.push(format!("{hit}.toml（{}）", p.display()));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let all_names = config_names(&entry_list);
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
    // CF-01（29 WP-29.2）：configs/** 每个 toml 头部必须声明 consumer
    // （`# consumer: <crate::path | 集成层占位（产线注入）>`）——配置存在
    // 但无消费者必须显式声明（S6）。头部 12 行内查找。
    let mut no_consumer: Vec<String> = Vec::new();
    for path in collect_toml_files(&configs_dir) {
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let header: String = text.lines().take(12).collect::<Vec<_>>().join("\n");
        if !header.contains("# consumer:") {
            no_consumer.push(path.display().to_string());
        }
    }
    if !no_consumer.is_empty() {
        for n in &no_consumer {
            println!("CONFIG-REFS-NO-CONSUMER: {n}");
        }
        return Err(invalid(format!(
            "config-refs: {} 个 toml 无 consumer 声明（CF-01 纪律：配置存在但无消费者必须显式声明）",
            no_consumer.len()
        )));
    }
    Ok(())
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
            // 排除 Cargo/lockfile 等已知非 configs 模板名。
            let is_known_other = matches!(
                word,
                "Cargo" | "cargo" | "deny" | "clippy" | "rust-toolchain" | "gate-exemptions"
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
    match rest.rfind('/') {
        Some(idx2) => &rest[idx2 + 1..],
        None => rest,
    }
}

fn config_names(entries: &[fs::DirEntry]) -> Vec<String> {
    let mut names: Vec<String> = entries
        .iter()
        .filter_map(|e| {
            let p = e.path();
            if p.extension().map(|x| x == "toml").unwrap_or(false) {
                p.file_stem().map(|s| s.to_string_lossy().into_owned())
            } else {
                None
            }
        })
        .collect();
    names.sort();
    names
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
    println!("CONFIG-SYNTAX → {}（checked {}，errors {}）", out.display(), count, bad.len());
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
    let (mode, tail) = args
        .split_first()
        .ok_or_else(|| invalid("gate requires a subcommand: orphan-check|config-refs|mock-leak|config-syntax"))?;
    match mode.as_str() {
        "orphan-check" => command_orphan_check(root, tail),
        "config-refs" => command_config_refs(root, tail),
        "mock-leak" => command_mock_leak(root, tail),
        "config-syntax" => command_config_syntax(root, tail),
        other => Err(invalid(format!("unknown gate subcommand '{other}'"))),
    }
}

/// `qa gate mock-leak`：mock 泄漏执法（26-B §3.3）——WP-26.1 扫描器升级为
/// CI 阻断项（非零退出即红）。WP-26.3 完成后必须全绿。
pub fn command_mock_leak(root: &Path, _args: &[String]) -> DynResult<()> {
    let crates_dir = root.join("crates");
    let mut violations: Vec<String> = Vec::new();
    for (crate_name, files) in collect_crate_lib_files(&crates_dir) {
        for file in files {
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
            "pub mod entry;\npub mod ledger;\n",
        )
        .unwrap();
        std::fs::write(crate_dir.join("src/entry.rs"), "").unwrap();
        std::fs::write(crate_dir.join("src/ledger.rs"), "").unwrap();
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
            for (name, explicit) in parse_mod_decls(&text) {
                if let Some(mf) = resolve_mod_file(&f, &name, explicit.as_deref()) {
                    queue.push(mf);
                }
            }
        }
        assert!(reachable.contains(&src.join("entry.rs")), "entry 必须可达");
        assert!(
            reachable.contains(&src.join("ledger.rs")),
            "ledger 必须可达"
        );
        assert!(!reachable.contains(&src.join("orphan.rs")), "orphan 不可达");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
