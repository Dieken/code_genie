# Requirements Document

## Introduction

code_genie 是一个用 Rust 编写的输入法字根编码方案优化器，核心算法为模拟退火（`src/annealing.rs`），由 `rayon` 并行执行多个独立线程的退火实例（`main.rs`：`(0..num_threads).into_par_iter().map(simulated_annealing_resumable)`）。每个退火线程当前各自调用 `multi_start_init(ctx, cfg, thread_id)` 生成自己的初始分配（assignment），互不共享。

本特性新增**从既有结果给 `optimize` 播种（seed）**的能力：允许使用一份或多份**既有的 `output-keymap.txt`** 作为退火的初始解，替换 `multi_start_init` 的随机/贪心起点，从而对已经算出的较优方案做继续微调（fine-tune），而不必每次都从头随机起步。

提供两种播种入口（互补，二者择一使用）：

- **`--seed-dir <既有 output 目录>`**：读取该目录下各线程的 `thread-NN/output-keymap.txt` 作为种子，按「线程→目录」轮询映射分配给本次运行的各退火线程（线程数多于种子目录数时 round-robin 循环复用；少于时只用前若干个）。
- **`--seed-keymap <keymap 文件>`**：读取单个 keymap 文件作为种子，所有退火线程使用同一个初始解。

关键设计前提：

- **播种只替换退火初始化**，校准（calibrate）、退火主循环、refine 收尾等其余流程**保持不变**；播种走 fresh 路径（`Evaluator::new_full_only`、起始步 0、简码延迟激活），**不是** resume 续算。
- **种子文件仅用于重建初始 assignment**；本次运行的输入文件（`input-*.txt`）与 `config.toml` 一律使用本次 `optimize` 正常运行时的最新版本（即当前 `cfg` 与当前 `ctx`），**不读取**种子目录里的 `inputs/` 备份。
- 种子 keymap 用现成的 `loader::load_keymap(keymap_path, division_path)` 解析为「子字根名 → 键位」映射，再据**当前** `ctx`（当前 division/fixed/allowed 决定的字根组）转换成 assignment，并做约束校验。

本文档使用 EARS 模式描述验收标准，所有需求遵循 INCOSE 质量规则。

## Glossary

- **优化器（Optimizer）**：code_genie 整体程序。
- **退火器（Annealer）**：模拟退火主循环逻辑，位于 `src/annealing.rs`。
- **退火线程（SA_Thread）**：`rayon` 并行执行的单个独立退火实例，由 `thread_id`（0 起）标识。
- **分配（Assignment）**：字根组到键位的映射 `assignment[group] = key`，退火搜索的决策变量；类型 `Vec<u8>`，长度为 `ctx.num_groups`。
- **字根组（Group）**：`ctx.groups[gi]`，含 `roots: Vec<String>` 与 `allowed_keys: Vec<u8>`；`assignment[gi]` 为该组所选键位。
- **固定字根（Fixed_Root）**：`input-fixed.txt` 中只给单个键位的字根，存于 `ctx.fixed_roots`，不参与 assignment。
- **受限组（Constrained_Group）**：`input-fixed.txt` 中给多个候选键位的字根组，其 `allowed_keys` 为这些候选键。
- **允许键集（Allowed_Keys）**：每个组可取键位集合 `ctx.groups[gi].allowed_keys`；动态组默认取 `config.toml` 的 `keys.allowed` 全局键集，受限组取其声明的候选键。
- **种子（Seed）**：用于替换某退火线程初始解的一份既有 `output-keymap.txt`。
- **种子目录（Seed_Dir）**：`--seed-dir` 指向的既有 output 目录，其下含若干 `thread-NN/output-keymap.txt`。
- **种子 keymap 文件（Seed_Keymap）**：`--seed-keymap` 指向的单个 keymap 文件。
- **keymap 文件**：每行 `基础字根名\t编码\t使用次数`，编码为首字母大写的键位串（如 `Wko` → 键位 `[w, k, o]`）；`output-keymap.txt` 即此格式。
- **划分文件（Division_File）**：`config.toml` 的 `files.splits`（亦即拆分表/division），用于 `load_keymap` 确定每个基础字根的子字根后缀顺序。
- **种子分配（Seed_Assignment）**：由种子 keymap 据当前 `ctx` 转换并校验后得到的 `assignment: Vec<u8>`。
- **轮询映射（Round_Robin_Mapping）**：当退火线程数与种子数不等时，线程 `i` 取第 `i % seed_count` 个种子。
- **多起点初始化（Multi_Start_Init）**：现有 `multi_start_init(ctx, cfg, thread_id)`，被播种替换的初始化入口。
- **基线版本（Baseline）**：本特性实现前的当前版本。

