# Requirements Document

## Introduction

code_genie 是一个使用 Rust 编写的输入法编码方案优化器，核心算法为模拟退火（`src/annealing.rs`）。优化过程中，退火主循环每步调用 `Evaluator::try_move` / `try_swap`（`src/evaluator.rs`）来评估候选解。对全码指标，单步采用 O(组内字数) 的增量更新（`update_char` 的 swap_remove + 增量碰撞）。但当开启「简码优化」（配置 `weights.simple_code.enabled = true`）后，简码指标走的是 `Evaluator::rebuild_simple` → `SimpleEvaluator::full_rebuild`，会对每个简码级别遍历全部汉字重建 HashMap（`build_level`），再全量扫描所有全码桶计算简码重码（`compute_simple_collisions`）。这使单步成本从 O(组内字数) 暴涨到 O(简码级数 × 全部汉字数 + code_space)，导致开启简码优化后速度极其缓慢。

经分析确认的具体放大因素包括：
- 移动被拒绝或触发 `_max` 硬约束回滚时会再做一次全量重建，低温下几乎每步进行两次全量重建。
- `has_simple_impact` 依据的 `group_to_simple_affected[group]` 几乎总是非空，导致几乎每步都触发简码重建。
- 初始化阶段 `multi_start_init` 的多个候选、`enhanced_hill_climb`、`coordinate_descent` 都反复构建并重建 `SimpleEvaluator`，预热阶段的简码计算几乎是无效功。

本特性的目标是：在不改变（或不显著降低）优化质量的前提下，大幅提升开启简码优化后的退火速度。实现策略为直接替换旧实现，仅做性能优化所必需的改动，并通过增量与全量一致性、周期性对账、结束校验来保证最终上报指标的精确性。

本文档使用 EARS 模式描述验收标准，所有需求遵循 INCOSE 质量规则。

## Glossary

- **优化器（Optimizer）**：code_genie 整体程序，执行编码方案优化。
- **退火器（Annealer）**：模拟退火主循环逻辑，位于 `src/annealing.rs`，逐步驱动评估与接受/拒绝决策。
- **主评估器（Evaluator）**：`src/evaluator.rs` 中的 `Evaluator`，维护全码指标并持有简码评估器。
- **简码评估器（SimpleEvaluator）**：`src/evaluator.rs` 中的 `SimpleEvaluator`，负责简码相关指标计算。
- **分配（Assignment）**：字根组到键位的映射 `assignment[group] = key`，是退火搜索的唯一决策变量。
- **简码级别（Simple_Level）**：一个简码等级，对应配置中的一个 `SimpleLevelConfig`，包含 `code_num` 与候选规则。
- **简码编码（Simple_Code）**：某汉字在某简码级别下按规则计算得到的编码值（整数）。
- **简码桶（Simple_Bucket）**：同一简码级别中映射到同一简码编码的候选字集合。
- **候选字集合（Candidate_Set）**：在初始化时按累计字频覆盖率选定的、允许参与出简的汉字集合，频率固定故集合静态不变。
- **出简（Selected_For_Simple）**：某汉字在某简码级别被实际选中分配简码的状态。
- **全码桶（Full_Code_Bucket）**：主评估器中映射到同一全码编码的汉字集合（`code_to_chars`）。
- **首选标记（is_first_candidate）**：标记某汉字在其全码桶中是否为首选字（首选字选重键长为 0，非首选字为 1），仅依赖全码桶，与简码分配无关。
- **简码重码（Simple_Collision）**：全码桶在去除该桶中已出简的字后仍存在的重码，统计结果仅计入简码得分。
- **全码分数（full_score）**：主评估器维护的全码指标得分分量。
- **简码分数（simple_score）**：简码评估器维护的简码指标得分分量。
- **进度（Progress）**：当前退火步数除以总步数，记为 `p = step / total_steps`，取值区间 [0, 1)。
- **激活进度阈值（simple_start_progress）**：简码计算开始参与目标函数的进度阈值，记为 `p_start`。
- **渐进时长（simple_ramp_progress）**：简码权重从 0 渐进到目标权重所跨越的进度长度，记为 `p_ramp`。
- **目标简码权重（W）**：配置项 `weights.simple_code.simple_code_weight`，简码分量在权重曲线终点的有效权重。
- **有效简码权重（w_simple_eff）**：在进度 `p` 处实际作用于简码分数的权重，由权重曲线给出。
- **激活闩锁（Activation_Latch）**：一旦简码计算被激活即永久保持激活的状态标志。
- **激活升温因子（simple_activation_reheat）**：简码激活当刻应用的独立升温倍率，默认 1.0（不升温），与现有 `reheat_factor` 相互独立。
- **周期对账（Periodic_Reconciliation）**：每隔 M 步对全码与简码指标做一次全量重算，用以校正增量计算的浮点漂移，其中 `M = total_steps × reconcile_interval_ratio`。
- **覆盖率（Coverage）**：候选字集合的累计字频之和除以总字频。
- **空格上屏（Space_Commit）**：某简码级别的属性，表示该级简码需要再敲一个空格键才能上屏（即实际击键数比简码键位数多 1）；在 output 文件中以简码末尾追加下划线 `_` 表示。
- **固定简码（Fixed_Simple_Code）**：由配置预先定义的「汉字 → 简码」映射，简码为字面键位串、可选以 `_` 结尾表示空格上屏；固定简码的汉字不参与退火的简码分配。

