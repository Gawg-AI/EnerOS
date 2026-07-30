//! DID 文档存储.
//!
//! 基于 BTreeMap 的内存 DID 文档存储，支持创建、更新、查询操作。

use alloc::collections::BTreeMap;
use alloc::string::String;

use crate::types::{Did, DidDocument};
use crate::DidError;

/// DID 文档存储.
///
/// 使用 BTreeMap 以 DID 字符串为键存储文档。
#[derive(Debug, Default)]
pub struct DocumentStore {
    docs: BTreeMap<String, DidDocument>,
}

impl DocumentStore {
    /// 创建空的文档存储.
    pub fn new() -> Self {
        Self {
            docs: BTreeMap::new(),
        }
    }

    /// 创建 DID 文档.
    ///
    /// # 错误
    /// - [`DidError::AlreadyExists`][]: DID 已存在
    pub fn create(&mut self, doc: DidDocument) -> Result<(), DidError> {
        let key = doc.id.to_did_string();
        if self.docs.contains_key(&key) {
            return Err(DidError::AlreadyExists);
        }
        self.docs.insert(key, doc);
        Ok(())
    }

    /// 更新 DID 文档.
    ///
    /// # 错误
    /// - [`DidError::NotFound`][]: DID 不存在
    pub fn update(&mut self, doc: DidDocument) -> Result<(), DidError> {
        let key = doc.id.to_did_string();
        if !self.docs.contains_key(&key) {
            return Err(DidError::NotFound);
        }
        self.docs.insert(key, doc);
        Ok(())
    }

    /// 获取 DID 文档.
    ///
    /// # 错误
    /// - [`DidError::NotFound`][]: DID 不存在
    pub fn get(&self, did: &Did) -> Result<&DidDocument, DidError> {
        let key = did.to_did_string();
        self.docs.get(&key).ok_or(DidError::NotFound)
    }

    /// 检查 DID 是否存在.
    pub fn contains(&self, did: &Did) -> bool {
        let key = did.to_did_string();
        self.docs.contains_key(&key)
    }

    /// 返回存储的文档数量.
    pub fn len(&self) -> usize {
        self.docs.len()
    }

    /// 存储是否为空.
    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AuthMethod;

    fn make_doc(id: &str) -> DidDocument {
        DidDocument {
            id: Did::new("eneros", "main", id),
            public_key: alloc::vec![0x04; 65],
            auth_methods: alloc::vec![AuthMethod::Sm2Signature],
            service_endpoints: alloc::vec![],
            created: 1_700_000_000,
            updated: 1_700_000_000,
        }
    }

    #[test]
    fn test_create_and_get() {
        let mut store = DocumentStore::new();
        let doc = make_doc("1");
        store.create(doc.clone()).unwrap();
        let retrieved = store.get(&doc.id).unwrap();
        assert_eq!(retrieved, &doc);
    }

    #[test]
    fn test_create_duplicate() {
        let mut store = DocumentStore::new();
        let doc = make_doc("1");
        store.create(doc.clone()).unwrap();
        assert_eq!(store.create(doc), Err(DidError::AlreadyExists));
    }

    #[test]
    fn test_update() {
        let mut store = DocumentStore::new();
        let mut doc = make_doc("1");
        store.create(doc.clone()).unwrap();
        doc.updated = 1_700_001_000;
        store.update(doc.clone()).unwrap();
        let retrieved = store.get(&doc.id).unwrap();
        assert_eq!(retrieved.updated, 1_700_001_000);
    }

    #[test]
    fn test_update_not_found() {
        let mut store = DocumentStore::new();
        let doc = make_doc("nonexistent");
        assert_eq!(store.update(doc), Err(DidError::NotFound));
    }

    #[test]
    fn test_get_not_found() {
        let store = DocumentStore::new();
        let did = Did::new("eneros", "main", "missing");
        assert_eq!(store.get(&did), Err(DidError::NotFound));
    }

    #[test]
    fn test_len_and_is_empty() {
        let mut store = DocumentStore::new();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
        store.create(make_doc("1")).unwrap();
        assert!(!store.is_empty());
        assert_eq!(store.len(), 1);
        store.create(make_doc("2")).unwrap();
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn test_contains() {
        let mut store = DocumentStore::new();
        let doc = make_doc("1");
        assert!(!store.contains(&doc.id));
        store.create(doc.clone()).unwrap();
        assert!(store.contains(&doc.id));
    }
}
