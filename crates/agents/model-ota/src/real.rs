//! model-ota 真实传输后端（v0.233.0，B3-10）。
//!
//! [`OtaTransport`] 此前仅 [`MockOtaTransport`]。本模块提供
//! [`HttpOtaTransport`]：经注入 HTTP 闭包（真实云端请求）实现
//! 清单查询与断点续传分块下载；闭包由集成层接真实 HTTP/gRPC。

use alloc::string::String;
use alloc::vec::Vec;

use crate::ota_client::ModelInfo;
use crate::{OtaError, OtaTransport};

/// 基于注入 HTTP 闭包的 OTA 传输（真实后端）。
///
/// - `fetch_fn`: GET /manifest?current=<v> → JSON 清单（或空 = 无更新）
/// - `download_fn`: GET /model/<id>?offset=&len= → 分块字节
pub struct HttpOtaTransport<F, D> {
    /// 清单查询闭包。
    fetch_fn: F,
    /// 分块下载闭包。
    download_fn: D,
    /// 云端服务地址（诊断）。
    #[allow(dead_code)]
    server: String,
}

impl<F, D> HttpOtaTransport<F, D>
where
    F: FnMut(&str) -> Result<Option<ModelInfo>, OtaError>,
    D: FnMut(&str, u64, u64) -> Result<Vec<u8>, OtaError>,
{
    /// 创建真实 OTA 传输。
    pub fn new(server: &str, fetch_fn: F, download_fn: D) -> Self {
        Self {
            fetch_fn,
            download_fn,
            server: String::from(server),
        }
    }
}

impl<F, D> OtaTransport for HttpOtaTransport<F, D>
where
    F: FnMut(&str) -> Result<Option<ModelInfo>, OtaError>,
    D: FnMut(&str, u64, u64) -> Result<Vec<u8>, OtaError>,
{
    fn fetch_latest(&mut self, current_version: &str) -> Result<Option<ModelInfo>, OtaError> {
        (self.fetch_fn)(current_version)
    }

    fn download_range(&mut self, model_id: &str, offset: u64, len: u64) -> Result<Vec<u8>, OtaError> {
        (self.download_fn)(model_id, offset, len)
    }
}

// ============================================================================
// Tests（真实闭包注入）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ota_client::{ModelFormat, ModelType};

    fn info(version: &str) -> ModelInfo {
        ModelInfo {
            model_id: String::from("m1"),
            version: String::from(version),
            size: 100,
            model_type: ModelType::Llm,
            format: ModelFormat::Gguf,
            checksum: [0u8; 32],
        }
    }

    #[test]
    fn test_fetch_latest_end_to_end() {
        let mut t = HttpOtaTransport::new(
            "https://cloud.example.com",
            |v: &str| {
                assert_eq!(v, "1.0");
                Ok(Some(info("2.0")))
            },
            |_id: &str, _o: u64, _l: u64| Ok(vec![1, 2, 3]),
        );
        let latest = t.fetch_latest("1.0").expect("fetch");
        assert_eq!(latest.unwrap().version, "2.0");
    }

    #[test]
    fn test_fetch_no_update() {
        let mut t = HttpOtaTransport::new(
            "https://cloud.example.com",
            |_v: &str| Ok(None),
            |_id: &str, _o: u64, _l: u64| Ok(Vec::new()),
        );
        assert!(t.fetch_latest("1.0").expect("fetch").is_none());
    }

    #[test]
    fn test_download_range_passthrough() {
        let mut t = HttpOtaTransport::new(
            "https://cloud.example.com",
            |_v: &str| Ok(None),
            |id: &str, offset: u64, len: u64| {
                assert_eq!(id, "m1");
                assert_eq!(offset, 100);
                assert_eq!(len, 64);
                Ok(vec![0xAA; 64])
            },
        );
        let data = t.download_range("m1", 100, 64).expect("download");
        assert_eq!(data.len(), 64);
    }

    #[test]
    fn test_transport_error_propagates() {
        let mut t = HttpOtaTransport::new(
            "https://cloud.example.com",
            |_v: &str| Err(OtaError::TransportError),
            |_id: &str, _o: u64, _l: u64| Ok(Vec::new()),
        );
        assert!(t.fetch_latest("1.0").is_err());
    }
}
