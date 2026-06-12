# 设计文档：SA 主循环冲突导向算子集成

## Overview

本特性将"冲突导向邻域算子"（`try_resolve_conflict`）集成进模拟退火（SA）主循环 `simulated_annealing`，使主循环能够以可配置概率直接针对重码冲突进行邻域移动，从而更高效地降低重码数（collision_count）与重码率（collision_rate）。

设计的核心约束有三点：

1. **零行为变更（默认关闭）**：新增配置项 `conflict_probability` 默认值为 `0.0`。当其为 `0.0` 时，SA 主循环不初始化、不维护、不刷新冲突缓存，也不调用 `try_resolve_conflict`，每一步的邻域分发逻辑与本特性引入前逐字节一致。
2. **性能可控**：`find_collision_groups` 复杂度为 O(num_chars) 加 HashMap 构建，单步执行过于昂贵。主循环改为维护一份缓存的冲突列表（Collision_Cache），并每隔 `conflict_refresh_interval` 步重建一次。
3. **评分语义不变**：冲突导向移动复用现有 `evaluator.try_move`，依据 `get_score` 与 Metropolis 准则接受/回滚，不修改 `Evaluator`、`scale`、`targets`、`equiv_cv` 等任何评分相关逻辑。

本特性新增四个 `AnnealingConfig` 配置项，全部带 serde 默认值，保证旧配置文件在缺省这些字段时仍能正常解析且行为不变。本分支仅涉及 SA，不包含 AMHB。

### 设计决策与依据

- **复用现有算子而非新增算子**：`try_resolve_conflict` 已存在且被 `enhanced_hill_climb` 使用，仅需将其接入主循环并参数化采样窗口，改动面最小、风险最低。
- **缓存而非每步重算**：满足需求 2 对性能的诉求；缓存可能略微"过期"（在 refresh 间隔内键位会变化），但这是可接受的近似——冲突组对仍指向真实存在过的冲突热点，且每次 `try_move` 都会用当前 assignment 重新评分，正确性不受影响。
- **排序策略参数化而非新函数**：通过给 `find_collision_groups` 增加一个布尔参数控制排序键，两种策略返回相同集合、仅顺序不同，避免重复构建冲突列表的逻辑。

## Architecture

冲突导向逻辑（下称 Conflict_Module）嵌入在 `simulated_annealing` 主循环内部，不引入新模块或新文件，仅修改 `src/config.rs` 与 `src/annealing.rs`。

```mermaid
flowchart TD
    Start[simulated_annealing 进入] --> InitCheck{conflict_probability > 0.0?}
    InitCheck -- 否 --> LoopNoConflict[主循环: 仅既有 swap/move 分发]
    InitCheck -- 是 --> InitCache[find_collision_groups 初始化 Collision_Cache<br/>refresh 计数清零]
    InitCache --> Loop[进入主循环]

    Loop --> RefreshCheck{interval>0 且<br/>已满 interval 步?}
    RefreshCheck -- 是 --> Rebuild[重建 Collision_Cache<br/>计数清零]
    RefreshCheck -- 否 --> Sample
    Rebuild --> Sample[采样 r ∈ [0,1)]

    Sample --> Dispatch{r < conflict_probability<br/>且 Cache 非空?}
    Dispatch -- 是 --> Conflict[try_resolve_conflict<br/>传入当前 temp]
    Dispatch -- 否 --> SwapMove[既有 swap/move 分发]

    Conflict --> Eval[get_score / 记录最优 / reheat / 扰动]
    SwapMove --> Eval
    LoopNoConflict --> Eval
    Eval --> NextStep{还有步数?}
    NextStep -- 是 --> Loop
    NextStep -- 否 --> Final[最终精炼并返回]
```

关键点：

- **单次随机抽样**：每步只抽一次 `r ∈ [0.0, 1.0)`，用同一个 `r` 决定走冲突路径还是既有 swap/move 路径（需求 3.1）。冲突路径与 swap/move 路径互斥，每步只执行其一。
- **缓存生命周期**：仅当 `conflict_probability > 0.0` 时才存在。`conflict_refresh_interval == 0` 表示初始化后不再重建（需求 2.3）。
- **回退路径**：当 `r < conflict_probability` 但缓存为空时，回退到既有 swap/move 分发，且不调用 `try_resolve_conflict`（需求 3.4）。

