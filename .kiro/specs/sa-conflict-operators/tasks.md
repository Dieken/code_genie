# Implementation Plan: SA 主循环冲突导向算子集成

## Overview

本实现计划将冲突导向算子（`try_resolve_conflict`）增量式地集成进模拟退火主循环。任务顺序保证每一步完成后代码均可编译：先扩展 `AnnealingConfig` 配置项与默认值，再调整 `find_collision_groups` 签名与排序裁决并同步既有调用点，随后参数化 `try_resolve_conflict` 的采样窗口，接着改造 `simulated_annealing` 主循环接线，再同步配置文件，最后补充属性测试与单元测试并完成整体构建/测试与向后兼容验证。所有实现使用 Rust。

## Tasks

- [x] 1. 扩展 `AnnealingConfig` 配置项、默认值与向后兼容
  - [x] 1.1 在 `src/config.rs` 新增四个配置字段及 serde 默认值函数
    - 在 `AnnealingConfig` 结构体尾部新增 `conflict_probability: f64`、`conflict_refresh_interval: usize`、`conflict_sample_window: usize`、`conflict_weight_by_freq: bool`，分别标注 `#[serde(default = "...")]`
    - 新增默认值函数 `default_conflict_probability() -> f64 { 0.0 }`、`default_conflict_refresh_interval() -> usize { 1000 }`、`default_conflict_sample_window() -> usize { 20 }`、`default_conflict_weight_by_freq() -> bool { false }`
    - 更新 `Default for Config` 中 `annealing: AnnealingConfig { ... }` 构造块，显式追加四个字段为 `0.0 / 1000 / 20 / false`
    - _Requirements: 1.1, 1.2, 1.3, 1.4, 1.7_

  - [x]* 1.2 编写配置默认值与向后兼容单元测试
    - 解析不含任何新字段的 `[annealing]` 配置，断言四字段取默认值且解析成功
    - 解析仅含部分新字段的配置，断言显式字段取显式值、缺失字段取默认值
    - 断言 `Config::default().annealing` 四字段为默认值
    - 确认现有 `config.rs` 解析测试在新增字段后仍通过
    - _Requirements: 1.5, 1.6, 1.7_

- [x] 2. 改造 `find_collision_groups` 排序策略与稳定裁决
  - [x] 2.1 为 `find_collision_groups` 增加 `weight_by_freq` 参数并实现降序排序与稳定次键
    - 在 `src/annealing.rs` 中将签名改为 `find_collision_groups(ctx, assignment, weight_by_freq: bool)`
    - `weight_by_freq == false` 时权重为参与冲突编码的汉字数量（`chars.len()`）；`true` 时为这些汉字的字频之和（`ctx.char_infos[ci].frequency` 求和）
    - 排序：主键第三字段（weight）降序，次键 `(g1, g2)` 升序裁决，保证结果可复现、不依赖 HashMap 遍历顺序
    - _Requirements: 5.1, 5.2, 5.3, 5.4_

  - [x] 2.2 更新 `find_collision_groups` 既有调用点
    - 为主循环低温智能扰动块中的调用补传 `weight_by_freq = false`
    - 为 `enhanced_hill_climb` 中的调用补传 `weight_by_freq = false`
    - 保持这两条既有路径行为不变（按数量排序）
    - _Requirements: 5.1_

- [x] 3. 参数化 `try_resolve_conflict` 的采样窗口
  - [x] 3.1 用 `sample_window` 参数替换硬编码 20 并增加边界守卫
    - 在 `src/annealing.rs` 中将签名改为接受 `sample_window: usize`（及现有 `temp` 等参数）
    - 开头增加 `if collisions.is_empty() || sample_window == 0 { return false; }` 守卫
    - 有效窗口取 `sample_window.min(collisions.len())`，在该范围内用 `rng.gen_range(0..window)` 采样
    - 后续选 g1/g2、随机新键位、`evaluator.try_move` 接受/回滚逻辑保持不变
    - 同步更新 `enhanced_hill_climb` 中对 `try_resolve_conflict` 的调用以传入窗口参数
    - _Requirements: 4.1, 4.2, 4.3, 4.4, 6.1_

  - [x]* 3.2 编写采样窗口边界单元测试
    - 非空列表 + `window=0`：返回 false 且 assignment 不变
    - 空列表 + 任意 window：返回 false 且 assignment 不变
    - _Requirements: 4.3, 4.4_

