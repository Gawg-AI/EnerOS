#!/bin/bash
# EnerOS 分区创建脚本
# 创建 GPT 分区表，5 分区 A/B 布局：EFI / RootA / RootB / Data / Config
# 此文件由 build.sh source

create_partitions() {
    local image="$1"
    local efi_size="$2"     # 如 "512M"
    local root_size="$3"    # 如 "1536M"（每个 Root 分区大小）
    local config_size="$4"  # 如 "256M"

    echo "  创建 GPT 分区表（5 分区 A/B 布局）..."

    # 转换大小为扇区数（每扇区 512 字节）
    local efi_sectors root_sectors config_sectors
    efi_sectors=$(echo "$efi_size" | numfmt --from=iec | awk '{print int($1/512)}')
    root_sectors=$(echo "$root_size" | numfmt --from=iec | awk '{print int($1/512)}')
    config_sectors=$(echo "$config_size" | numfmt --from=iec | awk '{print int($1/512)}')

    # 获取镜像总扇区数
    local total_sectors
    total_sectors=$(blockdev --getsz "$image" 2>/dev/null || \
                    stat -c %s "$image" | awk '{print int($1/512)}')

    # 计算各分区起始和结束扇区（1MB 对齐，起始扇区 2048）
    local p1_start=2048
    local p1_end=$((p1_start + efi_sectors - 1))

    local p2_start=$((p1_end + 1))
    local p2_end=$((p2_start + root_sectors - 1))

    local p3_start=$((p2_end + 1))
    local p3_end=$((p3_start + root_sectors - 1))

    # Config 分区放在磁盘末尾（留 34 扇区给 GPT 备份表）
    local p5_end=$((total_sectors - 34))
    local p5_start=$((p5_end - config_sectors + 1))

    # Data 分区填充 RootB 和 Config 之间的空间
    local p4_start=$((p3_end + 1))
    local p4_end=$((p5_start - 1))

    # 清除现有分区表
    sgdisk --zap-all "$image" 2>/dev/null || true

    # Partition 1: EFI System Partition (FAT32)
    sgdisk --new=1:${p1_start}:${p1_end} \
           --typecode=1:EF00 \
           --change-name=1:"EFI System" \
           "$image"

    # Partition 2: RootA (ext4, Active 槽位)
    sgdisk --new=2:${p2_start}:${p2_end} \
           --typecode=2:8300 \
           --change-name=2:"EnerOS Root A" \
           "$image"

    # Partition 3: RootB (ext4, Inactive 槽位，OTA 更新目标)
    sgdisk --new=3:${p3_start}:${p3_end} \
           --typecode=3:8300 \
           --change-name=3:"EnerOS Root B" \
           "$image"

    # Partition 4: Data (ext4, 共享数据分区)
    sgdisk --new=4:${p4_start}:${p4_end} \
           --typecode=4:8300 \
           --change-name=4:"EnerOS Data" \
           "$image"

    # Partition 5: Config (ext4, 共享配置分区：slot-state + machine.yaml + keys/)
    sgdisk --new=5:${p5_start}:${p5_end} \
           --typecode=5:8300 \
           --change-name=5:"EnerOS Config" \
           "$image"

    # 打印分区表
    sgdisk -p "$image"

    echo "  分区已创建（格式化在 loop 挂载后进行）"
}

# 格式化分区（在 loop 挂载后调用）
format_partitions() {
    local efi_part="$1"
    local roota_part="$2"
    local rootb_part="$3"
    local data_part="$4"
    local config_part="$5"

    echo "  格式化 EFI 分区 (FAT32)..."
    mkfs.vfat -F 32 -n EFI "$efi_part"

    echo "  格式化 RootA 分区 (ext4)..."
    mkfs.ext4 -F -L eneros-root-a "$roota_part"

    echo "  格式化 RootB 分区 (ext4)..."
    mkfs.ext4 -F -L eneros-root-b "$rootb_part"

    echo "  格式化 Data 分区 (ext4)..."
    mkfs.ext4 -F -L eneros-data "$data_part"

    echo "  格式化 Config 分区 (ext4)..."
    mkfs.ext4 -F -L eneros-config "$config_part"
}