## Components and Interfaces

### 1. `AnnealingConfig`（`src/config.rs`）

新增四个字段，全部使用 `#[serde(default = "...")]` 指向独立的默认值函数，保证旧配置文件（缺少这些字段）仍能解析成功。

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct AnnealingConfig {
    // ... 既有字段保持不变 ...
    pub max_parts: usize,

    /// 冲突导向移动的执行概率（0.0~1.0），0.0 表示关闭本特性
    #[serde(default = "default_conflict_probability")]
    pub conflict_probability: f64,

    /// 每隔多少步重建一次冲突缓存；0 表示初始化后不再重建
    #[serde(default = "default_conflict_refresh_interval")]
    pub conflict_refresh_interval: usize,

    /// 在排序后冲突列表的前 N 个元素中采样
    #[serde(default = "default_conflict_sample_window")]
    pub conflict_sample_window: usize,

    /// 冲突组排序是否按频率加权（true=按字频之和，false=按汉字数量）
    #[serde(default = "default_conflict_weight_by_freq")]
    pub conflict_weight_by_freq: bool,
}

fn default_conflict_probability() -> f64 { 0.0 }
fn default_conflict_refresh_interval() -> usize { 1000 }
fn default_conflict_sample_window() -> usize { 20 }
fn default_conflict_weight_by_freq() -> bool { false }
```

**取值范围说明：**

- `conflict_probability`：语义上有效范围为 `[0.0, 1.0]`（含两端）。代码不强制校验；超出范围时，`< 0.0` 等效于关闭、`>= 1.0` 等效于每步都尝试冲突路径。文档与示例配置以默认值 `0.0` 给出。
- `conflict_refresh_interval`：`usize`，`0` 为合法特例（不再重建）。
- `conflict_sample_window`：`usize`，`0` 为合法特例（`try_resolve_conflict` 直接返回 false）。
- `conflict_weight_by_freq`：`bool`。

**`Default for Config` 更新**：在 `annealing: AnnealingConfig { ... }` 构造块尾部显式追加四个字段，与默认值函数保持一致：

```rust
annealing: AnnealingConfig {
    // ... 既有字段 ...
    max_parts: 3,
    conflict_probability: 0.0,
    conflict_refresh_interval: 1000,
    conflict_sample_window: 20,
    conflict_weight_by_freq: false,
},
```

> 注：`Config` 的派生 `Deserialize` 当前不使用 `deny_unknown_fields`（现有 `config.toml.example` 中已包含若干 `AnnealingConfig` 未定义的字段如 `perturb_temp_multiplier` 且仍能解析），因此新增带 serde 默认值的字段不会破坏既有解析行为。

### 2. `find_collision_groups`（`src/annealing.rs`）

签名增加一个 `weight_by_freq: bool` 参数控制排序键。返回类型仍为 `Vec<(usize, usize, usize)>`，集合内容（冲突组对）在两种策略下完全相同，仅排序顺序不同（需求 5.4）。

```rust
fn find_collision_groups(
    ctx: &OptContext,
    assignment: &[u8],
    weight_by_freq: bool,
) -> Vec<(usize, usize, usize)> {
    let code_to_chars = build_code_to_chars(ctx, assignment);
    let mut collisions: Vec<(usize, usize, usize)> = Vec::new();

    for chars in code_to_chars.values() {
        if chars.len() < 2 {
            continue;
        }
        // 冲突权重：count 策略用 chars.len()；freq 策略用共享该编码的汉字字频之和
        let weight: usize = if weight_by_freq {
            chars.iter().map(|&ci| ctx.char_infos[ci].frequency as usize).sum()
        } else {
            chars.len()
        };

        let mut groups_in_conflict: HashSet<usize> = HashSet::new();
        for &ci in chars {
            for &p in &ctx.char_infos[ci].parts {
                if p >= GROUP_MARKER {
                    groups_in_conflict.insert((p - GROUP_MARKER) as usize);
                }
            }
        }
        let groups: Vec<usize> = groups_in_conflict.into_iter().collect();
        for i in 0..groups.len() {
            for j in (i + 1)..groups.len() {
                collisions.push((groups[i], groups[j], weight));
            }
        }
    }

    // 主键：第三字段降序；次键（稳定裁决）：(g1, g2) 升序
    collisions.sort_by(|a, b| {
        b.2.cmp(&a.2)
            .then(a.0.cmp(&b.0))
            .then(a.1.cmp(&b.1))
    });
    collisions
}
```

**元组第三字段（weight）语义：**

- `weight_by_freq == false`（现状）：第三字段为"参与该冲突编码的汉字个数"（`chars.len()`），冲突列表按数量降序排序，对应优化 collision_count（需求 5.1）。
- `weight_by_freq == true`：第三字段为"共享该冲突编码的汉字字频之和"（`ctx.char_infos[ci].frequency` 求和），冲突列表按频率之和降序排序，对应优化 collision_rate（需求 5.2）。频率之和直接从共享同一冲突编码的 `chars` 集合计算，无需经由 `group_to_chars`（`chars` 即该编码下冲突的汉字索引列表）。

**稳定的次序裁决（需求 5.3）：** 当排序主键相等时，按 `(g1, g2)` 字典序升序裁决。由于在每个 `chars` 桶内 `g1 < g2`（由 `i < j` 的双重循环保证），`(g1, g2)` 唯一标识一个冲突组对，该裁决规则使排序结果可复现、不依赖 HashMap 遍历顺序。

> 注：现有代码 `sort_by(|a, b| b.2.cmp(&a.2))` 不含次键，HashMap 遍历顺序的非确定性会让等权冲突对的相对顺序在不同运行间不稳定。增加次键后两种策略均可复现。

**调用点更新**：现有两处调用（主循环低温扰动块、`enhanced_hill_climb` 中的调用）需补传 `weight_by_freq` 参数。为保持这些既有路径行为不变，它们传入 `false`（保留按数量排序的现状）。

### 3. `try_resolve_conflict`（`src/annealing.rs`）

将硬编码的采样窗口 `20` 替换为参数 `sample_window: usize`。新增对 `sample_window == 0` 与空缓存的提前返回。

```rust
fn try_resolve_conflict(
    ctx: &OptContext,
    assignment: &mut [u8],
    evaluator: &mut Evaluator,
    collisions: &[(usize, usize, usize)],
    sample_window: usize,
    temp: f64,
    rng: &mut ThreadRng,
) -> bool {
    if collisions.is_empty() || sample_window == 0 {
        return false;
    }

    // 有效窗口 = min(sample_window, 列表长度)
    let window = sample_window.min(collisions.len());
    let idx = rng.gen_range(0..window);
    let (g1, g2, _) = collisions[idx];

    // ... 后续逻辑（选 g1/g2 之一、随机新键位、evaluator.try_move）保持不变 ...
}
```

行为对应：

- 有效窗口长度 `> 0` 时，在 `min(sample_window, len)` 范围内均匀采样一个冲突组（需求 4.1）。
- `sample_window >= len` 时在整个列表上采样（需求 4.2，由 `min` 自然得到）。
- `sample_window == 0` → 返回 false，不执行移动（需求 4.3）。
- 缓存为空 → 返回 false（需求 4.4）。

接受/回滚仍由 `evaluator.try_move` 依据 `get_score` 与 Metropolis 准则完成（需求 6.1）。

### 4. `simulated_annealing` 主循环（`src/annealing.rs`）

**新增局部状态（在主循环 `for step in 0..steps` 之前）：**

```rust
let conflict_prob = cfg.annealing.conflict_probability;
let conflict_refresh = cfg.annealing.conflict_refresh_interval;
let conflict_window = cfg.annealing.conflict_sample_window;
let conflict_weight_by_freq = cfg.annealing.conflict_weight_by_freq;
let conflict_enabled = conflict_prob > 0.0;

