//! 板级 adapter 注册表与装配选择（WP-70.12.2）。
//!
//! # 为什么需要注册表
//! 旧路径在 `root-task/src/platform.rs` 里**硬编码** `QemuVirtAdapter`：任何
//! 目标板卡镜像都会用 QEMU 的平台事实（一致内存、无 reset/clock）装配——
//! 真板上就是静默的错误事实。注册表把"这个 SKU 用哪个 adapter"变成**显式
//! 声明 + 未注册即拒绝**，并保留 QEMU 开发入口。
//!
//! # 判据
//! - 未声明选择（开发镜像）→ 用注册的开发平台 adapter（QEMU virt）；
//! - 声明了选择但该 SKU **没有注册 adapter** → [`BoardRefusal::NoAdapterForSku`]
//!   （fail-closed：不回退 QEMU——这正是"运时猜 compatible 再兜底"的反面）；
//! - 未注册 SKU 的 adapter 一旦出现，必须同时给出该 SKU 的冻结记录摘要
//!   （无摘要的实板 adapter 不接受）。

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use eneros_hal::platform_adapter::PlatformAdapter;

use crate::platform::BoardSelection;

/// 装配拒绝原因（全部 fail-closed）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoardRefusal {
    /// 声明了板级选择，但该 SKU 未注册 adapter（不回退开发平台）。
    NoAdapterForSku {
        sku_id: String,
        registered: Vec<String>,
    },
    /// 注册项形态非法（摘要缺失/空串）。
    RegistrationIncomplete {
        sku_id: String,
        reason: &'static str,
    },
    /// 选择形态非法。
    Selection(crate::platform::SelectionError),
}

impl BoardRefusal {
    pub fn reason(&self) -> String {
        match self {
            BoardRefusal::NoAdapterForSku { sku_id, registered } => {
                alloc::format!(
                    "board-no-adapter-for-sku:{}:registered=[{}]",
                    sku_id,
                    registered.join(",")
                )
            }
            BoardRefusal::RegistrationIncomplete { sku_id, reason } => {
                alloc::format!("board-registration-incomplete:{sku_id}:{reason}")
            }
            BoardRefusal::Selection(e) => e.reason().to_string(),
        }
    }
}

/// 一条 adapter 注册项。
pub struct RegisteredAdapter {
    /// SKU id（与兼容目录/冻结记录同域）。
    pub sku_id: &'static str,
    /// 板卡修订（`"unknown"` 表示未冻结——该 adapter 不得用于产品装配）。
    pub revision: &'static str,
    /// 冻结记录摘要（结构账本摘要；空 = 未冻结）。
    pub record_digest: &'static str,
    /// 平台 adapter 实例（`'static`：root 装配期取引用）。
    pub adapter: &'static dyn PlatformAdapter,
}

/// 板级 adapter 注册表。
pub struct AdapterRegistry {
    entries: Vec<RegisteredAdapter>,
}

impl AdapterRegistry {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// 注册（**不**静默覆盖：同 SKU 重复注册即拒绝）。
    pub fn register(&mut self, entry: RegisteredAdapter) -> Result<(), BoardRefusal> {
        if entry.sku_id.trim().is_empty() {
            return Err(BoardRefusal::RegistrationIncomplete {
                sku_id: entry.sku_id.to_string(),
                reason: "sku-empty",
            });
        }
        if entry.revision.trim().is_empty() {
            return Err(BoardRefusal::RegistrationIncomplete {
                sku_id: entry.sku_id.to_string(),
                reason: "revision-empty",
            });
        }
        if self.entries.iter().any(|e| e.sku_id == entry.sku_id) {
            return Err(BoardRefusal::RegistrationIncomplete {
                sku_id: entry.sku_id.to_string(),
                reason: "duplicate",
            });
        }
        self.entries.push(entry);
        Ok(())
    }

    /// 已注册 SKU（报告/拒绝理由用）。
    pub fn skus(&self) -> Vec<String> {
        self.entries.iter().map(|e| e.sku_id.to_string()).collect()
    }

    /// 按 SKU 取 adapter（含修订比对）。
    pub fn get(&self, sku_id: &str, revision: &str) -> Option<&RegisteredAdapter> {
        self.entries
            .iter()
            .find(|e| e.sku_id == sku_id && e.revision == revision)
    }

