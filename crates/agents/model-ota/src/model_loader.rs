//! 模型热加载器（v0.111.0，D9/D10）.
//!
//! `alloc::sync::Arc` + 无锁 &mut self 单线程惯例（v0.110.0 D4 同先例）：
//! - `current: Arc<ModelInstance>` — 在运行模型（读侧 Arc 克隆，写侧 &mut self 排他）
//! - `previous: Option<Arc<ModelInstance>>` — 上一版本（回滚目标）
//! - `loading: Option<ModelInstance>` — 已加载待切换模型
//! - `white_list: Vec<[u8; 32]>` — 允许加载的模型 SM3 哈希白名单（构造注入，D11）
//!
//! 删除蓝图 `swap_lock`/`AlreadySwapping`（&mut self 编译期排他）与
//! `ModelInstance.ref_count: AtomicU32`（Arc 强引用计数承载生命周期）。

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::mem;

use eneros_crypto::sm3_hash;

use crate::ota_client::ModelInfo;
use crate::OtaError;

/// 模型实例（D10：删除蓝图 `ref_count`，Arc 强引用计数承载生命周期）.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelInstance {
    /// 模型清单.
    pub info: ModelInfo,
    /// 模型字节.
    pub data: Vec<u8>,
    /// 加载完成时间戳（毫秒）.
    pub loaded_at: u64,
}

/// 模型热加载器（白名单 + 原子热切换 + 回滚，D9）.
pub struct HotLoader {
    /// 在运行模型实例.
    current: Arc<ModelInstance>,
    /// 上一版本实例（回滚目标）.
    previous: Option<Arc<ModelInstance>>,
    /// 已加载待切换实例.
    loading: Option<ModelInstance>,
    /// 允许加载的模型 SM3 哈希白名单.
    white_list: Vec<[u8; 32]>,
}

impl HotLoader {
    /// 构造热加载器（initial 为首个在运行模型；previous/loading 初始为 None）.
    pub fn new(current: ModelInstance, white_list: Vec<[u8; 32]>) -> Self {
        Self {
            current: Arc::new(current),
            previous: None,
            loading: None,
            white_list,
        }
    }

    /// 加载新模型到待切换区（§4.3：哈希不在白名单 → `Err(NotInWhitelist)` 且
    /// loading 保持不变）.
    pub fn load_new(&mut self, data: &[u8], info: &ModelInfo, now: u64) -> Result<(), OtaError> {
        let hash = sm3_hash(data);
        if !self.white_list.contains(&hash) {
            return Err(OtaError::NotInWhitelist);
        }
        self.loading = Some(ModelInstance {
            info: info.clone(),
            data: data.to_vec(),
            loaded_at: now,
        });
        Ok(())
    }

    /// 原子切换：待切换模型成为 current，原 current 留存为 previous.
    ///
    /// loading 为 None → `Err(NothingToSwap)`；成功返回新 current 的 Arc 克隆。
    pub fn swap(&mut self) -> Result<Arc<ModelInstance>, OtaError> {
        let loading = self.loading.take().ok_or(OtaError::NothingToSwap)?;
        let old_current = mem::replace(&mut self.current, Arc::new(loading));
        self.previous = Some(old_current);
        Ok(Arc::clone(&self.current))
    }

    /// 回滚：current 与 previous 交换（蓝图 §6.4）.
    ///
    /// previous 为 None → `Err(NoPreviousVersion)`。
    pub fn rollback(&mut self) -> Result<(), OtaError> {
        let previous = self.previous.take().ok_or(OtaError::NoPreviousVersion)?;
        let old_current = mem::replace(&mut self.current, previous);
        self.previous = Some(old_current);
        Ok(())
    }

    /// 当前在运行模型实例（Arc 克隆）.
    pub fn current(&self) -> Arc<ModelInstance> {
        Arc::clone(&self.current)
    }

