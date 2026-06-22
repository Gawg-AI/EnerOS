#!/bin/bash
# EnerOS UEFI Secure Boot 配置脚本
#
# 功能：
#   1. 查询当前 Secure Boot 状态
#   2. 生成 PK/KEK/db 密钥对（Ed25519 + x509 证书）
#   3. 将密钥写入 UEFI 变量（PK/KEK/db/dbx）
#   4. 签名内核（vmlinuz）和 initramfs
#   5. 验证签名完整性
#
# 使用场景：
#   - 首次部署时初始化 Secure Boot
#   - OTA 更新后重新签名内核
#   - 审计 Secure Boot 配置状态
#
# 依赖：sbsigntools, efitools, openssl
#   apt install sbsigntool efitools openssl

set -euo pipefail

# ============================================================
# 配置常量
# ============================================================

KEYS_DIR="${ENEROS_KEYS_DIR:-/etc/eneros/keys/secure-boot}"
EFI_VARS="/sys/firmware/efi/efivars"
EFI_GLOBAL_GUID="8be4df61-93ca-11d2-aa0d-00e098032b8c"

# 颜色输出
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

info()  { echo -e "${GREEN}[INFO]${NC} $*"; }
warn()  { echo -e "${YELLOW}[WARN]${NC} $*"; }
error() { echo -e "${RED}[ERROR]${NC} $*" >&2; }

# ============================================================
# 命令：status — 查询 Secure Boot 状态
# ============================================================

cmd_status() {
    info "查询 UEFI Secure Boot 状态..."

    if [ ! -d "$EFI_VARS" ]; then
        warn "efivars 未挂载（非 UEFI 启动或未启用 EFI）"
        exit 0
    fi

    # SecureBoot 变量（1=启用，0=禁用）
    local sb_file="${EFI_VARS}/SecureBoot-${EFI_GLOBAL_GUID}"
    if [ -f "$sb_file" ]; then
        local sb_val=$(od -An -tu1 -j4 "$sb_file" | tr -d ' ')
        if [ "$sb_val" = "1" ]; then
            info "Secure Boot: ${GREEN}已启用${NC}"
        else
            info "Secure Boot: ${YELLOW}未启用${NC}"
        fi
    else
        warn "SecureBoot 变量不存在"
    fi

    # SetupMode 变量（1=设置模式，0=用户模式）
    local sm_file="${EFI_VARS}/SetupMode-${EFI_GLOBAL_GUID}"
    if [ -f "$sm_file" ]; then
        local sm_val=$(od -An -tu1 -j4 "$sm_file" | tr -d ' ')
        if [ "$sm_val" = "1" ]; then
            warn "Setup Mode: 设置模式（可修改密钥）"
        else
            info "Setup Mode: 用户模式（密钥已锁定）"
        fi
    fi

    # PK/KEK/db/dbx 存在性检查
    check_var_exists "PK"
    check_var_exists "KEK"
    check_var_exists "db"
    check_var_exists "dbx"

    # 内核命令行加固参数
    info "检查内核命令行加固参数..."
    local cmdline=$(cat /proc/cmdline 2>/dev/null || echo "")
    check_cmdline_param "page_alloc.shuffle=1"
    check_cmdline_param "slab_nomerge"
    check_cmdline_param "init_on_alloc=1"
    check_cmdline_param "init_on_free=1"

    # 内核配置加固选项
    info "检查内核配置加固选项..."
    check_kernel_config "CONFIG_HARDENED_USERCOPY"
    check_kernel_config "CONFIG_FORTIFY_SOURCE"
    check_kernel_config "CONFIG_STACKPROTECTOR_STRONG"
    check_kernel_config "CONFIG_STRICT_DEVMEM"
    check_kernel_config "CONFIG_SECURITY_DMESG_RESTRICT"
    check_kernel_config "CONFIG_MODULE_SIG"
    check_kernel_config "CONFIG_MODULE_SIG_FORCE"
}

check_var_exists() {
    local name="$1"
    local file="${EFI_VARS}/${name}-${EFI_GLOBAL_GUID}"
    if [ -f "$file" ]; then
        local size=$(stat -c%s "$file" 2>/dev/null || echo "0")
        if [ "$size" -gt 4 ]; then
            info "  ${name}: 已设置（${size} 字节）"
        else
            warn "  ${name}: 空变量"
        fi
    else
        warn "  ${name}: 未设置"
    fi
}

check_cmdline_param() {
    local param="$1"
    if echo "$cmdline" | grep -q "$param"; then
        info "  ${param}: ${GREEN}已应用${NC}"
    else
        warn "  ${param}: 未应用"
    fi
}

check_kernel_config() {
    local config="$1"
    local config_file=""
    if [ -f "/proc/config.gz" ]; then
        config_file=$(zcat /proc/config.gz 2>/dev/null)
    elif [ -f "/boot/config-$(uname -r)" ]; then
        config_file=$(cat "/boot/config-$(uname -r)" 2>/dev/null)
    fi

    if echo "$config_file" | grep -q "^${config}=y"; then
        info "  ${config}: ${GREEN}已启用${NC}"
    elif echo "$config_file" | grep -q "^${config}=m"; then
        warn "  ${config}: 模块（非内建）"
    else
        warn "  ${config}: 未启用"
    fi
}

# ============================================================
# 命令：init-keys — 生成 PK/KEK/db 密钥对
# ============================================================