    /// 解析装配所用的 adapter。
    ///
    /// `selection = None` → 开发入口（调用方给默认 adapter，通常 QEMU virt）。
    pub fn resolve<'a>(
        &'a self,
        selection: Option<&BoardSelection>,
        development_default: &'a dyn PlatformAdapter,
    ) -> Result<&'a dyn PlatformAdapter, BoardRefusal> {
        let Some(sel) = selection else {
            return Ok(development_default);
        };
        sel.validate().map_err(BoardRefusal::Selection)?;
        match self.get(&sel.sku_id, &sel.revision) {
            Some(entry) => {
                if !sel.record_digest.is_empty()
                    && !entry.record_digest.is_empty()
                    && sel.record_digest != entry.record_digest
                {
                    return Err(BoardRefusal::RegistrationIncomplete {
                        sku_id: sel.sku_id.clone(),
                        reason: "record-digest-mismatch",
                    });
                }
                Ok(entry.adapter)
            }
            None => Err(BoardRefusal::NoAdapterForSku {
                sku_id: sel.sku_id.clone(),
                registered: self.skus(),
            }),
        }
    }
}

impl Default for AdapterRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// 编译期装配选择（`ENEROS_BOARD_SKU` / `ENEROS_BOARD_REVISION` /
/// `ENEROS_BOARD_RECORD_DIGEST` / `ENEROS_BOARD_DESCRIPTION_DIGEST`）。
///
/// 全部未给 → `None`（开发镜像）。**只给部分** → `Err`：装配期拒绝歧义
/// （半套绑定的镜像不得运行）。
pub fn assembly_selection(
    sku: Option<&str>,
    revision: Option<&str>,
    record_digest: Option<&str>,
    description_digest: Option<&str>,
) -> Result<Option<BoardSelection>, BoardRefusal> {
    let any = sku.is_some() || revision.is_some() || record_digest.is_some();
    if !any {
        return Ok(None);
    }
    let sku = sku.ok_or(BoardRefusal::RegistrationIncomplete {
        sku_id: String::from("<unset>"),
        reason: "sku-unset",
    })?;
    let revision = revision.ok_or(BoardRefusal::RegistrationIncomplete {
        sku_id: sku.to_string(),
        reason: "revision-unset",
    })?;
    let selection = BoardSelection {
        sku_id: sku.to_string(),
        revision: revision.to_string(),
        record_digest: record_digest.unwrap_or("").to_string(),
        description_digest: description_digest.unwrap_or("").to_string(),
    };
    selection.validate().map_err(BoardRefusal::Selection)?;
    Ok(Some(selection))
}

/// adapter 工厂 trait（`no_std` 下无法用闭包泛型注册时的显式接缝）。
pub trait BoardAdapter {
    /// SKU id。
    fn sku_id(&self) -> &'static str;
    /// 板卡修订。
    fn revision(&self) -> &'static str;
    /// 冻结记录摘要。
    fn record_digest(&self) -> &'static str;
    /// 平台 adapter。
    fn adapter(&self) -> &'static dyn PlatformAdapter;
}

#[cfg(test)]
mod tests {
    use eneros_hal::inventory::ValidatedInventory;
    use eneros_hal::platform_adapter::{HalRuntime, PlatformError, PlatformInput};

    use super::*;

    /// 占位 adapter（只用于注册/解析路径；`discover` 恒拒绝）。
    struct StubAdapter;

    impl PlatformAdapter for StubAdapter {
        fn discover(&self, _input: &PlatformInput) -> Result<ValidatedInventory, PlatformError> {
            Err(PlatformError::Unsupported)
        }
        fn build_hal(&self, _inv: &ValidatedInventory) -> Result<HalRuntime, PlatformError> {
            Err(PlatformError::Unsupported)
        }
    }

    static DEV: StubAdapter = StubAdapter;
    static HW: StubAdapter = StubAdapter;
    static HW_DUP: StubAdapter = StubAdapter;
    /// 注册用的冻结记录摘要（静态字符串：注册表要求 `'static`）。
    const REC_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const REC_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn registry() -> AdapterRegistry {
        let mut r = AdapterRegistry::new();
        r.register(RegisteredAdapter {
            sku_id: "acme-board",
            revision: "rev-A",
            record_digest: REC_A,
            adapter: &HW,
        })
        .unwrap();
        r
    }

    fn selection(sku: &str, revision: &str, digest: &str) -> BoardSelection {
        BoardSelection {
            sku_id: sku.to_string(),
            revision: revision.to_string(),
            record_digest: if digest.is_empty() {
                String::new()
            } else {
                digest.to_string()
            },
            description_digest: String::new(),
        }
    }

