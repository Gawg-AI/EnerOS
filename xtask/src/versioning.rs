//! 版本 bump 工具（蓝图/44-00 §3 / WP-44.1）。
//!
//! `cargo xtask bump-version patch|minor|major` 或 `--set <x.y.z>`：
//! - 改根 manifest `[workspace.package].version`（`version.workspace = true`
//!   的成员随继承自动生效）；
//! - 同时把成员 manifest 里**字面** `[package].version` 归一到同一目标版本
//!   （清偿历史 `0.<卷>.<patch>` 遗留字段）；
//! - 不触碰依赖版本，不写 Cargo.lock（下次 cargo 构建自动同步）；
//! - `--from <ver>`：根字段当前值不符即拒（脚本化幂等防护）；成员字面量
//!   不做 from 校验，一律归一；
//! - `--dry-run`：只打印计划不写文件；
//! - `--manifest-path <p>`：默认 `<repo>/Cargo.toml`（供测试与外部工程用）。

use std::path::{Path, PathBuf};

use super::{invalid, DynResult};

pub fn command_bump_version(root: &Path, args: &[String]) -> DynResult<()> {
    let mut mode: Option<String> = None;
    let mut set: Option<String> = None;
    let mut from: Option<String> = None;
    let mut dry_run = false;
    let mut manifest_path: Option<PathBuf> = None;

    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "patch" | "minor" | "major" => {
                if mode.is_some() || set.is_some() {
                    return Err(invalid("目标重复：patch|minor|major 与 --set 只能取一"));
                }
                mode = Some(a.to_string());
            }
            "--set" => {
                if mode.is_some() || set.is_some() {
                    return Err(invalid("目标重复：patch|minor|major 与 --set 只能取一"));
                }
                set = Some(
                    it.next()
                        .ok_or_else(|| invalid("--set 需要版本参数"))?
                        .clone(),
                );
            }
            "--from" => {
                from = Some(
                    it.next()
                        .ok_or_else(|| invalid("--from 需要版本参数"))?
                        .clone(),
                )
            }
            "--dry-run" => dry_run = true,
            "--manifest-path" => {
                manifest_path = Some(PathBuf::from(
                    it.next()
                        .ok_or_else(|| invalid("--manifest-path 需要路径参数"))?,
                ))
            }
            other => return Err(invalid(format!("bump-version 未知参数 '{other}'"))),
        }
    }

    let manifest = manifest_path.unwrap_or_else(|| root.join("Cargo.toml"));
    let current = read_workspace_version(&manifest)?;
    if let Some(expected) = &from {
        if &current != expected {
            return Err(invalid(format!(
                "--from 不匹配：根字段当前为 '{current}'，期望 '{expected}'（拒绝执行）"
            )));
        }
    }
    let new_version = match (&mode, &set) {
        (Some(m), None) => {
            if current.contains('-') {
                return Err(invalid(format!(
                    "当前版本 '{current}' 带预发布标记，禁止隐式 bump（44-00 §3）；请用 --set 显式指定"
                )));
            }
            bump(&current, m)?
        }
        (None, Some(v)) => {
            validate_xyz(v)?;
            v.clone()
        }
        _ => {
            return Err(invalid(
                "需要且只需要一个目标：patch|minor|major 或 --set <x.y.z>",
            ))
        }
    };

    let members = parse_members(&manifest)?;
    let mut plan: Vec<(PathBuf, String)> = vec![(manifest.clone(), new_version.clone())];
    let mut inherited = 0usize;
    for member_manifest in members {
        match member_version_kind(&member_manifest)? {
            MemberVersionKind::Literal => {
                plan.push((member_manifest, new_version.clone()));
            }
            MemberVersionKind::Inherited => inherited += 1,
            MemberVersionKind::None => {}
        }
    }

    println!(
        "BUMP-PLAN {current} -> {new_version} dry_run={dry_run} from_check={}",
        from.is_some()
    );
    for (path, new) in &plan {
        println!("  {}: -> {new}", path.display());
    }
    println!(
        "BUMP-SUMMARY workspace_field=1 literal_fields={} inherited_members={inherited}",
        plan.len() - 1
    );
    if dry_run {
        return Ok(());
    }

    for (path, new) in &plan {
        let section = if path == &manifest {
            "[workspace.package]"
        } else {
            "[package]"
        };
        let wrote = edit_version_field(path, section, new)?;
        if wrote != 1 {
            return Err(invalid(format!(
                "{} 的 {section} version 字段改写数异常：{wrote}",
                path.display()
            )));
        }
    }
    println!("BUMP-DONE root={current} new={new_version}");
    Ok(())
}

fn validate_xyz(v: &str) -> DynResult<()> {
    let parts: Vec<&str> = v.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|p| p.parse::<u64>().is_err()) {
        return Err(invalid(format!("目标版本 '{v}' 非 x.y.z 数字格式")));
    }
    Ok(())
}

