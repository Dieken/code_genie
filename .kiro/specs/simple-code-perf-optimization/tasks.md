# Implementation Plan: 简码评估性能优化

## Overview

本计划将设计文档拆解为一系列增量编码步骤，严格遵循「最小改动约束」（需求 19）：仅修改与简码评估增量化、简码激活与权重渐进、相关配置项、相关日志输出、增量与全量一致性校验直接相关的代码，不引入无关重构。

实现语言为 Rust（与现有代码库一致，设计未使用伪代码）。改造范围限定在四处：`src/context.rs`（预计算字段）、`src/config.rs` + 两个 toml（配置项）、`src/evaluator.rs`（主评估器与简码评估器增量化）、`src/annealing.rs`（退火主循环集成与日志）。

测试策略遵循设计：用 `proptest`（仓库已在 `annealing.rs` 测试中使用）实现 15 条正确性属性，每条 ≥100 次迭代，并在每个属性测试顶部标注标签注释，格式为：
`// Feature: simple-code-perf-optimization, Property {number}: {property_text}`
此外包含配置解析单元测试、配置文件烟雾测试、激活/升温集成测试与日志输出单元测试。

标记 `*` 的子任务为测试相关任务，可选，可为加速 MVP 跳过；核心实现任务不带 `*`。

## Tasks

- [x] 1. OptContext 新增静态预计算字段（`src/context.rs`）
  - [x] 1.1 实现简码静态预计算字段
    - 在 `OptContext` 新增并在 `OptContext::new`（启用简码时）一次性计算：`simple_candidate_chars`（按 `simple_coverage_ratio` 字频降序累加选出的候选字集合）、`simple_is_candidate` 候选字位图、`simple_actual_coverage` 与候选字数、`group_to_simple_affected_candidate`（`group_to_simple_affected` 与候选集求交并裁剪，用 `Vec` 保证顺序确定）、`simple_base_saving[ci][li] = char_infos[ci].parts.len() - level_instructions[li] 步数`、`simple_level_capacity[li] = code_base^L`、`simple_assign_mode`
    - 现有接口 `calc_simple_code`/`calc_simple_equiv`/`resolve_key` 保持不变；为 `get_simple_keys` 增加写入复用缓冲区的内部变体，保留现有 `Vec` 版本
    - _Requirements: 3.2, 4.5, 4.8, 7.3, 7.4, 7.6_

  - [x] 1.2 编写候选字集合属性测试
    - **Property 6: 候选字集合为覆盖率达标的最小频率前缀且静态**
    - 顶部标注：`// Feature: simple-code-perf-optimization, Property 6: ...`，proptest ≥100 次迭代
    - **Validates: Requirements 7.3, 7.4, 4.8**

  - [x] 1.3 编写受影响交集属性测试
    - **Property 7: 受影响交集预计算正确**
    - 顶部标注：`// Feature: simple-code-perf-optimization, Property 7: ...`，proptest ≥100 次迭代
    - **Validates: Requirements 1.5, 7.6**

  - [x] 1.4 编写 base_saving 属性测试
    - **Property 5: base_saving 预计算正确**
    - 顶部标注：`// Feature: simple-code-perf-optimization, Property 5: ...`，proptest ≥100 次迭代
    - **Validates: Requirements 4.5**

- [x] 2. 新增配置项与校验（`src/config.rs`）
  - [x] 2.1 新增 6 个配置项、默认值、枚举与校验
    - 在 `SimpleCodeWeights` 新增带 `#[serde(default = ...)]` 的字段：`simple_start_progress`(0.6)、`simple_ramp_progress`(0.1)、`simple_activation_reheat`(1.0)、`simple_coverage_ratio`(0.90)、`reconcile_interval_ratio`(0.05)、`simple_assign_mode`("efficiency")
    - 定义 `SimpleAssignMode { Frequency, Efficiency }` 枚举与字符串解析（非法值回落 `Efficiency` 并告警）
    - 实现 `validate_simple_activation()`：钳制 `simple_start_progress` 到 [0,1)、负值钳 0、`start+ramp>1` 钳定 `ramp`、`(start,ramp)=(0,0)` 置 `hard_activate`，越界时输出告警
    - 将新值经 `WeightConfig`/上下文构造路径透传给 `OptContext`
    - _Requirements: 4.1, 4.2, 7.1, 7.2, 8.2, 9.1, 12.1, 13.1, 13.2, 13.3, 13.4, 13.5, 15.2, 15.3, 17.1, 17.2_

  - [x] 2.2 编写配置解析单元测试
    - 缺失新增项时解析为既定默认值；`simple_assign_mode` 合法/非法字符串解析；沿用现有 `#[serde(default)]` 测试风格
    - _Requirements: 4.1, 4.2, 7.2, 8.2, 9.1, 12.1, 15.3, 17.1, 17.2_

  - [x] 2.3 编写配置钳制属性测试
    - **Property 11: 激活与渐进配置钳制不变量**
    - 顶部标注：`// Feature: simple-code-perf-optimization, Property 11: ...`，proptest ≥100 次迭代（含负值、≥1、和>1 等非法输入）
    - **Validates: Requirements 13.1, 13.2, 13.3, 13.4, 13.5**

