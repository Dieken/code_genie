# Requirements Document

## Introduction

本功能将 CodeGenie 模拟退火优化器的评分机制从"绝对值最小化"改造为"目标偏差导向"。
用户可以在 `config.toml` 中为全码和简码的各项指标设置优化目标值，优化器将以"超出目标的程度"
作为惩罚依据，而非无限制地追求指标最小化。同时引入硬约束（`_max` 后缀字段），在退火过程中
直接拒绝超出上限的方案，以加速收敛。

此外，本功能允许用户在 `config.toml` 中手动配置 `ScaleConfig`（量纲缩放因子），
当所有字段均已手动指定时可跳过自动校准（calibrate）阶段。

## Glossary

- **ScaleConfig**：量纲缩放配置，包含各指标的缩放因子（`scale_i`），用于将不同量纲的指标归一化，使其在得分计算中具有可比性。`scale_i = 1 / 典型值_i`，由 calibrate 自动计算或用户手动配置。
- **calibrate**：自动校准过程，根据初始状态的各指标值计算 `ScaleConfig`，令 `scale_i = 1 / 初始值_i`。
- **ScaleConfigToml 典型值**：`[scale]` 段中填写的值与 `targets` 中的指标同量纲（即各指标的"典型值"），程序读入后自动取倒数转换为 `ScaleConfig` 内部使用的缩放因子。例如填写 `collision_count = 100.0` 表示典型重码数约为 100，程序内部转换为 `scale = 1/100 = 0.01`。
- **TargetsConfig**：目标配置，包含 `[targets.full_code]` 和 `[targets.simple_code]` 两段，分别设置全码和简码各指标的优化目标值及硬约束上限。
- **目标偏差公式**：令 `d = max(0, v_i - target_i) * scale_i`，则 `score_i = w_i * (d + d^2 + low_weight * (v_i * scale_i))`，其中 `v_i` 为当前指标值，`target_i` 为目标值，`scale_i` 为量纲缩放因子（`= 1/初始值`），`w_i` 为权重，`low_weight` 为低权重系数。超标量小时线性项主导（避免平方过小），超标量大时平方项主导（加速收敛）。
- **硬约束（_max）**：在退火接受/拒绝阶段，若某指标当前值超过对应的 `_max` 设置（且 `_max != 0`），则直接拒绝该方案变动，不进入 Metropolis 判断。
- **Evaluator**：主评估器，负责计算全码得分（`compute_full_score`）和综合得分（`compute_score`）。
- **SimpleEvaluator**：简码评估器，负责计算简码得分（`compute_simple_score`）。
- **OptContext**：优化上下文，持有 `scale_config: ScaleConfig` 和 `weights: WeightConfig`，在退火过程中被所有线程共享（只读）。

## Requirements

### Requirement 1: 在 config.toml 中支持手动配置 ScaleConfig

**User Story:** 作为输入法设计者，我希望能在配置文件中手动指定各指标的量纲缩放因子，
以便在已知合理缩放值的情况下跳过自动校准阶段，节省启动时间并获得更稳定的优化行为。

#### Acceptance Criteria

1. THE `Config` 结构体 SHALL 新增可选的 `[scale]` 配置段，对应 `ScaleConfig` 的全部 10 个字段：
   `collision_count`、`collision_rate`、`equivalence`、`equiv_cv`、`distribution`、
   `simple_freq`、`simple_equiv`、`simple_dist`、`simple_collision_count`、`simple_collision_rate`。
   **`[scale]` 中填写的是各指标的"典型值"（与 `targets` 中的指标同量纲），程序读入后自动取倒数转换为缩放因子。**

2. WHEN `config.toml` 中未配置 `[scale]` 段时，THE `Config` 加载器 SHALL 使用 `ScaleConfig::default()`（所有字段为 `1.0`）作为初始值，并继续执行 calibrate 流程。