// 冲突缓存：仅在启用时初始化（需求 2.1 / 2.4）
let mut collisions: Vec<(usize, usize, usize)> = if conflict_enabled {
    find_collision_groups(ctx, &assignment, conflict_weight_by_freq)
} else {
    Vec::new()
};
let mut steps_since_refresh = 0usize;
```

**主循环内：温度计算之后、邻域分发之前插入刷新逻辑（需求 2.2 / 2.3）：**

```rust
if conflict_enabled {
    if conflict_refresh > 0 && steps_since_refresh >= conflict_refresh {
        collisions = find_collision_groups(ctx, &assignment, conflict_weight_by_freq);
        steps_since_refresh = 0;
    } else {
        steps_since_refresh += 1;
    }
}
```

**邻域分发改造（需求 3）：** 将现有的 swap/move 分发块包裹进单次抽样判定。每步抽一次 `r`：

```rust
let r = rng.gen::<f64>(); // r ∈ [0.0, 1.0)，每步恰好一次（需求 3.1）

if conflict_enabled && r < conflict_prob && !collisions.is_empty() {
    // 冲突路径（需求 3.2）；传入当前 temp（需求 3.6）
    try_resolve_conflict(
        ctx, &mut assignment, &mut evaluator,
        &collisions, conflict_window, temp, &mut rng,
    );
} else {
    // 既有 swap/move 分发（需求 3.3 / 3.4 / 3.5），逐字节保留原逻辑：
    let swap_prob = swap_prob_base
        + (1.0 - swap_prob_base) * (step as f64 / steps as f64) * 0.3;
    if rng.gen::<f64>() < swap_prob && n_groups >= 2 {
        // ... 原 try_swap / try_move 分发不变 ...
    } else {
        // ... 原 try_move 分支不变 ...
    }
}
```

**零行为变更保证（需求 3.5）：** 当 `conflict_prob == 0.0` 时：

- `conflict_enabled == false`，缓存不初始化、刷新逻辑整体跳过、不调用 `find_collision_groups`（需求 2.4）。
- 分发判定 `conflict_enabled && ...` 短路为假，必然进入 `else` 分支执行原 swap/move 逻辑。
- 唯一新增的随机调用是每步开头的 `let r = rng.gen::<f64>();`。**这会改变 RNG 序列，导致与历史运行的逐位结果不同。** 为严格满足"与本特性引入前完全一致的分发逻辑"，采用如下处理：当 `conflict_enabled == false` 时，**跳过 `r` 的抽样**，直接进入原 swap/move 块；仅在 `conflict_enabled == true` 时才抽取 `r`。这样关闭特性时连 RNG 消耗序列都与原实现一致。

  ```rust
  if conflict_enabled {
      let r = rng.gen::<f64>();
      if r < conflict_prob && !collisions.is_empty() {
          try_resolve_conflict(/* ... temp ... */);
      } else {
          // 既有 swap/move 分发
      }
  } else {
      // 既有 swap/move 分发（与原实现 RNG 消耗序列一致）
  }
  ```

  两个分支中的"既有 swap/move 分发"为同一段逻辑，可抽取为闭包或保持内联复制；设计上以"关闭时 RNG 序列不变"为准绳。

> 注：低温智能扰动块中已有的 `find_collision_groups(ctx, &assignment)` 调用属于既有行为，本特性仅为其补传 `weight_by_freq = false`，不改变其触发条件与效果。

### 5. `Evaluator` / `get_score` / `scale` / `targets` / `equiv_cv`

**不做任何修改**（需求 6.2）。冲突路径与既有 swap/move 路径最终都调用同一个 `evaluator.try_move`，因此对同一键位状态的评分数值完全一致（需求 6.3）。

## Data Models

本特性不引入新的持久化数据结构，仅新增配置字段与一个主循环内的局部缓存变量。

| 名称 | 类型 | 位置 | 说明 |
|------|------|------|------|
| `conflict_probability` | `f64` | `AnnealingConfig` | 冲突路径执行概率，默认 0.0 |
| `conflict_refresh_interval` | `usize` | `AnnealingConfig` | 缓存重建间隔（步），默认 1000，0=不重建 |
| `conflict_sample_window` | `usize` | `AnnealingConfig` | 采样窗口大小，默认 20，0=不动作 |
| `conflict_weight_by_freq` | `bool` | `AnnealingConfig` | 排序策略开关，默认 false |
| `collisions` | `Vec<(usize, usize, usize)>` | `simulated_annealing` 局部 | Collision_Cache：`(g1, g2, weight)`，`g1 < g2` |
| `steps_since_refresh` | `usize` | `simulated_annealing` 局部 | 自上次重建以来的步数计数 |

**Collision_Cache 元组 `(g1, g2, weight)`：**

- `g1`、`g2`：冲突的两个字根组索引，恒满足 `g1 < g2`。
- `weight`：排序权重。`weight_by_freq == false` 时为参与该冲突编码的汉字数量；`true` 时为这些汉字的字频之和（`u64` 求和后存入 `usize`）。
- 列表按 `weight` 降序排序，等值时按 `(g1, g2)` 升序裁决。

## Correctness Properties

*属性（property）是指在系统所有有效执行中都应成立的特征或行为——本质上是对系统应当做什么的形式化陈述。属性是人类可读规约与机器可验证正确性保证之间的桥梁。*

本特性中可作为属性测试的部分集中在两个纯函数行为上：`find_collision_groups` 的排序语义，以及 `try_resolve_conflict` 的采样窗口边界。主循环控制流接线（缓存何时初始化/刷新、按概率分发、传入温度）依赖 RNG 与温度调度，难以表达为对任意输入的通用属性，将通过单元/集成示例与代码审查验证（见 Testing Strategy）。

### Property 1: 采样窗口落在有效范围内

*对任意*非空冲突列表 `collisions` 与任意 `sample_window > 0`，`try_resolve_conflict` 选中的冲突组下标恒小于 `min(sample_window, collisions.len())`；当 `sample_window == 0` 或 `collisions` 为空时返回 `false` 且不修改 assignment。

**Validates: Requirements 4.1, 4.2, 4.3, 4.4**

### Property 2: 排序按所选权重降序

*对任意*键位分配 `assignment`，无论 `weight_by_freq` 取 false 还是 true，`find_collision_groups` 返回列表中相邻元素的权重字段非递增（降序排列）。

**Validates: Requirements 5.1, 5.2, 2.5**

### Property 3: 排序结果可复现（裁决确定性）

*对任意*键位分配 `assignment`，在固定 `weight_by_freq` 下多次调用 `find_collision_groups`，每次返回的有序向量逐元素完全相同（不受 HashMap 遍历顺序影响）。

**Validates: Requirements 5.3**

### Property 4: 两种排序策略产出相同冲突组集合

*对任意*键位分配 `assignment`，`weight_by_freq == false` 与 `weight_by_freq == true` 两种策略返回的冲突组对集合（以 `(g1, g2)` 标识）相等、元素数量相同，仅排列顺序可能不同。

**Validates: Requirements 5.4**

## Error Handling

本特性不引入新的错误类型或失败路径，主要通过边界处理保证健壮性：

- **空冲突列表**：`try_resolve_conflict` 在 `collisions.is_empty()` 时立即返回 `false`，主循环本步等效于无操作，不 panic（需求 4.4）。
- **采样窗口为 0**：`sample_window == 0` 时返回 `false`，避免 `rng.gen_range(0..0)` 触发的 panic（需求 4.3）。
- **窗口超过列表长度**：通过 `sample_window.min(collisions.len())` 钳制，杜绝越界采样（需求 4.2）。
- **刷新间隔为 0**：`conflict_refresh > 0` 的守卫使 `interval == 0` 时不再重建缓存，避免每步昂贵重算（需求 2.3）。
- **配置缺失字段**：serde 默认值函数保证旧配置解析不报错；`Config::load_from_path` 既有逻辑在整体解析失败时回退到 `Config::default()`，本特性不改变该回退行为。
- **配置取值越界**：`conflict_probability` 超出 `[0.0, 1.0]` 不会 panic（`< 0.0` 等效关闭，`>= 1.0` 等效每步尝试冲突路径），由文档与示例引导正确取值，代码不强制校验以保持与现有配置项一致的宽松风格。

## Testing Strategy

采用单元测试 + 属性测试的双重策略。本特性的纯函数排序逻辑与采样边界适合属性测试，配置解析与主循环接线适合示例/集成测试。

### 属性测试（Property-Based Testing）

适用范围：`find_collision_groups` 的排序语义与 `try_resolve_conflict` 的采样边界——它们对任意 `assignment` / 任意列表与窗口都应成立通用规则。

- 选用 Rust 生态的属性测试库 `proptest`（按需加入 `[dev-dependencies]`），不自行实现属性测试框架。
- 每个属性测试至少运行 100 次迭代（proptest 默认 256 cases 满足要求）。
- 每个属性测试以注释标注对应设计属性，格式：`// Feature: sa-conflict-operators, Property {编号}: {属性文本}`。
- 每条正确性属性以单个属性测试实现：
  - **Property 1**：生成随机 `Vec<(usize,usize,usize)>` 与随机 `sample_window`（含 0 与大于长度的值），断言采样索引落在 `min(window, len)` 内；window=0 或空列表时返回 false。生成器需覆盖 `sample_window == 0`、`> len`、空列表等边界（覆盖需求 4.3/4.4 的边界用例）。
  - **Property 2**：生成随机 `assignment`，对两种 `weight_by_freq` 分别断言返回列表权重降序。
  - **Property 3**：对同一随机 `assignment` 多次调用，断言两次结果向量逐元素相等。
  - **Property 4**：生成随机 `assignment`，将两种策略输出按 `(g1,g2)` 归一化后断言集合与长度相等。

