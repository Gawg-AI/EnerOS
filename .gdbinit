# EnerOS / Power Native Agent OS — GDB Configuration
# Version: v0.1.0
# 用法：gdb-multiarch -x .gdbinit

# ===== 目标架构 =====
set architecture aarch64

# ===== 连接 QEMU GDB server =====
target remote :1234

# ===== 调试符号加载 =====
symbol-file target/aarch64-unknown-none/release/eneros-runtime

# ===== 断点设置 =====
# 在 EnerOS 根任务入口设置断点
break eneros_runtime::_start

# ===== 调试选项 =====
set print pretty on
set print array on
set print union on
set disassembly-flavor att
set output-radix 16

# ===== 自动加载 =====
set auto-load safe-path /

# ===== 启动提示 =====
echo \n=== EnerOS v0.1.0 GDB Debug Session ===\n
echo Connected to QEMU on port 1234\n
echo Breakpoint set at eneros_runtime::_start\n
echo Type 'continue' to start execution.\n\n
