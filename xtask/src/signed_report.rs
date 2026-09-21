//! Deepening-v1 §B.2.2：证据报告「序列化 → sha256 → 可选 ed25519 签名 → 落盘
//! .json（+ .sig）」链的单点实现。
//!
//! 迁移前该链在 `qa.rs` 里逐字复演 3 次（compliance-report / perf-baseline /
//! perf-measure），ed25519 私钥装载复演 4 次，侧车路径推导复演 6 次。
//!
//! **R-B3（签名字节级差异）纪律**：本模块只做搬移，不改任何字节序——
//! - sha256 的预镜像 = **摘要回填前**的 `serde_json::to_vec_pretty` 字节；
//! - ed25519 的预镜像 = **摘要已回填、签名仍空**的那次序列化字节；
//! - 内嵌形态（compliance）落盘的是**第三次**序列化（含签名字段，无侧车）；
//! - 侧车形态（perf）落盘的是**第二次**序列化（无签名字段），侧车为 128 hex
//!   且**无尾随换行**；`sign-evidence` 的侧车则带尾随换行——两者词形不同，
//!   逐点沿用，见 `sidecar_path` / `json_sidecar_path`。
//!
//! 逐字节证明：`qa.rs` 的 `arg_contract::b3_*` 锁定测试 + 本模块 tests。

use std::fs;
use std::path::{Path, PathBuf};

use ed25519_dalek::{Signer, SigningKey};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{invalid, DynResult};

/// 无状态命名空间：报告签名链的入口（xtask 是 host 工具，可用 std）。
pub(crate) struct SignedReport;

/// 一次封链的产出（供调用方打印，不含任何副作用）。
pub(crate) struct Sealed {
    /// 回填进 JSON 的 sha256（小写 hex）。
    pub(crate) digest_hex: String,
    /// 签名 hex（小写）；未签名时为空串。
    pub(crate) signature_hex: String,
}

impl SignedReport {
    /// 读 ed25519 私钥（raw 32 字节 seed）。
    ///
    /// 文案与语义逐字保留迁移前 4 处调用点：`read sign key: {e}` /
    /// `"key must be 32 bytes"`（后者在现实现下不可达——`&bytes[..32]` 恒为
    /// 32 长度，短文件按现语义切片越界 panic，>32 的文件截断取前 32 字节）。
    /// 收拢不顺手改（蓝图 G3），由 `qa::arg_contract` 锁定。
    pub(crate) fn load_key(key_path: &Path) -> DynResult<SigningKey> {
        let key_bytes = fs::read(key_path).map_err(|e| invalid(format!("read sign key: {e}")))?;
        Ok(SigningKey::from_bytes(
            &key_bytes[..32]
                .try_into()
                .map_err(|_| invalid("key must be 32 bytes"))?,
        ))
    }

    /// 文件/报告字节的 ed25519 签名（小写 hex，64 字节 → 128 字符）。
    pub(crate) fn sign_hex(key: &SigningKey, bytes: &[u8]) -> String {
        Self::hex(&key.sign(bytes).to_bytes())
    }

    /// 侧车路径 = 对整串路径追加 `.sig`（sign-evidence 写入、
    /// verify-evidence-sig 与 compliance 门禁读取）。
    pub(crate) fn sidecar_path(path: &Path) -> PathBuf {
        PathBuf::from(format!("{}.sig", path.display()))
    }

    /// 侧车路径 = `with_extension("json.sig")`（perf-baseline / perf-measure
    /// 写入、perf-gate 读取）。
    ///
    /// 与 [`Self::sidecar_path`] **不等价**：本者替换扩展名（`x.bak` →
    /// `x.json.sig`），追加者保留（`x.bak` → `x.bak.sig`）。两者各自沿用原点
    /// 词形，不得合并。
    pub(crate) fn json_sidecar_path(path: &Path) -> PathBuf {
        path.with_extension("json.sig")
    }