- [x] 4. 改造 `simulated_annealing` 主循环接线
  - [x] 4.1 引入冲突缓存局部状态与按概率分发逻辑
    - 在主循环前读取四个配置项，计算 `conflict_enabled = conflict_prob > 0.0`
    - 仅当 `conflict_enabled` 时通过 `find_collision_groups` 初始化 `collisions` 缓存并将 `steps_since_refresh` 清零；否则缓存为空且不调用 `find_collision_groups`
    - 主循环内温度计算后插入刷新逻辑：`conflict_refresh > 0 且 steps_since_refresh >= conflict_refresh` 时重建缓存并清零计数，否则计数加一
    - 邻域分发：`conflict_enabled` 为真时每步抽取一次 `r ∈ [0.0,1.0)`，若 `r < conflict_prob 且缓存非空` 则调用 `try_resolve_conflict` 并传入当前 `temp`，否则走既有 swap/move 分发；`conflict_enabled` 为假时跳过 `r` 抽样直接走既有 swap/move 分发，保证关闭时 RNG 消耗序列与原实现一致
    - 既有 swap/move 分发逻辑逐字节保留
    - _Requirements: 2.1, 2.2, 2.3, 2.4, 2.5, 3.1, 3.2, 3.3, 3.4, 3.5, 3.6, 6.1_

- [x] 5. 同步配置示例文件
  - [x] 5.1 在 `config.toml.example` 与 `moling/config.toml` 的 `[annealing]` 段追加四个配置项
    - `config.toml.example`：四项取默认值 `conflict_probability = 0.0`、`conflict_refresh_interval = 1000`、`conflict_sample_window = 20`、`conflict_weight_by_freq = false`（默认关闭），每项附非空中文行内注释
    - `moling/config.toml`：四项取 `conflict_probability = 0.25`、`conflict_refresh_interval = 1000`、`conflict_sample_window = 20`、`conflict_weight_by_freq = true`（开启本特性），每项附非空中文行内注释
    - _Requirements: 7.1, 7.2, 7.3, 7.4_

- [x] 6. 检查点 - 确保编译与已有测试通过
  - 运行 `cargo build` 与 `cargo test`，确保已实现部分通过；如有问题先行修复，必要时向用户提问。