- [x] 3. 同步配置项到 toml 文件
  - [x] 3.1 同步 `config.toml.example`
    - 在 `[weights.simple_code]` 段新增 6 项，附注释说明与默认值，默认值与代码内置一致
    - _Requirements: 18.1, 18.3, 18.4_

  - [x] 3.2 同步 `moling/config.toml`
    - 在 `[weights.simple_code]` 段新增 6 项，附注释说明与默认值，默认值与代码内置一致
    - _Requirements: 18.2, 18.3, 18.4_

  - [x] 3.3 编写配置文件烟雾测试
    - 解析两个 toml，断言 6 个新增项存在、带注释、默认值与代码内置默认一致（放置于独立集成测试文件，避免与 `config.rs` 冲突）
    - _Requirements: 18.1, 18.2, 18.4_

- [x] 4. 主评估器 is_first_candidate / bucket_first 增量维护（`src/evaluator.rs`）
  - [x] 4.1 在 update_char 内增量维护首选标记
    - 在 `Evaluator` 新增 `is_first_candidate: Vec<bool>` 与 `bucket_first: Vec<usize>` 字段
    - 在 `update_char` 现有全码桶 swap_remove/插入逻辑后增量维护：移除恰为首选字时复用/合并 `rescan_bucket_max` 为 `rescan_bucket_first` 重扫 `(max_freq, 最小 ci)`，加入时按频率/ci 规则更新首选；保证仅依赖全码桶、与简码无关
    - 确认全码重码（`total_collisions`/`collision_frequency`）计算保持独立于出简状态
    - _Requirements: 5.1, 5.2, 5.3, 5.4, 14.1_

  - [x] 4.2 编写首选标记属性测试
    - **Property 3: 首选标记与全码桶重算一致**
    - 顶部标注：`// Feature: simple-code-perf-optimization, Property 3: ...`，proptest ≥100 次迭代
    - **Validates: Requirements 5.1, 5.2, 5.3, 5.4, 4.6**

  - [x] 4.3 编写全码重码独立性属性测试
    - **Property 14: 全码重码独立于出简状态**
    - 顶部标注：`// Feature: simple-code-perf-optimization, Property 14: ...`，proptest ≥100 次迭代
    - **Validates: Requirements 14.1**

- [x] 5. SimpleEvaluator 增量化数据结构（`src/evaluator.rs`）
  - [x] 5.1 重构 SimpleEvaluator / SimpleLevelTracker 为增量化结构
    - `SimpleBucket { members: Vec<usize>, freq_sum: u64 }`；`SimpleLevelTracker` 用按编码空间直接索引的 `buckets: Vec<SimpleBucket>`（容量 = `ctx.simple_level_capacity[li]`）替代 HashMap，新增 `current_simple_code[ci]`、`selected[ci]`、级别聚合（`covered_freq`/`equiv_weighted`/`equiv_freq_sum`/`key_usage`/`key_presses`）
    - `SimpleEvaluator` 新增 `all_assigned_flags`、`simple_collision_count`/`simple_collision_freq`/`simple_collision_rate`、`cached_simple_score`/`simple_score_dirty` 及去堆分配缓冲 `key_buf`/`snapshot`/`dirty_buckets`/`pending_chars`
    - 调整 `new`/`full_rebuild` 使用候选字集合（仅遍历 `simple_candidate_chars`）初始化增量状态；`get_simple_score`/`get_simple_metrics` 保持签名与语义
    - 简码热路径改用复用缓冲区版本的 `get_simple_keys`，避免每步堆分配
    - _Requirements: 1.2, 3.1, 3.2, 3.3, 7.5, 8.6_

  - [x] 5.2 编写去堆分配验证单元测试
    - 验证热路径复用缓冲区、不在每步分配新容器；复用缓冲路径与全量路径结果一致
    - _Requirements: 3.1, 3.3_

- [x] 6. 两种分配模式排序键与桶内局部排序（`src/evaluator.rs`）
  - [x] 6.1 实现 cmp_in_bucket 与桶内局部排序
    - Frequency 模式排序键 `key = freq`；Efficiency 模式 `key = freq × (base_saving[ci][li] + sel_len)`，`sel_len` 由 `is_first_candidate` 取 0/1
    - 并列裁决：排序键相等先按 `freq` 降序、再按 `ci` 升序；仅对受影响桶内候选列表局部排序，不维护跨桶全局有序结构
    - _Requirements: 4.3, 4.4, 4.6, 4.7, 5.4, 6.1, 6.2, 6.3_

  - [x] 6.2 编写桶内选中集合属性测试
    - **Property 4: 桶内选中集合符合所选模式的排序键**
    - 顶部标注：`// Feature: simple-code-perf-optimization, Property 4: ...`，proptest ≥100 次迭代（参数化 frequency/efficiency）
    - **Validates: Requirements 4.3, 4.4, 4.7, 6.2, 6.3**

