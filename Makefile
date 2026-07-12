# ============================================================
# EnerOS / Power Native Agent OS — Makefile
# Version: v0.23.0
# 蓝图：phase0.md §v0.4.0
# ============================================================
# 统一构建入口：seL4 kernel + Rust user-space + QEMU 验证
# ============================================================

# ===== 项目配置 =====
PROJECT_NAME := eneros
VERSION := 0.23.0

# ===== 工具链 =====
CARGO := cargo
RUSTC := rustc
GCC := aarch64-linux-gnu-gcc
OBJCOPY := llvm-objcopy
OBJDUMP := llvm-objdump
DTC := dtc
QEMU := qemu-system-aarch64
GDB := gdb-multiarch

# ===== 目标平台 =====
TARGET := aarch64-unknown-none

# ===== seL4 配置 =====
SEL4_VERSION := 14.0.0
SEL4_SRC_DIR := $(CURDIR)/seL4
SEL4_BUILD_DIR := $(CURDIR)/build/sel4
SEL4_PREFIX := $(CURDIR)/build/sel4-prefix
SEL4_KERNEL := $(SEL4_BUILD_DIR)/kernel/kernel.elf
SEL4_IMAGE := $(CURDIR)/build/$(PROJECT_NAME)-$(VERSION).img

# ===== Rust 运行时配置 =====
RUNTIME_CRATE := eneros-runtime
RUNTIME_ELF := $(CURDIR)/target/$(TARGET)/release/$(RUNTIME_CRATE)
RUNTIME_BIN := $(CURDIR)/build/$(RUNTIME_CRATE).bin

# ===== QEMU 配置 =====
QEMU_MACHINE := virt
QEMU_CPU := cortex-a57
QEMU_MEM := 128M
QEMU_DTB := $(CURDIR)/build/qemu-virt.dtb
QEMU_SERIAL := -serial mon:stdio
QEMU_GDB_PORT := 1234

# ===== 设备树配置 =====
DTS_FILE := $(CURDIR)/configs/qemu-virt.dts

# ===== 构建目录 =====
BUILD_DIR := $(CURDIR)/build

# ===== 默认目标 =====
.PHONY: all
all: build

# ============================================================
# 构建目标
# ============================================================

# ===== 全量构建 =====
.PHONY: build
build: dtb sel4-build runtime-build image
	@echo "[BUILD] EnerOS v$(VERSION) build complete."
	@echo "  Image: $(SEL4_IMAGE)"
	@echo "  DTB:   $(QEMU_DTB)"

# ===== 设备树编译 =====
.PHONY: dtb
dtb: $(QEMU_DTB)

$(QEMU_DTB): $(DTS_FILE)
	@echo "[DTB] Compiling device tree..."
	@mkdir -p $(BUILD_DIR)
	$(DTC) -I dts -O dtb -o $(QEMU_DTB) $(DTS_FILE)
	@echo "[DTB] $(QEMU_DTB)"

# ===== seL4 内核构建 =====
.PHONY: sel4-build
sel4-build: $(SEL4_KERNEL)

$(SEL4_KERNEL):
	@echo "[SEL4] Building seL4 kernel v$(SEL4_VERSION)..."
	@mkdir -p $(SEL4_BUILD_DIR)
	@cd $(SEL4_BUILD_DIR) && cmake \
		-DCMAKE_TOOLCHAIN_FILE=../cmake-toolchains/aarch64.cmake \
		-DKernelPlatform=qemu-arm-virt \
		-DKernelARMPlatform=qemu-arm-virt \
		-DKernelVerificationBuild=OFF \
		-DKernelDebugBuild=ON \
		-DKernelMaxNumNodes=2 \
		$(SEL4_SRC_DIR)
	@cd $(SEL4_BUILD_DIR) && ninja kernel.elf
	@echo "[SEL4] $(SEL4_KERNEL)"

# ===== Rust 运行时库构建 =====
.PHONY: runtime-build
runtime-build:
	@echo "[RUST] Building runtime library (no_std + build-std)..."
	$(CARGO) build -p $(RUNTIME_CRATE) --target $(TARGET) -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem
	@echo "[RUST] runtime library built for $(TARGET)"

# ===== board crate 构建 =====
.PHONY: board-build
board-build:
	@echo "[RUST] Building board crate (no_std + build-std)..."
	$(CARGO) build -p eneros-board --target $(TARGET) -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem
	@echo "[RUST] board crate built for $(TARGET)"

# ===== hello 用户态组件构建 =====
.PHONY: hello-build
hello-build:
	@echo "[RUST] Building hello userland component (no_std + build-std)..."
	$(CARGO) build -p eneros-hello --target $(TARGET) -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem
	@echo "[RUST] hello component built for $(TARGET)"

