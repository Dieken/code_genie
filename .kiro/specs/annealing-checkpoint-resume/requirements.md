# Requirements Document

## Introduction

code_genie 是一个用 Rust 编写的输入法字根编码方案优化器，核心算法为模拟退火（`src/annealing.rs`），由 `rayon` 并行执行多个独立线程的退火实例（`main.rs`：`(0..num_threads).into_par_iter().map(simulated_annealing)`）。一次完整优化可能运行很久（百万~千万步 × 多线程）。目前一旦进程退出（正常结束之外的任何中断），全部进度丢失，无法续算。

本特性新增**退火进度 checkpoint（断点续算）**能力：
- 以「进度汇报」的频率周期性地把每个退火线程的状态写出到 **output 目录**下的 checkpoint 文件（而非当前工作目录，避免多个 code_genie 实例并行运行时互相覆盖）。
- 采用「**每线程一个 checkpoint 文件**」的布局：各线程在自己的汇报节奏独立写自己的文件，无跨线程锁/协调，对退火热路径干扰最小。
- 退火开始前把本次运行依赖的**输入文件备份**到 output 目录，使 resume 时能用与首次运行完全一致的输入重建优化上下文。
- 妥善处理 **Ctrl-C 中断**：捕获信号后令各线程写出最新 checkpoint 再退出，不丢失进度。
- 新增 **resume 子命令**：指向既有 output 目录即可从断点继续优化，并复用同一 output 目录（不新建）。

参考实现：本仓库 commit `9be44b1`（`src/checkpoint.rs` + resume 子命令 + ctrlc）。本特性在其思路基础上做三点关键调整：(a) 周期性写而非仅 Ctrl-C 时写；(b) 写入 output 目录而非当前目录；(c) 每线程一个文件而非单一全局文件。同时适配**当前**更复杂的退火主循环（简码延迟激活、按分量存储最优解等）。

设计前提（沿用参考实现的关键简化）：checkpoint 只保存退火的**控制状态**与当前解 `assignment`，**不序列化** `Evaluator`（含稀疏桶等大结构）；resume 时由 `Evaluator::new(&assignment)` 重建评估器。这使 checkpoint 文件很小、写入开销极低。

本文档使用 EARS 模式描述验收标准，所有需求遵循 INCOSE 质量规则。

## Glossary

- **优化器（Optimizer）**：code_genie 整体程序。
- **退火器（Annealer）**：模拟退火主循环逻辑，位于 `src/annealing.rs`。
- **退火线程（SA_Thread）**：`rayon` 并行执行的单个独立退火实例，由 `thread_id` 标识。
- **分配（Assignment）**：字根组到键位的映射 `assignment[group] = key`，退火搜索的决策变量。
- **输出目录（Output_Dir）**：本次运行创建的 `output-{YYYYMMDD-HHMMSS}/` 目录。
- **Checkpoint 目录（Checkpoint_Dir）**：`{Output_Dir}/checkpoint/`，存放 checkpoint 文件。
- **输入备份目录（Inputs_Dir）**：`{Output_Dir}/inputs/`，存放退火前备份的输入文件副本。
- **线程检查点（Thread_Checkpoint）**：单个退火线程在某一步的可续算控制状态，序列化为 `{Checkpoint_Dir}/thread-{NN}.json`。
- **检查点元信息（Checkpoint_Meta）**：本次运行的全局信息（含校准得到的 `scale_config`、温度参数、总步数、线程数、格式版本），序列化为 `{Checkpoint_Dir}/meta.json`，运行期不变。
- **汇报间隔（Report_Interval）**：退火进度汇报的步数间隔，当前为 `max(1, total_steps / 20)`。
- **检查点间隔（Checkpoint_Interval）**：写出 checkpoint 的步数间隔，由配置比例换算 `max(1, floor(total_steps × checkpoint_interval_ratio))`。
- **激活闩锁（Simple_Activation_Latch）**：简码计算一旦激活即永久保持激活的状态标志（`simple_activated`）。
- **停止标志（Stop_Flag）**：跨线程共享的 `Arc<AtomicBool>`，Ctrl-C 时置位，退火线程据此提前退出并写出最新 checkpoint。
- **原子写（Atomic_Write）**：先写 `*.tmp` 临时文件再 `rename` 到目标路径，避免写入中途中断导致文件损坏。
- **规范检查点文件（Canonical_Checkpoint）**：`{Checkpoint_Dir}/thread-{NN}.json`，恒为该线程最新的线程检查点（真实文件，非符号链接），是 resume 的稳定入口。
- **归档检查点文件（Archived_Checkpoint）**：旧版本线程检查点，命名为 `{Checkpoint_Dir}/thread-{NN}-{TIMESTAMP}.json`，写新版本时由旧的规范文件重命名而来，永不删除，供手动回滚。
- **依赖输入文件（Input_Files）**：本次运行所读取的输入文件集合，见需求 4。
- **基线版本（Baseline）**：本特性实现前的当前版本。