## Requirements

### 需求 1：简码评估全面增量化

**用户故事：** 作为优化器使用者，我希望开启简码优化后单步评估只触及受影响的少量数据，从而让退火速度大幅提升。

#### 验收标准

1. THE 简码评估器 SHALL 将简码状态保持为分配（Assignment）的确定性纯函数，不引入除分配以外的搜索决策变量。
2. THE 简码评估器 SHALL 为每个简码级别维护增量数据结构，包括简码编码到候选字的桶、桶频率和、出简标记，以及每个候选字的当前简码编码缓存。
3. WHEN 退火器移动一个字根组，THE 简码评估器 SHALL 仅处理该组的 `group_to_simple_affected[group]` 与候选字集合的交集所含的汉字与其所属简码桶。
4. WHEN 一个受影响汉字的简码编码发生变化，THE 简码评估器 SHALL 对其旧简码桶执行移除、对新简码桶执行加入的增量维护，并同步更新对应桶的频率和与出简标记。
5. WHEN 退火器移动一个字根组且该组的受影响交集为空，THE 简码评估器 SHALL 跳过简码重算并保持简码分数不变。

### 需求 2：取消拒绝/回滚时的二次全量重建

**用户故事：** 作为优化器使用者，我希望被拒绝的移动不再触发昂贵的简码重建，从而消除低温阶段每步两次全量重建的开销。

#### 验收标准

1. WHEN 退火器评估一个候选移动，THE 简码评估器 SHALL 计算简码分数的增量变化而不立即提交，仅在该移动被接受时提交增量结果。
2. IF 一个候选移动被拒绝或因 `_max` 硬约束触发回滚，THEN THE 简码评估器 SHALL 通过对受影响的少量桶项做快照还原来恢复状态，而不执行全量重建。
3. WHEN 一个移动被拒绝或回滚后，THE 简码评估器 SHALL 使简码分数与受影响桶状态恢复到该移动之前的值。

### 需求 3：热路径去堆分配

**用户故事：** 作为优化器使用者，我希望简码热路径避免重复堆分配，从而降低单步常数开销。

#### 验收标准

1. THE 简码评估器 SHALL 在简码键位序列与候选字临时集合的计算中复用预分配缓冲区或栈上存储，避免每步新建堆分配的容器。
2. THE 简码评估器 SHALL 以按编码空间直接索引的向量替代以简码编码为键的哈希表来组织简码桶。
3. WHEN 简码热路径产生临时键位序列，THE 简码评估器 SHALL 通过复用缓冲区返回结果而不为每次调用分配新的堆容器。

### 需求 4：两种简码分配模式（全局二选一）

**用户故事：** 作为方案设计者，我希望可以在「字频模式」与「简码效率模式」之间全局选择，从而按不同目标决定争用桶内谁出简。

#### 验收标准

1. THE 优化器 SHALL 在 `weights.simple_code` 下提供全局配置项 `simple_assign_mode`，取值为 `"frequency"`（字频模式）或 `"efficiency"`（简码效率模式），用于二选一地选择简码桶内出简候选的排序键。
2. WHEN 配置文件缺失 `simple_assign_mode`，THE 优化器 SHALL 采用默认值 `"efficiency"`（简码效率模式）。
3. WHERE 配置为字频模式，THE 简码评估器 SHALL 以字频 `freq` 作为简码桶内出简候选的排序键。
4. WHERE 配置为简码效率模式，THE 简码评估器 SHALL 以 `freq * (full_len + sel_len - simple_len)` 作为简码桶内出简候选的排序键，其中 `full_len` 为该字全码码长，`simple_len` 为该级简码码长，`sel_len` 为选重键长。
5. THE 简码评估器 SHALL 将每个（汉字，级别）的 `base_saving = full_len - simple_len` 作为编译期常量计算，其中 `full_len` 等于 `char_infos[ci].parts.len()`，`simple_len` 等于该级 instruction 的步数。
6. THE 简码评估器 SHALL 在该字位于其全码桶时取 `sel_len = 0`（首选字）、否则取 `sel_len = 1`（非首选字），且不考虑候选翻页。
7. WHEN 两个出简候选的排序键相等，THE 简码评估器 SHALL 先按字频 `freq` 裁决、再按汉字索引 `ci` 裁决，以保证结果可复现。
8. THE 简码评估器 SHALL 在两种模式下都使用初始化时按累计字频覆盖率固定选出的候选字集合，且该集合在优化过程中保持不变。