# ===== hal HAL trait 规范构建 =====
.PHONY: hal-build
hal-build:
	@echo "[RUST] Building hal crate (no_std + build-std)..."
	$(CARGO) build -p eneros-hal --target $(TARGET) -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem
	@echo "[RUST] hal crate built for $(TARGET)"

# ===== hal mock 测试 =====
.PHONY: hal-test
hal-test:
	@echo "[TEST] Running eneros-hal mock tests..."
	$(CARGO) test -p eneros-hal --features mock
	@echo "[TEST] eneros-hal mock tests passed."

# ===== mm 内存管理 crate 构建 =====
.PHONY: mm-build
mm-build:
	@echo "[RUST] Building mm crate (no_std + build-std)..."
	$(CARGO) build -p eneros-mm --target $(TARGET) -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem
	@echo "[RUST] mm crate built for $(TARGET)"

# ===== mm 内存管理测试 =====
.PHONY: mm-test
mm-test:
	@echo "[TEST] Running eneros-mm tests..."
	$(CARGO) test -p eneros-mm
	@echo "[TEST] eneros-mm tests passed."

# ===== heap 内核堆分配器构建 =====
.PHONY: heap-build
heap-build:
	@echo "[RUST] Building heap crate (no_std + build-std)..."
	$(CARGO) build -p eneros-heap --target $(TARGET) -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem
	@echo "[RUST] heap crate built for $(TARGET)"

# ===== heap 内核堆分配器测试 =====
.PHONY: heap-test
heap-test:
	@echo "[TEST] Running eneros-heap tests..."
	$(CARGO) test -p eneros-heap
	@echo "[TEST] eneros-heap tests passed."

# ===== user-heap 用户态堆分配器构建 =====
.PHONY: user-heap-build
user-heap-build:
	@echo "[RUST] Building user-heap crate (no_std + build-std)..."
	$(CARGO) build -p eneros-user-heap --target $(TARGET) -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem
	@echo "[RUST] user-heap crate built for $(TARGET)"

# ===== user-heap 用户态堆分配器测试 =====
.PHONY: user-heap-test
user-heap-test:
	@echo "[TEST] Running eneros-user-heap tests..."
	$(CARGO) test -p eneros-user-heap
	@echo "[TEST] eneros-user-heap tests passed."

## time crate
time-build:
	$(CARGO) build -p eneros-time

time-test:
	$(CARGO) test -p eneros-time

## watchdog crate
watchdog-build:
	$(CARGO) build -p eneros-watchdog

watchdog-test:
	$(CARGO) test -p eneros-watchdog

## panic crate
panic-build:
	$(CARGO) build -p eneros-panic

panic-test:
	$(CARGO) test -p eneros-panic

## smp crate
smp-build:
	$(CARGO) build -p eneros-smp

smp-test:
	$(CARGO) test -p eneros-smp

## sched crate
sched-build:
	$(CARGO) build -p eneros-sched

sched-test:
	$(CARGO) test -p eneros-sched

## power crate (v0.17.1 Edge Box 电源管理)
power-build:
	$(CARGO) build -p eneros-power

power-test:
	$(CARGO) test -p eneros-power

## ipc crate (v0.20.0/v0.21.0 IPC 与 SPSC Ring)
ipc-build:
	$(CARGO) build -p eneros-ipc

ipc-test:
	$(CARGO) test -p eneros-ipc

## controlbus crate (v0.22.0 Control Bus + TTL + 双平面)
controlbus-build:
	$(CARGO) build -p eneros-controlbus

controlbus-test:
	$(CARGO) test -p eneros-controlbus

## storage crate (v0.23.0 eMMC/NVMe Block Device)
storage-build:
	$(CARGO) build -p eneros-storage

storage-test:
	$(CARGO) test -p eneros-storage

# ===== 镜像合并 =====
.PHONY: image
image: runtime-build
	@echo "[IMAGE] Merging seL4 + runtime image..."
	@mkdir -p $(BUILD_DIR)
	$(OBJCOPY) -O binary $(RUNTIME_ELF) $(RUNTIME_BIN)
	@echo "[IMAGE] $(SEL4_IMAGE)"

# ============================================================
# 运行与调试
# ============================================================

# ===== QEMU 运行 =====
.PHONY: run
run: build
	@echo "[RUN] Starting QEMU..."
	$(QEMU) \
		-machine $(QEMU_MACHINE) \
		-cpu $(QEMU_CPU) \
		-m $(QEMU_MEM) \
		-smp 2 \
		-kernel $(SEL4_KERNEL) \
		-dtb $(QEMU_DTB) \
		-append "console=ttyAMA0" \
		$(QEMU_SERIAL) \
		-nographic