- [x] 7. 编写正确性属性的属性测试（proptest）
  - [x]* 7.1 添加 proptest 依赖并准备最小 `OptContext` 测试构造
    - 按需在 `Cargo.toml` 的 `[dev-dependencies]` 加入 `proptest`
    - 优先复用现有测试上下文构造；若无则在测试模块内构造最小 `OptContext`（少量字根组、若干带 `parts`/`frequency` 的 `CharInfo`），供 `find_collision_groups` 运行
    - _Requirements: 5.1, 5.2_

  - [x]* 7.2 Property 1：采样窗口落在有效范围内
    - **Property 1: 采样窗口落在有效范围内**
    - 生成随机 `Vec<(usize,usize,usize)>` 与随机 `sample_window`（覆盖 0、大于长度、空列表等边界），断言选中下标 `< min(sample_window, len)`；window=0 或空列表时返回 false 且不修改 assignment
    - 至少运行 100 个用例；注释标注 `// Feature: sa-conflict-operators, Property 1: ...`
    - **Validates: Requirements 4.1, 4.2, 4.3, 4.4**

  - [x]* 7.3 Property 2：排序按所选权重降序
    - **Property 2: 排序按所选权重降序**
    - 生成随机 `assignment`，对 `weight_by_freq` 取 false 与 true 分别断言返回列表相邻元素权重字段非递增
    - 至少运行 100 个用例；注释标注 `// Feature: sa-conflict-operators, Property 2: ...`
    - **Validates: Requirements 5.1, 5.2, 2.5**

  - [x]* 7.4 Property 3：排序结果可复现（裁决确定性）
    - **Property 3: 排序结果可复现（裁决确定性）**
    - 对同一随机 `assignment` 在固定 `weight_by_freq` 下多次调用 `find_collision_groups`，断言返回向量逐元素完全相同
    - 至少运行 100 个用例；注释标注 `// Feature: sa-conflict-operators, Property 3: ...`
    - **Validates: Requirements 5.3**

  - [x]* 7.5 Property 4：两种排序策略产出相同冲突组集合
    - **Property 4: 两种排序策略产出相同冲突组集合**
    - 生成随机 `assignment`，将 false 与 true 两种策略输出按 `(g1,g2)` 归一化后断言集合相等、元素数量相同
    - 至少运行 100 个用例；注释标注 `// Feature: sa-conflict-operators, Property 4: ...`
    - **Validates: Requirements 5.4**

- [x] 8. 评分一致性与零行为变更验证
  - [x]* 8.1 编写评分一致性单元测试
    - 对若干键位状态，确认冲突路径与既有 swap/move 路径最终调用同一 `get_score`、评分数值（含目标偏差、scale、equiv_cv 各分量）完全相等
    - _Requirements: 6.2, 6.3_

  - [x]* 8.2 编写零行为变更验证测试
    - 固定种子下以 `conflict_probability = 0.0` 运行短 SA，验证不额外调用 `find_collision_groups`、RNG 消耗序列与基线一致、结果与基线相同
    - _Requirements: 2.4, 3.5_

- [x] 9. 最终检查点 - 完整构建、测试与向后兼容验证
  - 运行 `cargo build --release` 与 `cargo test`，修复所有失败
  - 验证向后兼容：现有 `config.toml`（缺省新字段）仍能正常解析
  - 验证关闭时零行为变更：`conflict_probability = 0.0` 时分发逻辑与 RNG 序列与基线一致
  - 如有问题先行修复，必要时向用户提问。

## Notes

- 标记 `*` 的子任务为可选测试任务，可为加速 MVP 跳过；核心实现任务不可跳过。
- 每个任务引用其实现的具体需求条款，便于追溯。
- 任务顺序保证每步完成后代码可编译：配置 → 排序函数签名/调用点 → 采样窗口 → 主循环接线 → 配置文件 → 测试。
- 属性测试覆盖设计文档中的四条正确性属性；单元测试覆盖配置默认值/部分解析、采样窗口边界、评分一致性与零行为变更。
- 主循环接线（缓存初始化/刷新、单次抽样分发、温度传入）依赖 RNG 与温度调度，通过单元/集成示例与代码审查验证，不强制纳入属性测试。

## Task Dependency Graph

```json
{
  "waves": [
    { "id": 0, "tasks": ["1.1", "5.1"] },
    { "id": 1, "tasks": ["1.2", "2.1", "7.1"] },
    { "id": 2, "tasks": ["2.2"] },
    { "id": 3, "tasks": ["3.1"] },
    { "id": 4, "tasks": ["3.2"] },
    { "id": 5, "tasks": ["4.1"] },
    { "id": 6, "tasks": ["7.2"] },
    { "id": 7, "tasks": ["7.3"] },
    { "id": 8, "tasks": ["7.4"] },
    { "id": 9, "tasks": ["7.5"] },
    { "id": 10, "tasks": ["8.1"] },
    { "id": 11, "tasks": ["8.2"] }
  ]
}
```
