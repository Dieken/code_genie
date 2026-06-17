# Requirements Document

## Introduction

code_genie 是一个用 Rust 编写的输入法字根编码方案优化器，核心算法为模拟退火（`src/annealing.rs`），由 `rayon` 并行执行多个独立线程的退火实例（`main.rs`：`(0..num_threads).into_par_iter().map(simulated_annealing)`，线程数 8~12）。

经内存分析确认：程序的主要内存占用来自一批**按编码值 `code` 直接索引、大小等于 `code_space` 的密集数组**，而非汉字个数（算码使用的汉字数为 11177）。关键事实：

- 编码空间 `code_space = code_base ^ max_parts`，其中 `code_base = EQUIV_TABLE_SIZE + 1 = 32`（`src/types.rs` / `src/context.rs`）。`max_parts` 取值 3~5，对应 `code_space` 为 32,768 / 1,048,576 / 33,554,432，随码长指数膨胀。
- 主评估器 `Evaluator`（`src/evaluator.rs`）持有四个 `code_space` 大小的数组：`code_to_chars: Vec<Vec<usize>>`、`bucket_freq_sum: Vec<u64>`、`bucket_max_freq: Vec<u64>`、`bucket_first: Vec<usize>`。简码评估器 `SimpleEvaluator` 另持有 `bucket_collision_contrib`（`code_space` 大小）以及每个简码级别 `code_base^L` 大小的 `buckets`、`bucket_gen`。
- 上述结构按线程复制（每个退火线程一份 `Evaluator` 及其 `SimpleEvaluator`）。在 `max_parts = 5` 时单线程已达数 GB 量级，多线程必然 OOM；`max_parts = 4` 时每线程数十 MB、多线程数百 MB。

关键可优化性质：**任意时刻不同 `code` 的数量恒 ≤ 汉字数 n_chars（每个汉字贡献一个全码编码，故非空桶 ≤ 11177）**。当 `max_parts = 5` 时占用率仅 `11177 / 33.5M ≈ 0.03%`，密集数组绝大部分为空。因此可将这些密集数组替换为「仅存非空桶」的稀疏结构，使内存随非空桶数（≤ n_chars）增长而非随 `code_space` 增长。

本特性的目标是：在**不改变优化质量、不改变任何对外指标与输出**的前提下，将上述 `code_space` 量级的内存占用降至非空桶量级（≤ n_chars）。实现策略为引入一个统一的桶存储抽象 `BucketStore`，按 `code_space` 阈值在「密集（dense，保持现状零回归）」与「稀疏（sparse，FxHashMap）」两种后端之间自适应选择，使小码长场景保持基线性能、大码长场景从「OOM/不可运行」变为「可运行」。

本文档使用 EARS 模式描述验收标准，所有需求遵循 INCOSE 质量规则。

## Glossary

- **优化器（Optimizer）**：code_genie 整体程序。
- **退火器（Annealer）**：模拟退火主循环逻辑，位于 `src/annealing.rs`。
- **主评估器（Evaluator）**：`src/evaluator.rs` 中的 `Evaluator`，维护全码指标并持有简码评估器。
- **简码评估器（SimpleEvaluator）**：`src/evaluator.rs` 中的 `SimpleEvaluator`，负责简码相关指标计算。
- **编码值（Code）**：某汉字按当前分配计算得到的全码或简码编码（非负整数），取值范围 `[0, code_space)` 或 `[0, code_base^L)`。
- **编码空间（Code_Space）**：`code_space = code_base ^ max_parts`，全码编码的取值上界。
- **全码桶（Full_Code_Bucket）**：映射到同一全码编码的汉字集合，即现 `code_to_chars[code]`。
- **简码桶（Simple_Bucket）**：某简码级别中映射到同一简码编码的候选字集合。
- **非空桶（Nonempty_Bucket）**：成员数 ≥ 1 的桶；任意时刻非空全码桶数 ≤ n_chars。
- **密集后端（Dense_Backend）**：以 `Vec`（按 `code` 直接索引、大小 = `code_space`）实现的桶存储，等价于现状实现。
- **稀疏后端（Sparse_Backend）**：以 `FxHashMap<code, …>` 实现的桶存储，仅存非空桶。
- **桶存储抽象（BucketStore）**：统一封装密集/稀疏两种后端、对调用方提供一致访问接口的类型。
- **空桶语义（Empty_Bucket_Semantics）**：缺失的桶（稀疏后端中键不存在）等价于 `{members: [], freq_sum: 0, max_freq: 0, first: MAX}`，与密集后端中从未写入的桶一致。
- **n_chars**：参与编码的汉字总数（本工作场景为 11177），等于 `ctx.char_infos.len()`。
- **基线版本（Baseline）**：本特性实现前的当前版本。