### 需求 5：首选标记 is_first_candidate

**用户故事：** 作为方案设计者，我希望选重键长由全码桶首选关系决定且随全码桶增量维护，从而让简码效率模式的排序键正确且高效。

#### 验收标准

1. THE 主评估器 SHALL 为每个汉字维护首选标记 `is_first_candidate`，该标记表示该汉字是否为其全码桶中的首选字。
2. THE 首选标记 `is_first_candidate` SHALL 仅依赖全码桶，且与简码分配无关。
3. WHEN 一个汉字所在的全码桶因移动发生变化且简码已激活（`simple_active == true`），THE 主评估器 SHALL 增量更新受影响汉字的首选标记 `is_first_candidate`；WHILE 简码未激活（关闭或延迟激活前），THE 主评估器 SHALL 跳过首选标记维护以保持全码热路径基线性能（见需求 25）。
4. WHEN 简码效率模式需要选重键长 `sel_len`，THE 简码评估器 SHALL 在 `is_first_candidate` 为真时取 `sel_len = 0`、为假时取 `sel_len = 1`。

### 需求 6：简码桶内局部排序策略

**用户故事：** 作为优化器使用者，我希望排序只在受影响的少数短候选列表上进行，从而避免维护全局有序结构的开销。

#### 验收标准

1. THE 简码评估器 SHALL 不维护跨简码桶的全局有序结构。
2. WHEN 一个简码桶的候选集合因移动发生变化，THE 简码评估器 SHALL 仅对该受影响简码桶内的候选列表执行局部排序。
3. THE 简码评估器 SHALL 依据当前分配模式所选的排序键及其裁决键执行简码桶内的局部排序。

### 需求 7：用累计字频覆盖率挑选候选字

**用户故事：** 作为方案设计者，我希望用「累计字频覆盖率」选出可出简的候选字集合，从而把简码计算限定在高频字上并减少无效重算。

#### 验收标准

1. THE 优化器 SHALL 在 `weights.simple_code` 下提供配置项 `simple_coverage_ratio`，用累计字频覆盖率来选出可出简的候选字集合。
2. WHEN 配置文件缺失 `simple_coverage_ratio`，THE 优化器 SHALL 采用默认值 `0.90`。
3. WHEN 优化器启动，THE 优化器 SHALL 按字频降序累加，直至累计覆盖率达到配置阈值，一次性确定候选字集合。
4. THE 候选字集合 SHALL 在整个优化过程中保持不变。
5. WHEN 简码评估器构建某简码级别，THE 简码评估器 SHALL 仅遍历候选字集合而不遍历全部汉字。
6. WHEN 优化器启动，THE 优化器 SHALL 预先计算 `group_to_simple_affected[group]` 与候选字集合的交集，使仅影响非候选字的移动不触发简码重算。
7. WHEN 优化器进入「配置确认」日志阶段，THE 优化器 SHALL 输出实际达到的覆盖率与对应的候选字数。

### 需求 8：退火后期激活简码计算

**用户故事：** 作为优化器使用者，我希望简码计算在退火后期才激活，从而让预热与早期探索避免无效的简码开销。

#### 验收标准

1. THE 退火器 SHALL 以进度 `p = step / total_steps` 作为简码计算的激活触发条件。
2. WHEN 配置文件缺失 `simple_start_progress`，THE 优化器 SHALL 采用默认值 `0.6`。
3. WHEN 进度首次达到或超过 `simple_start_progress`，THE 退火器 SHALL 激活简码计算。
4. WHILE 简码计算已激活，THE 退火器 SHALL 通过激活闩锁保持激活状态，不因后续温度非单调（如升温）而关闭简码计算。
5. WHILE 进度小于 `simple_start_progress`，THE 退火器 SHALL 使简码分数对目标函数的贡献为 0。
6. WHEN 简码计算被激活，THE 简码评估器 SHALL 执行一次全量构建以初始化增量状态。

### 需求 9：简码权重渐进曲线