- [x] 7. 简码增量更新算法 apply_move_incremental（`src/evaluator.rs`）
  - [x] 7.1 实现 apply_move_incremental
    - 受影响裁剪：仅遍历 `group_to_simple_affected_candidate[r]`，空交集直接返回且 `simple_score` 不变
    - 阶段 1：对每个受影响候选字各级别做「旧桶移除/新桶加入」，同步 `freq_sum` 与 `current_simple_code`，标记 dirty 桶
    - 阶段 2：按级别升序处理 dirty 桶，局部重排选出前 `code_num`，通过 `pending_chars` 传播跨级排除，并增量维护级别聚合与 `selected`/`all_assigned_flags`
    - 阶段 3：简码重码双来源增量（移动组改变全码的桶 + `all_assigned_flags` 翻转的桶），在少数全码桶上重算并差量更新 `simple_collision_count`/`simple_collision_freq`/`simple_collision_rate`，置 `simple_score_dirty`
    - _Requirements: 1.1, 1.3, 1.4, 1.5, 7.5, 14.2, 14.3, 14.4_

  - [x] 7.2 编写增量与全量一致性属性测试
    - **Property 1: 简码增量维护与全量重建一致**
    - 顶部标注：`// Feature: simple-code-perf-optimization, Property 1: ...`，proptest ≥100 次迭代（参数化 frequency/efficiency，对随机移动序列逐字段比对 `full_rebuild`）
    - **Validates: Requirements 1.1, 1.2, 1.3, 1.4, 6.1, 6.2, 6.3, 7.5, 8.6, 10.1, 14.2, 14.3, 14.4, 15.1, 17.3, 17.5**

- [x] 8. 快照、提交与回滚机制（`src/evaluator.rs`）
  - [x] 8.1 实现 SimpleSnapshot 与 commit/rollback
    - `apply_move_incremental` 在修改前对被触碰的桶项/条目/级别聚合/简码重码标量写入复用快照缓冲
    - `commit()` 清空快照确认增量；`rollback()` 逆序还原桶成员、`freq_sum`、`current_simple_code`、`selected`、`all_assigned_flags`、级别聚合标量、简码重码标量与 `cached_simple_score`，恢复至移动前
    - _Requirements: 2.1, 2.2, 2.3, 3.1_

  - [x] 8.2 编写回滚 round-trip 属性测试
    - **Property 2: 拒绝/回滚与未移动等价（round-trip）**
    - 顶部标注：`// Feature: simple-code-perf-optimization, Property 2: ...`，proptest ≥100 次迭代
    - **Validates: Requirements 2.1, 2.2, 2.3, 3.1**

- [x] 9. 分量分数缓存与最佳解按分量存储（`src/evaluator.rs`）
  - [x] 9.1 实现分量缓存、O(1) 合成与 best 重算辅助
    - `Evaluator` 新增 `cached_full_score`/`full_score_dirty`、`simple_active`、`current_simple_weight`
    - `compute_score`/`get_score` 改为 `weight_full_code * full_score + current_simple_weight * simple_score`，未激活时 `simple_score` 与 `current_simple_weight` 均为 0
    - 新增 `activate_simple(ctx, assignment)`（一次性构建 `SimpleEvaluator` 并置 `simple_active=true`）、`reconcile(ctx, assignment)`（全量重算覆盖增量值）、`apply_simple_for_move(...)`（替换 `rebuild_simple` 调用点，走增量+快照，回滚分支调用 `rollback`）、`best_total(weight_full, weight_simple_eff)`（O(1) 按分量合成）
    - _Requirements: 8.6, 10.1, 10.2, 10.3, 11.1, 11.2, 11.3_

  - [x] 9.2 编写激活前简码贡献为零属性测试
    - **Property 9: 激活前简码贡献为零**
    - 顶部标注：`// Feature: simple-code-perf-optimization, Property 9: ...`，proptest ≥100 次迭代（含 `enabled=false`）
    - **Validates: Requirements 8.5, 10.2, 17.4**

  - [x] 9.3 编写最佳解综合得分重算属性测试
    - **Property 10: 最佳解综合得分按分量以当前权重重算**
    - 顶部标注：`// Feature: simple-code-perf-optimization, Property 10: ...`，proptest ≥100 次迭代
    - **Validates: Requirements 11.1, 11.2, 11.3, 16.5**

- [x] 10. 退火主循环集成（`src/annealing.rs`）
  - [x] 10.1 集成延迟激活、权重曲线、对账与结束校验
    - 在 `simulated_annealing` 内新增局部状态：激活闩锁 `simple_activated`、进度 `p = step/steps`、`hard_activate`
    - 实现纯函数 `w_simple_eff(p)`（smoothstep，含 `p_start`/`p_ramp` 分段）与对账间隔 `M = max(1, floor(steps × reconcile_interval_ratio))`
    - 进度首达 `simple_start_progress`（或硬激活）时调用 `activate_simple` 并对 `temp_multiplier` 应用独立的 `simple_activation_reheat`（与 `reheat_factor` 解耦）；仅在有效权重变化的步令 `score_dirty=true`
    - 最佳解改为按分量 `best_full_score`/`best_simple_score` 存储，比较时用当前 `w_eff` 重算 `best_total`
    - 每 M 步调用 `evaluator.reconcile(...)`，循环结束强制再 `reconcile` 一次做全量校验；替换原 `rebuild_simple` 调用点为增量+快照路径
    - _Requirements: 8.1, 8.3, 8.4, 8.5, 9.2, 9.3, 9.4, 9.5, 11.2, 12.2, 12.3, 12.4, 15.4, 15.5, 15.6, 17.3, 17.4_

  - [x] 10.2 编写对账等于全量属性测试
    - **Property 13: 对账以全量结果覆盖增量值**
    - 顶部标注：`// Feature: simple-code-perf-optimization, Property 13: ...`，proptest ≥100 次迭代
    - **Validates: Requirements 15.4, 15.5, 15.6**

  - [x] 10.3 编写激活与升温集成测试
    - 小规模端到端跑若干步：断言激活事件触发、温度乘子按 `simple_activation_reheat` 变化且与 `reheat_factor` 解耦、关闭 reheat/扰动时仍正确激活并渐进
    - _Requirements: 12.2, 12.3, 12.4_