    #[test]
    fn development_images_use_default_adapter() {
        let r = registry();
        let chosen = r.resolve(None, &DEV).expect("dev entry");
        assert!(chosen.discover(&PlatformInput::default()).is_err());
        // 开发入口不问 SKU。
        assert_eq!(r.skus(), alloc::vec!["acme-board".to_string()]);
    }

    #[test]
    fn declared_but_unregistered_sku_is_refused_not_fallen_back() {
        let r = registry();
        let sel = selection("phytium-d2000", "rev-A", "");
        let err = match r.resolve(Some(&sel), &DEV) {
            Err(e) => e,
            Ok(_) => panic!("unregistered SKU must be refused"),
        };
        assert!(matches!(err, BoardRefusal::NoAdapterForSku { .. }));
        assert_eq!(
            err.reason(),
            "board-no-adapter-for-sku:phytium-d2000:registered=[acme-board]"
        );
        // 修订不一致同样拒绝（换修订 = 新组合）。
        let sel = selection("acme-board", "rev-B", "");
        match r.resolve(Some(&sel), &DEV) {
            Err(BoardRefusal::NoAdapterForSku { .. }) => {}
            Err(other) => panic!("unexpected refusal {other:?}"),
            Ok(_) => panic!("revision mismatch must be refused"),
        }
    }

    #[test]
    fn registered_sku_resolves_and_record_digest_must_match() {
        let r = registry();
        let sel = selection("acme-board", "rev-A", REC_A);
        assert!(r.resolve(Some(&sel), &DEV).is_ok());
        let sel = selection("acme-board", "rev-A", REC_B);
        let err = match r.resolve(Some(&sel), &DEV) {
            Err(e) => e,
            Ok(_) => panic!("record digest mismatch must be refused"),
        };
        assert_eq!(
            err.reason(),
            "board-registration-incomplete:acme-board:record-digest-mismatch"
        );
    }

    #[test]
    fn duplicate_and_incomplete_registration_are_rejected() {
        let mut r = registry();
        let err = r
            .register(RegisteredAdapter {
                sku_id: "acme-board",
                revision: "rev-A",
                record_digest: "",
                adapter: &HW_DUP,
            })
            .unwrap_err();
        assert_eq!(
            err.reason(),
            "board-registration-incomplete:acme-board:duplicate"
        );
        let err = r
            .register(RegisteredAdapter {
                sku_id: "",
                revision: "rev-A",
                record_digest: "",
                adapter: &HW_DUP,
            })
            .unwrap_err();
        assert_eq!(err.reason(), "board-registration-incomplete::sku-empty");
    }

    #[test]
    fn partial_assembly_binding_is_refused() {
        // 只给 SKU 不给修订 → 拒绝（半套绑定不得运行）。
        let err = assembly_selection(Some("acme-board"), None, None, None).unwrap_err();
        assert_eq!(
            err.reason(),
            "board-registration-incomplete:acme-board:revision-unset"
        );
        let err = assembly_selection(None, Some("rev-A"), None, None).unwrap_err();
        assert_eq!(
            err.reason(),
            "board-registration-incomplete:<unset>:sku-unset"
        );
        // 全空 → 开发入口。
        assert_eq!(assembly_selection(None, None, None, None).unwrap(), None);
        // 全套 → 选择成立。
        let sel = assembly_selection(Some("acme-board"), Some("rev-A"), Some(REC_A), None)
            .unwrap()
            .unwrap();
        assert_eq!(sel.sku_id, "acme-board");
        assert!(!sel.binds_description());
        // 摘要形态非法 → 拒绝。
        assert!(assembly_selection(Some("acme-board"), Some("rev-A"), Some("nope"), None).is_err());
    }

    #[test]
    fn board_adapter_trait_exposes_identity() {
        struct Impl;
        impl BoardAdapter for Impl {
            fn sku_id(&self) -> &'static str {
                "acme-board"
            }
            fn revision(&self) -> &'static str {
                "rev-A"
            }
            fn record_digest(&self) -> &'static str {
                "digest"
            }
            fn adapter(&self) -> &'static dyn PlatformAdapter {
                &HW
            }
        }
        assert_eq!(Impl.sku_id(), "acme-board");
        assert!(Impl.adapter().discover(&PlatformInput::default()).is_err());
    }
}
