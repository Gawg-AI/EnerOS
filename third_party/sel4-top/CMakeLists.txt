# EnerOS-seL4 顶层构建：内核 + libsel4 + elfloader + root task（qemu-arm-virt/GICv3）
# 用法（见 third_party/sel4-build.sh）：
#   cmake -G Ninja -DCMAKE_TOOLCHAIN_FILE=<sel4>/gcc.cmake \
#     -DCROSS_COMPILER_PREFIX=aarch64-none-elf- \
#     -DKernelArch=arm -DKernelSel4Arch=aarch64 -DKernelPlatform=qemu-arm-virt \
#     -DKernelSel4ArchAarch64=ON -DARM_CPU=cortex-a57 -DQEMU_GIC_VERSION=3 \
#     -DSEL4_SRC=<sel4> -DELFLOADER_SRC=<elfloader> -DDTC=<dtc> \
#     -DQEMU_BINARY=<qemu> -B <build> third_party/sel4-top
cmake_minimum_required(VERSION 3.16.0)
project(EnerOS-seL4 C ASM)

# ---- seL4 内核（kernel.elf + kernel_Config/sel4_Config）----
set(SEL4_SRC "${SEL4_SRC}" CACHE PATH "seL4 kernel source")
add_subdirectory(${SEL4_SRC} kernel)

# ---- libsel4（生成 sel4_autoconf + sel4 target + 用户态桩）----
add_subdirectory(${SEL4_SRC}/libsel4 libsel4)

# ---- elfloader（加载内核 + rootserver cpio；依赖 sel4_autoconf）----
set(ELFLOADER_SRC "${ELFLOADER_SRC}" CACHE PATH "elfloader source")
add_subdirectory(${ELFLOADER_SRC} elfloader)
# libcpio（util_libs 的 cpio 库，本环境手工构建为静态库）
target_link_directories(elfloader PRIVATE ${CMAKE_BINARY_DIR}/elfloader)
target_link_libraries(elfloader PRIVATE ${CMAKE_BINARY_DIR}/elfloader/libcpio.a)

# ---- root task（无 libc，链接 libsel4 的 sel4 target）----
#
# 两条路径，由 RUST_ROOT_TASK 选项选择：
#  - OFF（默认）：C root task（root_task.c），含 G2 闭环 + G3 virtio I/O。
#  - ON：预编译 Rust ELF（eneros-hello），证明 Rust→seL4 FFI 端到端通（Track A2）。
#        不含 G3 virtio；用于 FFI 验证，不替代 C root task 的产品地位。
option(RUST_ROOT_TASK "Use pre-built Rust ELF as root task (Track A2 FFI proof)" OFF)

if(RUST_ROOT_TASK)
  # 使用预编译的 Rust ELF——cargo build -p eneros-hello --target aarch64-unknown-none
  # 在 sel4-build-rust.sh 中完成。CMake 只负责把它复制到 elfloader 期望的位置。
  set(RUST_ROOT_ELF "" CACHE PATH "Path to pre-built Rust root task ELF (eneros-hello)")
  if(NOT RUST_ROOT_ELF)
    message(FATAL_ERROR "RUST_ROOT_TASK=ON requires -DRUST_ROOT_ELF=<path-to-eneros-hello-elf>")
  endif()
  add_custom_target(eneros-root ALL
    COMMAND ${CMAKE_COMMAND} -E copy_if_different
            ${RUST_ROOT_ELF} ${CMAKE_BINARY_DIR}/eneros-root
    BYPRODUCTS ${CMAKE_BINARY_DIR}/eneros-root
    DEPENDS ${RUST_ROOT_ELF}
    COMMENT "Using Rust root task: ${RUST_ROOT_ELF}"
  )
else()
  add_executable(eneros-root
    ${CMAKE_CURRENT_LIST_DIR}/../sel4-root/root_task.c
  )
  target_compile_options(eneros-root PRIVATE -ffreestanding -fno-builtin -O2)
  target_include_directories(eneros-root PRIVATE
    ${CMAKE_BINARY_DIR}/kernel/gen_headers
    ${CMAKE_BINARY_DIR}/kernel/gen_config
    ${CMAKE_BINARY_DIR}/libsel4/gen_config
    ${CMAKE_BINARY_DIR}/libsel4/autoconf
    ${CMAKE_BINARY_DIR}/kernel/autoconf
    ${CMAKE_BINARY_DIR}/libsel4/include
    ${SEL4_SRC}/libsel4/include
    ${SEL4_SRC}/libsel4/arch_include/arm
    ${SEL4_SRC}/libsel4/sel4_arch_include/aarch64
    ${SEL4_SRC}/libsel4/sel4_plat_include/qemu-arm-virt
    ${SEL4_SRC}/libsel4/mode_include/64
  )
  target_link_libraries(eneros-root PRIVATE sel4 gcc)
  target_link_options(eneros-root PRIVATE -nostdlib -static -Wl,-u_sel4_start -Wl,-e_start)
endif()

include(rootserver)
DeclareRootserver(eneros-root)