**用户故事：** 作为方案设计者，我希望简码权重从激活点起按平滑曲线渐进到目标权重，从而避免目标函数突变造成的解抖动。

#### 验收标准

1. WHEN 配置文件缺失 `simple_ramp_progress`，THE 优化器 SHALL 采用默认值 `0.1`。
2. WHILE 进度 `p` 小于 `simple_start_progress`，THE 退火器 SHALL 取有效简码权重 `w_simple_eff = 0`。
3. WHILE 进度 `p` 满足 `simple_start_progress <= p < simple_start_progress + simple_ramp_progress`，THE 退火器 SHALL 以 `α = (p - p_start) / p_ramp`、`s = α * α * (3 - 2 * α)` 计算有效简码权重 `w_simple_eff = W * s`。
4. WHILE 进度 `p` 大于或等于 `simple_start_progress + simple_ramp_progress`，THE 退火器 SHALL 取有效简码权重 `w_simple_eff = W`。
5. THE 退火器 SHALL 以 `total = weight_full_code * full_score + w_simple_eff(p) * simple_score` 计算综合得分。

### 需求 10：分量分数缓存

**用户故事：** 作为优化器使用者，我希望全码分数与简码分数分别增量维护，从而在权重随时间变化时仍能 O(1) 合成综合得分。

#### 验收标准

1. THE 主评估器 SHALL 分别增量维护全码分数 `full_score` 与简码分数 `simple_score`。
2. WHILE 简码计算尚未激活，THE 主评估器 SHALL 使简码分数 `simple_score` 恒为 0。
3. WHEN 需要综合得分，THE 主评估器 SHALL 以当前有效简码权重将 `full_score` 与 `simple_score` 实时合并，而不重新全量计算分数。

### 需求 11：最佳解按分量存储与可比较

**用户故事：** 作为优化器使用者，我希望最佳解按分量存储并用当前权重重算，从而解决目标函数随时间移动导致最佳解被冻结的问题。

#### 验收标准

1. THE 退火器 SHALL 以全码分量 `best_full_score` 与简码分量 `best_simple_score` 的形式存储最佳解。
2. WHEN 退火器比较当前解与最佳解，THE 退火器 SHALL 以当前有效简码权重重算最佳解的综合得分 `best_total`，并基于重算结果进行比较。
3. THE 退火器 SHALL 以 O(1) 复杂度完成最佳解综合得分的重算。

### 需求 12：激活升温为独立可选参数

**用户故事：** 作为方案设计者，我希望激活当刻的升温是独立可选参数，从而与既有 reheat 机制解耦且默认不改变行为。

#### 验收标准

1. THE 优化器 SHALL 提供独立配置项 `simple_activation_reheat`，默认值为 1.0 表示不升温。
2. WHEN 简码计算被激活，THE 退火器 SHALL 应用 `simple_activation_reheat` 作为升温倍率。
3. THE 退火器 SHALL 使 `simple_activation_reheat` 独立于现有 `reheat_factor`，不复用后者的取值。
4. WHERE 现有 reheat 与扰动等配置均关闭，THE 退火器 SHALL 仍正确执行简码激活与权重渐进的核心逻辑。

### 需求 13：激活相关配置校验与告警

**用户故事：** 作为优化器使用者，我希望对激活与渐进参数做边界校验与告警，从而避免无效配置导致权重升不到目标值。

#### 验收标准

1. THE 优化器 SHALL 将 `simple_start_progress` 钳制到区间 [0, 1)。
2. IF `simple_start_progress` 大于或等于 1，THEN THE 优化器 SHALL 输出告警并将其钳制到有效区间。
3. IF `simple_start_progress` 与 `simple_ramp_progress` 之和大于 1，THEN THE 优化器 SHALL 输出告警并对取值进行钳制。
4. IF `simple_start_progress` 或 `simple_ramp_progress` 为负值，THEN THE 优化器 SHALL 将其钳制为 0。
5. WHERE `simple_start_progress` 与 `simple_ramp_progress` 均为 0，THE 退火器 SHALL 从优化开始即硬激活简码计算作为兼容档。

### 需求 14：简码重码语义保持不变

**用户故事：** 作为方案设计者，我希望简码重码的语义与原实现完全一致，从而保证优化质量不被改变。

#### 验收标准

1. THE 主评估器 SHALL 使全码重码计算（`total_collisions` 与 `collision_frequency`）独立于出简状态，不因出简而改变。
2. THE 简码评估器 SHALL 通过在全码桶中去除已出简的字后统计剩余重码来计算简码重码，并仅将结果计入简码分数。
3. THE 简码评估器 SHALL 仅在受影响的少数全码桶上增量更新简码重码，而不每次全量扫描整个编码空间。
4. THE 简码评估器 SHALL 使简码重码的计算结果与原全量实现在相同分配下保持一致。

