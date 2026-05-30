# Implementation Plan: 目标偏差优化（target-deviation-optimization）

## Overview

将 CodeGenie 模拟退火优化器的评分机制从"绝对值最小化"改造为"目标偏差导向"。
实现顺序：config.rs 新增数据结构 → context.rs 新增字段 → main.rs 跳过 calibrate 逻辑
→ evaluator.rs 修改评分公式和硬约束 → 更新 config.toml.example → 编写测试。

## Tasks

- [x] 1. 在 `config.rs` 中新增配置数据结构
  - [x] 1.1 新增 `ScaleConfigToml` 结构体（所有字段为 `Option<f64>`）
    - 在 `config.rs` 中新增 `ScaleConfigToml` 结构体，包含 10 个 `Option<f64>` 字段：
      `collision_count`、`collision_rate`、`equivalence`、`equiv_cv`、`distribution`、
      `simple_freq`、`simple_equiv`、`simple_dist`、`simple_collision_count`、`simple_collision_rate`
    - 派生 `Debug, Clone, Deserialize, Default`
    - _Requirements: 1.1_

  - [x] 1.2 新增 `FullCodeTargets` 结构体及其 `Default` 实现
    - 新增 `FullCodeTargets` 结构体，包含：`enabled: bool`、5 个目标值字段（`f64`）、
      `low_weight: f64`、5 个 `_max` 字段（`f64`）
    - 手动实现 `Default`，将 `enabled` 设为 `false`，`low_weight` 设为 `0.01`，其余字段为 `0.0`
    - 派生 `Debug, Clone, Deserialize`
    - _Requirements: 2.1, 3.1_

  - [x] 1.3 新增 `SimpleCodeTargets` 结构体及其 `Default` 实现
    - 新增 `SimpleCodeTargets` 结构体，包含：`enabled: bool`、5 个目标值字段（`f64`，含 `freq`）、
      `low_weight: f64`、5 个 `_max` 字段（`f64`，含 `freq_max`）
    - 手动实现 `Default`，将 `enabled` 设为 `false`，`low_weight` 设为 `0.01`，其余字段为 `0.0`
    - 派生 `Debug, Clone, Deserialize`
    - _Requirements: 2.2, 3.2_

  - [x] 1.4 新增 `TargetsConfig` 结构体，修改 `Config` 结构体，新增辅助方法
    - 新增 `TargetsConfig { full_code: FullCodeTargets, simple_code: SimpleCodeTargets }`，
      派生 `Debug, Clone, Deserialize, Default`
    - 在 `Config` 结构体中新增两个可选字段：
      `pub scale: Option<ScaleConfigToml>` 和 `pub targets: Option<TargetsConfig>`
    - 在 `impl Config` 中新增 `get_targets_config(&self) -> TargetsConfig` 方法，
      缺失时返回 `TargetsConfig::default()`
    - _Requirements: 1.1, 2.1, 2.2_

- [x] 2. 在 `context.rs` 中为 `OptContext` 新增 `targets_config` 字段
  - [x] 2.1 修改 `OptContext` 结构体和 `new()` 函数签名
    - 在 `OptContext` 结构体中新增 `pub targets_config: TargetsConfig` 字段
    - 在 `use crate::config::...` 导入中新增 `TargetsConfig`
    - 修改 `OptContext::new()` 签名，在末尾新增参数 `targets_config: TargetsConfig`
    - 在 `Self { ... }` 初始化块中新增 `targets_config` 字段赋值
    - _Requirements: 6.6_

  - [x] 2.2 更新所有 `OptContext::new()` 调用点
    - 在 `main.rs` 的 `run_evaluate()` 中，`OptContext::new()` 调用末尾传入 `TargetsConfig::default()`
    - 在 `main.rs` 的 `run_optimize()` 中，临时 `temp_ctx` 的 `OptContext::new()` 调用末尾传入 `TargetsConfig::default()`
    - 正式 `ctx` 的 `OptContext::new()` 调用末尾传入 `targets_config`（后续步骤中完善）
    - _Requirements: 6.6_

