# Requirements Document

## Introduction

本特性将现有的"冲突导向邻域算子"（`try_resolve_conflict`，可选地包含 `try_key_reorganization` / `try_triple_swap`）集成进模拟退火（SA）主循环 `simulated_annealing`。当前这些算子仅被 `enhanced_hill_climb` 使用。集成的目的，是让 SA 主循环能够更直接地降低重码数（collision_count）与重码率（collision_rate），从而更高效地逼近用户的硬性优化目标（重码数 < 750、重码率 < 0.00035、当量 < 90、分布 < 10）。

为避免 `find_collision_groups`（复杂度为 O(num_chars) 加 HashMap 构建，单步执行过于昂贵）拖慢主循环，SA 主循环将维护一份缓存的冲突列表，并每隔 N 步重建一次。每一步以概率 `conflict_probability` 执行冲突导向移动；当该概率为 0 时，行为与现状完全一致。

本特性新增四个配置项，全部带 serde 默认值，保证旧配置文件在缺省这些字段时仍能正常解析且行为不变。本特性不修改评分/评估逻辑（`get_score`、`scale`、`targets`、`equiv_cv` 等），仅作用于 SA 主循环的邻域选择与冲突缓存。本分支仅涉及 SA，不包含 AMHB。

## Glossary

- **SA（模拟退火）主循环**: `src/annealing.rs` 中的 `simulated_annealing` 函数，逐步执行邻域移动并按 Metropolis 准则接受/回滚。
- **Conflict_Module（冲突导向模块）**: 本特性新增的、嵌入 SA 主循环的逻辑，负责维护冲突缓存、按概率分发冲突导向移动。
- **Collision_Cache（冲突缓存）**: SA 主循环中持有的、由 `find_collision_groups` 产出的冲突组列表，类型为 `Vec<(usize, usize, usize)>`，分别表示冲突字根组对及其参与的汉字数量。
- **find_collision_groups**: `src/annealing.rs` 中构建冲突组列表的函数，返回已排序的冲突列表。
- **try_resolve_conflict**: 冲突导向邻域算子，从冲突列表中采样一对冲突组并尝试将其中之一移动到新键位，内部调用 `evaluator.try_move`。
- **try_move / try_swap**: `Evaluator` 上现有的邻域操作方法，内部执行 Metropolis 接受 + 回滚，并使用完整的 `get_score`。
- **swap_probability**: 现有配置项，控制 SA 每步选择交换移动还是点移动的概率。
- **conflict_probability**: 新增配置项（f64，默认 0.0），表示某一步使用冲突导向移动的概率；0 表示关闭本特性。
- **conflict_refresh_interval**: 新增配置项（usize，默认 1000），表示每隔多少步重建一次冲突缓存。
- **conflict_sample_window**: 新增配置项（usize，默认 20），表示在排序后的前 N 个冲突组中采样，取代 `try_resolve_conflict` 中硬编码的 20。
- **conflict_weight_by_freq**: 新增配置项（bool，默认 false），表示冲突组是否按频率加权排序。
- **AnnealingConfig**: `src/config.rs` 中的配置结构体，对应 TOML 的 `[annealing]` 段。
- **collision_count（重码数）/ collision_rate（重码率）**: 优化指标，分别为重码字数与按频率加权的重码占比。
- **频率加权排序**: 冲突组按其参与汉字的频率之和降序排序（针对 collision_rate）；与之相对的是按参与汉字数量降序排序（现状，针对 collision_count）。

## Requirements

### 需求 1：新增冲突导向配置项并保持向后兼容

**User Story:** 作为优化器的使用者，我希望通过配置开关来启用或调节冲突导向移动，以便在不影响已有配置文件的前提下控制该特性。

#### 验收标准