### 需求 15：增量与全量一致性、周期对账与结束校验

**用户故事：** 作为优化器使用者，我希望增量计算与全量计算结果一致并周期性对账，从而保证最终上报指标精确无漂移。

#### 验收标准

1. FOR ALL 分配，THE 简码评估器 SHALL 使增量维护得到的简码指标与全量重建得到的简码指标一致。
2. THE 优化器 SHALL 在 `weights.simple_code` 下提供比例型配置项 `reconcile_interval_ratio`，并以 `M = total_steps × reconcile_interval_ratio`（向下取整且不小于 1）确定周期对账间隔步数 M，而不使用固定步数。
3. WHEN 配置文件缺失 `reconcile_interval_ratio`，THE 优化器 SHALL 采用默认值 `0.05`。
4. THE 退火器 SHALL 每隔 M 步执行一次全码与简码指标的全量重算以校正浮点漂移。
5. WHEN 周期对账执行，THE 退火器 SHALL 以全量重算结果替换当前增量维护的全码与简码指标值。
6. WHEN 优化结束，THE 优化器 SHALL 强制执行一次全量重建校验，使最终上报的全码与简码指标为精确值。

### 需求 16：日志增强

**用户故事：** 作为优化器使用者，我希望日志清晰反映简码激活、权重渐进与分量分数，从而便于观察与调参。

#### 验收标准

1. WHEN 简码计算被激活，THE 退火器 SHALL 输出「简码已激活」事件日志。
2. WHILE 处于权重渐进期，THE 退火器 SHALL 在 `simple_start_progress + (i / 10) * simple_ramp_progress`（i 取 0 到 9）共 10 个进度点输出当前有效简码权重，并包含进度、`α` 与有效权重。
3. WHEN 权重渐进结束，THE 退火器 SHALL 输出「已达目标权重 W」日志。
4. WHEN 退火器输出当前分数与最佳分数日志，THE 退火器 SHALL 在总分之外额外输出全码分量 `weight_full_code * full_score` 与简码分量 `w_simple_eff * simple_score`。
5. THE 退火器 SHALL 以当前有效简码权重重算最佳解的分量，使最佳解分量在跨步之间可比较。
6. WHEN 优化器进入「配置确认」阶段，THE 优化器 SHALL 输出候选字覆盖率与候选字数。

### 需求 17：新增配置项与向后兼容

**用户故事：** 作为优化器使用者，我希望新行为以合理默认值直接替换旧实现，从而无需修改既有配置即可获得正确且更快的优化。

#### 验收标准

1. THE 优化器 SHALL 新增配置项 `simple_start_progress`（默认 `0.6`）、`simple_ramp_progress`（默认 `0.1`）、`simple_activation_reheat`（默认 `1.0`）、`simple_coverage_ratio`（默认 `0.90`）、`reconcile_interval_ratio`（默认 `0.05`）以及简码分配模式选择项 `simple_assign_mode`（默认 `"efficiency"`）。
2. WHEN 配置文件缺失上述新增配置项，THE 优化器 SHALL 为每个缺失项采用预设的默认值。
3. THE 优化器 SHALL 以增量化的新简码实现直接替换旧的 `full_rebuild` 热路径实现。
4. WHERE `weights.simple_code.enabled` 为假，THE 优化器 SHALL 跳过简码评估，使行为与未启用简码时一致。
5. THE 优化器 SHALL 使在相同分配与相同有效权重下的最终上报指标与旧实现保持一致。

### 需求 18：新增配置项同步到示例与运行配置文件

**用户故事：** 作为优化器使用者，我希望优化器新增的所有配置项同步出现在示例配置与运行配置文件中，从而无需查阅代码即可了解并设置这些配置项。

#### 验收标准

1. THE 优化器 SHALL 将新增的全部配置项（`simple_start_progress`、`simple_ramp_progress`、`simple_activation_reheat`、`simple_coverage_ratio`、`reconcile_interval_ratio`、`simple_assign_mode`）同步写入 `config.toml.example` 文件。
2. THE 优化器 SHALL 将新增的全部配置项同步写入 `moling/config.toml` 文件。
3. WHEN 新增配置项写入上述配置文件，THE 优化器 SHALL 为每个配置项附带注释说明与对应默认值。
4. THE 优化器 SHALL 使两个配置文件中新增配置项的默认值与代码内置默认值一致。

### 需求 19：最小改动约束

**用户故事：** 作为代码维护者，我希望本特性只专注性能优化及相关的正确性校验与日志输出，从而避免无关重构扩大改动范围与回归风险。

