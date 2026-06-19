//! EnerOS 安装器 (eneros-installer)
//!
//! 交互式 CLI 安装器，支持：
//! - 将 EnerOS 镜像安装到目标磁盘（5 分区 A/B 布局）
//! - 生成 PXE 网络启动配置
//!
//! 分区布局（GPT）：
//!   1. EFI    (FAT32, 512M)   — 共享，GRUB 引导
//!   2. RootA  (ext4,  1536M)  — 活跃槽位
//!   3. RootB  (ext4,  1536M)  — OTA 更新目标
//!   4. Data   (ext4,  剩余)   — 共享数据
//!   5. Config (ext4,  256M)   — slot-state.json + eneros-machine.yaml
//!
//! 安装流程：分区 → 格式化 → 写镜像 → 安装 GRUB → 写配置 → 卸载

// 非 Linux 平台上，安装相关函数仅用于测试或被门控，允许 dead_code
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use anyhow::{Context, Result};
use clap::Parser;
use std::fs;
use std::path::Path;

use eneros_os::update::MachineConfig;

#[cfg(target_os = "linux")]
use anyhow::anyhow;
#[cfg(target_os = "linux")]
use eneros_os::update::AbPartition;
#[cfg(target_os = "linux")]
use std::process::Command;

/// EnerOS 安装器
#[derive(Parser, Debug)]
#[command(
    name = "eneros-installer",
    version,
    about = "EnerOS 安装器 — 交互式 CLI + PXE 配置生成"
)]
struct Cli {
    /// 目标磁盘设备（如 /dev/sda）
    #[arg(long)]
    disk: Option<String>,

    /// 镜像文件路径
    #[arg(long)]
    image: Option<String>,

    /// 机器配置文件路径
    #[arg(long)]
    machine_config: Option<String>,

    /// 生成 PXE 配置到指定目录
    #[arg(long)]
    generate_pxe: Option<String>,

    /// 跳过确认提示
    #[arg(long)]
    yes: bool,
}

// ============================================================================
// PXE 配置生成（跨平台，可测试）
// ============================================================================

/// 生成 pxelinux.cfg/default 内容
fn generate_pxe_default_config() -> String {
    let mut s = String::new();
    s.push_str("DEFAULT eneros\n");
    s.push_str("LABEL eneros\n");
    s.push_str("    MENU LABEL EnerOS Install\n");
    s.push_str("    KERNEL eneros/vmlinuz\n");
    s.push_str("    INITRD eneros/initramfs.img\n");
    s.push_str("    APPEND root=/dev/ram0 rw eneros.install=1 console=ttyS0,115200\n");
    s
}

/// 生成 dhcpd.conf 片段
fn generate_dhcpd_fragment() -> String {
    let mut s = String::new();
    s.push_str("# EnerOS PXE boot\n");
    s.push_str("subnet 192.168.1.0 netmask 255.255.255.0 {\n");
    s.push_str("    range 192.168.1.100 192.168.1.200;\n");
    s.push_str("    next-server 192.168.1.1;\n");
    s.push_str("    filename \"pxelinux.0\";\n");
    s.push_str("}\n");
    s
}

/// 生成 PXE 配置到指定目录（跨平台文件操作）
fn cmd_generate_pxe(output_dir: &str) -> Result<()> {
    let base = Path::new(output_dir);

    // 创建目录结构
    let pxelinux_cfg = base.join("pxelinux.cfg");
    let eneros_dir = base.join("eneros");
    fs::create_dir_all(&pxelinux_cfg)
        .with_context(|| format!("创建目录失败: {}", pxelinux_cfg.display()))?;
    fs::create_dir_all(&eneros_dir)
        .with_context(|| format!("创建目录失败: {}", eneros_dir.display()))?;

    // 生成 pxelinux.cfg/default
    let pxe_default_path = pxelinux_cfg.join("default");
    fs::write(&pxe_default_path, generate_pxe_default_config())
        .with_context(|| format!("写入失败: {}", pxe_default_path.display()))?;
    println!("已生成: {}", pxe_default_path.display());

    // 生成 dhcpd.conf.fragment
    let dhcpd_path = base.join("dhcpd.conf.fragment");
    fs::write(&dhcpd_path, generate_dhcpd_fragment())
        .with_context(|| format!("写入失败: {}", dhcpd_path.display()))?;
    println!("已生成: {}", dhcpd_path.display());

    // 复制 vmlinuz + initramfs.img（如果存在）
    for kernel_file in ["vmlinuz", "initramfs.img"] {
        let src = Path::new("/boot").join(kernel_file);
        let dst = eneros_dir.join(kernel_file);
        if src.exists() {
            fs::copy(&src, &dst)
                .with_context(|| format!("复制 {} 失败", kernel_file))?;
            println!("已复制: {} -> {}", src.display(), dst.display());
        } else {
            println!(
                "提示: {} 不存在，请手动复制到 {}",
                src.display(),
                dst.display()
            );
        }
    }

    println!("\nPXE 配置生成完成: {}", base.display());
    println!("将 {} 目录内容部署到 TFTP 服务器根目录", base.display());
    println!("将 dhcpd.conf.fragment 内容合并到 DHCP 服务器配置");

    Ok(())
}