## Requirements

### 需求 1：主评估器全码桶四数组稀疏化

**用户故事：** 作为优化器使用者，我希望主评估器不再为每个可能的编码值预分配存储，使内存随实际非空桶数（≤ n_chars）增长，从而在大码长（`max_parts = 5`）下可运行。

#### 验收标准

1. THE 主评估器 SHALL 将现有四个 `code_space` 大小的密集数组（`code_to_chars`、`bucket_freq_sum`、`bucket_max_freq`、`bucket_first`）的逻辑状态统一由桶存储抽象（BucketStore）承载，使按 `code` 索引的存储不再恒为 `code_space` 大小。
2. THE 桶存储抽象 SHALL 将同一 `code` 的成员列表、频率和、最大频率、首选字索引组织在一起，使一次按 `code` 的查找即可获取该桶的全部聚合状态。
3. WHERE 桶存储采用稀疏后端，THE 主评估器 SHALL 仅为非空桶分配存储，使按 `code` 索引的存储规模为 O(非空桶数) 且非空桶数 ≤ n_chars。
4. THE 按汉字下标 `ci` 索引的结构（`char_bucket_pos`、`current_codes`、`current_equiv_val`）SHALL 保持为 n_chars 大小，不被本特性改为稀疏。

### 需求 2：稀疏桶有界性（空桶移除不变量）

**用户故事：** 作为优化器使用者，我希望稀疏存储不随退火步数累积历史出现过的编码，从而其内存占用始终被非空桶数界定。

#### 验收标准

1. WHEN 一个全码桶在增量更新（`update_char`）中成员数降为 0，THE 主评估器 SHALL 从稀疏后端移除该桶的条目，使其不再占用存储。
2. THE 稀疏后端中存在条目的桶集合 SHALL 恒等于当前非空桶集合（不残留空桶条目）。
3. FOR ALL 优化过程中的任意步，稀疏后端的条目数 SHALL NOT 超过 n_chars。
4. WHERE 桶存储采用稀疏后端，THE 读取一个缺失桶 SHALL 返回空桶语义（成员为空、`freq_sum = 0`、`max_freq = 0`、`first = MAX`），与密集后端中从未写入的桶逐字段一致。

### 需求 3：行为与指标等价（增量、全量、输出均不变）

**用户故事：** 作为优化器使用者，我希望内存优化不改变任何优化结果与上报指标，从而本改动为纯粹的内存（与大码长下的可运行性）优化。

#### 验收标准

1. FOR ALL 分配（Assignment），THE 主评估器经桶存储抽象得到的全码指标（`total_collisions`、`collision_frequency`、`total_equiv_weighted` 等）SHALL 与基线版本在相同分配下逐字段一致。
2. FOR ALL 分配，THE 简码评估器经桶存储抽象得到的简码指标 SHALL 与基线版本在相同分配下一致。
3. THE 主评估器的全码重码计算 SHALL 保持独立于出简状态，不因本改动而改变其语义。
4. THE 增量更新（`try_move`/`try_swap`）的结果 SHALL 与全量重建在相同分配下保持一致（沿用现有「增量 == 全量重建」一致性约束）。
5. THE 本特性 SHALL NOT 改变任何输出文件（keymap、encode、simple-codes、combined、distribution、equiv-dist、summary）的内容。
6. THE 本特性 SHALL NOT 改变首选字（`is_first_candidate`/`bucket_first`）的选取规则与其增量维护时机（仍仅在简码激活时维护）。

### 需求 4：简码评估器 code_space 量级结构稀疏化

**用户故事：** 作为优化器使用者，我希望简码评估器中按编码索引的大数组也随非空桶数增长，从而简码激活后内存同样受界定。

#### 验收标准

1. THE 简码评估器 SHALL 将 `bucket_collision_contrib`（现 `code_space` 大小、按全码编码索引）改为仅存非零贡献的稀疏结构，缺失键等价于贡献 `(0, 0)`。
2. WHEN 某全码桶的简码重码贡献在差量维护后变为 `(0, 0)`，THE 简码评估器 SHALL 移除该桶的稀疏条目，使其条目数受非空桶数界定。
3. THE 简码评估器 SHALL 将每个简码级别按简码编码索引的 `buckets`（现 `code_base^L` 大小）改由桶存储抽象承载，使其存储随该级非空简码桶数增长。
4. WHERE 简码级别的「桶触碰代际」机制（`bucket_gen`，现 `code_base^L` 大小）用于 O(1) 去重与复位，THE 简码评估器 SHALL 以不依赖 `code_base^L` 大小数组的等价机制（如稀疏标记或工作集去重）替代，保持去重与回滚语义不变。
5. THE `ctx.simple_level_capacity[li]` SHALL 仍可用作简码编码合法性的上界校验（如 `debug_assert`），但 SHALL NOT 再用于分配 `code_base^L` 大小的运行期数组。
6. FOR ALL 分配，简码评估器稀疏化后得到的出简选择与简码指标 SHALL 与基线版本一致。