#### 验收标准

1. THE 优化器 SHALL 仅修改与简码评估增量化、简码激活与权重渐进、相关配置项、相关日志输出、增量与全量一致性校验直接相关的代码。
2. THE 优化器 SHALL 不引入与上述性能优化目标无关的重构。
3. WHERE 某处代码与本特性的性能优化、正确性校验与日志输出目标无直接关联，THE 优化器 SHALL 保持该处代码不变。

### 需求 20：简码级别的空格上屏配置

**用户故事：** 作为方案设计者，我希望每个简码级别能声明「是否需要空格上屏」，从而让简码效率模式的码长计算反映真实击键成本，并在输出中标记需要空格上屏的简码。

#### 验收标准

1. THE 优化器 SHALL 在每个 `[[simple_levels]]` 级别配置下提供布尔配置项 `space_commit`，表示该级简码是否需要空格上屏。
2. WHEN 某简码级别配置缺失 `space_commit`，THE 优化器 SHALL 采用默认值 `false`。
3. WHERE 某简码级别的 `space_commit` 为真且分配模式为简码效率模式，THE 简码评估器 SHALL 在效率排序键中将该级简码长度 `simple_len` 视为「该级指令步数加 1」，即该级 `base_saving = full_len - (simple_len + 1)`。
4. THE 简码评估器 SHALL 将空格上屏导致的简码长度加 1 作为退火优化前已知的确定值，在初始化期一次性预计算到 `simple_base_saving`，且该值不随分配变化。
5. WHERE 某简码级别的 `space_commit` 为真，WHEN 优化器将该级简码输出到 output 文件，THE 优化器 SHALL 在该简码末尾追加一个下划线 `_`（例如简码 `j` 输出为 `j_`）。
6. WHERE 某简码级别的 `space_commit` 为假，THE 优化器 SHALL 不改变该级简码的长度计算，且输出时不追加下划线。
7. WHERE 某简码级别的 `space_commit` 为真，THE 简码评估器 SHALL 在该级简码的有效击键序列末尾计入一个空格键（`KEY_SPACE`），使该空格键计入加权当量（末位键到空格的转移当量）与分布偏差（`key_usage[KEY_SPACE]` 与 `key_presses` 各计一次），并使效率排序键中的简码长度加 1。
8. WHERE 某简码级别的 `space_commit` 为假，THE 简码评估器 SHALL 不在该级简码计入尾随空格键，使加权当量与分布偏差均不含该空格、且效率排序键长度不加 1。
9. THE 字频模式排序键（以 `freq` 为键）SHALL 不受空格上屏影响。

### 需求 21：固定简码映射

**用户故事：** 作为方案设计者，我希望在 config.toml 中预先定义少量「汉字 → 简码」映射，从而让这些汉字使用固定简码、不参与退火分配，且与按覆盖率选取候选字的计算解耦。

#### 验收标准

1. THE 优化器 SHALL 在 `config.toml` 中以内联表 `[fixed_simple_codes]` 提供固定简码映射（形如 `"汉字" = "简码"`），其中简码为字面键位串。
2. THE 固定简码的简码字符串 SHALL 允许以下划线 `_` 结尾，表示该简码需要空格上屏。
3. THE 优化器 SHALL 使固定简码映射不影响按 `simple_coverage_ratio` 从全集汉字选取候选字集合的计算，即候选字集合仍以全集汉字按字频覆盖率选取。
4. WHEN 候选字集合选取完成后，THE 优化器 SHALL 从候选字集合中移除已在固定简码映射中预先分配的汉字，使其不参与退火的简码分配。
5. THE 优化器 SHALL 根据固定简码去除结尾下划线后的码长确定其所属简码级别（码长等于该级简码键位数的级别）。
6. THE 优化器 SHALL 以固定简码自身的结尾下划线为输出与长度/当量/分布计算的依据（固定简码即最终输出状态，不依据级别 `space_commit` 重建下划线）；并对结尾下划线与级别 `space_commit` 的一致性做非对称校验：
   - IF 某固定简码以 `_` 结尾但其所属级别的 `space_commit` 为假，THEN THE 优化器 SHALL 在解析配置时报错（终止）。
   - WHERE 某固定简码不以 `_` 结尾但其所属级别的 `space_commit` 为真，THE 优化器 SHALL 仅输出告警并按固定简码原样接受（不额外添加下划线）。