    /// 上一版本实例（只读引用）.
    pub fn previous(&self) -> Option<&Arc<ModelInstance>> {
        self.previous.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;
    use alloc::vec;

    use super::*;
    use crate::ota_client::{ModelSignature, SigAlgorithm};

    /// 构造测试清单（哈希/大小按 data 实算，无真实签名）.
    fn make_info(model_id: &str, version: &str, data: &[u8]) -> ModelInfo {
        ModelInfo {
            model_id: model_id.to_string(),
            version: version.to_string(),
            hash: sm3_hash(data),
            size: data.len() as u64,
            signature: ModelSignature {
                algorithm: SigAlgorithm::Sm2Sm3,
                signature: Vec::new(),
                timestamp: 0,
            },
            created_at: 0,
            capabilities: Vec::new(),
        }
    }

    fn make_instance(version: &str, data: &[u8], loaded_at: u64) -> ModelInstance {
        ModelInstance {
            info: make_info("m", version, data),
            data: data.to_vec(),
            loaded_at,
        }
    }

    /// HL14 load_new 不在白名单 → Err(NotInWhitelist)，loading 保持 None.
    #[test]
    fn hl14_load_new_not_in_whitelist() {
        let v1 = make_instance("1.0.0", b"v1-data", 100);
        let loader_white = vec![sm3_hash(b"v1-data")];
        let mut loader = HotLoader::new(v1, loader_white);
        let bad = b"v2-untrusted";
        let info = make_info("m", "2.0.0", bad);
        assert_eq!(
            loader.load_new(bad, &info, 200),
            Err(OtaError::NotInWhitelist)
        );
        assert!(loader.loading.is_none());
    }

    /// HL15 load_new 成功 → loading 就绪（swap 可行前置）.
    #[test]
    fn hl15_load_new_success() {
        let v1 = make_instance("1.0.0", b"v1-data", 100);
        let white = vec![sm3_hash(b"v1-data"), sm3_hash(b"v2-data")];
        let mut loader = HotLoader::new(v1, white);
        let info = make_info("m", "2.0.0", b"v2-data");
        assert_eq!(loader.load_new(b"v2-data", &info, 200), Ok(()));
        let loading = loader.loading.as_ref().unwrap();
        assert_eq!(loading.info.version, "2.0.0");
        assert_eq!(loading.data, b"v2-data".to_vec());
        assert_eq!(loading.loaded_at, 200);
    }

    /// HL16 swap 无 loading → Err(NothingToSwap).
    #[test]
    fn hl16_swap_without_loading() {
        let v1 = make_instance("1.0.0", b"v1-data", 100);
        let mut loader = HotLoader::new(v1, Vec::new());
        assert_eq!(loader.swap(), Err(OtaError::NothingToSwap));
    }

    /// HL17 swap 成功 → current 更新 + previous 留存 + loading 清空.
    #[test]
    fn hl17_swap_success() {
        let v1 = make_instance("1.0.0", b"v1-data", 100);
        let white = vec![sm3_hash(b"v2-data")];
        let mut loader = HotLoader::new(v1, white);
        let info = make_info("m", "2.0.0", b"v2-data");
        loader.load_new(b"v2-data", &info, 200).unwrap();

        let new_current = loader.swap().unwrap();
        assert_eq!(new_current.info.version, "2.0.0");
        assert_eq!(new_current.loaded_at, 200);
        assert_eq!(loader.current().info.version, "2.0.0");
        let previous = loader.previous().unwrap();
        assert_eq!(previous.info.version, "1.0.0");
        assert!(loader.loading.is_none());
    }

    /// HL18 连续两次 swap → previous 轮转正确（v1→v2→v3）.
    #[test]
    fn hl18_double_swap_rotation() {
        let v1 = make_instance("1.0.0", b"v1-data", 100);
        let white = vec![sm3_hash(b"v2-data"), sm3_hash(b"v3-data")];
        let mut loader = HotLoader::new(v1, white);

        let info2 = make_info("m", "2.0.0", b"v2-data");
        loader.load_new(b"v2-data", &info2, 200).unwrap();
        loader.swap().unwrap();
        assert_eq!(loader.current().info.version, "2.0.0");
        assert_eq!(loader.previous().unwrap().info.version, "1.0.0");

        let info3 = make_info("m", "3.0.0", b"v3-data");
        loader.load_new(b"v3-data", &info3, 300).unwrap();
        loader.swap().unwrap();
        assert_eq!(loader.current().info.version, "3.0.0");
        assert_eq!(loader.previous().unwrap().info.version, "2.0.0");
        assert!(loader.loading.is_none());
    }

    /// HL19 rollback 无 previous → Err(NoPreviousVersion).
    #[test]
    fn hl19_rollback_without_previous() {
        let v1 = make_instance("1.0.0", b"v1-data", 100);
        let mut loader = HotLoader::new(v1, Vec::new());
        assert_eq!(loader.rollback(), Err(OtaError::NoPreviousVersion));
    }

    /// HL20 rollback 成功 → current 恢复上一版，previous 变为被替换下的版本.
    #[test]
    fn hl20_rollback_success() {
        let v1 = make_instance("1.0.0", b"v1-data", 100);
        let white = vec![sm3_hash(b"v2-data")];
        let mut loader = HotLoader::new(v1, white);
        let info2 = make_info("m", "2.0.0", b"v2-data");
        loader.load_new(b"v2-data", &info2, 200).unwrap();
        loader.swap().unwrap();

        assert_eq!(loader.rollback(), Ok(()));
        assert_eq!(loader.current().info.version, "1.0.0");
        assert_eq!(loader.current().data, b"v1-data".to_vec());
        assert_eq!(loader.previous().unwrap().info.version, "2.0.0");
    }

    /// HL21 current() Arc 克隆 → 数据一致 + loaded_at 保留.
    #[test]
    fn hl21_current_arc_clone() {
        let v1 = make_instance("1.0.0", b"v1-data", 123);
        let loader = HotLoader::new(v1, Vec::new());
        let a = loader.current();
        let b = loader.current();
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(a.data, b"v1-data".to_vec());
        assert_eq!(a.loaded_at, 123);
        assert_eq!(a.info.version, "1.0.0");
    }
}
