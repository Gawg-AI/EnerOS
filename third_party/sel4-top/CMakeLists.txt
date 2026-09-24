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
# elfloader 源（master 版 common.c）引用 <cpio/cpio.h>：提供 util_libs 头文件路径。
set(UTIL_LIBS_INCLUDE "" CACHE PATH "util_libs libcpio include dir")
if(NOT "${UTIL_LIBS_INCLUDE}" STREQUAL "")
  target_include_directories(elfloader PRIVATE ${UTIL_LIBS_INCLUDE})
endif()

# ---- root task（无 libc，链接 libsel4 的 sel4 target）----
#
# 两条路径，由 RUST_ROOT_TASK 选项选择（WP-74.2.7 定位订正，原注释与
# ADR-0023 §4 及 CI 现实颠倒、「eneros-hello」系陈旧名）：
#  - ON（产品主线）：预编译 Rust ELF（eneros-root-task，由
#    sel4-build-rust.sh 构建）——gkernel 全链（CI）与全部生产证据
#    （xtask qemu-prod）依赖此路径（ADR-0023 §4 载明）。
#  - OFF：C root task（root_task.c），含 G2 闭环 + G3 virtio I/O——
#    **证据位**（CI g0 job 5 项 eneros.sel4.v1 词形），非产品路径；
#    KernelRootCNodeSizeBits 停在 13（REG-74-A-SIZEBITS 裁决项）。
#    与 R15-G 的「删 C 路径」误读划清界限：root_task.c 不删。
option(RUST_ROOT_TASK "Use pre-built Rust ELF as root task (Track A2 FFI proof)" OFF)

if(RUST_ROOT_TASK)
  # 使用预编译的 Rust ELF（eneros-root-task）——cargo build 在
  # sel4-build-rust.sh 中完成。CMake 只负责把它复制到 elfloader 期望的
  # 位置。
  # 注意：不能经 add_custom_target(eneros-root) + 同路径 custom_command
  # （ninja 同名输出冲突：phony target 与 OUTPUT 均叫 eneros-root），
  # 也不能用 DeclareRootserver（其 ROOTSERVER_IMAGE 推导依赖
  # OUTPUT_NAME/BINARY_DIR，对 custom target 指向 eneros-root 文件本身）。
  # 因此本分支手工构造 rootserver_image target 并显式设置
  # ROOTSERVER_IMAGE 属性（elfloader-tool 经该属性读取 rootserver）。
  set(RUST_ROOT_ELF "" CACHE PATH "Path to pre-built Rust root task ELF (eneros-root-task)")
  if(NOT RUST_ROOT_ELF)
    message(FATAL_ERROR "RUST_ROOT_TASK=ON requires -DRUST_ROOT_ELF=<path-to-eneros-root-task-elf>")
  endif()
  set(ROOTSERVER_IMAGE "${CMAKE_BINARY_DIR}/rootserver.elf")
  add_custom_command(
    OUTPUT "${ROOTSERVER_IMAGE}"
    COMMAND ${CMAKE_COMMAND} -E copy_if_different
            ${RUST_ROOT_ELF} "${ROOTSERVER_IMAGE}"
    DEPENDS ${RUST_ROOT_ELF}
    COMMENT "Using Rust root task: ${RUST_ROOT_ELF}"
  )
  add_custom_target(eneros-root ALL DEPENDS "${ROOTSERVER_IMAGE}")
  # elfloader 需要 rootserver_image target + ROOTSERVER_IMAGE 属性。
  add_custom_target(rootserver_image ALL DEPENDS elfloader "${ROOTSERVER_IMAGE}")
  set_property(TARGET rootserver_image PROPERTY ROOTSERVER_IMAGE "${ROOTSERVER_IMAGE}")
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

  include(rootserver)
  DeclareRootserver(eneros-root)
endif()