> 测试需要构造可用的 `OptContext`。优先复用现有测试中已有的小规模上下文构造方式（若无则在测试模块内构造最小 `OptContext`：少量字根组、若干带 `parts`/`frequency` 的 `CharInfo`），以便 `find_collision_groups` 可运行。

### 单元测试 / 示例测试

- **配置默认值与向后兼容（需求 1.1–1.7）**：
  - 解析不含任何新字段的 `[annealing]`（复用 `config.rs` 现有最小配置前缀），断言四字段为默认值且解析成功（1.5）。
  - 解析仅含部分新字段的配置，断言显式字段取值、缺失字段取默认（1.6）。
  - 断言 `Config::default().annealing` 四字段为默认值（1.7）。
  - 确认现有 `config.rs` 解析测试在新增字段后仍通过（serde 默认值保证）。
- **采样窗口边界显式用例（需求 4.3/4.4）**：非空列表 + `window=0` 返回 false；空列表 + 任意 window 返回 false 且 assignment 不变。
- **评分一致性（需求 6.3）**：对若干键位状态，确认冲突路径与既有路径最终调用同一 `get_score`、评分数值相等。

### 集成 / 审查验证（不适合 PBT）

- **主循环接线（需求 2.1–2.4、3.1–3.4、3.6、6.1）**：通过代码审查 + 小规模运行验证缓存初始化/刷新触发、单次抽样分发、温度传入、经 `evaluator.try_move` 接受/回滚。
- **零行为变更（需求 3.5）**：固定种子下以 `conflict_probability = 0.0` 运行短 SA，验证 `find_collision_groups` 不因本特性被额外调用、RNG 消耗序列与基线一致、结果与基线相同。
- **评估器不被修改（需求 6.2）**：通过 diff/审查确认未触及 `Evaluator`、`scale`、`targets`、`equiv_cv`。
- **配置文件同步（需求 7.1–7.4）**：检查 `config.toml.example` 与 `moling/config.toml` 均含四字段且附非空中文注释；`moling/config.toml` 取值 `0.25 / 1000 / 20 / true`；`config.toml.example` 取默认值 `0.0 / 1000 / 20 / false`。

## 配置文件更新（需求 7）

### `config.toml.example`（`[annealing]` 段，默认关闭）

在既有字段之后追加：

```toml
# 冲突导向算子（默认关闭：probability = 0.0）
conflict_probability = 0.0       # 每步执行冲突导向移动的概率（0.0~1.0），0.0 表示关闭
conflict_refresh_interval = 1000 # 每隔多少步重建一次冲突缓存；0 表示初始化后不再重建
conflict_sample_window = 20      # 在排序后冲突列表的前 N 个中采样
conflict_weight_by_freq = false  # 冲突排序：false=按汉字数量(优化重码数)，true=按字频之和(优化重码率)
```

### `moling/config.toml`（`[annealing]` 段，开启本特性）

在既有字段之后追加：

```toml
# 冲突导向算子（已开启）
conflict_probability = 0.25      # 每步以 0.25 概率执行冲突导向移动
conflict_refresh_interval = 1000 # 每 1000 步重建一次冲突缓存
conflict_sample_window = 20      # 在排序后冲突列表的前 20 个中采样
conflict_weight_by_freq = true   # 按字频之和排序，侧重优化重码率
```