// ============================================================================
// GRUB 配置生成（跨平台，供 cmd_install 使用）
// ============================================================================

/// 生成 GRUB grubenv 文件内容（固定 1024 字节，GRUB 环境块要求）
///
/// 初始状态：next_slot=A, boot_count=0
fn generate_grubenv() -> Vec<u8> {
    let mut content = String::new();
    content.push_str("# GRUB Environment Block\n");
    content.push_str("next_slot=A\n");
    content.push_str("boot_count=0\n");

    // GRUB 环境块必须恰好 1024 字节，用 '#' 填充
    let mut bytes = content.into_bytes();
    while bytes.len() < 1024 {
        bytes.push(b'#');
    }
    // 确保不超过 1024（正常不会，但防御性处理）
    bytes.truncate(1024);
    bytes
}

/// 生成 GRUB grub.cfg 配置内容
///
/// 根据 MachineConfig 的内核启动参数生成 A/B 双槽位启动菜单。
fn generate_grub_cfg(config: &MachineConfig) -> String {
    let kernel_params = config.generate_kernel_cmdline();
    let extra_params = if kernel_params.is_empty() {
        String::new()
    } else {
        format!(" \\\n          {}", kernel_params)
    };

    let mut s = String::new();
    s.push_str("# EnerOS GRUB UEFI configuration\n");
    s.push_str("# 5 分区 A/B 布局：EFI(gpt1) / RootA(gpt2) / RootB(gpt3) / Data(gpt4) / Config(gpt5)\n");
    s.push_str("# 由 eneros-installer 生成\n\n");

    s.push_str("set timeout=3\n");
    s.push_str("set default=0\n\n");

    s.push_str("insmod part_gpt\n");
    s.push_str("insmod ext2\n");
    s.push_str("insmod fat\n");
    s.push_str("insmod search\n");
    s.push_str("insmod search_fs_uuid\n\n");

    s.push_str("set menu_color_normal=white/blue\n");
    s.push_str("set menu_color_highlight=black/light-gray\n\n");

    s.push_str("# 加载 grubenv 环境变量块\n");
    s.push_str("set root=(hd0,gpt1)\n");
    s.push_str("load_env -f /EFI/ENEROS/grubenv\n\n");

    s.push_str("# 根据 next_slot 选择默认启动项\n");
    s.push_str("if [ \"${next_slot}\" = \"B\" ]; then\n");
    s.push_str("    set default=1\n");
    s.push_str("else\n");
    s.push_str("    set default=0\n");
    s.push_str("fi\n\n");

    s.push_str("# boot_count 超过 3 次自动回退\n");
    s.push_str("if [ \"${boot_count}\" != \"\" ]; then\n");
    s.push_str("    if [ \"${boot_count}\" -ge 3 ]; then\n");
    s.push_str("        if [ \"${next_slot}\" = \"A\" ]; then\n");
    s.push_str("            set next_slot=B\n");
    s.push_str("            set default=1\n");
    s.push_str("        else\n");
    s.push_str("            set next_slot=A\n");
    s.push_str("            set default=0\n");
    s.push_str("        fi\n");
    s.push_str("        set boot_count=0\n");
    s.push_str("        save_env -f /EFI/ENEROS/grubenv next_slot boot_count\n");
    s.push_str("    fi\n");
    s.push_str("fi\n\n");

    // Slot A
    s.push_str("# Menu entry 0: Slot A\n");
    s.push_str("menuentry \"EnerOS Power-Native OS (Slot A)\" {\n");
    s.push_str("    set root=(hd0,gpt2)\n");
    s.push_str("    echo \"Loading EnerOS kernel (Slot A)...\"\n");
    s.push_str("    linux /boot/vmlinuz-eneros root=/dev/sda2 ro rootfstype=ext4 \\\n");
    s.push_str(&format!(
        "          console=ttyS0,115200 console=tty0 panic=10{} \\\n",
        extra_params
    ));
    s.push_str("          ENEROS_BOOT_SLOT=A\n");
    s.push_str("    echo \"Loading initramfs...\"\n");
    s.push_str("    initrd /boot/initramfs.img\n");
    s.push_str("}\n\n");

    // Slot B
    s.push_str("# Menu entry 1: Slot B\n");
    s.push_str("menuentry \"EnerOS Power-Native OS (Slot B)\" {\n");
    s.push_str("    set root=(hd0,gpt3)\n");
    s.push_str("    echo \"Loading EnerOS kernel (Slot B)...\"\n");
    s.push_str("    linux /boot/vmlinuz-eneros root=/dev/sda3 ro rootfstype=ext4 \\\n");
    s.push_str(&format!(
        "          console=ttyS0,115200 console=tty0 panic=10{} \\\n",
        extra_params
    ));
    s.push_str("          ENEROS_BOOT_SLOT=B\n");
    s.push_str("    echo \"Loading initramfs...\"\n");
    s.push_str("    initrd /boot/initramfs.img\n");
    s.push_str("}\n\n");

    // Recovery
    s.push_str("# Menu entry 2: Recovery\n");
    s.push_str("menuentry \"EnerOS Power-Native OS (Recovery)\" {\n");
    s.push_str("    set root=(hd0,gpt2)\n");
    s.push_str("    echo \"Loading EnerOS kernel (Recovery)...\"\n");
    s.push_str("    linux /boot/vmlinuz-eneros root=/dev/sda2 ro rootfstype=ext4 \\\n");
    s.push_str("          single console=ttyS0,115200 console=tty0 \\\n");
    s.push_str("          ENEROS_BOOT_SLOT=A\n");
    s.push_str("    echo \"Loading initramfs...\"\n");
    s.push_str("    initrd /boot/initramfs.img\n");
    s.push_str("}\n");

    s
}