## Requirements

### 需求 1：`--seed-dir` 从既有 output 目录播种

**用户故事：** 作为优化器使用者，我希望用 `--seed-dir` 指向一次既有运行的 output 目录，从而让本次退火从那次各线程算出的结果继续微调。

#### 验收标准

1. THE 优化器 SHALL 为 `optimize` 子命令提供 `--seed-dir <DIR>` 选项，接受一个既有 output 目录路径。
2. WHEN 指定 `--seed-dir <DIR>`，THE 优化器 SHALL 读取 `{DIR}/thread-{NN}/output-keymap.txt`（`NN` 为两位零填充）作为种子，SHALL NOT 读取 `{DIR}/output-keymap.txt`。
3. THE 优化器 SHALL 按 `thread-NN` 子目录的编号升序收集种子，得到种子序列 `seeds[0..K]`（`K` 为找到的种子个数）。
4. IF `{DIR}` 不存在或其下不含任何 `thread-{NN}/output-keymap.txt`，THEN THE 优化器 SHALL 输出明确错误并安全退出（非零退出码），SHALL NOT 静默回退到普通随机初始化。
5. THE 优化器 SHALL 仅用 `{DIR}` 中的 `output-keymap.txt` 作为种子来源，SHALL NOT 读取 `{DIR}/inputs/`、`{DIR}/checkpoint/` 或其它产物用于本次运行的输入或配置。

### 需求 2：`--seed-keymap` 从单个 keymap 文件播种

**用户故事：** 作为优化器使用者，我希望用 `--seed-keymap` 指定单个 keymap 文件，从而让所有退火线程都从同一个既有方案继续微调。

#### 验收标准

1. THE 优化器 SHALL 为 `optimize` 子命令提供 `--seed-keymap <FILE>` 选项，接受单个 keymap 文件路径。
2. WHEN 指定 `--seed-keymap <FILE>`，THE 优化器 SHALL 将该文件解析为唯一种子，并令全部退火线程使用同一个种子分配。
3. IF `{FILE}` 不存在或无法解析出任何有效编码行，THEN THE 优化器 SHALL 输出明确错误并安全退出（非零退出码）。

### 需求 3：两种播种入口互斥

**用户故事：** 作为优化器使用者，我希望播种入口的语义明确，从而不会因同时指定两种来源而产生歧义。

#### 验收标准

1. IF `--seed-dir` 与 `--seed-keymap` 同时被指定，THEN THE 优化器 SHALL 输出明确错误并安全退出，SHALL NOT 任意选择其一。
2. WHEN 既未指定 `--seed-dir` 也未指定 `--seed-keymap`，THE 优化器 SHALL 保持基线行为，即各线程用 `multi_start_init` 初始化（本特性不改变无播种时的任何行为）。

### 需求 4：keymap 转换为校验合格的种子分配

**用户故事：** 作为优化器使用者，我希望种子 keymap 被据当前方案正确转换并校验，从而播种得到的初始解是当前方案下的一个合法分配。

#### 验收标准

1. WHEN 解析种子，THE 优化器 SHALL 用 `loader::load_keymap(keymap_path, division_path)` 把 keymap 解析为「子字根名 → 键位索引」映射，其中 `division_path` 取本次运行 `cfg.files.splits`（当前最新版本）。
2. WHEN 由「子字根名 → 键位」构造 Seed_Assignment，THE 优化器 SHALL 对每个 `(子字根名, 键位)`：用 `ctx.root_to_group` 定位组 `gi` 并置 `seed_assignment[gi] = 键位`；对不属于任何动态/受限组的字根名（如固定字根）SHALL 跳过。
3. WHEN 为某组设置键位，IF 该键位不在 `ctx.groups[gi].allowed_keys` 中，THEN THE 优化器 SHALL 视为约束违例，输出包含该字根名与键位的明确错误并安全退出。
4. IF 同一组的不同子字根名在 keymap 中映射到不同键位（组内不一致），THEN THE 优化器 SHALL 输出包含该组信息的明确错误并安全退出。
5. THE 约束校验 SHALL 完全基于当前 `ctx`（当前 division/fixed/allowed 决定的组与 allowed_keys），SHALL NOT 依赖种子目录里备份的旧输入。
6. THE 优化器 SHALL NOT 在代码中写死 `max_parts`、`n_chars`、可用键数等运行时才确定的量；所有此类数值 SHALL 取自当前 `ctx`/`cfg`。