- [x] 11. 日志增强（`src/annealing.rs`）
  - [x] 11.1 实现激活、渐进与分量日志
    - 激活时输出「简码已激活」事件；渐进期在 `simple_start_progress + (i/10) * simple_ramp_progress`（i=0..9）共 10 点输出 `p/α/有效权重`；渐进结束输出「已达目标权重 W」
    - 当前/最佳分数日志在总分外额外输出全码分量 `weight_full_code * full_score` 与简码分量 `w_simple_eff * simple_score`，并用当前权重重算最佳解分量
    - 「配置确认」阶段输出候选字覆盖率与候选字数
    - _Requirements: 7.7, 16.1, 16.2, 16.3, 16.4, 16.5, 16.6_

  - [x] 11.2 编写日志输出单元测试
    - 断言配置确认输出覆盖率与候选字数、渐进期 10 点输出 `p/α/w`、当前与最佳日志含全码与简码分量
    - _Requirements: 7.7, 16.2, 16.4, 16.6_

- [x] 12. 纯函数属性测试（`src/annealing.rs`）
  - [x] 12.1 编写权重曲线属性测试
    - **Property 8: 有效简码权重曲线符合分段定义**
    - 顶部标注：`// Feature: simple-code-perf-optimization, Property 8: ...`，proptest ≥100 次迭代（含两个边界点连续性）
    - **Validates: Requirements 9.2, 9.3, 9.4, 9.5**

  - [x] 12.2 编写对账间隔 M 计算属性测试
    - **Property 12: 对账间隔 M 的计算**
    - 顶部标注：`// Feature: simple-code-perf-optimization, Property 12: ...`，proptest ≥100 次迭代
    - **Validates: Requirements 15.2**

  - [x] 12.3 编写激活闩锁单调属性测试
    - **Property 15: 激活闩锁单调**
    - 顶部标注：`// Feature: simple-code-perf-optimization, Property 15: ...`，proptest ≥100 次迭代（含非单调/升温回退序列）
    - **Validates: Requirements 8.3, 8.4**

- [x] 13. Checkpoint - 构建与全部测试通过
  - 运行 `cargo build` 与 `cargo test` 确保编译与全部单元/属性/集成测试通过；如出现增量与全量不一致或回滚不等价的失败用例，依据失败反例修复后重跑。Ensure all tests pass, ask the user if questions arise.

## Notes

- 标记 `*` 的子任务为可选测试任务，可为加速 MVP 跳过；核心实现任务不带 `*`。
- 每个子任务都引用了对应的需求条款，保证可追溯性。
- 每条正确性属性对应单独一个属性测试子任务，并要求在测试顶部标注 `// Feature: simple-code-perf-optimization, Property {number}: {property_text}` 标签，proptest ≥100 次迭代。
- 严格遵循需求 19 的最小改动约束：仅触及简码增量化、激活/渐进、相关配置、相关日志与一致性校验，不做无关重构。
- 性能目标（开启简码后单步成本回落到 O(受影响字数) 量级）为非功能目标，由增量/全量一致性（Property 1）、回滚等价（Property 2）与对账校验（Property 13）共同保障正确性；性能本身通过任务 13 的构建/测试与人工观察确认，不作为独立编码任务。

## 后续优化（Backlog，不在本次验收范围）

以下为已知的、有意为之的留口，记录为后续优化项，不计入本次任务完成度：

- [x] B1. 阶段 2 出简选择的精确增量化（`src/evaluator.rs`，`apply_move_incremental`）【已完成】
  - 现状（已实现）：阶段 2 改为 `do_incremental_selection`——「仅对 dirty 桶做细粒度局部重排 + 通过 `pending` 做跨级排除传播」，并以「首选翻转重排种子」捕获 Efficiency 模式下 `is_first_candidate` 翻转导致的排序键变化；阶段 2 不再调用 `rebuild_selection`。单步复杂度降到 O(受影响字数 × 级别相关小量)，由观测计数器 `last_stage2_visits` 与单元测试 `b1_stage2_work_is_local_not_candidate_set_size` 证明其不随候选字总集规模增长。
  - 回滚：采用细粒度撤销（option b）——桶成员代际快照 + 级别聚合整存 + `current_simple_code`/`selected`/选中贡献撤销日志逆序回放 + `all_assigned_flags` 起始值回写，回滚成本 O(受影响项)，不退化为 O(级别容量)。
  - 验收：Property 1（增量=全量）、Property 2（回滚 round-trip）、Property 13（对账=全量）全绿；新增 `prop_b1_incremental_selection_local_and_consistent` 属性测试与 `b1_stage2_work_is_local_not_candidate_set_size` 单元测试。
  - _Requirements: 1.1, 1.3, 1.4, 7.5_

## 新增功能任务：空格上屏与固定简码（需求 20/21）