// ============================================================================
// 安装逻辑（Linux only）
// ============================================================================

#[cfg(target_os = "linux")]
fn cmd_install(disk: &str, image: &str, machine_config: Option<&str>, yes: bool) -> Result<()> {
    // 加载机器配置（未指定则用默认值）
    let config = match machine_config {
        Some(path) => {
            println!("加载机器配置: {}", path);
            let c = MachineConfig::load_from_yaml(Path::new(path))
                .map_err(|e| anyhow!("加载机器配置失败: {}", e))?;
            c.validate().map_err(|e| anyhow!("机器配置校验失败: {}", e))?;
            println!("机器配置校验通过");
            c
        }
        None => {
            println!("未指定机器配置，使用默认值");
            MachineConfig::default()
        }
    };

    // 1. 列出磁盘信息
    println!("\n=== 磁盘信息 ===");
    let partitions_content = fs::read_to_string("/proc/partitions")
        .context("读取 /proc/partitions 失败")?;
    println!("{}", partitions_content);

    // 尝试 lsblk 获取更详细信息
    let _ = Command::new("lsblk").arg(disk).status();

    // 2. 确认磁盘
    if !yes {
        println!("\n⚠️  即将在 {} 上创建分区并安装 EnerOS！", disk);
        println!("    此操作将清除磁盘上的所有数据！");
        print!("    确认继续？(yes/no): ");
        use std::io::Write;
        std::io::stdout().flush().ok();
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if input.trim() != "yes" {
            println!("安装已取消");
            return Ok(());
        }
    }

    let efi_size = config.partitions.efi_size_mb;
    let root_size = config.partitions.root_size_mb;
    let config_size = config.partitions.config_size_mb;

    // 3. 创建分区（sgdisk）
    println!("\n=== 创建 GPT 分区表 ===");
    run_command(
        "sgdisk",
        &["--zap-all", disk],
        "清除现有分区表",
    )?;

    // Partition 1: EFI (FAT32)
    run_command(
        "sgdisk",
        &[
            &format!("--new=1:0:+{}M", efi_size),
            "--typecode=1:EF00",
            "--change-name=1:EFI System",
            disk,
        ],
        "创建 EFI 分区",
    )?;

    // Partition 2: RootA (ext4)
    run_command(
        "sgdisk",
        &[
            &format!("--new=2:0:+{}M", root_size),
            "--typecode=2:8300",
            "--change-name=2:EnerOS Root A",
            disk,
        ],
        "创建 RootA 分区",
    )?;

    // Partition 3: RootB (ext4)
    run_command(
        "sgdisk",
        &[
            &format!("--new=3:0:+{}M", root_size),
            "--typecode=3:8300",
            "--change-name=3:EnerOS Root B",
            disk,
        ],
        "创建 RootB 分区",
    )?;

    // Partition 5: Config (ext4) — 放在 Data 之前，Data 取剩余空间
    run_command(
        "sgdisk",
        &[
            &format!("--new=5:0:+{}M", config_size),
            "--typecode=5:8300",
            "--change-name=5:EnerOS Config",
            disk,
        ],
        "创建 Config 分区",
    )?;

    // Partition 4: Data (ext4, 剩余空间)
    run_command(
        "sgdisk",
        &[
            "--new=4:0:0",
            "--typecode=4:8300",
            "--change-name=4:EnerOS Data",
            disk,
        ],
        "创建 Data 分区（剩余空间）",
    )?;

    // 刷新分区表
    run_command("partprobe", &[disk], "刷新分区表").ok();

    // 4. 格式化分区
    println!("\n=== 格式化分区 ===");
    let p1 = format!("{}1", disk);
    let p2 = format!("{}2", disk);
    let p3 = format!("{}3", disk);
    let p4 = format!("{}4", disk);
    let p5 = format!("{}5", disk);

    run_command(
        "mkfs.vfat",
        &["-F", "32", "-n", "EFI", &p1],
        "格式化 EFI (FAT32)",
    )?;
    run_command(
        "mkfs.ext4",
        &["-F", "-L", "eneros-root-a", &p2],
        "格式化 RootA (ext4)",
    )?;
    run_command(
        "mkfs.ext4",
        &["-F", "-L", "eneros-root-b", &p3],
        "格式化 RootB (ext4)",
    )?;
    run_command(
        "mkfs.ext4",
        &["-F", "-L", "eneros-data", &p4],
        "格式化 Data (ext4)",
    )?;
    run_command(
        "mkfs.ext4",
        &["-F", "-L", "eneros-config", &p5],
        "格式化 Config (ext4)",
    )?;

    // 5. 挂载 RootA，写入镜像
    println!("\n=== 写入镜像到 RootA ===");
    let mount_root = "/mnt/eneros-root";
    fs::create_dir_all(mount_root)?;
    run_command("mount", &[&p2, mount_root], "挂载 RootA")?;

    // 写入镜像（dd 或解压）
    let image_path = Path::new(image);
    if image_path.extension().and_then(|e| e.to_str()) == Some("img") {
        println!("使用 dd 写入镜像: {}", image);
        run_command(
            "dd",
            &[
                &format!("if={}", image),
                &format!("of={}", mount_root),
                "bs=4M",
                "status=progress",
            ],
            "dd 写入镜像",
        )?;
    } else {
        // 解压 tar.gz 镜像到 RootA
        println!("解压镜像到 RootA: {}", image);
        run_command(
            "tar",
            &["-xzf", image, "-C", mount_root],
            "解压镜像",
        )?;
    }

    // 6. 挂载 EFI，安装 GRUB
    println!("\n=== 安装 GRUB 引导 ===");
    let mount_efi = "/mnt/eneros-efi";
    fs::create_dir_all(mount_efi)?;
    run_command("mount", &[&p1, mount_efi], "挂载 EFI")?;

    // 确定 GRUB 目标架构
    let grub_target = match config.hardware.arch.as_str() {
        "x86_64" => "x86_64-efi",
        "aarch64" => "arm64-efi",
        other => return Err(anyhow!("不支持的架构: {}", other)),
    };

    run_command(
        "grub-install",
        &[
            &format!("--target={}", grub_target),
            &format!("--efi-directory={}", mount_efi),
            "--bootloader-id=ENEROS",
            "--removable",
            &format!("--boot-directory={}/boot", mount_root),
        ],
        "grub-install",
    )?;

    // 创建 EFI/ENEROS 目录并复制 grub.cfg + grubenv
    let efi_eneros = format!("{}/EFI/ENEROS", mount_efi);
    fs::create_dir_all(&efi_eneros)?;

    let grub_cfg_content = generate_grub_cfg(&config);
    let grub_cfg_path = format!("{}/grub.cfg", efi_eneros);
    fs::write(&grub_cfg_path, &grub_cfg_content)?;
    println!("已写入: {}", grub_cfg_path);

    // 同时复制到 RootA /boot/grub/grub.cfg
    let boot_grub = format!("{}/boot/grub", mount_root);
    fs::create_dir_all(&boot_grub)?;
    fs::write(format!("{}/grub.cfg", boot_grub), &grub_cfg_content)?;
    println!("已写入: {}/grub.cfg", boot_grub);

    // 写入 grubenv（1024 字节固定大小）
    let grubenv_path = format!("{}/grubenv", efi_eneros);
    fs::write(&grubenv_path, generate_grubenv())?;
    println!("已写入: {}", grubenv_path);

    // 卸载 EFI
    run_command("umount", &[mount_efi], "卸载 EFI")?;

    // 7. 挂载 Config，写入 slot-state.json + eneros-machine.yaml
    println!("\n=== 写入 Config 分区 ===");
    let mount_config = "/mnt/eneros-config";
    fs::create_dir_all(mount_config)?;
    run_command("mount", &[&p5, mount_config], "挂载 Config")?;

    // 写入 slot-state.json（默认 Slot A 活跃）
    let ab = AbPartition::default();
    let slot_state_path = format!("{}/slot-state.json", mount_config);
    ab.save_to_file(Path::new(&slot_state_path))
        .map_err(|e| anyhow!("写入 slot-state.json 失败: {}", e))?;
    println!("已写入: {}", slot_state_path);

    // 写入 eneros-machine.yaml
    let machine_yaml_path = format!("{}/eneros-machine.yaml", mount_config);
    config
        .save_to_yaml(Path::new(&machine_yaml_path))
        .map_err(|e| anyhow!("写入 eneros-machine.yaml 失败: {}", e))?;
    println!("已写入: {}", machine_yaml_path);

    run_command("umount", &[mount_config], "卸载 Config")?;

    // 8. 挂载 Data，创建 /data/updates/
    println!("\n=== 初始化 Data 分区 ===");
    let mount_data = "/mnt/eneros-data";
    fs::create_dir_all(mount_data)?;
    run_command("mount", &[&p4, mount_data], "挂载 Data")?;

    let updates_dir = format!("{}/updates", mount_data);
    fs::create_dir_all(&updates_dir)?;
    println!("已创建: {}", updates_dir);

    run_command("umount", &[mount_data], "卸载 Data")?;

    // 9. 卸载 RootA
    run_command("umount", &[mount_root], "卸载 RootA")?;

    // 10. 完成
    println!("\n=== 安装完成 ===");
    println!("EnerOS 已成功安装到 {}", disk);
    println!("分区布局：");
    println!("  {}1 — EFI    (FAT32, {}M)", disk, efi_size);
    println!("  {}2 — RootA  (ext4, {}M)  [活跃]", disk, root_size);
    println!("  {}3 — RootB  (ext4, {}M)  [OTA 目标]", disk, root_size);
    println!("  {}4 — Data   (ext4, 剩余)", disk);
    println!("  {}5 — Config (ext4, {}M)", disk, config_size);
    println!("\n请重启系统以启动 EnerOS。");

    Ok(())
}