### 需求 5：密集/稀疏后端自适应选择

**用户故事：** 作为优化器使用者，我希望小码长场景保持基线的极致性能、大码长场景自动改用稀疏存储，从而无需手动配置即可兼顾性能与可运行性。

#### 验收标准

1. THE 桶存储抽象 SHALL 在构建时依据 `code_space`（或简码级别的 `code_base^L`）与一个阈值，自适应选择密集后端或稀疏后端。
2. WHERE `code_space` 不超过阈值，THE 桶存储抽象 SHALL 采用密集后端，使其单步访问为直接索引、与基线性能逐字节等价（无哈希查找开销）。
3. WHERE `code_space` 超过阈值，THE 桶存储抽象 SHALL 采用稀疏后端，使内存随非空桶数增长。
4. THE 后端选择阈值 SHALL 以常量或可推导的形式确定，使 `max_parts ≤ 4`（`code_space ≤ 1,048,576`）默认走密集后端、`max_parts = 5`（`code_space = 33,554,432`）默认走稀疏后端。
5. THE 后端选择 SHALL 对调用方透明：两种后端经同一接口暴露相同语义，使调用点代码与后端无关。

### 需求 6：窄整型键与成员表示（降低常数内存）

**用户故事：** 作为优化器使用者，我希望桶成员与编码键使用恰好够用的窄整型，从而在两种后端下都进一步降低内存常数。

#### 验收标准

1. WHERE 桶成员存储汉字下标 `ci`，THE 桶存储抽象 SHALL 以 `u32` 表示成员（n_chars ≤ 11177 远小于 `u32::MAX`），而非 `usize`。
2. WHERE 稀疏后端以编码值 `code` 为键，THE 桶存储抽象 SHALL 以 `u32` 表示键（`code_base^max_parts` 在 `max_parts ≤ 6` 时 `< u32::MAX`）。
3. IF 配置使 `code_space` 超出 `u32` 表示范围（`max_parts ≥ 7`），THEN THE 优化器 SHALL 在构建时检测并报错（终止），而非静默截断。
4. THE 窄整型表示 SHALL NOT 改变任何对外语义或指标（仅为内部内存表示优化）。

### 需求 7：全量扫描去 code_space 化

**用户故事：** 作为优化器使用者，我希望全量构建与全量重建不再遍历整个编码空间，从而消除 `max_parts = 5` 下数千万次空桶空转，顺带获得 CPU 提速。

#### 验收标准

1. WHEN 主评估器执行全量构建（`new_impl` 中的碰撞统计与首选字初始化）或全量重建，THE 主评估器 SHALL 仅遍历非空桶，而非遍历 `[0, code_space)` 全区间。
2. WHEN 简码评估器执行全量重码重算（`recompute_collisions_full`），THE 简码评估器 SHALL 仅遍历非空全码桶，而非遍历 `[0, code_space)` 全区间。
3. THE 全量扫描去 code_space 化 SHALL NOT 改变扫描得到的聚合结果（重码数、重码频率、首选字、贡献缓存等），仅改变遍历范围。
4. WHERE 桶存储采用密集后端，THE 全量扫描 SHALL 在功能上仍仅对非空桶产生贡献，且其结果与基线一致。

### 需求 8：快照与回滚一致性

**用户故事：** 作为优化器使用者，我希望被拒绝的移动在稀疏存储下仍能精确回滚到移动前状态，从而保证退火接受/拒绝语义不变。

#### 验收标准

1. WHEN 一个候选移动被拒绝或因 `_max` 硬约束触发回滚，THE 主评估器与简码评估器 SHALL 将受影响桶（含其成员、`freq_sum`、`max_freq`、`first` 及简码相关贡献）精确还原至移动前状态。
2. WHEN 回滚使某桶恢复为空，THE 桶存储抽象 SHALL 在稀疏后端中移除该桶条目，使有界性不变量（需求 2）在回滚后仍成立。
3. WHEN 回滚使某此前为空的桶恢复为非空，THE 桶存储抽象 SHALL 在稀疏后端中重建该桶条目，使其成员与聚合与移动前一致。
4. THE 现有基于「记录修改前值、逆序回放」的快照/回滚机制 SHALL 在桶存储抽象下保持等价语义，回滚成本仍为 O(受影响项)。