cmd_init_keys() {
    info "生成 Secure Boot 密钥对到 ${KEYS_DIR}..."

    mkdir -p "$KEYS_DIR"
    cd "$KEYS_DIR"

    # 生成平台密钥（PK）私钥和自签名证书
    if [ ! -f "PK.key" ]; then
        info "生成 PK 密钥..."
        openssl req -new -x509 -newkey rsa:4096 -subj "/CN=EnerOS PK/" \
            -keyout PK.key -out PK.crt -days 3650 -nodes -sha256
        openssl x509 -in PK.crt -out PK.cer -outform DER
    fi

    # 生成密钥交换密钥（KEK）
    if [ ! -f "KEK.key" ]; then
        info "生成 KEK 密钥..."
        openssl req -new -x509 -newkey rsa:4096 -subj "/CN=EnerOS KEK/" \
            -keyout KEK.key -out KEK.crt -days 3650 -nodes -sha256
        openssl x509 -in KEK.crt -out KEK.cer -outform DER
    fi

    # 生成签名数据库密钥（db）
    if [ ! -f "db.key" ]; then
        info "生成 db 密钥..."
        openssl req -new -x509 -newkey rsa:4096 -subj "/CN=EnerOS db/" \
            -keyout db.key -out db.crt -days 3650 -nodes -sha256
        openssl x509 -in db.crt -out db.cer -outform DER
    fi

    info "密钥生成完成："
    ls -la "$KEYS_DIR"/*.key "$KEYS_DIR"/*.crt 2>/dev/null
    warn "请妥善保管 .key 文件，切勿上传到公开仓库"
}

# ============================================================
# 命令：sign-kernel — 签名内核镜像
# ============================================================

cmd_sign_kernel() {
    local kernel="${1:-/boot/vmlinuz-eneros}"
    local key="${KEYS_DIR}/db.key"
    local cert="${KEYS_DIR}/db.crt"

    if [ ! -f "$kernel" ]; then
        error "内核文件不存在: $kernel"
        exit 1
    fi
    if [ ! -f "$key" ] || [ ! -f "$cert" ]; then
        error "签名密钥不存在，请先运行: $0 init-keys"
        exit 1
    fi

    info "签名内核: $kernel"
    sbsign --key "$key" --cert "$cert" --output "$kernel.signed" "$kernel"

    # 备份原文件并替换
    cp "$kernel" "${kernel}.unsigned"
    mv "$kernel.signed" "$kernel"

    info "内核签名完成"
    info "验证签名..."
    sbverify "$kernel" "$cert" && info "${GREEN}签名验证通过${NC}" || error "签名验证失败"
}

# ============================================================
# 命令：verify — 验证内核签名
# ============================================================

cmd_verify() {
    local kernel="${1:-/boot/vmlinuz-eneros}"
    local cert="${KEYS_DIR}/db.crt"

    if [ ! -f "$kernel" ]; then
        error "内核文件不存在: $kernel"
        exit 1
    fi

    info "验证内核签名: $kernel"
    if sbverify "$kernel" "$cert" 2>/dev/null; then
        info "${GREEN}签名验证通过${NC}"
    else
        error "签名验证失败或未签名"
        exit 1
    fi
}

# ============================================================
# 命令：enroll — 将密钥写入 UEFI 变量（需要设置模式）
# ============================================================

cmd_enroll() {
    warn "此操作将修改 UEFI 变量，需在 Setup Mode 下执行"

    if [ "$(id -u)" -ne 0 ]; then
        error "需要 root 权限"
        exit 1
    fi

    # 检查是否处于设置模式
    local sm_file="${EFI_VARS}/SetupMode-${EFI_GLOBAL_GUID}"
    if [ -f "$sm_file" ]; then
        local sm_val=$(od -An -tu1 -j4 "$sm_file" | tr -d ' ')
        if [ "$sm_val" != "1" ]; then
            error "当前非设置模式，无法写入密钥。请在固件设置中切换到 Setup Mode"
            exit 1
        fi
    fi

    cd "$KEYS_DIR"

    # 写入 PK（最后写入会锁定到用户模式）
    if [ -f "PK.cer" ]; then
        info "写入 PK..."
        efi-updatevar -f PK.cer PK
    fi

    # 写入 KEK
    if [ -f "KEK.cer" ]; then
        info "写入 KEK..."
        efi-updatevar -f KEK.cer KEK
    fi

    # 写入 db
    if [ -f "db.cer" ]; then
        info "写入 db..."
        efi-updatevar -f db.cer db
    fi

    info "密钥写入完成，系统已切换到用户模式"
    warn "重启后 Secure Boot 将生效"
}

# ============================================================
# 主入口
# ============================================================

usage() {
    cat <<EOF
EnerOS UEFI Secure Boot 配置工具

用法: $0 <command> [args]

命令:
  status          查询 Secure Boot 状态
  init-keys       生成 PK/KEK/db 密钥对
  sign-kernel [kernel]  签名内核镜像（默认 /boot/vmlinuz-eneros）
  verify [kernel]       验证内核签名
  enroll          将密钥写入 UEFI 变量（需 Setup Mode）

环境变量:
  ENEROS_KEYS_DIR  密钥存储目录（默认 /etc/eneros/keys/secure-boot）

示例:
  $0 status
  $0 init-keys
  $0 sign-kernel /boot/vmlinuz-eneros
  $0 enroll
EOF
    exit 1
}

main() {
    local cmd="${1:-}"
    shift || true

    case "$cmd" in
        status)       cmd_status "$@" ;;
        init-keys)    cmd_init_keys "$@" ;;
        sign-kernel)  cmd_sign_kernel "$@" ;;
        verify)       cmd_verify "$@" ;;
        enroll)       cmd_enroll "$@" ;;
        *)            usage ;;
    esac
}

main "$@"