## Requirements

### 需求 1：周期性写出每线程 checkpoint

**用户故事：** 作为优化器使用者，我希望退火过程中按汇报频率周期性地保存每个线程的进度，从而在任何时刻中断后都能从最近的检查点继续。

#### 验收标准

1. THE 退火器 SHALL 为每个退火线程把最新线程检查点写入规范检查点文件 `{Checkpoint_Dir}/thread-{NN}.json`（真实文件，`NN` 为两位零填充的 `thread_id`）。
2. WHILE 退火主循环运行，THE 退火线程 SHALL 每隔 Checkpoint_Interval 步写出一次自己的线程检查点。
3. THE 各退火线程 SHALL 各自独立写出自己的 checkpoint 文件，SHALL NOT 与其他线程共享同一文件或共享写锁。
4. THE 写出线程检查点 SHALL 仅在汇报/检查点边界（非每步）发生，使其不增加退火单步热路径的开销。
5. WHERE 简码整体关闭或开启，THE 周期性 checkpoint 写出 SHALL 同样进行（与简码是否启用无关）。

### 需求 2：checkpoint 写入 output 目录（避免多实例互相覆盖）

**用户故事：** 作为同时运行多个 code_genie 实例的使用者，我希望各实例的 checkpoint 互不干扰，从而可以并行跑多个优化而不互相覆盖。

#### 验收标准

1. THE 优化器 SHALL 将全部 checkpoint 文件写入本次运行的 `{Output_Dir}/checkpoint/` 目录，SHALL NOT 写入当前工作目录。
2. THE 优化器 SHALL 在退火开始前创建 `{Checkpoint_Dir}` 目录（若不存在）。
3. THE 不同 `Output_Dir`（不同时间戳）的运行 SHALL 写入各自独立的 Checkpoint_Dir，使并行实例的 checkpoint 互不覆盖。

### 需求 3：原子写保证 checkpoint 文件不被写坏

**用户故事：** 作为优化器使用者，我希望即使在写 checkpoint 的瞬间进程被杀死，已有的 checkpoint 也不被损坏，从而总能从一个完整的检查点恢复。

#### 验收标准

1. WHEN 写出任一 checkpoint 文件（线程检查点或元信息），THE 优化器 SHALL 先写入同目录下的临时文件再 `rename` 到目标路径（原子写）。
2. IF 写入在 `rename` 之前被中断，THEN 目标 checkpoint 文件 SHALL 保持为上一次成功写入的完整内容（不被部分写入损坏）。
3. THE 某一线程 checkpoint 文件的写入失败或中断 SHALL NOT 损坏其他线程的 checkpoint 文件。

### 需求 4：退火前备份依赖输入文件

**用户故事：** 作为优化器使用者，我希望本次运行用到的输入文件被备份，从而 resume 时能用与首次运行完全一致的输入，结果可比、可续。

#### 验收标准