> 以下为后续追加的两个功能特性的任务，独立于前述性能优化的验收范围。沿用属性测试 + 单元/烟雾测试策略，每条新属性对应一个属性测试子任务，顶部标注 `// Feature: simple-code-perf-optimization, Property {number}: {property_text}`，proptest ≥100 次迭代。

- [x] 14. 简码级别空格上屏（需求 20）
  - [x] 14.1 新增 `space_commit` 配置项与 base_saving 调整
    - `src/config.rs` 的 `SimpleLevelConfig` 与 `src/types.rs` 的 `SimpleCodeLevel` 各新增 `space_commit: bool`（`#[serde(default)]` 缺省 false），经 `get_simple_code_config()` 透传
    - `src/context.rs` 预计算 `simple_base_saving` 时：`space_commit` 为真的级别取 `simple_len = 指令步数 + 1`，即 `base_saving = full_len - (simple_len + 1)`
    - _Requirements: 20.1, 20.2, 20.3, 20.4, 20.9_

  - [x] 14.2 空格键计入当量与分布
    - `src/context.rs` 的 `calc_simple_equiv`：将末位键到 `KEY_SPACE` 的转移项改为「仅当该级 `space_commit` 为真时累加」（口径统一，记录该行为变更）
    - 简码出简的分布统计（`rebuild_selection` 等）：`space_commit` 为真时对 `key_usage[KEY_SPACE]` 与 `key_presses` 各计一次空格键
    - _Requirements: 20.7, 20.8_

  - [x] 14.3 输出追加空格上屏下划线
    - `src/output.rs` 的 `save_simple_code_output` 与 `save_combined_output` 简码段：拼接简码字符串后，若该级 `space_commit` 为真则追加 `_`
    - _Requirements: 20.5, 20.6_

  - [x] 14.4 编写空格上屏属性/单元测试
    - **Property 16: 空格上屏的 base_saving 与当量/分布口径**，proptest ≥100 次迭代（参数化 `space_commit` 真/假）
    - **Property 17: 空格上屏的输出表示**（断言尾随 `_` 的有无）
    - **Validates: Requirements 20.3, 20.4, 20.5, 20.6, 20.7, 20.8**

- [x] 15. 固定简码映射（需求 21）
  - [x] 15.1 内联固定简码配置解析、级别归属与一致性校验
    - `src/config.rs` 的 `Config` 新增顶层 `fixed_simple_codes: Option<BTreeMap<String, String>>`，对应 TOML 顶层内联表 `[fixed_simple_codes]`；解析为 `Vec<(char, String)>`
    - 按核心码串（去结尾 `_`）键位数确定所属级别；校验结尾 `_` 与该级 `space_commit` 一致、且有效长度 < 全码长度，不满足则告警并拒绝该条
    - _Requirements: 21.1, 21.2, 21.5, 21.6, 22.3_

  - [x] 15.2 在两个 toml 添加注释示例（需求 21.11）
    - 在 `config.toml.example` 与 `moling/config.toml` 添加默认整段注释掉的 `[fixed_simple_codes]` 示例，为「不、是、我、的、了」分别示例「u、i、o、e、a」；并在 `[[simple_levels]]` 注明 `space_commit` 字段（默认 false）
    - 忽略 `code_genie2` 目录（另一 git worktree）
    - _Requirements: 21.11, 20.1_

  - [x] 15.3 OptContext 预计算固定简码静态字段
    - `src/context.rs` 新增并一次性计算：`simple_fixed_assigned`、`simple_fixed_occupancy[li][code]`、固定简码常量贡献（`fixed_covered_freq`/`fixed_equiv_weighted`/`fixed_equiv_freq_sum`/`fixed_key_usage`/`fixed_key_presses`，含空格上屏时的尾随空格）
    - 候选字集合按覆盖率在全集选取后，剔除 `simple_fixed_assigned` 的字（同步 `simple_candidate_chars` 与 `simple_is_candidate`）
    - _Requirements: 21.3, 21.4, 21.8, 21.9_

  - [x] 15.4 SimpleEvaluator 集成固定简码
    - 固定简码字在 `SimpleEvaluator` 初始化时置 `all_assigned_flags[ci] = true` 且永不翻转（简码重码排除）
    - `rebuild_selection` / `do_incremental_selection` 选取出简时，桶可选名额改为 `code_num - simple_fixed_occupancy[li][code]`（下限 0）
    - 简码覆盖率/当量/分布聚合并入固定简码常量偏置
    - _Requirements: 21.7, 21.8, 21.9_

  - [x] 15.5 输出固定简码
    - `src/output.rs` 各级输出固定简码字（保留结尾 `_`），并计入 `globally_assigned` 防重复
    - _Requirements: 21.10_

  - [x] 15.6 编写固定简码属性测试
    - **Property 18: 固定简码与候选字集合解耦且占用名额**，proptest ≥100 次迭代
    - **Property 19: 固定简码的恒定出简贡献**，proptest ≥100 次迭代（含级别归属/一致性校验、跨移动序列 `all_assigned_flags` 恒真与常量贡献）
    - **Validates: Requirements 21.3, 21.4, 21.5, 21.6, 21.7, 21.8, 21.9**