### 需求 5：缺失组报警并随机合法填充

**用户故事：** 作为优化器使用者，我希望当种子未覆盖到某些当前组时仍能跑通，从而即使种子方案与当前方案不完全一致也能继续优化。

#### 验收标准

1. WHEN 由种子构造 Seed_Assignment 后存在未被赋值的动态/受限组，THE 优化器 SHALL 对每个这样的组从其 `allowed_keys` 中随机选取一个合法键位填充。
2. WHEN 发生缺失组随机填充，THE 优化器 SHALL 输出告警，说明被随机填充的组数量（以提醒种子与当前方案不完全匹配）。
3. THE 缺失组随机填充 SHALL 使各组最终都获得 `allowed_keys` 内的合法键位，使 Seed_Assignment 为当前方案下的合法分配。
4. THE 缺失组的随机填充 SHALL 使用 `thread_rng()`（与现有初始化随机源一致）。

### 需求 6：种子仅替换退火初始化，其余流程不变

**用户故事：** 作为优化器使用者，我希望播种只改变退火起点，从而校准、主循环、收尾等行为与基线完全一致，结果可比。

#### 验收标准

1. WHEN 为某退火线程播种，THE 退火器 SHALL 用 Seed_Assignment 替换该线程 `multi_start_init` 的输出作为初始解，其余初始化路径（`Evaluator::new_full_only`、起始步 0、简码延迟激活）SHALL 与基线 fresh 运行一致。
2. THE 播种 SHALL NOT 改变缩放校准（calibrate）流程，校准 SHALL 照常运行并产生本次运行的 `scale_config`。
3. THE 播种 SHALL NOT 改变退火主循环、温度调度、接受判定、简码激活与渐进、refine 收尾及最终上报指标的逻辑。
4. THE 播种 SHALL 走 fresh 路径而非 resume 路径，SHALL NOT 复用任何 checkpoint 或从非零步续算。
5. THE 播种线程 SHALL 照常按需求（既有 checkpoint 特性）周期性写出自己的 checkpoint 到本次运行的 `{Output_Dir}/checkpoint/`。

### 需求 7：线程与种子的轮询映射

**用户故事：** 作为优化器使用者，我希望线程数与种子数不等时映射规则清晰可预期，从而知道每个线程用了哪个种子。

#### 验收标准

1. WHEN 种子个数为 `K`（`K ≥ 1`）且退火线程数为 `T`，THE 优化器 SHALL 令线程 `i`（`0 ≤ i < T`）使用第 `i % K` 个种子（round-robin 循环复用）。
2. WHEN `T < K`，THE 优化器 SHALL 只使用前 `T` 个种子（线程 `i` 用第 `i` 个），其余种子不使用。
3. WHEN 使用 `--seed-keymap`（`K = 1`），THE 优化器 SHALL 令所有线程使用该唯一种子（与轮询规则 `i % 1 == 0` 一致）。
4. THE 同一个 Seed_Assignment 被多个线程复用时，各线程 SHALL 各自拥有该分配的独立副本，互不影响后续退火搜索。

### 需求 8：播种过程的日志可观测

**用户故事：** 作为优化器使用者，我希望日志清楚说明播种来源与映射结果，从而确认本次运行确实从期望的种子起步。

#### 验收标准

1. WHEN 启用播种，THE 优化器 SHALL 在退火开始前输出播种来源（`--seed-dir` 路径或 `--seed-keymap` 路径）与找到的种子个数 `K`。
2. WHEN 启用播种，THE 优化器 SHALL 输出线程→种子的映射概述（如线程数 `T`、映射规则为 round-robin）。
3. WHEN 任一种子发生缺失组随机填充，THE 优化器 SHALL 按需求 5.2 输出该种子被随机填充的组数量。

### 需求 9：最小改动与行为保持

**用户故事：** 作为代码维护者，我希望本特性只新增播种所需的代码，从而不扩大改动范围与回归风险。

#### 验收标准

1. THE 优化器 SHALL 仅修改与播种相关的代码：`optimize` 的 CLI 选项、种子解析与「线程→种子」映射、keymap→assignment 转换与校验、以及退火初始化处接入 Seed_Assignment 的钩子。
2. THE 本特性 SHALL NOT 改变无播种（既未指定 `--seed-dir` 也未指定 `--seed-keymap`）一次性运行的最终产物内容。
3. THE 本特性 SHALL 复用既有 `loader::load_keymap`，SHALL NOT 重复实现 keymap 解析逻辑。
4. THE 本特性 SHALL NOT 引入新的第三方依赖。