7. WHEN 简码评估器为某级别的某个简码编码桶选取出简候选，THE 简码评估器 SHALL 仅分配 `max(0, code_num − 该桶固定占用数)` 个优化简码；固定占用数本身不受 `code_num` 限制（固定简码为权威预分配，可达到或超过 `code_num`，此时该桶退火不再分配），且固定简码不因占用达到/超过 `code_num` 而被拒绝。
8. THE 简码评估器 SHALL 将固定简码的汉字视为已出简：其字频计入简码覆盖率、其简码键位（含其自身结尾下划线对应的尾随空格）计入加权当量与分布偏差，且在简码重码统计中作为「已出简」从全码桶排除。
9. THE 固定简码的汉字对简码各项指标的贡献 SHALL 为不随分配变化的常量（因固定简码为字面键位串，与字根到键位的分配无关）。
10. WHEN 优化器将简码输出到 output 文件，THE 优化器 SHALL 输出固定简码并保留其结尾下划线（如需空格上屏）。
11. THE 优化器 SHALL 在 `config.toml.example` 与 `moling/config.toml` 中提供固定简码的注释示例（默认整段注释掉），为「不、是、我、的、了」分别示例简码「u、i、o、e、a」，仅用于表达配置格式。
12. WHERE 某 `[[simple_levels]]` 级别的 `code_num` 为 0，THE 优化器 SHALL 使归属该级的固定简码仍然生效（输出并从候选集排除），且该级退火不分配任何简码；为此 `get_simple_code_config` SHALL 保留「有固定简码按码长归属到其上」的 `code_num=0` 级别，并丢弃无固定简码归属的 `code_num=0` 级别以避免无谓开销。

### 需求 22：简码长度必须严格短于全码

**用户故事：** 作为方案设计者，我希望系统保证任何被分配的简码长度都严格短于对应字的全码长度，从而避免出现「简码不比全码短」的无意义简码。

#### 验收标准

1. THE 简码评估器 SHALL 仅当某候选字在某级别的有效简码长度严格小于该字全码长度时，才允许该字在该级别出简；其中有效简码长度等于该级指令步数加（`space_commit` 为真时再加 1），全码长度等于 `char_infos[ci].parts.len()`。
2. WHERE 某候选字在某级别的有效简码长度大于或等于其全码长度，THE 简码评估器 SHALL 不将该字纳入该级别的简码桶，使其在该级别不出简。
3. THE 优化器 SHALL 对固定简码施加同一约束：IF 某固定简码的有效长度（含空格上屏）大于或等于对应字的全码长度，THEN THE 优化器 SHALL 输出告警并拒绝该条固定简码。
4. THE 该长度约束 SHALL 在退火优化前由静态预计算确定，并在全量重建与增量更新两条路径上保持一致。

### 需求 24：warmup/坐标下降按调用上下文区分简码（Init/校准关闭、最终精炼增量）

**用户故事：** 作为优化器使用者，我希望多起点初始化（warmup）与坐标下降在「Init/校准」上下文不做任何简码计算以消除全量重建开销，而在「SA 结尾最终精炼」上下文保持简码以增量方式持续更新，使精炼分数与主循环最佳分同口径、接受判定正确。

#### 验收标准

1. THE `enhanced_hill_climb`（含 `hill_climb_warmup`）与 `coordinate_descent` SHALL 接受一个 `disable_simple: bool` 参数以区分调用上下文：`true` 表示 Init/校准预热，`false` 表示 SA 结尾最终精炼。
2. WHEN `disable_simple == true` 且简码已启用，THE 优化器 SHALL 通过 `Evaluator::new_full_only`（`build_simple=false`）构建评估器，**跳过急切 `SimpleEvaluator` 构建**，使 `simple_eval = None`、`simple_active=false`、`current_simple_weight=0.0`，整个过程不执行任何简码计算（`has_simple_impact` 恒为 false，`coordinate_descent` 的 probe-then-revert 循环不进入简码分支），分配决策退化为纯全码（与 SA 主循环 `p < p_start` 阶段语义一致）。
3. WHEN `disable_simple == false` 且简码已启用，THE 优化器 SHALL 通过 `Evaluator::new` 保持急切构建的 `SimpleEvaluator` 激活状态（`simple_active=true`、`current_simple_weight=weight_simple_code`），并在 probe/回滚/应用最优三处均以**增量**方式更新简码（`coordinate_descent` 使用 `apply_simple_for_move` + `rollback_simple`/`commit_simple`，禁止调用 `rebuild_simple`；`enhanced_hill_climb` 沿用算子内置的 try_move/try_swap/try_triple_swap 增量简码）。
4. WHERE 最终精炼以 `disable_simple == false` 运行，THE 精炼返回分数 SHALL 与 SA 主循环 `best_score` 同口径（均为 `weight_full_code·full + weight_simple_code·simple`），从而使「`final_score < best_score`」「`cd_score < best_score`」的接受判定在简码维度上正确，不会以纯全码分误判。
5. THE 调用上下文 SHALL 按如下绑定：`multi_start_init` 内部的 `hill_climb_warmup`（×候选数）与 `coordinate_descent` 传 `true`；`simulated_annealing` 结尾最终精炼的 `hill_climb_warmup` 与 `coordinate_descent` 传 `false`。
6. THE 上述改动 SHALL 不影响这两个函数的全码计算逻辑、分配结果质量及行为；当简码整体关闭（`enable_simple_code=false`）时 `disable_simple` 参数无实际效果，`new_full_only` 与 `new` 均产出 `simple_eval=None`，两条路径均与基线一致（零变化）。
7. THE 校准阶段的 `smart_init` SHALL 同样受益：校准用的初始分配经 `disable_simple=true` 的全码引导生成（全程零简码构建），校准完成后 `initial_eval = Evaluator::new(...)` 重建时由急切构建的 `SimpleEvaluator` 提供**唯一一次**全量简码指标观测（ScaleConfig 观测来源），语义正确。
8. THE `new_full_only` 优化 SHALL 消除 `multi_start_init` 中被丢弃的急切简码构建（每候选一次 + 坐标下降一次，约 `候选数+1` 次/阶段）：校准阶段因此「全码优化 0 次简码构建 + 观测 1 次」；Init 阶段（`simulated_annealing` → `multi_start_init`）同样消除这些被丢弃构建，仅保留 `multi_start_init` 返回后 SA 主循环自身的**一次**必要 `Evaluator::new`（延迟激活的工作评估器）。