1. THE AnnealingConfig SHALL 包含一个名为 conflict_probability 的 f64 字段，取值范围为 0.0 至 1.0（含两端），并通过 serde 默认值机制在该字段缺失时取值 0.0。
2. THE AnnealingConfig SHALL 包含一个名为 conflict_refresh_interval 的 usize 字段，并通过 serde 默认值机制在该字段缺失时取值 1000。
3. THE AnnealingConfig SHALL 包含一个名为 conflict_sample_window 的 usize 字段，并通过 serde 默认值机制在该字段缺失时取值 20。
4. THE AnnealingConfig SHALL 包含一个名为 conflict_weight_by_freq 的 bool 字段，并通过 serde 默认值机制在该字段缺失时取值 false。
5. WHEN 一个 `[annealing]` 配置缺失上述四个字段中的任意一个或多个被解析，THE AnnealingConfig SHALL 成功完成解析（不产生错误），并将每个缺失字段取其默认值（conflict_probability=0.0、conflict_refresh_interval=1000、conflict_sample_window=20、conflict_weight_by_freq=false）。
6. WHEN 一个 `[annealing]` 配置中部分新字段存在、部分缺失被解析，THE AnnealingConfig SHALL 对已存在字段取其显式值，对缺失字段取其默认值。
7. THE Default 实现（`Default for Config`）SHALL 将 conflict_probability 设为 0.0、conflict_refresh_interval 设为 1000、conflict_sample_window 设为 20、conflict_weight_by_freq 设为 false。

### 需求 2：维护并定期刷新冲突缓存

**User Story:** 作为优化器的开发者，我希望 SA 主循环缓存冲突列表并定期重建，以便冲突导向移动可用，同时避免每步重算带来的性能损耗。

#### 验收标准

1. WHERE conflict_probability 大于 0.0，WHEN SA 主循环开始执行，THE SA_主循环 SHALL 通过 `find_collision_groups` 初始化 Collision_Cache，并将刷新计数清零。
2. WHERE conflict_probability 大于 0.0 且 conflict_refresh_interval 大于 0，WHEN 自上次构建 Collision_Cache 以来已执行满 conflict_refresh_interval 步，THE SA_主循环 SHALL 通过 `find_collision_groups` 重建一次 Collision_Cache 并将刷新计数清零。
3. WHERE conflict_probability 大于 0.0 且 conflict_refresh_interval 等于 0，THE SA_主循环 SHALL 仅保留初始化得到的 Collision_Cache 且不再重建。
4. WHERE conflict_probability 等于 0.0，THE SA_主循环 SHALL 不初始化、不维护、不刷新 Collision_Cache，且不调用 `find_collision_groups`。
5. THE Collision_Cache SHALL 保存 `find_collision_groups` 按当前排序策略（见需求 5）返回的已排序冲突组列表。

### 需求 3：按概率执行冲突导向移动

**User Story:** 作为优化器的使用者，我希望 SA 每步以一定概率执行冲突导向移动，以便更直接地降低重码数与重码率。

#### 验收标准

1. WHEN SA 主循环执行某一步，THE SA_主循环 SHALL 在 [0.0, 1.0) 区间内进行恰好一次均匀随机抽样得到 r，并以同一个 r 作为本步所有分发判定的依据。
2. WHEN SA 主循环执行某一步且 r 小于 conflict_probability 且 Collision_Cache 非空，THE SA_主循环 SHALL 调用 `try_resolve_conflict` 恰好一次执行冲突导向移动，且本步不再执行由 swap_probability 控制的交换/点移动分发。
3. IF SA 主循环执行某一步时 r 不小于 conflict_probability，THEN THE SA_主循环 SHALL 退回到由 swap_probability 控制的既有交换/点移动分发逻辑，且不调用 `try_resolve_conflict`。
4. IF SA 主循环执行某一步时 r 小于 conflict_probability 但 Collision_Cache 为空，THEN THE SA_主循环 SHALL 退回到由 swap_probability 控制的既有交换/点移动分发逻辑，且不调用 `try_resolve_conflict`。
5. WHERE conflict_probability 等于 0.0，THE SA_主循环 SHALL 在所有步骤上执行与本特性引入前完全一致的交换/点移动分发逻辑，且不调用 `try_resolve_conflict`。
6. WHEN 调用 `try_resolve_conflict`，THE SA_主循环 SHALL 传入当前步所用的温度值，以保持 Metropolis 接受/回滚判定与既有移动一致。