- [x] 16. 简码长度严格短于全码（需求 22）
  - [x] 16.1 静态长度资格过滤
    - `src/context.rs` 预计算每个 `(ci, li)` 的出简资格：`effective_simple_len(li) = 指令步数 + (space_commit ? 1 : 0) < full_len(ci)`（可与 `simple_base_saving` 一并算，或新增 `simple_eligible` 位图）
    - 在 `rebuild_selection` 与 `apply_move_incremental` 的候选字入桶步骤加入该资格判定：不合格的 `(ci, li)` 等价于 `calc_simple_code` 返回 `None`
    - 固定简码若违反约束在加载期拒绝（见 15.1）
    - _Requirements: 22.1, 22.2, 22.4_

  - [x] 16.2 编写长度约束属性测试
    - **Property 20: 简码长度严格短于全码**，proptest ≥100 次迭代
    - **Validates: Requirements 22.1, 22.2, 22.3, 22.4**

- [x] 17. 回归校验与 Checkpoint
  - [x] 17.1 含空格上屏/固定简码/长度约束下的增量=全量回归
    - 在 Property 1/2/13 的上下文构造中纳入 `space_commit`、固定简码与长度约束，确认增量=全量、回滚 round-trip、对账=全量仍逐字段成立
    - _Requirements: 20.7, 21.8, 21.9, 22.4_
  - [x] 17.2 构建与全部测试通过
    - 运行 `cargo build` 与 `cargo test`，确保需求 20/21/22 新增实现与测试全绿，且既有 58 lib + 2 smoke 测试无回归

- [x] 18. 输出镜像评估器选择（需求 23）
  - [x] 18.1 输出复用评估器出简选择
    - `SimpleEvaluator` 新增 `selected_ordered`（按级别升序、桶编码升序、桶内排序键返回出简 `(li, ci)`）
    - `src/output.rs` 新增 `evaluator_simple_selection` 与 `simple_code_str`；`save_simple_code_output` 与 `save_combined_code_output` 改为按评估器选择输出，不再按字频重新推导（自动一致 efficiency 排序/sel_len/固定占用扣减/跨级排除）
    - _Requirements: 23.1, 23.2, 23.3, 23.4_
  - [x] 18.2 编写输出占用回归测试
    - 断言 output 选择对每个 (级别, 桶) 满足「优化出简 + 固定占用 ≤ code_num」，固定字不重复出简
    - **Validates: Requirements 23.1, 23.3, 23.4**

- [x] 19. 固定简码语义修订（确认点 1/2/4）
  - [x] 19.1 code_num=0 级别按需保留 + 占用不再硬拒绝 + 下划线以固定简码自身为准
    - `config.rs::get_simple_code_config`：code_num=0 级别仅在有固定简码按码长归属时保留（需求 21.12）
    - `context.rs` 固定简码处理：移除「占用 ≥ code_num 即拒绝」（改为 `max(0, code_num−占用)`，需求 21.7）；以固定简码自身结尾下划线为输出/长度/当量/分布依据，并做非对称校验（`space_commit=false`+下划线 → 报错；`space_commit=true`+无下划线 → 告警接受，需求 21.6）
    - _Requirements: 21.6, 21.7, 21.12_
  - [x] 19.2 更新/新增回归测试
    - 调整 prop18 占用断言为 `sel ≤ max(0, code_num−占用)`；新增 code_num=0+固定简码、非对称校验（`#[should_panic]`）、`get_simple_code_config` 级别保留测试
    - **Validates: Requirements 21.6, 21.7, 21.12**

- [x] 20. warmup 与坐标下降按调用上下文区分简码（需求 24）
  - [x] 20.1 `enhanced_hill_climb`/`hill_climb_warmup` 增 `disable_simple` 参数
    - 签名加 `disable_simple: bool`；`Evaluator::new` 后仅当 `ctx.enable_simple_code && disable_simple` 才置 `simple_active=false; current_simple_weight=0.0; score_dirty=true`
    - `disable_simple=false` 时保持简码激活，沿用算子内置增量（try_move/try_swap/try_triple_swap）
    - _Requirements: 24.1, 24.2, 24.3, 24.6_
  - [x] 20.2 `coordinate_descent` 增 `disable_simple` 参数，精炼上下文走增量简码
    - 签名加 `disable_simple: bool`；`disable_simple=true` 时置零关闭（probe 循环 `has_simple_impact` 短路，消除 ~3840 次全量重建）
    - `disable_simple=false` 时三处（前向探测/回滚/应用最优）改用增量：`apply_simple_for_move` + `rollback_simple`/`commit_simple`，**移除全部 `rebuild_simple`**
    - _Requirements: 24.1, 24.2, 24.3, 24.6_
  - [x] 20.3 调用点绑定与评分口径
    - `multi_start_init` 内两处传 `true`（Init/校准关闭）；`simulated_annealing` 结尾最终精炼两处传 `false`（增量简码）
    - 验证最终精炼 `final_score`/`cd_score` 与 `best_score` 同口径（均含 `weight_simple_code`），修正历史 commit `e16ab24` 的纯全码误判
    - _Requirements: 24.4, 24.5, 24.7_
  - [x] 20.4 `Evaluator::new_full_only` 跳过急切简码构建
    - 重构 `Evaluator::new` → `new_impl(ctx, assignment, build_simple)`；新增 `new_full_only`（`build_simple=false`，`simple_eval=None`）
    - `enhanced_hill_climb`/`coordinate_descent` 在 `disable_simple=true` 时改用 `new_full_only`，消除 warmup 每候选被丢弃的全量简码构建（约 `候选数+1` 次/阶段）
    - 校准变为「全码优化 0 次简码构建 + 观测 1 次」；Init 共用 `multi_start_init` 自动同样受益，仅保留 SA 主循环自身一次必要 `Evaluator::new`
    - _Requirements: 24.2, 24.6, 24.7, 24.8_