3. WHEN `config.toml` 中配置了 `[scale]` 段，且全码 5 个字段（`collision_count`、`collision_rate`、`equivalence`、`equiv_cv`、`distribution`）均已显式设置，且满足以下条件之一时，THE `run_optimize` 函数 SHALL 跳过 `calibrate_scales()` 调用：
   - `weights.simple_code.enabled = false`（简码未启用，无需简码 scale 字段）
   - `weights.simple_code.enabled = true` 且简码 5 个字段（`simple_freq`、`simple_equiv`、`simple_dist`、`simple_collision_count`、`simple_collision_rate`）也均已显式设置

4. WHEN `config.toml` 中配置了 `[scale]` 段但仅设置了部分字段时，THE `run_optimize` 函数 SHALL 先执行 `calibrate_scales()` 得到自动校准值，再用用户显式配置的字段（取倒数后）覆盖对应的自动校准值（部分覆盖语义）。

5. THE `Config` 加载器 SHALL 在启动日志中打印当前 ScaleConfig 的来源（"手动配置"、"自动校准"或"部分手动覆盖"），以及各字段的最终值。

---

### Requirement 2: 在 config.toml 中支持配置优化目标（TargetsConfig）

**User Story:** 作为输入法设计者，我希望能为全码和简码的各项指标设置优化目标值，
使优化器在指标达到目标后不再强制继续压缩，从而在多目标之间取得更合理的平衡。

#### Acceptance Criteria

1. THE `Config` 结构体 SHALL 新增可选的 `[targets.full_code]` 配置段，包含以下字段（均为可选，未配置时使用默认值）：
   - `enabled`：`bool`，默认 `false`，控制是否启用全码目标偏差模式
   - `collision_count`：`f64`，全码重码数目标值
   - `collision_rate`：`f64`，全码重码率目标值
   - `equivalence`：`f64`，全码当量目标值
   - `equiv_cv`：`f64`，全码当量变异系数目标值
   - `distribution`：`f64`，全码分布偏差目标值
   - `low_weight`：`f64`，默认 `0.01`，低权重系数

2. THE `Config` 结构体 SHALL 新增可选的 `[targets.simple_code]` 配置段，包含以下字段（均为可选，未配置时使用默认值）：
   - `enabled`：`bool`，默认 `false`，控制是否启用简码目标偏差模式
   - `collision_count`：`f64`，简码重码数目标值
   - `collision_rate`：`f64`，简码重码率目标值
   - `freq`：`f64`，简码频率覆盖率目标值（注意：频率覆盖损失 = `1 - freq`，目标偏差基于损失值计算）
   - `equiv`：`f64`，简码当量目标值
   - `dist`：`f64`，简码分布偏差目标值
   - `low_weight`：`f64`，默认 `0.01`，低权重系数

3. WHEN `[targets.full_code]` 或 `[targets.simple_code]` 中某指标的目标值未配置时，THE `Config` 加载器 SHALL 将该指标的目标值默认为 `0.0`（即退化为纯 `low_weight` 惩罚，等价于原始绝对值最小化的弱化版本）。

4. THE `Config` 加载器 SHALL 在 `config.toml` 解析失败时给出明确的错误提示，指出哪个字段格式不正确。

---

### Requirement 3: 在 TargetsConfig 中增加 _max 硬约束字段

**User Story:** 作为输入法设计者，我希望能为各指标设置硬性上限，
使退火过程在遇到严重超标的方案时直接拒绝，而不是通过 Metropolis 概率接受，
从而加速收敛并避免优化结果偏离可接受范围。

#### Acceptance Criteria

1. THE `[targets.full_code]` 配置段 SHALL 支持以下 `_max` 后缀字段（均为可选，默认 `0.0`，`0.0` 表示不启用该约束）：
   - `collision_count_max`：`f64`，全码重码数硬约束上限
   - `collision_rate_max`：`f64`，全码重码率硬约束上限
   - `equivalence_max`：`f64`，全码当量硬约束上限
   - `equiv_cv_max`：`f64`，全码当量变异系数硬约束上限
   - `distribution_max`：`f64`，全码分布偏差硬约束上限