### 需求 4：冲突导向移动的采样窗口可配置

**User Story:** 作为优化器的使用者，我希望控制冲突采样窗口大小，以便调节冲突导向移动聚焦于最严重冲突的程度。

#### 验收标准

1. WHEN 调用 `try_resolve_conflict` 且有效窗口长度大于 0，THE try_resolve_conflict SHALL 在排序后 Collision_Cache 的前 min(conflict_sample_window, Collision_Cache 长度) 个元素范围内，以均匀随机方式选取恰好一个冲突组（取代此前硬编码的数值 20）。
2. WHEN conflict_sample_window 大于或等于 Collision_Cache 的长度，THE try_resolve_conflict SHALL 在整个 Collision_Cache 范围内采样。
3. IF conflict_sample_window 等于 0，THEN THE try_resolve_conflict SHALL 不执行任何移动并返回 false。
4. IF Collision_Cache 为空，THEN THE try_resolve_conflict SHALL 不执行任何移动并返回 false。

### 需求 5：冲突组排序策略可配置

**User Story:** 作为优化器的使用者，我希望选择冲突组按数量排序还是按频率加权排序，以便分别针对重码数或重码率进行优化。

#### 验收标准

1. WHERE conflict_weight_by_freq 等于 false，THE find_collision_groups SHALL 按冲突组参与汉字的数量（参与该冲突编码的汉字个数）降序排序冲突列表（保持现有行为）。
2. WHERE conflict_weight_by_freq 等于 true，THE find_collision_groups SHALL 按冲突组参与汉字的频率之和（参与汉字字频之和）降序排序冲突列表。
3. WHEN 排序键相等（数量相等或频率之和相等），THE find_collision_groups SHALL 采用稳定的次序裁决规则，使排序结果可复现。
4. THE find_collision_groups SHALL 在两种排序策略下返回元素相同、数量相同的冲突组集合，仅排列顺序不同。

### 需求 6：保持评分逻辑不变

**User Story:** 作为优化器的开发者，我希望冲突导向移动复用现有评分，以便目标偏差/scale/equiv_cv 评分语义保持一致、无需改动评估器。

#### 验收标准

1. WHEN Conflict_Module 执行冲突导向移动，THE try_resolve_conflict SHALL 通过 `evaluator.try_move` 依据 `get_score` 分数与 Metropolis 准则进行接受/回滚判定：接受时保留新键位，回滚时恢复移动前键位。
2. THE Conflict_Module SHALL NOT 修改 `Evaluator` 的 `get_score` 计算、`scale`、`targets` 或 `equiv_cv` 相关逻辑。
3. WHEN 同一键位状态分别经由冲突导向路径与既有交换/点移动路径触发评分，THE Evaluator SHALL 返回数值完全相等的分数（含目标偏差、scale、equiv_cv 各分量）。

### 需求 7：配置示例文件同步更新

**User Story:** 作为优化器的使用者，我希望配置示例文件包含新参数及说明，以便了解如何启用与调节本特性。

#### 验收标准

1. THE config.toml.example SHALL 在 `[annealing]` 段中包含 conflict_probability、conflict_refresh_interval、conflict_sample_window、conflict_weight_by_freq 四个配置项，且每项附带说明其用途的非空中文行内注释。
2. THE moling/config.toml SHALL 在 `[annealing]` 段中包含上述四个配置项，且每项附带说明其用途的非空中文行内注释。
3. THE moling/config.toml SHALL 将 conflict_probability 赋值为 0.25、conflict_refresh_interval 赋值为 1000、conflict_sample_window 赋值为 20、conflict_weight_by_freq 赋值为 true，以开启本特性。
4. THE config.toml.example SHALL 将四个配置项分别赋值为各自默认值（conflict_probability=0.0、conflict_refresh_interval=1000、conflict_sample_window=20、conflict_weight_by_freq=false），以保持示例配置默认关闭本特性。