- [x] 21. 首选字维护仅在简码激活时进行，消除全码路径回归（需求 25）
  - [x] 21.1 `update_char` 首选字维护门控
    - 移除/插入两分支的 `is_first_candidate`/`bucket_first` 维护改为仅 `simple_active` 为真时执行
    - 新增仅求最大频率的 `rescan_bucket_max`；`simple_active` 为假时移除分支用它、插入分支只更新 max（与基线 `27fcc6d` 逐字节等价）
    - _Requirements: 25.1, 25.2, 25.6_
  - [x] 21.2 激活时一次性重建首选字
    - 新增 `rebuild_first_candidates(ctx)`，在 `activate_simple` 置 `simple_active=true` 且构建 `SimpleEvaluator` 之前调用
    - 因激活前无人读取首选标记（`has_simple_impact` 短路），重建结果与全程增量维护在激活时刻一致
    - _Requirements: 25.3, 25.4, 25.5_
  - [x] 21.3 测试覆盖
    - `first_candidate_tests::make_ctx` 增 `enable_simple` 参数：一致性属性测试在简码激活下覆盖增量维护路径；resort 缓冲不增长测试在简码关闭下覆盖纯全码路径
    - _Requirements: 25.1, 25.4_
  - [x] 21.4 resort 种子只登记候选字（性能优化）
    - `update_char` 三处首选翻转登记加 `if ctx.simple_is_candidate[...]` 守卫，只 push 候选字，缩小 `apply_simple_for_move` 的种子扫描；与「push 全部、apply 时过滤」行为等价
    - 由 prop1（增量=全量）、prop3（首选一致）、b1（stage2 局部）现有属性测试保证正确性
    - _Requirements: 25.1_

- [x] 22. 简码激活时重定价最优解（需求 26）
  - [x] 22.1 激活分支内对 `best_assignment` 重算真实简码分量
    - 紧随 `activate_simple` 之后，用 `Evaluator::new(ctx, &best_assignment)` 重算 `best_full_score`/`best_simple_score`/`best_metrics`/`best_simple_metrics`/`best_score`
    - 仅简码启用时执行；一次性，O(简码全量构建)
    - _Requirements: 26.1, 26.2, 26.3, 26.4, 26.5_
  - [x] 22.2 测试：激活后最优解可被综合更优解更新
    - 由现有端到端激活测试覆盖；重定价逻辑保证 `best_simple_score` 不再恒为 0
    - _Requirements: 26.1, 26.3_

- [x] 23. 激活时机默认值调整与动态校验告警（需求 27）
  - [x] 23.1 默认值调整：`simple_start_progress` 0.6→0.4、`simple_activation_reheat` 1.0→1.2
    - 改 `default_simple_start_progress`、`default_simple_activation_reheat`；同步 `config.toml.example` 与 `moling/config.toml`（忽略 `code_genie2/`），对两项附注释与推荐区间
    - _Requirements: 27.1, 27.2_
  - [x] 23.2 动态校验告警（钳制 + 温度建议分置）
    - `validate_simple_activation` 内：`reheat < 1.0` 钳 1.0 + 告警；`simulated_annealing` 改从 clamped 克隆读 `simple_reheat`
    - `simulated_annealing` thread 0（复用已构建 `schedule`，只打印一次）：`reheat > reheat_hi`（`reheat_hi = temp_start/base_temp(p_start)`）告警并给合理范围 `[1.0, reheat_hi]` 与推荐；`p_start > comfort_progress` 告警并给建议区间
    - 阈值/范围/建议值由 `temp_start/temp_end/comfort_temp/comfort_width/total_steps/simple_start_progress` 经 `TemperatureSchedule` 与解析式动态计算，不写死；简码关闭时跳过
    - _Requirements: 27.3, 27.4, 27.5, 27.6, 27.7_
  - [x] 23.3 测试：reheat<1 钳制；>=1 不钳制
    - _Requirements: 27.3, 27.4_

- [x] 24. 分数分量日志增强（需求 28）
  - [x] 24.1 三分量得分日志
    - `[T0] 初始化完成`/`最终爬山改进`/`坐标下降精炼`/`最终得分` 及最终结果块「综合得分」追加 综合/全码分量/简码分量
    - _Requirements: 28.1, 28.2, 28.3, 28.4_
  - [x] 24.2 简码子分数
    - 新增 `SimpleMetricScores` 与 `Evaluator::get_simple_metric_scores`（镜像 `compute_simple_score`）；最终结果块「简码」各子指标按 `(分: X)` 输出；子分数之和等于简码总分
    - _Requirements: 28.5, 28.6, 28.7_
  - [x] 24.3 测试：子分数自洽与简码关闭归零
    - `get_simple_metric_scores` 子分数之和等于 total 且等于 `get_metric_scores().total_simple`；简码关闭时全为 0
    - _Requirements: 28.6, 28.7_