1. WHEN 优化器创建 Output_Dir 后、退火开始前，THE 优化器 SHALL 将本次运行依赖的全部输入文件复制到 `{Output_Dir}/inputs/` 目录。
2. THE 被备份的依赖输入文件 SHALL 包含：CLI 指定的配置文件（`-c` 路径，默认 `config.toml`）、`files.fixed`、`files.dynamic`、`files.splits`、`files.pair_equiv`、`files.key_dist` 所指向的文件。
3. THE 备份 SHALL 以输入文件的原始文件名（basename）保存到 Inputs_Dir。
4. IF 某依赖输入文件不存在或复制失败，THEN THE 优化器 SHALL 输出告警；该失败 SHALL NOT 阻止已成功备份的其它文件，且 SHALL NOT 中止优化（除非该文件本就是优化所必需而导致后续加载失败）。
5. THE 备份 SHALL 不修改原始输入文件（只读复制）。

### 需求 5：线程检查点内容足以忠实续算当前退火

**用户故事：** 作为优化器使用者，我希望检查点保存了足够的状态，从而 resume 后退火能从断点忠实继续（最优解不丢、激活状态不丢、温度状态不丢）。

#### 验收标准

1. THE 线程检查点 SHALL 包含 `thread_id`、当前解 `assignment`、当前已完成步数 `current_step`、以及该检查点的生成时间戳 `timestamp`（用于归档命名）。
2. THE 线程检查点 SHALL 包含按分量存储的最优解：`best_assignment`、`best_full_score`、`best_simple_score`、`best_score`、`best_metrics`、`best_simple_metrics`。
3. THE 线程检查点 SHALL 包含退火控制状态：温度乘子 `temp_multiplier`、连续未改进步数 `steps_since_improve`、上次汇报最优得分 `last_best_score`。
4. THE 线程检查点 SHALL 包含简码激活闩锁 `simple_activated`。
5. THE 线程检查点 SHALL NOT 序列化 `Evaluator`（全码桶、简码评估器等）；resume 时 SHALL 由 `Evaluator::new(&assignment)` 据当前解重建评估器。
6. THE 随机数发生器状态 SHALL NOT 被序列化（沿用 `thread_rng()`）；resume 后随机轨迹不要求与中断前逐步一致，但最优解与控制状态 SHALL 被忠实恢复。

### 需求 6：元信息保证 resume 上下文一致

**用户故事：** 作为优化器使用者，我希望 resume 用与首次运行一致的缩放因子与温度参数重建上下文，从而续算的评分口径不漂移。

#### 验收标准

1. THE 优化器 SHALL 在退火开始前（缩放校准完成后）将检查点元信息写入 `{Checkpoint_Dir}/meta.json`，且运行期不再修改。
2. THE 检查点元信息 SHALL 包含：格式版本号、校准得到的 `scale_config`、实际使用的温度参数、总步数 `total_steps`、线程数 `num_threads`、保存时间戳。
3. WHEN resume，THE 优化器 SHALL 复用元信息中的 `scale_config`，SHALL NOT 重新执行缩放校准（避免因校准的随机性导致评分口径漂移）。
4. THE 元信息 SHALL 记录格式版本号，供加载时做兼容性校验。

### 需求 7：resume 子命令从既有 output 目录续算

**用户故事：** 作为优化器使用者，我希望用一条 resume 命令指向之前的 output 目录就能继续优化，从而无需手动拼接参数。

#### 验收标准

