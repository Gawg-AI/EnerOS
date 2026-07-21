//! SM2 椭圆曲线公钥密码算法 (GB/T 32918.1~5-2017).
//!
//! 提供 SM2 椭圆曲线运算（点加/点倍/标量乘法）和密钥对生成。
//!
//! # 国标引用
//! - GB/T 32918.1-2017 第1部分：总则
//! - GB/T 32918.5-2017 第5部分：参数定义
//!
//! # 曲线参数
//! SM2 推荐曲线（sm2p256v1）使用 256-bit 素域 GF(p)，曲线方程为
//! y^2 = x^3 + ax + b (mod p)。基点 G 的阶为 n。
//!
//! 所有常量以 `U256` 小端 limb 格式存储（`limbs[0]` 为最低有效字）。

use crate::bigint::U256;

pub mod encrypt;
pub mod keypair;
pub mod sign;

pub use encrypt::{sm2_decrypt, sm2_encrypt};
pub use keypair::{EcPoint, Sm2KeyPair, Sm2PrivateKey, Sm2PublicKey};
pub use sign::{sm2_sign, sm2_verify, Sm2Signature, Sm2Signer};

// ============================================================
// SM2 曲线参数 (GB/T 32918.5-2017 推荐曲线 sm2p256v1)
// ============================================================
// hex 为大端表示；limbs[0] = LSB, limbs[3] = MSB
//
// P  = FFFFFFFEFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF00000000FFFFFFFFFFFFFFFF
// A  = FFFFFFFEFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF00000000FFFFFFFFFFFFFFFC
// B  = 28E9FA9E9D9F5E344D5A9E4BCF6509A7F39789F515AB8F92DDBCBD414D940E93
// N  = FFFFFFFEFFFFFFFFFFFFFFFFFFFFFFFF7203DF6B21C6052B53BBF40939D54123
// Gx = 32C4AE2C1F1981195F9904466A39C9948FE30BBFF2660BE1715A4589334C74C7
// Gy = BC3736A2F4F6779C59BDCEE36B692153D0A9877CC62A474002DF32E52139F0A0

/// SM2 素域模数 p.
pub const SM2_P: U256 = U256 {
    limbs: [
        0xFFFFFFFFFFFFFFFF,
        0xFFFFFFFF00000000,
        0xFFFFFFFFFFFFFFFF,
        0xFFFFFFFEFFFFFFFF,
    ],
};

/// SM2 曲线参数 a.
pub const SM2_A: U256 = U256 {
    limbs: [
        0xFFFFFFFFFFFFFFFC,
        0xFFFFFFFF00000000,
        0xFFFFFFFFFFFFFFFF,
        0xFFFFFFFEFFFFFFFF,
    ],
};

/// SM2 曲线参数 b.
pub const SM2_B: U256 = U256 {
    limbs: [
        0xDDBCBD414D940E93,
        0xF39789F515AB8F92,
        0x4D5A9E4BCF6509A7,
        0x28E9FA9E9D9F5E34,
    ],
};

/// SM2 基点 G 的阶 n.
pub const SM2_N: U256 = U256 {
    limbs: [
        0x53BBF40939D54123,
        0x7203DF6B21C6052B,
        0xFFFFFFFFFFFFFFFF,
        0xFFFFFFFEFFFFFFFF,
    ],
};

/// SM2 基点 G 的 x 坐标.
pub const SM2_GX: U256 = U256 {
    limbs: [
        0x715A4589334C74C7,
        0x8FE30BBFF2660BE1,
        0x5F9904466A39C994,
        0x32C4AE2C1F198119,
    ],
};

/// SM2 基点 G 的 y 坐标.
pub const SM2_GY: U256 = U256 {
    limbs: [
        0x02DF32E52139F0A0,
        0xD0A9877CC62A4740,
        0x59BDCEE36B692153,
        0xBC3736A2F4F6779C,
    ],
};

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证所有曲线常量与十六进制大端表示一致.
    #[test]
    fn test_curve_constants_hex() {
        assert_eq!(
            SM2_P.to_hex(),
            "FFFFFFFEFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF00000000FFFFFFFFFFFFFFFF"
        );
        assert_eq!(
            SM2_A.to_hex(),
            "FFFFFFFEFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF00000000FFFFFFFFFFFFFFFC"
        );
        assert_eq!(
            SM2_B.to_hex(),
            "28E9FA9E9D9F5E344D5A9E4BCF6509A7F39789F515AB8F92DDBCBD414D940E93"
        );
        assert_eq!(
            SM2_N.to_hex(),
            "FFFFFFFEFFFFFFFFFFFFFFFFFFFFFFFF7203DF6B21C6052B53BBF40939D54123"
        );
        assert_eq!(
            SM2_GX.to_hex(),
            "32C4AE2C1F1981195F9904466A39C9948FE30BBFF2660BE1715A4589334C74C7"
        );
        assert_eq!(
            SM2_GY.to_hex(),
            "BC3736A2F4F6779C59BDCEE36B692153D0A9877CC62A474002DF32E52139F0A0"
        );
    }
}