2. THE `[targets.simple_code]` 配置段 SHALL 支持以下 `_max` 后缀字段（均为可选，默认 `0.0`）：
   - `collision_count_max`：`f64`，简码重码数硬约束上限
   - `collision_rate_max`：`f64`，简码重码率硬约束上限
   - `freq_max`：`f64`，简码频率覆盖率硬约束**下限**（`freq` 越高越好，当 `weighted_freq_coverage < freq_max` 时拒绝该方案，即覆盖率不能低于此值）
   - `equiv_max`：`f64`，简码当量硬约束上限
   - `dist_max`：`f64`，简码分布偏差硬约束上限

3. WHEN 某 `_max` 字段值为 `0.0` 时，THE `Evaluator` SHALL 不对该指标执行硬约束检查（视为未启用）。

---

### Requirement 4: 修改全码得分计算为目标偏差导向

**User Story:** 作为输入法设计者，我希望全码各指标的得分计算从"绝对值最小化"改为"目标偏差导向"，
使优化器在指标已达到目标时不再强制继续压缩，而是以较小的 `low_weight` 惩罚维持优化动力。

#### Acceptance Criteria

1. WHEN `[targets.full_code]` 的 `enabled` 为 `true` 时，THE `Evaluator` 的 `compute_full_score` 函数 SHALL 按以下公式计算每个全码指标的得分：

   ```
   d = max(0, v_i - target_i) * scale_i
   score_i = w_i * (d + d^2 + low_weight * (v_i * scale_i))
   ```

   其中：
   - `v_i`：该指标的当前值
   - `target_i`：该指标在 `[targets.full_code]` 中配置的目标值
   - `scale_i`：`ScaleConfig` 中对应的缩放因子（由 calibrate 计算为 `1/初始值`，或手动配置）
   - `w_i`：`WeightConfig` 中对应的权重
   - `low_weight`：`[targets.full_code]` 中配置的 `low_weight`（默认 `0.01`）

2. WHEN `[targets.full_code]` 的 `enabled` 为 `false` 时，THE `Evaluator` 的 `compute_full_score` 函数 SHALL 保持原有的绝对值最小化计算逻辑（`score_i = w_i * v_i * scale_i`），以保证向后兼容。

3. THE `compute_full_score` 函数 SHALL 对以下 5 个全码指标分别应用目标偏差公式：
   `collision_count`、`collision_rate`、`equivalence`（对应 `equiv_mean`）、`equiv_cv`、`distribution`（对应 `dist_deviation`）。

4. WHEN `v_i <= target_i` 时，THE `compute_full_score` 函数 SHALL 仅计算 `low_weight` 项（`w_i * low_weight * v_i * scale_i`），`d = 0`，超标惩罚项为 `0`。

5. WHEN `v_i > target_i` 时，THE `compute_full_score` 函数 SHALL 计算 `d = (v_i - target_i) * scale_i`，得分为 `w_i * (d + d^2 + low_weight * (v_i * scale_i))`。超标量小时（`d < 1`）线性项 `d` 主导，避免平方过小导致惩罚不足；超标量大时（`d > 1`）平方项 `d^2` 主导，加速收敛。

---

### Requirement 5: 修改简码得分计算为目标偏差导向

**User Story:** 作为输入法设计者，我希望简码各指标的得分计算也支持目标偏差导向，
与全码保持一致的优化语义。

#### Acceptance Criteria

1. WHEN `[targets.simple_code]` 的 `enabled` 为 `true` 时，THE `SimpleEvaluator` 的 `compute_simple_score` 函数 SHALL 按以下公式计算每个简码指标的得分：

   ```
   d = max(0, v_i - target_i) * scale_i
   score_i = w_i * (d + d^2 + low_weight * (v_i * scale_i))
   ```

   其中各变量含义与全码相同，`target_i` 来自 `[targets.simple_code]`，`low_weight` 默认 `0.01`。