1. THE 优化器 SHALL 提供 `resume` 子命令，接受一个指向既有 Output_Dir 的参数（如 `resume -d {Output_Dir}`）。
2. WHEN 执行 resume，THE 优化器 SHALL 从 `{Output_Dir}/inputs/` 读取备份的配置与输入文件重建优化上下文，SHALL NOT 依赖当前工作目录中可能已变化的输入文件。
3. WHEN 执行 resume，THE 优化器 SHALL 只读 `{Output_Dir}/inputs/` 下的文件，SHALL NOT 修改、覆盖或重新写入 inputs 目录中的任何文件。
4. WHEN 执行 resume，THE 优化器 SHALL 只读 `{Output_Dir}/checkpoint/meta.json`，SHALL NOT 重写或修改该文件；元信息中的 `scale_config`/`total_steps`/`num_threads`/温度参数视为本次续算的权威值。
5. WHEN 执行 resume，THE 优化器 SHALL 从各 `thread-{NN}.json` 恢复每线程状态，并为每个线程从其 `current_step` 继续退火至 `total_steps`。
6. WHEN 执行 resume，THE 优化器 SHALL 复用传入的 Output_Dir 作为本次（续算）的输出目录，SHALL NOT 新建带新时间戳的目录；其日志中「输出目录」SHALL 显示该既有目录。
7. WHILE resume 续算，THE 退火器 SHALL 继续按需求 1/13 的频率与归档规则把更新后的线程检查点写回同一 Checkpoint_Dir。
8. WHEN resume 完成续算，THE 优化器 SHALL 将最终结果产物写入该 Output_Dir（与正常完成时一致）。
9. IF 某线程的 `current_step` 已达到或超过 `total_steps`，THEN THE 退火器 SHALL 跳过该线程的主循环并直接进入收尾（不报错）。

### 需求 8：Ctrl-C 优雅中断并写出最新 checkpoint

**用户故事：** 作为优化器使用者，我希望按 Ctrl-C 能暂停优化并保存最新进度，从而稍后用 resume 继续而不丢失工作。

#### 验收标准

1. THE 优化器 SHALL 安装 Ctrl-C（SIGINT）信号处理器，将共享停止标志 Stop_Flag 置位。
2. WHILE 退火主循环运行，THE 退火线程 SHALL 周期性检查 Stop_Flag；WHEN 检测到置位，THE 退火线程 SHALL 写出自己的最新线程检查点并退出主循环。
3. WHEN 优化因 Ctrl-C 被中断，THE 优化器 SHALL 输出可用于恢复的提示（包含 resume 命令与 Output_Dir）。
4. THE Ctrl-C 时的 checkpoint 写出 SHALL 同样使用原子写（需求 3）。
5. WHERE 优化被 Ctrl-C 中断，THE 优化器 SHALL 不因中断而 panic 或留下损坏的 checkpoint 文件。

### 需求 9：检查点写出频率可配置

**用户故事：** 作为优化器使用者，我希望能调整 checkpoint 的写出频率，从而在「更频繁保存（更抗中断）」与「更少 I/O」之间权衡。

#### 验收标准

1. THE 优化器 SHALL 在配置中提供 `checkpoint_interval_ratio`，其单位为「占总步数 `total_steps` 的比例」（与既有 `reconcile_interval_ratio` 等比例型配置一致）。
2. THE 优化器 SHALL 以 `Checkpoint_Interval = max(1, floor(total_steps × checkpoint_interval_ratio))` 换算实际的检查点写出步数间隔。
3. WHEN 配置缺失 `checkpoint_interval_ratio`，THE 优化器 SHALL 采用默认值 `0.05`（即每 5% 总步数写一次，约 20 次/全程，与汇报频率一致）。
4. THE 优化器 SHALL 将 `checkpoint_interval_ratio` 及其单位说明同步写入 `config.toml.example` 与 `moling/config.toml`（带注释）。

### 需求 10：checkpoint 功能始终开启且开销可忽略

**用户故事：** 作为优化器使用者，我希望 checkpoint 功能默认始终启用，从而无需任何额外操作即可获得断点续算保护。

#### 验收标准

1. THE 优化器 SHALL 在 `optimize`（及 `resume`）流程中始终启用 checkpoint 写出，无需开关即生效。
2. THE checkpoint 写出 SHALL NOT 改变退火的搜索逻辑、接受判定与最终上报指标（仅为旁路持久化）。
3. THE checkpoint 写出 SHALL 仅在检查点边界发生，使其对总运行时间的影响可忽略。