# ===== QEMU 调试模式（带 GDB server）=====
.PHONY: gdb
gdb: build
	@echo "[GDB] Starting QEMU with GDB server on port $(QEMU_GDB_PORT)..."
	@echo "[GDB] Run 'gdb-multiarch -x .gdbinit' in another terminal."
	$(QEMU) \
		-machine $(QEMU_MACHINE) \
		-cpu $(QEMU_CPU) \
		-m $(QEMU_MEM) \
		-smp 2 \
		-kernel $(SEL4_KERNEL) \
		-dtb $(QEMU_DTB) \
		-append "console=ttyAMA0" \
		$(QEMU_SERIAL) \
		-gdb tcp::$(QEMU_GDB_PORT) \
		-S \
		-nographic

# ============================================================
# 清理
# ============================================================

# ===== 清理构建产物 =====
.PHONY: clean
clean:
	@echo "[CLEAN] Removing build artifacts..."
	rm -rf $(BUILD_DIR)
	$(CARGO) clean
	@echo "[CLEAN] Done."

# ===== 仅清理 seL4 构建 =====
.PHONY: clean-sel4
clean-sel4:
	@echo "[CLEAN] Removing seL4 build..."
	rm -rf $(SEL4_BUILD_DIR)
	@echo "[CLEAN] Done."

# ===== 仅清理 Rust 构建 =====
.PHONY: clean-rust
clean-rust:
	@echo "[CLEAN] Removing Rust build..."
	$(CARGO) clean
	@echo "[CLEAN] Done."

# ============================================================
# 本地 CI 预检
# ============================================================

# ===== 本地质量门禁 =====
.PHONY: ci-local
ci-local:
	@echo "[CI] Running local quality gate..."
	$(CARGO) run -p eneros-ci
	@echo "[CI] Local quality gate passed."

# ============================================================
# 工具与诊断
# ============================================================

# ===== 检查工具链 =====
.PHONY: check-tools
check-tools:
	@echo "[CHECK] Verifying toolchain..."
	@for tool in $(CARGO) $(RUSTC) $(GCC) $(QEMU) $(DTC) cmake ninja $(GDB); do \
		if command -v $$tool >/dev/null 2>&1; then \
			echo "  ✓ $$tool: $$($$tool --version 2>&1 | head -1)"; \
		else \
			echo "  ✗ $$tool: NOT FOUND"; \
		fi; \
	done

# ===== 显示版本信息 =====
.PHONY: version
version:
	@echo "EnerOS / Power Native Agent OS"
	@echo "Version: v$(VERSION)"
	@echo "Target:  $(TARGET)"
	@echo "seL4:    v$(SEL4_VERSION)"
	@echo "Rust:    $$($(RUSTC) --version)"

# ===== 帮助 =====
.PHONY: help
help:
	@echo "EnerOS v$(VERSION) Build System"
	@echo ""
	@echo "Available targets:"
	@echo "  build       - Build seL4 + Rust runtime + image (default)"
	@echo "  board-build - Build board crate (no_std + build-std)"
	@echo "  hello-build - Build hello userland component (no_std + build-std)"
	@echo "  hal-build   - Build hal crate (no_std + build-std)"
	@echo "  hal-test    - Run eneros-hal mock tests"
	@echo "  mm-build    - Build mm crate (no_std + build-std)"
	@echo "  mm-test     - Run eneros-mm tests"
	@echo "  heap-build  - Build heap crate (no_std + build-std)"
	@echo "  heap-test   - Run eneros-heap tests"
	@echo "  user-heap-build - Build user-heap crate (no_std + build-std)"
	@echo "  user-heap-test  - Run eneros-user-heap tests"
	@echo "  dtb         - Compile device tree (DTS -> DTB)"
	@echo "  run         - Build and run in QEMU"
	@echo "  gdb         - Build and run in QEMU with GDB server"
	@echo "  clean       - Remove all build artifacts"
	@echo "  clean-sel4  - Remove seL4 build only"
	@echo "  clean-rust  - Remove Rust build only"
	@echo "  check-tools - Verify toolchain installation"
	@echo "  version     - Print version information"
	@echo "  ci-local    - Run local CI quality gate (fmt/clippy/deny/test)"
	@echo "  smp-build   - Build smp crate"
	@echo "  smp-test    - Run eneros-smp tests"
	@echo "  sched-build - Build sched crate"
	@echo "  sched-test  - Run eneros-sched tests"
	@echo "  power-build - Build power crate (v0.17.1)"
	@echo "  power-test  - Run eneros-power tests"
	@echo "  ipc-build   - Build ipc crate (v0.20.0/v0.21.0)"
	@echo "  ipc-test    - Run eneros-ipc tests"
	@echo "  controlbus-build - Build controlbus crate (v0.22.0)"
	@echo "  controlbus-test  - Run eneros-controlbus tests"
	@echo "  storage-build    - Build storage crate (v0.23.0)"
	@echo "  storage-test     - Run eneros-storage tests"
	@echo "  help        - Show this help message"
