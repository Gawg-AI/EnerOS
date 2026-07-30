# BSP 认证规范 (v0.183.0)

## 概述

EnerOS BSP 认证体系为硬件平台提供标准化认证流程，确保 BSP（Board Support Package）
满足 EnerOS 运行的最低硬件抽象要求。

## 认证类别

| 类别     | 说明           | 默认强制 |
| -------- | -------------- | -------- |
| Cpu      | 中央处理器     | 是       |
| Mem      | 内存管理       | 是       |
| Clock    | 时钟源         | 是       |
| Irq      | 中断控制器     | 是       |
| Serial   | 串口通信       | 是       |
| Net      | 网络接口       | 是       |
| Storage  | 存储设备       | 是       |
| Crypto   | 硬件加密       | 是       |
| Power    | 电源管理       | 是       |

## 认证流程

1. **提交 (Submitted)** — 厂商提交硬件信息（vendor / model / cpu_arch）
2. **测试 (Testing)** — 系统自动执行 BSP 测试套件
3. **报告 (Report)** — 生成测试报告（通过/失败/跳过）
4. **认证 (Certified)** — 所有强制项通过，签发证书
5. **拒绝 (Rejected)** — 存在强制项失败，认证不通过

## 测试套件

- 每个认证项对应一个测试用例
- 测试报告包含：总用例数、通过数、失败数、跳过数
- 每个用例记录执行耗时（毫秒）

## 证书

认证通过后签发证书，包含：
- 证书编号 (cert_id)
- 硬件厂商 / 型号
- 规范版本
- 签发时间 / 过期时间

## API 概览

```rust
// 获取默认规范
let spec = eneros_bsp_cert::default_spec();

// 校验规范
eneros_bsp_cert::validate_spec(&spec)?;

// 执行测试
let mut kit = eneros_bsp_cert::MockTestKit::new(true);
let report = eneros_bsp_cert::run_tests(&mut kit, &spec);

// 完整认证流程
let mut engine = eneros_bsp_cert::BspCertEngine::new(true);
let case_id = engine.submit(hw_info)?;
let report = engine.run_tests(case_id)?;
let cert = engine.certify(case_id, &report)?;
```
