use alloc::vec::Vec;

/// 会话密钥（D9：密钥材料由调用方注入，防泄露不派生 Debug）。
#[derive(Clone, PartialEq)]
pub struct SessionKey {
    /// 密钥标识符。
    pub key_id: u32,
    /// SM4 对称密钥（128 bit）。
    pub key_data: [u8; 16],
    /// SM3-HMAC 认证密钥（256 bit）。
    pub mac_key: [u8; 32],
    /// 过期时间戳（绝对时间，单位由调用方约定）。
    pub expiry: u64,
}

/// 密钥管理器（多密钥存储 / 过期检测 / 密钥轮换）。
pub struct KeyMgmt {
    /// 本地密钥表（D6：Vec 替代 HashMap，no_std）。
    local_keys: Vec<SessionKey>,
    /// 密钥生命周期（绝对时间单位）。
    key_lifetime: u64,
    /// 下一个自动分配的 key_id。
    next_key_id: u32,
}

impl KeyMgmt {
    /// 创建空密钥表，初始 key_id = 1。
    pub fn new(key_lifetime: u64) -> Self {
        Self {
            local_keys: Vec::new(),
            key_lifetime,
            next_key_id: 1,
        }
    }

    /// 将外部密钥存入密钥表。
    pub fn add_key(&mut self, session: SessionKey) {
        if session.key_id >= self.next_key_id {
            self.next_key_id = session.key_id + 1;
        }
        self.local_keys.push(session);
    }

    /// 返回最近添加且未过期（expiry > now）的密钥；无则返回 KeyExpired。
    pub fn get_current_key(&self, now: u64) -> Result<&SessionKey, crate::SecError> {
        for key in self.local_keys.iter().rev() {
            if key.expiry > now {
                return Ok(key);
            }
        }
        Err(crate::SecError::KeyExpired)
    }

    /// 密钥轮换（D9：密钥材料由调用方注入）。
    ///
    /// 若当前密钥存在且未过期 → 不轮换，直接返回 Ok(())；
    /// 若无当前密钥或已过期 → 生成新密钥并追加。
    pub fn rotate_keys(
        &mut self,
        now: u64,
        new_key_data: [u8; 16],
        new_mac_key: [u8; 32],
    ) -> Result<(), crate::SecError> {
        if let Ok(current) = self.get_current_key(now) {
            if current.expiry > now {
                return Ok(());
            }
        }
        let new_key = SessionKey {
            key_id: self.next_key_id,
            key_data: new_key_data,
            mac_key: new_mac_key,
            expiry: now + self.key_lifetime,
        };
        self.local_keys.push(new_key);
        self.next_key_id += 1;
        Ok(())
    }

    /// 按 key_id 精确查找密钥；未命中返回 InvalidKeyId。
    pub fn get_key(&self, key_id: u32) -> Result<&SessionKey, crate::SecError> {
        for key in &self.local_keys {
            if key.key_id == key_id {
                return Ok(key);
            }
        }
        Err(crate::SecError::InvalidKeyId)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key_data() -> [u8; 16] {
        [0x42u8; 16]
    }

    fn test_mac_key() -> [u8; 32] {
        [0x24u8; 32]
    }

    /// KM1: new 创建空密钥表，get_current_key 返回 KeyExpired。
    #[test]
    fn km1_new_empty_table() {
        let km = KeyMgmt::new(3600);
        assert_eq!(
            km.get_current_key(0).err(),
            Some(crate::SecError::KeyExpired)
        );
    }

    /// KM2: add_key 存储密钥后 get_current_key 命中。
    #[test]
    fn km2_add_key_hits() {
        let mut km = KeyMgmt::new(3600);
        let sk = SessionKey {
            key_id: 1,
            key_data: test_key_data(),
            mac_key: test_mac_key(),
            expiry: 1000,
        };
        km.add_key(sk);
        let got = km.get_current_key(999).unwrap();
        assert_eq!(got.key_id, 1);
    }

    /// KM3: get_current_key 返回未过期密钥；多个未过期时取最近添加的。
    #[test]
    fn km3_get_current_key_unexpired_most_recent() {
        let mut km = KeyMgmt::new(3600);
        km.add_key(SessionKey {
            key_id: 1,
            key_data: test_key_data(),
            mac_key: test_mac_key(),
            expiry: 1000,
        });
        km.add_key(SessionKey {
            key_id: 2,
            key_data: [0xABu8; 16],
            mac_key: [0xBAu8; 32],
            expiry: 2000,
        });
        let got = km.get_current_key(1500).unwrap();
        assert_eq!(got.key_id, 2);
    }

    /// KM4: get_current_key 全过期返回 KeyExpired（expiry == now 视为过期）。
    #[test]
    fn km4_all_expired() {
        let mut km = KeyMgmt::new(3600);
        km.add_key(SessionKey {
            key_id: 1,
            key_data: test_key_data(),
            mac_key: test_mac_key(),
            expiry: 1000,
        });
        assert_eq!(
            km.get_current_key(1000).err(),
            Some(crate::SecError::KeyExpired)
        );
        assert_eq!(
            km.get_current_key(1001).err(),
            Some(crate::SecError::KeyExpired)
        );
    }

    /// KM5: rotate_keys 当前过期时生成新密钥（key_id 递增、expiry 正确、材料匹配）。
    #[test]
    fn km5_rotate_when_expired_generates_new_key() {
        let mut km = KeyMgmt::new(500);
        km.add_key(SessionKey {
            key_id: 1,
            key_data: test_key_data(),
            mac_key: test_mac_key(),
            expiry: 1000,
        });
        let new_data = [0x11u8; 16];
        let new_mac = [0x22u8; 32];
        km.rotate_keys(1001, new_data, new_mac).unwrap();

        let got = km.get_current_key(1001).unwrap();
        assert_eq!(got.key_id, 2);
        assert_eq!(got.key_data, new_data);
        assert_eq!(got.mac_key, new_mac);
        assert_eq!(got.expiry, 1501); // now + lifetime
    }

    /// KM6: rotate_keys 当前未过期时不轮换（密钥数、当前 key_id 不变）。
    #[test]
    fn km6_rotate_when_unexpired_no_rotation() {
        let mut km = KeyMgmt::new(500);
        km.add_key(SessionKey {
            key_id: 1,
            key_data: test_key_data(),
            mac_key: test_mac_key(),
            expiry: 1000,
        });
        km.rotate_keys(999, [0x11u8; 16], [0x22u8; 32]).unwrap();

        let got = km.get_current_key(999).unwrap();
        assert_eq!(got.key_id, 1);
        assert_eq!(got.key_data, test_key_data());
    }

    /// KM7: get_key 按 id 命中。
    #[test]
    fn km7_get_key_by_id_hits() {
        let mut km = KeyMgmt::new(3600);
        km.add_key(SessionKey {
            key_id: 7,
            key_data: test_key_data(),
            mac_key: test_mac_key(),
            expiry: 2000,
        });
        let got = km.get_key(7).unwrap();
        assert_eq!(got.key_id, 7);
        assert_eq!(got.key_data, test_key_data());
    }

    /// KM8: get_key 未命中返回 InvalidKeyId。
    #[test]
    fn km8_get_key_miss() {
        let km = KeyMgmt::new(3600);
        assert_eq!(km.get_key(42).err(), Some(crate::SecError::InvalidKeyId));
    }
}