- [x] 3. 在 `main.rs` 中实现 calibrate 跳过逻辑
  - [x] 3.1 新增 `resolve_scale_config` 辅助函数
    - 在 `main.rs` 中新增 `fn resolve_scale_config(cfg: &Config, calibrated: ScaleConfig) -> (ScaleConfig, &'static str)` 函数
    - 逻辑：`cfg.scale` 为 `None` 时返回 `(calibrated, "自动校准")`；
      所有 10 个字段均为 `Some` 时直接构建 `ScaleConfig` 并返回 `(sc, "手动配置")`；
      部分字段为 `Some` 时先用 calibrated 值，再用 `Some` 字段覆盖，返回 `(sc, "部分手动覆盖")`
    - _Requirements: 1.3, 1.4_

  - [x] 3.2 修改 `run_optimize()` 中的校准阶段，支持跳过 calibrate
    - 在校准阶段开始前，计算 `all_manual`（判断 `cfg.scale` 的所有 10 个字段是否均为 `Some`）
    - 若 `all_manual` 为 `true`：跳过构建 `temp_ctx`、`smart_init`、`Evaluator::new`、`calibrate_scales` 等步骤，
      直接调用 `resolve_scale_config(cfg, ScaleConfig::default())` 获得 `scale_config`
    - 若 `all_manual` 为 `false`：保留现有 calibrate 流程，最后调用 `resolve_scale_config(cfg, calibrated)` 合并结果
    - 在日志中打印 `scale_source`（"手动配置"/"自动校准"/"部分手动覆盖"）及各字段最终值
    - 在构建正式 `ctx` 时，调用 `cfg.get_targets_config()` 获取 `targets_config`，
      并将其传入 `OptContext::new()`
    - _Requirements: 1.3, 1.4, 1.5_

- [x] 4. 在 `evaluator.rs` 中修改全码和简码评分公式
  - [x] 4.1 修改 `Evaluator::compute_full_score` 为目标偏差导向
    - 在函数开头检查 `ctx.targets_config.full_code.enabled`
    - 若 `enabled = true`：按目标偏差公式计算每个全码指标的得分分量：
      `d = (v_i - target_i).max(0.0) * scale_i; score_i = w_i * (d + d*d + lw * (v_i * scale_i))`
      覆盖 5 个指标：`collision_count`、`collision_rate`、`equivalence`、`equiv_cv`、`distribution`
    - 若 `enabled = false`：保留原有线性公式（`score_i = w_i * v_i * scale_i`），不做任何改动
    - _Requirements: 4.1, 4.2, 4.3, 4.4, 4.5_

  - [x] 4.2 修改 `SimpleEvaluator::compute_simple_score` 为目标偏差导向
    - 在函数开头检查 `ctx.targets_config.simple_code.enabled`
    - 若 `enabled = true`：按目标偏差公式计算每个简码指标的得分分量，
      频率覆盖指标特殊处理：`v = 1.0 - sm.weighted_freq_coverage`，`target_v = 1.0 - t.freq`
    - 若 `enabled = false`：保留原有线性公式，不做任何改动
    - _Requirements: 5.1, 5.2, 5.3, 5.4_