fn bump(cur: &str, mode: &str) -> DynResult<String> {
    let core = cur.split('-').next().unwrap_or(cur);
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() != 3 {
        return Err(invalid(format!("当前版本 '{cur}' 非 x.y.z 格式")));
    }
    let mut nums: Vec<u64> = Vec::with_capacity(3);
    for p in &parts {
        nums.push(
            p.parse()
                .map_err(|_| invalid(format!("当前版本 '{cur}' 含非数字段 '{p}'")))?,
        );
    }
    match mode {
        "patch" => nums[2] += 1,
        "minor" => {
            nums[1] += 1;
            nums[2] = 0;
        }
        "major" => {
            nums[0] += 1;
            nums[1] = 0;
            nums[2] = 0;
        }
        other => return Err(invalid(format!("未知 bump 模式 '{other}'"))),
    }
    Ok(format!("{}.{}.{}", nums[0], nums[1], nums[2]))
}

fn read_workspace_version(manifest: &Path) -> DynResult<String> {
    let text = std::fs::read_to_string(manifest)
        .map_err(|e| invalid(format!("cannot read {}: {e}", manifest.display())))?;
    let mut in_workspace_package = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_workspace_package = t == "[workspace.package]";
            continue;
        }
        if in_workspace_package {
            if let Some((key, value)) = t.split_once('=') {
                if key.trim() == "version" {
                    let v = value.trim().trim_matches('"');
                    if !v.is_empty() {
                        return Ok(v.to_string());
                    }
                }
            }
        }
    }
    Err(invalid(format!(
        "{} 未找到 [workspace.package] version 字段",
        manifest.display()
    )))
}

fn parse_members(manifest: &Path) -> DynResult<Vec<PathBuf>> {
    let text = std::fs::read_to_string(manifest)
        .map_err(|e| invalid(format!("cannot read {}: {e}", manifest.display())))?;
    let mut names: Vec<String> = Vec::new();
    let mut in_workspace = false;
    let mut collecting = false;
    for line in text.lines() {
        let t = line.trim();
        if collecting {
            collect_member_line(t, &mut names);
            if t.contains(']') {
                collecting = false;
            }
            continue;
        }
        if t.starts_with('[') {
            in_workspace = t == "[workspace]";
            continue;
        }
        if in_workspace {
            if let Some(rest) = t.strip_prefix("members") {
                let rest = rest.trim_start();
                if let Some(array) = rest.strip_prefix('=') {
                    collecting = true;
                    collect_member_line(array, &mut names);
                    if array.contains(']') {
                        collecting = false;
                    }
                }
            }
        }
    }
    let base = manifest.parent().unwrap_or_else(|| Path::new("."));
    Ok(names
        .into_iter()
        .map(|n| base.join(n).join("Cargo.toml"))
        .collect())
}

fn collect_member_line(s: &str, out: &mut Vec<String>) {
    let s = s.trim_start();
    let s = s.strip_prefix('[').unwrap_or(s);
    let s = s.split(']').next().unwrap_or(s);
    for tok in s.split(',') {
        let tok = tok.trim();
        if let Some(stripped) = tok.strip_prefix('"').and_then(|x| x.strip_suffix('"')) {
            if !stripped.is_empty() {
                out.push(stripped.to_string());
            }
        }
    }
}

enum MemberVersionKind {
    Literal,
    Inherited,
    None,
}

fn member_version_kind(member_manifest: &Path) -> DynResult<MemberVersionKind> {
    let text = std::fs::read_to_string(member_manifest)
        .map_err(|e| invalid(format!("cannot read {}: {e}", member_manifest.display())))?;
    let mut section = "";
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            section = t;
            continue;
        }
        if section == "[package]" {
            if t.starts_with("version.workspace") {
                return Ok(MemberVersionKind::Inherited);
            }
            if let Some((key, _)) = t.split_once('=') {
                if key.trim() == "version" {
                    return Ok(MemberVersionKind::Literal);
                }
            }
        }
    }
    Ok(MemberVersionKind::None)
}

fn edit_version_field(path: &Path, section_name: &str, new_version: &str) -> DynResult<usize> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| invalid(format!("cannot read {}: {e}", path.display())))?;
    let mut out = String::with_capacity(text.len() + 32);
    let mut changed = 0usize;
    let mut section = "";
    for line in text.split_inclusive('\n') {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            section = t;
            out.push_str(line);
            continue;
        }
        if section == section_name {
            if let Some((key, _value)) = t.split_once('=') {
                if key.trim() == "version" && t.contains('"') {
                    let indent = &line[..line.len() - line.trim_start().len()];
                    out.push_str(&format!("{indent}{} = \"{new_version}\"\n", key.trim()));
                    changed += 1;
                    continue;
                }
            }
        }
        out.push_str(line);
    }
    if changed > 0 {
        std::fs::write(path, out)
            .map_err(|e| invalid(format!("cannot write {}: {e}", path.display())))?;
    }
    Ok(changed)
}