### 需求 9：占用保护判定语义保持

**用户故事：** 作为方案设计者，我希望简码占用保护（需求 33 的既有行为）在桶存储抽象下语义不变，从而不破坏简码不抢占受保护全码的约束。

#### 验收标准

1. WHERE `simple_protect_top_n == 0`，THE 占用保护判定 `is_code_blocked(code)` SHALL 等价于「全码桶 `code` 非空」，并经桶存储抽象以「该桶存在且成员非空」实现（稀疏后端中即键存在）。
2. WHERE `simple_protect_top_n > 0`，THE 占用保护判定 SHALL 仍由现有 `protect_count`（`FxHashMap`）维护，本特性不改变其逻辑。
3. THE 占用保护增量维护中对「旧桶变空 / 新桶变为恰含一字」的判定 SHALL 在桶存储抽象下得到与基线一致的结果。

### 需求 10：最小改动约束

**用户故事：** 作为代码维护者，我希望本特性只专注内存优化及其必需的正确性保持，从而避免无关重构扩大改动范围与回归风险。

#### 验收标准

1. THE 优化器 SHALL 仅修改与桶存储抽象引入、按 `code` 索引的密集数组替换、相关全量扫描去 code_space 化、相关快照/回滚适配直接相关的代码。
2. THE 优化器 SHALL NOT 引入与上述内存优化目标无关的重构。
3. WHERE 某处代码与本特性的内存优化与正确性保持目标无直接关联，THE 优化器 SHALL 保持该处代码不变。
4. THE 本特性 SHALL NOT 新增任何面向用户的配置项（后端选择为内部自适应，不暴露为配置）。

### 需求 11：一致性验证与回归防护

**用户故事：** 作为优化器使用者，我希望有自动化校验证明稀疏化后结果与基线/全量一致，从而放心地在大码长场景使用。

#### 验收标准

1. THE 优化器 SHALL 保持并通过现有的「增量 == 全量重建」「对账 == 全量重建」一致性属性测试（`proptest`），证明桶存储抽象未破坏增量正确性。
2. THE 优化器 SHALL 提供针对桶存储抽象的单元测试，覆盖：空桶语义、空桶移除有界性、密集与稀疏两种后端在相同操作序列下产生一致的可观察状态。
3. WHEN 在 `max_parts = 4` 的现有配置上运行优化，THE 优化器 SHALL 产生与基线一致的最终上报指标（在相同随机种子与相同步数下）。
4. WHERE 桶存储采用密集后端，THE 单步热路径性能 SHALL 与基线版本保持一致（无可测量的回归）。
5. THE 优化器 SHALL 在 `max_parts = 5` 配置下成功构建并运行至产生输出，而不发生因 `code_space` 量级分配导致的内存耗尽。

### 需求 12：后端选择的可观测性（日志提醒）

**用户故事：** 作为优化器使用者，我希望在启动时清楚看到每个桶存储选择了密集还是稀疏后端、依据是什么、密集后端的预估内存是多少，从而理解当前内存/性能特征的来源（`max_parts` 由输入法方案设计确定、不应为此更改）。

#### 验收标准

1. THE 优化器 SHALL 在「配置确认」阶段为每个桶存储（全码桶与每个简码级别桶）输出一条日志，说明其所选后端（密集 / 稀疏）。
2. THE 后端选择日志 SHALL 包含运行时计算得到的容量值（全码为 `code_space`，简码级别为该级 `code_base^L`）与所采用的后端选择阈值。
3. WHERE 某桶存储选择密集后端，THE 日志 SHALL 输出该后端按运行时容量与桶元素大小估算的内存占用（单线程），使内存来源对使用者透明。
4. WHERE 某桶存储选择稀疏后端，THE 日志 SHALL 说明其内存随非空桶数（其上界为运行时确定的汉字数 n_chars）增长，而非随容量增长。
5. THE 后端选择日志 SHALL 在整个优化过程中至多输出一次（在单线程的配置确认阶段），SHALL NOT 在每线程或每次评估器构建时重复输出。
6. THE 后端选择日志中的全部规模数字（`code_base`、`max_parts`、`code_space`、各级 `code_base^L`、n_chars、预估内存）SHALL 在运行时根据配置与输入计算，SHALL NOT 以字面常量硬编码于代码中；唯一允许的编译期常量为后端选择阈值本身。
7. WHERE 简码整体关闭（`enable_simple_code == false`），THE 简码级别桶的后端选择日志 SHALL 省略，仅输出全码桶一条。