    /// 形态 A（compliance-report）：sha256 与 signature 均回填 JSON 字段，
    /// 落盘第三次序列化（含签名），无侧车；key 必需（34 GV-07 强制签名）。
    pub(crate) fn seal_inline<T: Serialize>(
        root: &Path,
        out_path: &Path,
        report: &mut T,
        key: &SigningKey,
        set_digest: impl FnOnce(&mut T, &str),
        set_signature: impl FnOnce(&mut T, &str),
    ) -> DynResult<Sealed> {
        let (digest_hex, signed_bytes) = Self::seal_digest(report, set_digest)?;
        let signature_hex = Self::sign_hex(key, &signed_bytes);
        set_signature(report, &signature_hex);
        fs::create_dir_all(out_path.parent().unwrap_or(root))?;
        fs::write(out_path, serde_json::to_vec_pretty(report)?)?;
        Ok(Sealed {
            digest_hex,
            signature_hex,
        })
    }

    /// 形态 B（perf-baseline / perf-measure）：JSON 正文只嵌 sha256，签名写
    /// `json_sidecar_path` 侧车（无尾随换行）。key 缺省时打印
    /// `unsigned_warning` 并写**空**侧车（perf-gate 据此判 signature-missing）。
    pub(crate) fn seal_sidecar<T: Serialize>(
        root: &Path,
        out_path: &Path,
        report: &mut T,
        sign_key_path: Option<&Path>,
        unsigned_warning: &str,
        set_digest: impl FnOnce(&mut T, &str),
    ) -> DynResult<Sealed> {
        let (digest_hex, signed_bytes) = Self::seal_digest(report, set_digest)?;
        let signature_hex = match sign_key_path {
            Some(key_path) => Self::sign_hex(&Self::load_key(key_path)?, &signed_bytes),
            None => {
                println!("{unsigned_warning}");
                String::new()
            }
        };
        fs::create_dir_all(out_path.parent().unwrap_or(root))?;
        fs::write(out_path, &signed_bytes)?;
        fs::write(Self::json_sidecar_path(out_path), &signature_hex)?;
        Ok(Sealed {
            digest_hex,
            signature_hex,
        })
    }

    /// 链前两步（两种形态共用）：`to_vec_pretty` → sha256 → 回填摘要 →
    /// 再 `to_vec_pretty`；返回 (摘要 hex, 签名预镜像字节)。
    fn seal_digest<T: Serialize>(
        report: &mut T,
        set_digest: impl FnOnce(&mut T, &str),
    ) -> DynResult<(String, Vec<u8>)> {
        let pre_image = serde_json::to_vec_pretty(report)?;
        let digest_hex = Self::hex(&Sha256::digest(&pre_image));
        set_digest(report, &digest_hex);
        Ok((digest_hex, serde_json::to_vec_pretty(report)?))
    }