- [x] 25. 简码全局聚合标量增量维护（需求 29，方向 A）
  - [x] 25.1 新增全局聚合字段并在全量重建时初始化
    - `SimpleEvaluator` 增 `g_covered_freq/g_equiv_weighted/g_equiv_freq_sum/g_key_usage[]/g_key_presses`；新增 `recompute_global_aggregates`，在 `rebuild_internal` 末尾按 `Σ_级 + 固定常量` 填充（覆盖 `new`/`full_rebuild`/`reconcile`）
    - _Requirements: 29.1, 29.2, 29.8_
  - [x] 25.2 增量同步与读取改造
    - `select_char`/`deselect_contrib_only`/`refresh_char` 对全局量施加与级别相同的 Δ；`get_simple_metrics` 改为直接读全局量，删除跨级求和与固定偏置相加
    - _Requirements: 29.3, 29.4_
  - [x] 25.3 回滚纳入全局量
    - `SimpleSnapshot` 增 5 个全局量字段；`snapshot_aggregates` 整存、`rollback` 整体写回（`g_key_usage` 为定长数组，O(键数)）
    - _Requirements: 29.5_
  - [x] 25.4 一致性测试
    - 新增 `test_global_aggregates_equal_sum_of_levels_plus_fixed`：move 序列（含接受/回滚）后断言 `g_* == Σ_级 + 固定`；prop1（增量=全量）、prop13（reconcile=全量）保持全绿；简码关闭零影响
    - _Requirements: 29.6, 29.7, 29.8_

- [x] 26. 简码分布偏差增量化（需求 30，方向 B）
  - [x] 26.1 维护 g_dist_deviation 与每键贡献缓存
    - 新增 `g_dist_deviation`/`g_dist_contrib[]`；抽出 `key_dist_penalty` 纯函数与 `recompute_dist_full`；`recompute_global_aggregates` 末尾全量重算（覆盖构造/full_rebuild/reconcile）
    - _Requirements: 30.1, 30.5_
  - [x] 26.2 move 内键改动登记 + 末尾结算
    - select/deselect/refresh 改 `g_key_usage[k]` 时记入 `g_dirty_keys`（move 起始清空）；`apply_move_incremental` 末尾 `selection_may_change` 时 `finalize_dist`：presses 不变只更新被改动键，presses 变化全量回退
    - `get_simple_metrics` 直接读 `g_dist_deviation`，删除每步 O(键数) 循环
    - _Requirements: 30.2, 30.3, 30.4, 30.5_
  - [x] 26.3 回滚纳入分布偏差
    - `SimpleSnapshot` 增 `g_dist_deviation`/`g_dist_contrib`，整存/整体写回
    - _Requirements: 30.6_
  - [x] 26.4 非零配置一致性测试
    - 新增 `test_incremental_dist_matches_full_rebuild`：非零 `key_dist_config` 下 move 序列逐次断言增量 dist == 全量重建；prop1/prop13 保持全绿；简码关闭零影响
    - _Requirements: 30.7, 30.8_

## Task Dependency Graph

```json
{
  "waves": [
    { "id": 0, "tasks": ["2.1"] },
    { "id": 1, "tasks": ["1.1", "2.2", "3.1", "3.2"] },
    { "id": 2, "tasks": ["1.2", "2.3", "3.3", "4.1"] },
    { "id": 3, "tasks": ["1.3", "4.2"] },
    { "id": 4, "tasks": ["1.4", "4.3"] },
    { "id": 5, "tasks": ["5.1"] },
    { "id": 6, "tasks": ["5.2"] },
    { "id": 7, "tasks": ["6.1"] },
    { "id": 8, "tasks": ["6.2"] },
    { "id": 9, "tasks": ["7.1"] },
    { "id": 10, "tasks": ["7.2"] },
    { "id": 11, "tasks": ["8.1"] },
    { "id": 12, "tasks": ["8.2"] },
    { "id": 13, "tasks": ["9.1"] },
    { "id": 14, "tasks": ["9.2", "10.1"] },
    { "id": 15, "tasks": ["9.3", "10.2"] },
    { "id": 16, "tasks": ["10.3"] },
    { "id": 17, "tasks": ["11.1"] },
    { "id": 18, "tasks": ["11.2"] },
    { "id": 19, "tasks": ["12.1"] },
    { "id": 20, "tasks": ["12.2"] },
    { "id": 21, "tasks": ["12.3"] },
    { "id": 22, "tasks": ["14.1"] },
    { "id": 23, "tasks": ["14.2", "14.3", "14.4"] },
    { "id": 24, "tasks": ["15.1", "15.2"] },
    { "id": 25, "tasks": ["15.3"] },
    { "id": 26, "tasks": ["15.4", "15.5", "15.6"] },
    { "id": 27, "tasks": ["16.1"] },
    { "id": 28, "tasks": ["16.2"] },
    { "id": 29, "tasks": ["17.1", "17.2"] },
    { "id": 30, "tasks": ["18.1"] },
    { "id": 31, "tasks": ["18.2"] },
    { "id": 32, "tasks": ["19.1"] },
    { "id": 33, "tasks": ["19.2"] },
    { "id": 34, "tasks": ["20.1", "20.2"] }
  ]
}
```
