# SM2 第三方互操作向量（21-B C2）

来源：**OpenSSL 3.5.5（vetted 系统库）** `openssl pkeyutl -sign` 生成。
生成脚本：`tests/delivery/gen-sm2-vectors.sh`（重跑可复现）。

- `vectors.json`：10 组 `{ id, d, point, message, sig_der }`
  - `d`：私钥标量（openssl 生成密钥导出的 priv 值）
  - `point`：未压缩公钥点 `04||X||Y`（openssl 导出）
  - `sig_der`：openssl 对 `message` 的 SM2-SM3 签名（DER ECDSA-Sig-Value，
    distid = 国标默认 "1234567812345678"）
- `vectors.json.sha256`：哈希锚定（CI KAT job 先校验再跑测试）