- [x] 5. 在 `evaluator.rs` 中实现 `_max` 硬约束检查
  - [x] 5.1 新增 `check_full_code_max` 内联方法
    - 在 `impl Evaluator` 中新增 `#[inline(always)] fn check_full_code_max(&self, ctx: &OptContext) -> bool`
    - 依次检查 5 个全码 `_max` 字段（`collision_count_max`、`collision_rate_max`、
      `equivalence_max`、`equiv_cv_max`、`distribution_max`）
    - 每个字段：若 `_max > 0.0` 且当前值超过 `_max`，返回 `false`；否则继续
    - 全部通过则返回 `true`
    - _Requirements: 6.1, 6.2, 3.3_

  - [x] 5.2 新增 `check_simple_code_max` 内联方法
    - 在 `impl Evaluator` 中新增 `#[inline(always)] fn check_simple_code_max(&self, ctx: &OptContext) -> bool`
    - 若 `self.simple_eval` 为 `None`，直接返回 `true`
    - 否则从 `SimpleEvaluator` 读取当前简码指标，依次检查 5 个简码 `_max` 字段
    - `freq_max` 特殊处理：当 `freq_max > 0.0` 且 `sm.weighted_freq_coverage < freq_max` 时返回 `false`
    - _Requirements: 6.1, 6.3, 6.4, 3.3_

  - [x] 5.3 修改 `try_move` 函数，插入硬约束检查
    - 在"执行方案变动 + 可选 rebuild_simple"之后、"计算新得分"之前，插入：
      1. 调用 `self.check_full_code_max(ctx)`，若返回 `false` 则执行完整回滚并返回 `false`
      2. 若 `ctx.enable_simple_code && needs_simple`，调用 `self.check_simple_code_max(ctx)`，
         若返回 `false` 则执行完整回滚并返回 `false`
    - 回滚逻辑与现有 Metropolis 拒绝回滚完全一致（恢复 `key_weighted_usage`、`assignment`、
      重新 `update_char`、可选 `rebuild_simple`、恢复 `cached_score`）
    - _Requirements: 6.1, 6.2, 6.3, 6.5_

  - [x] 5.4 修改 `try_swap` 函数，插入硬约束检查
    - 与 `try_swap` 对称，在相同位置插入 `check_full_code_max` 和 `check_simple_code_max` 检查
    - 回滚逻辑与现有 Metropolis 拒绝回滚完全一致
    - _Requirements: 6.1, 6.2, 6.3, 6.5_

- [x] 6. 更新 `config.toml.example`，新增默认配置段
  - [x] 6.1 在 `config.toml.example` 末尾追加 `[scale]` 注释示例和 `[targets]` 默认配置段
    - 追加注释掉的 `[scale]` 段示例（保持注释，缺失时自动 calibrate 是推荐行为），包含全部 10 个字段及说明
    - 追加**未注释的** `[targets.full_code]` 段，`enabled = false`，所有目标值和 `_max` 字段均为 `0.0`，`low_weight = 0.01`，每个字段附带注释说明
    - 追加**未注释的** `[targets.simple_code]` 段，`enabled = false`，所有目标值和 `_max` 字段均为 `0.0`，`low_weight = 0.01`，每个字段附带注释说明
    - 确保 `enabled = false` 时行为与修改前完全一致（向后兼容）
    - _Requirements: 1.1, 2.1, 2.2, 3.1, 3.2_