/// 执行外部命令，失败时返回 anyhow 错误
#[cfg(target_os = "linux")]
fn run_command(program: &str, args: &[&str], description: &str) -> Result<()> {
    print!("  {}... ", description);
    use std::io::Write;
    std::io::stdout().flush().ok();

    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("执行 {} 失败（命令不存在？）", program))?;

    if status.success() {
        println!("完成");
        Ok(())
    } else {
        println!("失败");
        Err(anyhow!(
            "{} 退出码 {}",
            description,
            status.code().unwrap_or(-1)
        ))
    }
}

// ============================================================================
// main（平台隔离）
// ============================================================================

#[cfg(target_os = "linux")]
fn main() -> Result<()> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_target(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    // 分发命令
    if let Some(output_dir) = &cli.generate_pxe {
        return cmd_generate_pxe(output_dir);
    }

    // 安装模式：需要 disk + image
    let disk = cli
        .disk
        .as_deref()
        .ok_or_else(|| anyhow!("安装模式需要 --disk 参数"))?;
    let image = cli
        .image
        .as_deref()
        .ok_or_else(|| anyhow!("安装模式需要 --image 参数"))?;

    cmd_install(disk, image, cli.machine_config.as_deref(), cli.yes)
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("eneros-installer requires Linux");
    std::process::exit(1);
}

