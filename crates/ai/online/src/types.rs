//! 在线学习核心类型 (workspace v0.192.0).

use alloc::vec::Vec;

/// 计算设备.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputeDevice {
    /// NVIDIA CUDA GPU.
    Cuda,
    /// CPU 回退.
    Cpu,
}

/// 在线模型.
#[derive(Debug, Clone, PartialEq)]
pub struct OnlineModel {
    /// 模型 ID.
    pub model_id: u64,
    /// 模型版本号.
    pub version: u32,
    /// 参数张量（扁平化）.
    pub weights: Vec<f32>,
    /// 张量形状.
    pub shape: Vec<usize>,
    /// 计算设备.
    pub device: ComputeDevice,
}

impl OnlineModel {
    /// 创建新的在线模型.
    pub fn new(model_id: u64, weights: Vec<f32>, shape: Vec<usize>) -> Self {
        Self {
            model_id,
            version: 1,
            weights,
            shape,
            device: ComputeDevice::Cpu,
        }
    }

    /// 参数总数.
    pub fn param_count(&self) -> usize {
        self.weights.len()
    }
}

/// 训练批次.
#[derive(Debug, Clone, PartialEq)]
pub struct Batch {
    /// 特征数据（扁平化）.
    pub features: Vec<f32>,
    /// 标签数据.
    pub labels: Vec<f32>,
    /// 批次大小（样本数）.
    pub size: usize,
}

impl Batch {
    /// 创建新批次.
    pub fn new(features: Vec<f32>, labels: Vec<f32>, size: usize) -> Self {
        Self {
            features,
            labels,
            size,
        }
    }
}

/// 训练指标.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrainMetrics {
    /// 损失值.
    pub loss: f32,
    /// 平均绝对误差.
    pub mae: f32,
    /// 累计步数.
    pub steps: u32,
}

/// 微调状态.
#[derive(Debug, Clone)]
pub struct FinetuneState {
    /// 当前模型.
    pub model: OnlineModel,
    /// 学习率.
    pub lr: f32,
    /// 批次大小.
    pub batch_size: usize,
    /// 累计训练步数.
    pub steps: u32,
    /// 历史最优损失.
    pub best_loss: f32,
}

/// 设备选择器 — 优先 GPU，回退 CPU.
pub struct DeviceSelector;

impl DeviceSelector {
    /// 选择计算设备.
    ///
    /// 当前版本始终返回 Cpu（预留 CUDA FFI 接口）。
    pub fn select(&self) -> ComputeDevice {
        // 未来: if ffi::cuda_available() { Cuda } else { Cpu }
        ComputeDevice::Cpu
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_online_model_creation() {
        let model = OnlineModel::new(1, alloc::vec![0.1, 0.2, 0.3], alloc::vec![3]);
        assert_eq!(model.model_id, 1);
        assert_eq!(model.version, 1);
        assert_eq!(model.param_count(), 3);
        assert_eq!(model.device, ComputeDevice::Cpu);
    }

    #[test]
    fn test_batch_creation() {
        let batch = Batch::new(alloc::vec![1.0, 2.0, 3.0, 4.0], alloc::vec![0.5, 1.0], 2);
        assert_eq!(batch.size, 2);
        assert_eq!(batch.features.len(), 4);
        assert_eq!(batch.labels.len(), 2);
    }

    #[test]
    fn test_device_selector() {
        let selector = DeviceSelector;
        assert_eq!(selector.select(), ComputeDevice::Cpu);
    }

    #[test]
    fn test_train_metrics() {
        let m = TrainMetrics {
            loss: 0.5,
            mae: 0.3,
            steps: 10,
        };
        assert_eq!(m.loss, 0.5);
        assert_eq!(m.steps, 10);
    }
}