- [x] 7. 编写测试
  - [x] 7.1 编写单元测试（example-based）
    - 在 `src/config.rs` 末尾的 `#[cfg(test)]` 模块中新增以下测试：
      - `test_scale_config_toml_parse`：含 `[scale]` 段的 TOML 正确解析为 `ScaleConfigToml`
      - `test_scale_config_missing`：缺失 `[scale]` 段时 `cfg.scale` 为 `None`
      - `test_targets_full_code_parse`：`[targets.full_code]` 各字段正确解析
      - `test_targets_simple_code_parse`：`[targets.simple_code]` 各字段正确解析
      - `test_targets_defaults`：未配置字段默认为 `0.0`，`low_weight` 默认为 `0.01`
      - `test_simple_code_disabled_skips_max`：`enable_simple_code=false` 时简码 `_max` 不生效
    - _Requirements: 1.1, 1.2, 2.1, 2.2, 2.3, 6.4_

  - [ ]* 7.2 编写属性测试：Property 1 — `resolve_scale_config` 覆盖语义
    - 在 `Cargo.toml` 的 `[dev-dependencies]` 中新增 `proptest = "1"`
    - 在 `src/main.rs` 或独立测试文件中新增属性测试：
      对任意 `ScaleConfigToml` 和任意 calibrated `ScaleConfig`，
      `resolve_scale_config` 的返回值中 `Some` 字段等于手动值，`None` 字段等于 calibrated 值
    - **Property 1: resolve_scale_config 覆盖语义**
    - **Validates: Requirements 1.3, 1.4**

  - [ ]* 7.3 编写属性测试：Property 2 — 全码目标偏差公式正确性
    - 对任意非负的 `v`、`target`、`scale`、`weight`、`low_weight`，
      验证 `compute_full_score` 中每个指标的得分分量等于：
      `weight * ((max(0, v - target) * scale).powi(2) + low_weight * (v * scale))`
    - 特别验证 `v <= target` 时超出惩罚项为 0
    - **Property 2: 全码目标偏差公式正确性**
    - **Validates: Requirements 4.1, 4.4, 4.5**

  - [ ]* 7.4 编写属性测试：Property 3 — 全码 `enabled=false` 时向后兼容
    - 对任意编码方案，当 `full_code.enabled = false` 时，
      `compute_full_score` 的结果与原始线性公式完全一致
    - **Property 3: 全码 enabled=false 时向后兼容**
    - **Validates: Requirements 4.2**

  - [ ]* 7.5 编写属性测试：Property 4 — 简码目标偏差公式正确性（含频率覆盖损失方向）
    - 对任意简码指标值，当 `simple_code.enabled = true` 时，
      验证频率覆盖指标以 `v = 1 - coverage`、`target_v = 1 - t.freq` 计算，
      其余指标直接使用原始值，每个指标得分分量等于目标偏差公式计算值
    - **Property 4: 简码目标偏差公式正确性（含频率覆盖损失方向）**
    - **Validates: Requirements 5.1, 5.4**

  - [ ]* 7.6 编写属性测试：Property 5 — 简码 `enabled=false` 时向后兼容
    - 对任意简码方案，当 `simple_code.enabled = false` 时，
      `compute_simple_score` 的结果与原始线性公式完全一致
    - **Property 5: 简码 enabled=false 时向后兼容**
    - **Validates: Requirements 5.2**

  - [ ]* 7.7 编写属性测试：Property 6 — 非零 `_max` 约束必须拒绝超限方案
    - 构造使某全码指标（如 `total_collisions`）超过对应 `_max` 的场景，
      验证 `try_move`/`try_swap` 返回 `false` 且 `assignment` 恢复为变动前的值
    - **Property 6: 非零 _max 约束必须拒绝超限方案**
    - **Validates: Requirements 6.1, 6.2, 6.3**

  - [ ]* 7.8 编写属性测试：Property 7 — 所有 `_max = 0.0` 时不因硬约束拒绝方案
    - 当 `TargetsConfig` 中所有 `_max` 字段均为 `0.0` 时，
      验证 `try_move`/`try_swap` 不因 `_max` 检查而返回 `false`
    - **Property 7: 所有 _max=0.0 时不因硬约束拒绝方案**
    - **Validates: Requirements 3.3**

- [x] 8. 最终检查点
  - 运行 `cargo build` 确保编译通过，无 warning
  - 运行 `cargo test` 确保所有单元测试通过
  - 如有属性测试，运行 `cargo test --test proptest` 确保通过
  - 如有问题，请向用户说明

## Notes

- 任务标注 `*` 的为可选测试任务，可跳过以加快 MVP 进度
- 每个任务引用了具体的需求条款，便于追溯
- 实现顺序严格按照依赖关系：config.rs → context.rs → main.rs → evaluator.rs → 示例文件 → 测试
- `FullCodeTargets` 和 `SimpleCodeTargets` 的 `Default` 实现必须手动编写（`low_weight` 默认 `0.01`，不能用 `#[derive(Default)]`）
- `OptContext::new()` 签名变更会影响 `main.rs` 中所有调用点，需同步更新
- `check_full_code_max` 和 `check_simple_code_max` 中，当所有 `_max = 0.0` 时，分支条件均不成立，编译器可优化为零开销
- 属性测试需要在 `Cargo.toml` 中添加 `proptest = "1"` 依赖

## Task Dependency Graph

```json
{
  "waves": [
    { "id": 0, "tasks": ["1.1", "1.2", "1.3"] },
    { "id": 1, "tasks": ["1.4"] },
    { "id": 2, "tasks": ["2.1"] },
    { "id": 3, "tasks": ["2.2", "3.1"] },
    { "id": 4, "tasks": ["3.2"] },
    { "id": 5, "tasks": ["4.1", "4.2", "5.1", "5.2"] },
    { "id": 6, "tasks": ["5.3", "5.4"] },
    { "id": 7, "tasks": ["6.1"] },
    { "id": 8, "tasks": ["7.1", "7.2", "7.3", "7.4", "7.5", "7.6", "7.7", "7.8"] }
  ]
}
```