    /// 小写 hex（xtask 各模块各自持有同形 helper 的既有惯例）。
    fn hex(bytes: &[u8]) -> String {
        use std::fmt::Write;
        let mut s = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            let _ = write!(s, "{b:02x}");
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 与 compliance-report 同形（摘要 + 签名都进正文）。
    #[derive(Serialize, Clone, Debug, PartialEq)]
    struct Inline {
        schema: &'static str,
        sha256: String,
        signature: String,
        verdict: &'static str,
    }
    /// 与 perf-baseline/perf-measure 同形（正文只有摘要，签名走侧车）。
    #[derive(Serialize, Clone, Debug, PartialEq)]
    struct Sidecar {
        schema: &'static str,
        baseline_sha256: String,
        metrics: Vec<u64>,
    }

    const KEY_SEED: [u8; 32] = [7u8; 32];

    fn sandbox(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "eneros-signed-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 迁移前 qa.rs:1081-1096 的三步链——本模块必须逐字节复现。
    fn legacy_inline_chain(report: &mut Inline, key: &SigningKey) -> (Vec<u8>, String) {
        let json = serde_json::to_vec_pretty(report).unwrap();
        let digest_hex = SignedReport::hex(&Sha256::digest(&json));
        report.sha256 = digest_hex.clone();
        let json_signed = serde_json::to_vec_pretty(report).unwrap();
        report.signature = SignedReport::sign_hex(key, &json_signed);
        (serde_json::to_vec_pretty(report).unwrap(), digest_hex)
    }

    /// 迁移前 qa.rs:2171-2229（perf-baseline）/ 2341-2369（perf-measure）的链。
    fn legacy_sidecar_chain(
        report: &mut Sidecar,
        key: Option<&SigningKey>,
        field: fn(&mut Sidecar) -> &mut String,
    ) -> (Vec<u8>, String) {
        let json = serde_json::to_vec_pretty(report).unwrap();
        let digest_hex = SignedReport::hex(&Sha256::digest(&json));
        *field(report) = digest_hex.clone();
        let json_signed = serde_json::to_vec_pretty(report).unwrap();
        let signature = match key {
            Some(k) => SignedReport::sign_hex(k, &json_signed),
            None => String::new(),
        };
        (json_signed, signature)
    }

    #[test]
    fn seal_inline_matches_legacy_chain_byte_for_byte() {
        let root = sandbox("inline");
        let key = SigningKey::from_bytes(&KEY_SEED);
        let base = Inline {
            schema: "eneros.compliance-report.v1",
            sha256: String::new(),
            signature: String::new(),
            verdict: "assessment",
        };
        let out = root.join("nested/report.json");
        let mut want = base.clone();
        let (want_bytes, want_digest) = legacy_inline_chain(&mut want, &key);
        let mut got = base.clone();
        let sealed = SignedReport::seal_inline(
            &root,
            &out,
            &mut got,
            &key,
            |r, d| r.sha256 = d.to_owned(),
            |r, s| r.signature = s.to_owned(),
        )
        .unwrap();
        assert_eq!(sealed.digest_hex, want_digest);
        assert_eq!(fs::read(&out).unwrap(), want_bytes, "内嵌形态落盘字节漂移");
        assert_eq!(got, want, "内嵌形态回填结果漂移");
        // 内嵌形态不产侧车。
        assert!(!SignedReport::sidecar_path(&out).exists());
        assert!(!SignedReport::json_sidecar_path(&out).exists());
    }

    #[test]
    fn seal_sidecar_matches_legacy_chain_byte_for_byte() {
        let root = sandbox("sidecar");
        let key_path = root.join("k.bin");
        fs::write(&key_path, KEY_SEED).unwrap();
        let base = Sidecar {
            schema: "eneros.perf-regression.v1",
            baseline_sha256: String::new(),
            metrics: vec![1, 2, 3],
        };
        let out = root.join("pb.json");
        let mut want = base.clone();
        let (want_bytes, want_sig) =
            legacy_sidecar_chain(&mut want, Some(&SigningKey::from_bytes(&KEY_SEED)), |r| {
                &mut r.baseline_sha256
            });
        let mut got = base.clone();
        let sealed = SignedReport::seal_sidecar(
            &root,
            &out,
            &mut got,
            Some(&key_path),
            "WARN: test unsigned",
            |r, d| r.baseline_sha256 = d.to_owned(),
        )
        .unwrap();
        assert_eq!(
            fs::read(&out).unwrap(),
            want_bytes,
            "侧车形态 .json 字节漂移"
        );
        assert_eq!(got, want);
        assert_eq!(sealed.signature_hex, want_sig);
        // 侧车 = with_extension("json.sig")，128 hex、无尾随换行。
        let side = SignedReport::json_sidecar_path(&out);
        assert_eq!(side, root.join("pb.json.sig"));
        let on_disk = fs::read_to_string(&side).unwrap();
        assert_eq!(on_disk, want_sig);
        assert_eq!(on_disk.len(), 128);
        assert!(!on_disk.ends_with('\n'));
    }

    /// 未签名路径：侧车仍写（空串），调用方打印 WARN。
    #[test]
    fn seal_sidecar_without_key_writes_empty_sidecar() {
        let root = sandbox("unsigned");
        let out = root.join("u.json");
        let mut got = Sidecar {
            schema: "eneros.perf-regression.v1",
            baseline_sha256: String::new(),
            metrics: vec![],
        };
        let sealed = SignedReport::seal_sidecar(
            &root,
            &out,
            &mut got,
            None,
            "WARN: perf-baseline unsigned (pass --sign-key for tamper protection)",
            |r, d| r.baseline_sha256 = d.to_owned(),
        )
        .unwrap();
        assert_eq!(sealed.signature_hex, "");
        assert_eq!(
            fs::read_to_string(SignedReport::json_sidecar_path(&out)).unwrap(),
            ""
        );
        assert!(!got.baseline_sha256.is_empty(), "摘要仍须回填");
    }

    /// 读 key 失败发生在落盘之前（perf 现语义：不留半成品报告）。
    #[test]
    fn seal_sidecar_missing_key_writes_nothing() {
        let root = sandbox("bad-key");
        let out = root.join("x.json");
        let mut got = Sidecar {
            schema: "eneros.perf-regression.v1",
            baseline_sha256: String::new(),
            metrics: vec![],
        };
        let err = SignedReport::seal_sidecar(
            &root,
            &out,
            &mut got,
            Some(&root.join("no-such-key.bin")),
            "WARN: none",
            |r, d| r.baseline_sha256 = d.to_owned(),
        )
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
        assert!(err.starts_with("read sign key: "), "文案漂移：{err}");
        assert!(!out.exists());
    }

    #[test]
    fn load_key_keeps_legacy_wording_and_truncation() {
        let root = sandbox("load-key");
        let missing = SignedReport::load_key(&root.join("nope.bin"))
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(
            missing.starts_with("read sign key: "),
            "文案漂移：{missing}"
        );
        // >32 字节：截断取前 32 字节（与 32 字节 key 得同一把私钥）。
        let mut long = vec![9u8; 32];
        long.extend_from_slice(&[0u8; 8]);
        fs::write(root.join("long.bin"), &long).unwrap();
        fs::write(root.join("seed.bin"), [9u8; 32]).unwrap();
        assert_eq!(
            SignedReport::load_key(&root.join("long.bin"))
                .unwrap()
                .verifying_key()
                .to_bytes(),
            SignedReport::load_key(&root.join("seed.bin"))
                .unwrap()
                .verifying_key()
                .to_bytes()
        );
    }

    /// 两种侧车词形不等价（`<file>.sig` 追加 vs `with_extension`）。
    #[test]
    fn sidecar_path_shapes_are_distinct() {
        let json = Path::new("/e/qa_017.json");
        assert_eq!(
            SignedReport::sidecar_path(json),
            PathBuf::from("/e/qa_017.json.sig")
        );
        assert_eq!(
            SignedReport::json_sidecar_path(json),
            PathBuf::from("/e/qa_017.json.sig")
        );
        // 非 .json 扩展名时两者分叉——各自保留原点语义。
        let txt = Path::new("/e/transcript.log");
        assert_eq!(
            SignedReport::sidecar_path(txt),
            PathBuf::from("/e/transcript.log.sig")
        );
        assert_eq!(
            SignedReport::json_sidecar_path(txt),
            PathBuf::from("/e/transcript.json.sig")
        );
    }

    #[test]
    fn sign_hex_is_lowercase_deterministic_hex() {
        let key = SigningKey::from_bytes(&KEY_SEED);
        let sig = SignedReport::sign_hex(&key, b"eneros evidence payload\n");
        assert_eq!(sig.len(), 128);
        assert!(sig.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(sig, sig.to_lowercase());
        // 与 qa.rs 锁定测试同一夹具：逐字节 pin。
        assert_eq!(
            sig,
            "e97b8cd17381b0383bd19048e39138c106bb527b4ff7f021eb7b844857717b6b05b7635c1f61f854f90f1f8e2edf1d1b3fb26e0294427a2b8d5f030cd56da202"
        );
    }
}