### 需求 23：输出镜像评估器的出简选择

**用户故事：** 作为方案设计者，我希望输出文件的简码方案与评估器优化所用的出简选择完全一致，从而保证交付方案与被优化、被上报的指标相对应。

#### 验收标准

1. THE 优化器 SHALL 使输出文件（`output-simple-codes.txt` 与 `output-combined.txt`）的简码出简选择与评估器（`SimpleEvaluator`）的出简选择逐字一致，包括 `simple_assign_mode`（efficiency/frequency）排序键、`sel_len`、固定简码占用扣减与跨级排除。
2. THE 优化器 SHALL 使输出的简码顺序与评估器分配简码的顺序一致：按级别升序、同级别按简码桶编码升序、桶内按选择排序键 `cmp_in_bucket`。
3. WHERE 某简码桶被固定简码占用，THE 优化器 SHALL 使该桶输出的优化出简数不超过 `code_num` 减去固定占用，且「固定占用 + 优化出简」不超过 `code_num`。
4. THE 优化器 SHALL 在输出文件中保留固定简码及其结尾下划线，且不与优化出简重复。

### 需求 25：首选字维护仅在简码激活时进行（保持全码路径基线性能）

**用户故事：** 作为优化器使用者，我希望简码关闭或尚未激活时，全码优化热路径不为简码维护任何额外状态，从而使全码优化的行为、逻辑与性能与基线版本完全一致。

#### 验收标准

1. WHILE `simple_active == false`（简码关闭或延迟激活前），THE 主评估器 SHALL 在 `update_char` 中跳过首选字（`is_first_candidate`/`bucket_first`）的全部维护，仅维护全码桶聚合（`bucket_freq_sum`/`bucket_max_freq`/碰撞计数等），其行为与基线版本逐字节等价。
2. WHEN `update_char` 在 `simple_active == false` 下需要重扫桶最大频率，THE 主评估器 SHALL 使用仅求最大频率的 `rescan_bucket_max`（不跟踪首选字），而非求 (max, 首选) 的 `rescan_bucket_first`。
3. WHEN 简码计算被激活（`activate_simple`），THE 主评估器 SHALL 在构建 `SimpleEvaluator` 之前，据当前 `code_to_chars` 一次性全量重建 `is_first_candidate`/`bucket_first`，使其反映激活时刻的分配。
4. THE 激活时一次性重建的首选字结果 SHALL 与「自始至终对每步移动增量维护首选字」在激活时刻的状态完全一致（正确性保证）。
5. THE 「激活前不维护、激活时重建」策略 SHALL 成立，因为激活前没有任何读者读取首选标记（`has_simple_impact` 在 `!simple_active` 时短路返回 false，简码分量贡献为 0），且其重建开销为 O(字数)，远小于激活前在每步移动里反复增量维护的累计开销。
6. WHERE 简码整体关闭（`enable_simple_code == false`），THE 全码优化的行为、逻辑与单步热路径性能 SHALL 与基线版本 `27fcc6d` 保持一致（仅允许日志层面的差异）。