2. WHEN `[targets.simple_code]` 的 `enabled` 为 `false` 时，THE `SimpleEvaluator` 的 `compute_simple_score` 函数 SHALL 保持原有的绝对值最小化计算逻辑，以保证向后兼容。

3. THE `compute_simple_score` 函数 SHALL 对以下 5 个简码指标分别应用目标偏差公式：
   `collision_count`、`collision_rate`、`freq`（频率覆盖损失 = `1 - weighted_freq_coverage`）、`equiv`（对应 `equiv_mean`）、`dist`（对应 `dist_deviation`）。

4. WHEN 简码频率覆盖率指标启用目标偏差时，THE `compute_simple_score` 函数 SHALL 以频率覆盖**损失**（`freq_loss = 1 - weighted_freq_coverage`）作为 `v_i`，以 `1 - target_freq` 作为 `target_i`，保持与原有损失计算方向一致。

---

### Requirement 6: 在退火接受/拒绝逻辑中实现 _max 硬约束

**User Story:** 作为输入法设计者，我希望退火过程在遇到超出硬约束上限的方案时直接拒绝，
不进入 Metropolis 概率判断，以加速收敛并保证最终结果满足硬性约束。

#### Acceptance Criteria

1. WHEN `[targets.full_code]` 或 `[targets.simple_code]` 中存在非零的 `_max` 字段时，THE `Evaluator` 的 `try_move` 和 `try_swap` 函数 SHALL 在计算新得分之前，先检查新方案的各指标是否超过对应的 `_max` 值。

2. WHEN 新方案的某全码指标值超过对应的 `_max`（且该 `_max != 0.0`）时，THE `try_move` / `try_swap` 函数 SHALL 直接回滚该方案变动并返回 `false`，不进入得分计算和 Metropolis 判断。

3. WHEN 新方案的某简码指标值超过对应的 `_max`（且该 `_max != 0.0`）时，THE `try_move` / `try_swap` 函数 SHALL 直接回滚该方案变动并返回 `false`，不进入得分计算和 Metropolis 判断。

4. WHEN 简码评估未启用（`enable_simple_code = false`）时，THE `try_move` / `try_swap` 函数 SHALL 跳过简码 `_max` 检查。

5. WHEN 某次变动涉及简码影响（`needs_simple = true`）时，THE `try_move` / `try_swap` 函数 SHALL 在重建简码评估（`rebuild_simple`）之后再执行简码 `_max` 检查，以确保检查的是新方案的实际简码指标值。

6. THE `OptContext` SHALL 持有 `TargetsConfig` 的引用（或副本），使 `try_move` / `try_swap` 在热路径中可以直接访问 `_max` 字段，避免额外的间接寻址开销。

---

## 关键设计决策

### 决策 1：ScaleConfig 手动配置与 calibrate 的关系

**背景：** `calibrate_scales()` 的作用是将各指标的初始值作为量纲基准（`scale = 1/初始值`），
使不同量纲的指标在得分中具有相当的权重。当用户已知合理的典型值时，可以手动配置以跳过此步骤。

**`[scale]` 段的语义：** 用户填写的是各指标的"典型值"（与 `targets` 中的指标同量纲，便于配置时对照），程序读入后自动取倒数转换为 `ScaleConfig` 内部使用的缩放因子：
```
scale_i = 1 / 用户填写的典型值_i
```
例如：`collision_count = 100.0` → 内部 `scale = 0.01`，与 calibrate 计算 `scale = 1/初始重码数` 的语义完全一致。

**设计选择：**
- 采用**部分覆盖语义**：`[scale]` 段中每个字段均为可选（使用 `Option<f64>`）。
- 若全码 5 个字段均为 `Some`，且（简码未启用 OR 简码 5 个字段也均为 `Some`），则跳过 `calibrate_scales()`，直接使用用户配置（取倒数后）。
- 若部分字段为 `Some`，则先执行 `calibrate_scales()` 得到自动校准值，再用 `Some` 字段（取倒数后）覆盖。
- 若 `[scale]` 段完全缺失，则完全依赖 `calibrate_scales()`。
- 判断"是否全部手动配置"的逻辑在 `run_optimize()` 中实现，不修改 `calibrate_scales()` 函数签名。

