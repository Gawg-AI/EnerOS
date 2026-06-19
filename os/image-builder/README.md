# EnerOS Image Builder

创建 EnerOS Power-Native OS 的可启动 raw 镜像，支持 5 分区 A/B 布局和配置注入。

## Usage

### Build x86_64 image (默认)
```bash
./build.sh
```

### Build ARM64 image
```bash
ARCH=aarch64 ./build.sh
# 或
./build.sh --arch aarch64
```

### 自定义镜像大小
```bash
IMAGE_SIZE=8G ./build.sh
# 或
./build.sh --image-size 8G
```

### 指定机器配置文件
```bash
./build.sh --machine-config /path/to/eneros-machine.yaml
# 或
MACHINE_CONFIG=/path/to/eneros-machine.yaml ./build.sh
```

### 查看帮助
```bash
./build.sh --help
```

## Image Layout — 5 分区 A/B 布局

```
┌──────────────────────────────────────────────────┐
│  GPT Partition Table                              │
├──────────────────────────────────────────────────┤
│  Partition 1: EFI System (FAT32)        512MB    │
│  - /EFI/BOOT/BOOTX64.EFI (或 BOOTAA64.EFI)       │
│  - /EFI/ENEROS/grub.cfg                           │
│  - /EFI/ENEROS/grubenv (A/B 槽位状态)             │
│  - GRUB UEFI bootloader                           │
├──────────────────────────────────────────────────┤
│  Partition 2: EnerOS Root A (ext4)     1536MB    │
│  - Active 槽位（初始启动槽位）                     │
│  - /bin/eneros-init                                │
│  - /bin/eneros-api                                 │
│  - /etc/eneros/ (init.toml, network.toml)        │
│  - /boot/vmlinuz-eneros                            │
│  - /boot/initramfs.img                             │
│  - /lib/modules/                                   │
├──────────────────────────────────────────────────┤
│  Partition 3: EnerOS Root B (ext4)     1536MB    │
│  - Inactive 槽位（初始为空，OTA 更新目标）          │
│  - OTA 更新时写入新 rootfs                         │
├──────────────────────────────────────────────────┤
│  Partition 4: EnerOS Data (ext4)       剩余空间  │
│  - /data/updates/ (OTA 更新包存储)                │
│  - 共享数据（跨 A/B 槽位）                         │
├──────────────────────────────────────────────────┤
│  Partition 5: EnerOS Config (ext4)      256MB    │
│  - /slot-state.json (A/B 槽位状态)                │
│  - /eneros-machine.yaml (机器配置)                │
│  - /keys/ (密钥目录，待生成)                       │
│  - 共享配置（跨 A/B 槽位）                         │
└──────────────────────────────────────────────────┘
```

## A/B OTA 更新流程

EnerOS 采用 A/B 双槽位 OTA 更新机制，确保更新失败可回退：

1. **初始状态**：Slot A = Active + Good，Slot B = Inactive
2. **OTA 下载**：更新包下载到 `/data/updates/`
3. **写入 Inactive 槽位**：将更新写入 Slot B（当前 Inactive 槽位）
4. **切换槽位**：更新 `slot-state.json`，设置 `next_slot=B`，更新 grubenv
5. **重启**：GRUB 根据 `next_slot` 启动 Slot B
6. **启动成功**：eneros-init 标记 Slot B = Good，重置 boot_count
7. **启动失败**：boot_count >= 3 时，GRUB 自动回退到 Slot A

### GRUB 槽位切换逻辑

- `grubenv` 文件存储 `next_slot` 和 `boot_count` 变量
- GRUB 启动时加载 grubenv，根据 `next_slot` 选择默认启动项
- 如果 `boot_count >= 3`，自动切换到另一个槽位（防砖机制）
- eneros-init 启动成功后重置 `boot_count=0` 并标记槽位为 Good

### slot-state.json 格式

```json
{
  "version": 1,
  "next_slot": "A",
  "slots": {
    "A": { "state": "active", "status": "good", "boot_count": 0 },
    "B": { "state": "inactive", "status": "unknown", "boot_count": 0 }
  }
}
```

## 配置注入

build.sh 通过 `inject-config.sh` 将机器配置注入到 rootfs：

1. 读取 `eneros-machine.yaml`（声明式机器配置）
2. 生成 `init.toml`（根据 agents 列表）
3. 生成 `network.toml`（根据 network 配置）
4. 注入到 rootfs 的 `/etc/eneros/` 目录

### 机器配置文件

默认路径：`os/rootfs/files/etc/eneros/eneros-machine.yaml`

可通过 `--machine-config` 参数指定自定义路径。

配置内容包括：
- `hardware` — 硬件规格（架构、CPU、内存、磁盘）
- `partitions` — 分区大小（EFI、Root、Data、Config）
- `network` — 网络配置（hostname、接口）
- `boot` — 启动参数和 RT 配置
- `agents` — Agent 进程配置

## Testing with QEMU

```bash
# With KVM (faster, requires hardware virtualization)
qemu-system-x86_64 -drive file=output/eneros-x86_64.img,format=raw -m 2G -enable-kvm

# Without KVM
qemu-system-x86_64 -drive file=output/eneros-x86_64.img,format=raw -m 2G

# With serial console
qemu-system-x86_64 -drive file=output/eneros-x86_64.img,format=raw -m 2G -nographic

# ARM64 (需要 QEMU EFI 固件)
qemu-system-aarch64 -drive file=output/eneros-aarch64.img,format=raw -m 2G \
    -bios /usr/share/qemu-efi-aarch64/QEMU_EFI.fd -machine virt -cpu cortex-a57
```

## Dependencies

构建脚本需要以下依赖（Linux 构建环境）：

- `sgdisk` (gdisk package) — GPT 分区创建
- `mkfs.vfat` (dosfstools package) — FAT32 格式化
- `mkfs.ext4` (e2fsprogs package) — ext4 格式化
- `grub-install` 和 GRUB EFI binaries — 引导加载器安装
  - x86_64: `grub-efi-amd64-bin`
  - aarch64: `grub-efi-arm64-bin`
- `losetup` (util-linux package) — loop 设备管理
- `numfmt` (coreutils package) — 大小单位转换
- `blockdev` (util-linux package) — 获取设备扇区数

Install on Debian/Ubuntu:
```bash
# x86_64
apt install gdisk dosfstools e2fsprogs grub-efi-amd64-bin util-linux

# aarch64 (交叉构建)
apt install gdisk dosfstools e2fsprogs grub-efi-arm64-bin util-linux
```

## aarch64 支持

- `install-bootloader.sh` 已支持 `arm64-efi` 目标
- 5 分区 A/B 布局与架构无关，x86_64 和 aarch64 使用相同的分区结构
- GRUB 配置（grub.cfg）使用 `(hd0,gptN)` 设备路径，两种架构通用
- aarch64 使用 `BOOTAA64.EFI`，x86_64 使用 `BOOTX64.EFI`

## Files

| 文件 | 说明 |
|------|------|
| `build.sh` | 主构建脚本，协调整个镜像构建流程 |
| `create-partitions.sh` | 分区创建和格式化（5 分区 A/B 布局） |
| `install-bootloader.sh` | GRUB UEFI 引导加载器安装 |
| `inject-config.sh` | 机器配置注入（生成 init.toml 和 network.toml） |