// ============================================================================
// 测试（跨平台，测试 PXE/dhcpd/grub 配置生成）
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pxe_config_generation() {
        let config = generate_pxe_default_config();
        assert!(config.contains("DEFAULT eneros"));
        assert!(config.contains("LABEL eneros"));
        assert!(config.contains("MENU LABEL EnerOS Install"));
        assert!(config.contains("KERNEL eneros/vmlinuz"));
        assert!(config.contains("INITRD eneros/initramfs.img"));
        assert!(config.contains("APPEND root=/dev/ram0 rw eneros.install=1 console=ttyS0,115200"));
    }

    #[test]
    fn test_dhcpd_config_generation() {
        let config = generate_dhcpd_fragment();
        assert!(config.contains("# EnerOS PXE boot"));
        assert!(config.contains("subnet 192.168.1.0 netmask 255.255.255.0"));
        assert!(config.contains("range 192.168.1.100 192.168.1.200"));
        assert!(config.contains("next-server 192.168.1.1"));
        assert!(config.contains("filename \"pxelinux.0\""));
    }

    #[test]
    fn test_grubenv_is_1024_bytes() {
        let grubenv = generate_grubenv();
        assert_eq!(grubenv.len(), 1024, "grubenv 必须恰好 1024 字节");
    }

    #[test]
    fn test_grubenv_contains_initial_state() {
        let grubenv = generate_grubenv();
        let content = String::from_utf8_lossy(&grubenv);
        assert!(content.contains("# GRUB Environment Block"));
        assert!(content.contains("next_slot=A"));
        assert!(content.contains("boot_count=0"));
    }

    #[test]
    fn test_grub_cfg_contains_ab_slots() {
        let config = MachineConfig::default();
        let grub_cfg = generate_grub_cfg(&config);
        assert!(grub_cfg.contains("Slot A"));
        assert!(grub_cfg.contains("Slot B"));
        assert!(grub_cfg.contains("ENEROS_BOOT_SLOT=A"));
        assert!(grub_cfg.contains("ENEROS_BOOT_SLOT=B"));
        assert!(grub_cfg.contains("root=/dev/sda2"));
        assert!(grub_cfg.contains("root=/dev/sda3"));
        assert!(grub_cfg.contains("load_env -f /EFI/ENEROS/grubenv"));
    }

    #[test]
    fn test_grub_cfg_with_rt_config() {
        let mut config = MachineConfig::default();
        config.boot.rt_config.enabled = true;
        config.boot.rt_config.isolated_cpus = vec![2, 3];
        config.boot.rt_config.nohz_full = vec![2, 3];
        config.boot.rt_config.rcu_nocbs = vec![2, 3];
        let grub_cfg = generate_grub_cfg(&config);
        assert!(grub_cfg.contains("isolcpus=2,3"));
        assert!(grub_cfg.contains("nohz_full=2,3"));
        assert!(grub_cfg.contains("rcu_nocbs=2,3"));
        assert!(grub_cfg.contains("mlock=1"));
    }

    #[test]
    fn test_cmd_generate_pxe_creates_files() {
        let tmp = std::env::temp_dir().join("eneros-installer-pxe-test");
        let _ = std::fs::remove_dir_all(&tmp);

        cmd_generate_pxe(tmp.to_str().unwrap()).unwrap();

        // 验证目录结构
        assert!(tmp.join("pxelinux.cfg/default").exists());
        assert!(tmp.join("dhcpd.conf.fragment").exists());
        assert!(tmp.join("eneros").exists());

        // 验证文件内容
        let pxe_default = std::fs::read_to_string(tmp.join("pxelinux.cfg/default")).unwrap();
        assert!(pxe_default.contains("DEFAULT eneros"));

        let dhcpd = std::fs::read_to_string(tmp.join("dhcpd.conf.fragment")).unwrap();
        assert!(dhcpd.contains("subnet 192.168.1.0"));

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