**实现要点：**
- `config.rs` 中新增 `ScaleConfigToml` 结构体，所有字段为 `Option<f64>`，对应 TOML 的 `[scale]` 段。
- `Config` 结构体新增 `scale: Option<ScaleConfigToml>` 字段。
- `run_optimize()` 中的 `resolve_scale_config()` 函数负责：取倒数转换 + 判断是否跳过 calibrate + 合并结果。
- 跳过 calibrate 的条件：全码 5 个字段全设置，且（`simple_code.enabled = false` 或简码 5 个字段也全设置）。

### 决策 2：目标偏差公式的语义

**公式：** 令 `d = max(0, v_i - target_i) * scale_i`，则：
```
score_i = w_i * (d + d^2 + low_weight * (v_i * scale_i))
```

**`scale_i` 的来源：** `calibrate_scales()` 计算 `scale_i = 1 / 初始值_i`，与现有原始公式（`w_i * v_i * scale_i`）完全一致。代入后：
```
d = (v_i - target_i) / 初始值_i
```
即超出目标的部分除以初始值，归一化到无量纲空间。

**语义解释：**
- **当 `v_i <= target_i`（已达目标）：** `d = 0`，超标惩罚项为 `0`，仅保留 `low_weight` 项。
  `low_weight` 项提供微弱的持续优化动力，防止优化器在达到目标后完全停止改进。
- **当 `v_i > target_i`（超出目标）：** `d > 0`，同时有线性项 `d` 和平方项 `d^2`。
  - `d < 1`（微小超标）：线性项 `d` 主导，避免平方过小导致惩罚弱于 `low_weight` 项
  - `d > 1`（大幅超标）：平方项 `d^2` 主导，惩罚快速增长，加速收敛

**与原始公式的对比：**
- 原始：`score_i = w_i * v_i * scale_i`（线性，无目标概念）
- 新版：`score_i = w_i * (d + d^2 + low_weight * (v_i * scale_i))`，`d = max(0, v_i - target_i) * scale_i`
  （线性 + 平方惩罚超出部分，线性惩罚全量，`scale_i` 均在乘法位置，与现有代码一致）

### 决策 3：_max 硬约束的实现位置

**实现位置：** `evaluator.rs` 中的 `try_move()` 和 `try_swap()` 函数。

**实现时机：** 在执行方案变动（修改 `assignment`、调用 `update_char`）之后，
在计算新得分（`get_score`）之前插入硬约束检查。

**检查顺序：**
1. 执行方案变动（`assignment[r] = new_key`，`update_char`）
2. 若 `needs_simple`，执行 `rebuild_simple`
3. **执行 `_max` 硬约束检查**（读取当前 `Metrics` 和 `SimpleMetrics`）
4. 若任一指标超限，回滚方案变动，返回 `false`
5. 否则，计算新得分，执行 Metropolis 判断

**性能考量：**
- `_max` 检查需要读取当前指标值。全码指标（`total_collisions`、`collision_frequency` 等）
  在 `update_char` 后已是最新值，可直接读取，无需额外计算。
- 简码指标在 `rebuild_simple` 后已是最新值，可直接从 `SimpleEvaluator` 读取。
- 当所有 `_max` 字段均为 `0.0`（未启用）时，硬约束检查应被编译器优化为无开销的分支。
- `TargetsConfig` 应存储在 `OptContext` 中，避免在热路径中进行额外的间接寻址。

**回滚逻辑：** 与现有的 Metropolis 拒绝回滚逻辑相同，确保 `assignment`、
`key_weighted_usage`、`cached_score` 等状态完全恢复。