### 需求 11：续算正确性与状态一致

**用户故事：** 作为优化器使用者，我希望 resume 出来的状态与中断时刻一致，从而续算是「接着算」而非「重头算」或「丢失最优」。

#### 验收标准

1. WHEN resume 恢复某线程，THE 退火器 SHALL 用检查点中的 `assignment` 重建评估器，并将 `simple_active` 置为与 `simple_activated` 闩锁一致的状态。
2. WHEN resume 恢复某线程且 `simple_activated` 为真，THE 退火器 SHALL 据当前进度设置有效简码权重，使续算的综合得分口径与中断前一致。
3. THE resume 恢复的 `best_assignment`/`best_score`/`best_full_score`/`best_simple_score`/`best_metrics`/`best_simple_metrics` SHALL 与检查点保存值逐字段一致。
4. IF 元信息的格式版本与当前程序不兼容、或线程检查点数量与 `num_threads` 不一致、或缺少必要文件，THEN THE 优化器 SHALL 输出明确错误并安全退出（不静默产生错误结果）。

### 需求 12：最小改动与依赖

**用户故事：** 作为代码维护者，我希望本特性只新增断点续算所需的代码与依赖，从而不扩大改动范围与回归风险。

#### 验收标准

1. THE 优化器 SHALL 仅修改与 checkpoint 保存/恢复、输入备份、Ctrl-C 处理、resume 子命令、相关配置项直接相关的代码。
2. THE 优化器 SHALL 新增 `ctrlc` 依赖用于 SIGINT 处理；序列化复用既有的 `serde`/`serde_json`。
3. WHERE 为序列化 checkpoint 需要，THE 优化器 SHALL 为 `Metrics`、`SimpleMetrics`、`ScaleConfig` 等被持久化的类型添加 `Serialize`/`Deserialize` 派生，且不改变其既有语义。
4. THE 本特性 SHALL NOT 改变正常（无中断、不 resume）一次性运行的最终产物内容。

### 需求 13：旧 checkpoint 保留与归档（不删除、带时间戳、不使用符号链接）

**用户故事：** 作为优化器使用者，我希望写新 checkpoint 时不删除旧的，旧版本以时间戳后缀保留，从而在新检查点有问题时能手动回滚到任意历史版本；且该机制在 Windows 与 Unix 上都可靠工作。

#### 验收标准

1. WHEN 退火器写出某线程的新检查点，IF 该线程的规范检查点文件 `thread-{NN}.json` 已存在，THEN THE 退火器 SHALL 先将其重命名归档为 `thread-{NN}-{TIMESTAMP}.json`（`TIMESTAMP` 取被归档那一版自身的 `timestamp`），再写入新的规范检查点文件。
2. THE 退火器 SHALL NOT 删除任何归档检查点文件（历史版本永久保留，供手动回滚）。
3. THE 规范检查点文件 `thread-{NN}.json` SHALL 始终是真实文件且为该线程最新版本，SHALL NOT 使用符号链接（symlink）指向带时间戳的文件——以保证 Windows（无需管理员/开发者模式）与 Unix 一致可靠。
4. THE 归档命名所用 `TIMESTAMP` SHALL 具有足以区分相邻两次写出的精度（如毫秒级），使同一线程的多次归档不互相覆盖。
5. THE 手动回滚 SHALL 可通过将某个 `thread-{NN}-{TIMESTAMP}.json` 复制/重命名为 `thread-{NN}.json` 实现，且其后的 resume 读取该规范文件即生效。
6. THE 归档与写新文件 SHALL 仅使用文件 `rename` 与原子写（需求 3），不依赖符号链接或平台特定能力。
7. WHILE resume 续算，THE 归档规则 SHALL 同样适用：续算首次写出前，SHALL 将上一轮运行遗留的规范文件按其 `timestamp` 归档后再写新版本。
