/*
 * gmssl_helpers.c — OpenSSL 3 libcrypto 国密帮助层（ADR-022 binding）。
 *
 * 封装 SM3 / SM2 签名验签 / SM4-ECB 的最小调用序列。Rust 侧仅通过本文件
 * 导出的 5 个函数与 libcrypto 交互（FFI 面最小化）。
 *
 * 实现要点（2026-08-09 OpenSSL 3.5.5 实测确认）：
 * - SM2 密钥构造必须走 EVP_PKEY_fromdata（raw key 构造对 SM2 报
 *   "invalid field"）；且必须显式提供 pub 点（SM2 keymgmt 不从 priv 派生）。
 * - SM2 签名必须走 EVP_DigestSign（自动计算 Z = SM3(ENTL||ID||a||b||Gx||Gy||Px||Py)，
 *   再 e = SM3(Z||M)）；裸 EVP_PKEY_sign 要求调用方已算好 e。
 * - distid 通过 EVP_PKEY_CTX_set_params(OSSL_PKEY_PARAM_DIST_ID) 设置
 *   （legacy EVP_PKEY_CTX_set1_id 在 provider 路径不生效）。
 * - EVP_Cipher 对块密码返回处理字节数（SM4-ECB 单块 = 16），非 1。
 *
 * 错误纪律：所有函数返回 -1（失败）或 0（成功）；验签函数返回 1（有效）/
 * 0（无效）/-1（错误）。不打印、不吞错。
 */

#include <openssl/evp.h>
#include <openssl/core_names.h>
#include <openssl/param_build.h>
#include <openssl/bn.h>
#include <string.h>

#define SM2_GROUP_NAME "sm2"
#define DEFAULT_DIST_ID "1234567812345678"
#define DIST_ID_LEN 16

/* 从 d（32B）+ 未压缩点（65B）构造 SM2 密钥对。返回 EVP_PKEY* 或 NULL。 */
static EVP_PKEY *sm2_keypair_from_bytes(const unsigned char *d,
                                        const unsigned char *point) {
    EVP_PKEY_CTX *ctx = EVP_PKEY_CTX_new_from_name(NULL, "SM2", NULL);
    if (ctx == NULL)
        return NULL;
    if (EVP_PKEY_fromdata_init(ctx) != 1) {
        EVP_PKEY_CTX_free(ctx);
        return NULL;
    }
    OSSL_PARAM_BLD *bld = OSSL_PARAM_BLD_new();
    if (bld == NULL) {
        EVP_PKEY_CTX_free(ctx);
        return NULL;
    }
    BIGNUM *bn = BN_bin2bn(d, 32, NULL);
    int ok = bn != NULL
             && OSSL_PARAM_BLD_push_utf8_string(bld, OSSL_PKEY_PARAM_GROUP_NAME,
                                                SM2_GROUP_NAME, 0) == 1
             && OSSL_PARAM_BLD_push_BN(bld, OSSL_PKEY_PARAM_PRIV_KEY, bn) == 1
             && OSSL_PARAM_BLD_push_octet_string(bld, OSSL_PKEY_PARAM_PUB_KEY,
                                                 point, 65) == 1;
    if (!ok) {
        if (bn != NULL)
            BN_free(bn);
        OSSL_PARAM_BLD_free(bld);
        EVP_PKEY_CTX_free(ctx);
        return NULL;
    }
    OSSL_PARAM *params = OSSL_PARAM_BLD_to_param(bld);
    /* OSSL_PARAM_BLD_push_BN 只引用 BN（to_param 时才复制值），
     * 因此 BN 必须存活到 to_param 之后才能释放。 */
    if (bn != NULL)
        BN_free(bn);
    OSSL_PARAM_BLD_free(bld);
    if (params == NULL) {
        EVP_PKEY_CTX_free(ctx);
        return NULL;
    }
    EVP_PKEY *pkey = NULL;
    int rc = EVP_PKEY_fromdata(ctx, &pkey, EVP_PKEY_KEYPAIR, params);
    OSSL_PARAM_free(params);
    EVP_PKEY_CTX_free(ctx);
    return rc == 1 ? pkey : NULL;
}

/* 从 65B 未压缩点构造 SM2 公钥。返回 EVP_PKEY* 或 NULL。 */
static EVP_PKEY *sm2_pubkey_from_bytes(const unsigned char *point) {
    EVP_PKEY_CTX *ctx = EVP_PKEY_CTX_new_from_name(NULL, "SM2", NULL);
    if (ctx == NULL)
        return NULL;
    if (EVP_PKEY_fromdata_init(ctx) != 1) {
        EVP_PKEY_CTX_free(ctx);
        return NULL;
    }
    OSSL_PARAM_BLD *bld = OSSL_PARAM_BLD_new();
    if (bld == NULL) {
        EVP_PKEY_CTX_free(ctx);
        return NULL;
    }
    int ok = OSSL_PARAM_BLD_push_utf8_string(bld, OSSL_PKEY_PARAM_GROUP_NAME,
                                             SM2_GROUP_NAME, 0) == 1
             && OSSL_PARAM_BLD_push_octet_string(bld, OSSL_PKEY_PARAM_PUB_KEY,
                                                 point, 65) == 1;
    if (!ok) {
        OSSL_PARAM_BLD_free(bld);
        EVP_PKEY_CTX_free(ctx);
        return NULL;
    }
    OSSL_PARAM *params = OSSL_PARAM_BLD_to_param(bld);
    OSSL_PARAM_BLD_free(bld);
    if (params == NULL) {
        EVP_PKEY_CTX_free(ctx);
        return NULL;
    }
    EVP_PKEY *pkey = NULL;
    int rc = EVP_PKEY_fromdata(ctx, &pkey, EVP_PKEY_PUBLIC_KEY, params);
    OSSL_PARAM_free(params);
    EVP_PKEY_CTX_free(ctx);
    return rc == 1 ? pkey : NULL;
}

/* 在签名/验签 MD_CTX 上设置 distid（DigestSignInit 之后、签名之前）。 */
static int set_dist_id(EVP_PKEY_CTX *pctx) {
    if (pctx == NULL)
        return -1;
    OSSL_PARAM_BLD *bld = OSSL_PARAM_BLD_new();
    if (bld == NULL)
        return -1;
    int ok = OSSL_PARAM_BLD_push_octet_string(bld, OSSL_PKEY_PARAM_DIST_ID,
                                              DEFAULT_DIST_ID, DIST_ID_LEN) == 1;
    if (!ok) {
        OSSL_PARAM_BLD_free(bld);
        return -1;
    }
    OSSL_PARAM *params = OSSL_PARAM_BLD_to_param(bld);
    OSSL_PARAM_BLD_free(bld);
    if (params == NULL)
        return -1;
    int rc = EVP_PKEY_CTX_set_params(pctx, params);
    OSSL_PARAM_free(params);
    return rc == 1 ? 0 : -1;
}

/*
 * SM2 签名（DigestSign one-shot）。
 * 入参：d[32] 私钥标量；point[65] 未压缩公钥点；msg/msglen 消息。
 * 出参：sig 缓冲区（>=128B）；*siglen 入 = 缓冲容量，出 = DER 签名长度。
 * 返回：0 成功，-1 失败。
 */
int gmssl_sm2_sign(const unsigned char *d, const unsigned char *point,
                   const unsigned char *msg, size_t msglen,
                   unsigned char *sig, size_t *siglen) {
    EVP_PKEY *pkey = sm2_keypair_from_bytes(d, point);
    if (pkey == NULL)
        return -1;
    EVP_MD_CTX *mctx = EVP_MD_CTX_new();
    if (mctx == NULL) {
        EVP_PKEY_free(pkey);
        return -1;
    }
    EVP_PKEY_CTX *pctx = NULL;
    int rc = EVP_DigestSignInit(mctx, &pctx, EVP_sm3(), NULL, pkey);
    if (rc != 1 || set_dist_id(pctx) != 0) {
        EVP_MD_CTX_free(mctx);
        EVP_PKEY_free(pkey);
        return -1;
    }
    rc = EVP_DigestSign(mctx, sig, siglen, msg, msglen);
    EVP_MD_CTX_free(mctx);
    EVP_PKEY_free(pkey);
    return rc == 1 ? 0 : -1;
}

/*
 * SM2 验签。
 * 入参：point[65] 未压缩公钥点；msg/msglen；sig/siglen（DER）。
 * 返回：1 有效，0 无效，-1 错误。
 */
int gmssl_sm2_verify(const unsigned char *point, const unsigned char *msg,
                     size_t msglen, const unsigned char *sig, size_t siglen) {
    EVP_PKEY *pkey = sm2_pubkey_from_bytes(point);
    if (pkey == NULL)
        return -1;
    EVP_MD_CTX *mctx = EVP_MD_CTX_new();
    if (mctx == NULL) {
        EVP_PKEY_free(pkey);
        return -1;
    }
    EVP_PKEY_CTX *pctx = NULL;
    int rc = EVP_DigestVerifyInit(mctx, &pctx, EVP_sm3(), NULL, pkey);
    if (rc != 1 || set_dist_id(pctx) != 0) {
        EVP_MD_CTX_free(mctx);
        EVP_PKEY_free(pkey);
        return -1;
    }
    rc = EVP_DigestVerify(mctx, sig, siglen, msg, msglen);
    EVP_MD_CTX_free(mctx);
    EVP_PKEY_free(pkey);
    return rc; /* 1 有效，0 无效，负值错误 */
}

/*
 * SM3 摘要。
 * 入参：data/len。出参：md[32]。
 * 返回：0 成功，-1 失败。
 */
int gmssl_sm3(const unsigned char *data, size_t len, unsigned char *md) {
    unsigned int mdlen = 0;
    int rc = EVP_Digest(data, len, md, &mdlen, EVP_sm3(), NULL);
    return (rc == 1 && mdlen == 32) ? 0 : -1;
}

/*
 * SM4-ECB 单块（16B）。
 * 入参：key[16]；block[16]；encrypt 1=加密 0=解密。出参：out[16]。
 * 返回：0 成功，-1 失败。
 */
int gmssl_sm4_ecb_block(const unsigned char *key, const unsigned char *block,
                        int encrypt, unsigned char *out) {
    EVP_CIPHER_CTX *ctx = EVP_CIPHER_CTX_new();
    if (ctx == NULL)
        return -1;
    if (EVP_CipherInit_ex(ctx, EVP_sm4_ecb(), NULL, key, NULL, encrypt) != 1) {
        EVP_CIPHER_CTX_free(ctx);
        return -1;
    }
    int rc = EVP_Cipher(ctx, out, block, 16);
    EVP_CIPHER_CTX_free(ctx);
    /* EVP_Cipher 对块密码返回处理字节数（16 = 成功）。 */
    return rc == 16 ? 0 : -1;
}
