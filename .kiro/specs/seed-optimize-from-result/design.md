# Design Document: 从既有结果给 optimize 播种（seed）

## Overview

本特性给 `code_genie optimize` 新增两个互斥的 CLI 选项 `--seed-dir` 与 `--seed-keymap`，用既有的 `output-keymap.txt` 作为退火初始解，替换 `multi_start_init` 的随机/贪心起点，对已算出的较优方案做继续微调。

核心思路与现有代码的契合点：

- **初始化已是每线程独立**：`simulated_annealing_resumable` 在 fresh 分支调用 `multi_start_init(ctx, cfg, thread_id)`，每线程一份 assignment。播种只需在 fresh 分支提供一个「外部初始 assignment」覆盖即可，结构上与已有的 `resume: Option<&ThreadCheckpoint>` 覆盖同形，但走 fresh 路径（`Evaluator::new_full_only`、起始步 0、简码延迟激活）。
- **keymap 解析已有现成函数**：`loader::load_keymap(keymap_path, division_path) -> HashMap<String, u8>`（子字根名→键位）。再用 `ctx.root_to_group` 把它折叠成 `assignment: Vec<u8>` 并校验。
- **种子只影响起点**：校准、主循环、refine、checkpoint 写出等其余流程完全不变；本次运行的输入文件与配置一律用当前 `cfg`/`ctx`，不读种子目录的 `inputs/`。

数据流（optimize 启用播种时）：

```
CLI(--seed-dir/--seed-keymap)
   │
   ├─ 解析种子路径 → Vec<seed_keymap_path>            (main.rs)
   │
加载数据 / 校准 → ctx, cfg                              (run_optimize 现有流程，不变)
   │
对每个 seed_keymap_path:
   load_keymap(path, cfg.files.splits) → HashMap<String,u8>
   keymap_to_assignment(ctx, map, &mut rng) → Vec<u8>   (校验 + 缺失组随机填充)
   ⇒ seeds: Vec<Vec<u8>>  (长度 K)
   │
并行退火: 线程 i 用 seeds[i % K] 作为初始解
   simulated_annealing_resumable(..., seed = Some(&seeds[i % K]))
   │
finalize_and_save (不变)
```

## Architecture

### 改动模块一览

| 模块 | 改动 | 说明 |
|------|------|------|
| `src/main.rs` | `Commands::Optimize` 增加 `seed_dir`/`seed_keymap` 字段；`run_optimize` 增加种子解析与映射；并行调用传入种子 | CLI 与编排 |
| `src/loader.rs` | 新增 `keymap_to_assignment(ctx, root_to_key, rng) -> Result<(Vec<u8>, usize), String>` | keymap→assignment + 校验 + 缺失填充 |
| `src/annealing.rs` | `simulated_annealing_resumable` 增加 `seed: Option<&[u8]>` 参数；fresh 分支用 seed 覆盖 `multi_start_init` | 退火初始化接入点 |

无新依赖（需求 9.4）。

### CLI 设计

```rust
Optimize {
    #[arg(short = 'd', long = "output-dir")]
    output_dir: Option<String>,

    /// 从既有 output 目录播种：读取 {DIR}/thread-NN/output-keymap.txt 作为各线程初始解
    #[arg(long = "seed-dir")]
    seed_dir: Option<String>,

    /// 从单个 keymap 文件播种：所有线程使用同一初始解
    #[arg(long = "seed-keymap")]
    seed_keymap: Option<String>,
}
```

`run_optimize` 签名扩展为 `run_optimize(cfg, cli_config_path, output_dir_opt, seed_dir, seed_keymap)`。

### 种子来源解析（main.rs）

新增 `fn resolve_seed_paths(seed_dir: Option<&str>, seed_keymap: Option<&str>) -> Result<Vec<String>, String>`：

1. **互斥校验**（需求 3.1）：两者均 `Some` → `Err`。
2. **均为 `None`**（需求 3.2）：返回空 `Vec`，表示不播种（保持基线行为）。
3. **`--seed-keymap`**（需求 2）：校验文件存在 → 返回 `vec![path]`（`K = 1`）。
4. **`--seed-dir`**（需求 1）：
   - 枚举 `{DIR}` 下名为 `thread-{NN}`（两位数字）的子目录，对每个存在 `output-keymap.txt` 的，收集其路径。
   - 按 `NN` 数字升序排序（保证线程→种子映射稳定可预期，需求 1.3）。
   - 若结果为空 → `Err`（需求 1.4）。

错误一律在 `run_optimize` 入口处理：打印 `eprintln!` 后 `std::process::exit(1)`。

> 仅枚举 `thread-NN/output-keymap.txt`，不触碰 `{DIR}/inputs|checkpoint`（需求 1.5）。

### keymap → assignment 转换与校验（loader.rs）

```rust
/// 将 keymap（子字根名→键位）据当前 ctx 折叠为 assignment，并做约束校验。
/// 返回 (assignment, filled_missing_count)；filled_missing_count 为随机填充的缺失组数。
pub fn keymap_to_assignment(
    ctx: &OptContext,
    root_to_key: &HashMap<String, u8>,
    rng: &mut impl Rng,
) -> Result<(Vec<u8>, usize), String> {
    let n = ctx.num_groups;
    // None = 未赋值；用于检测缺失组与组内不一致
    let mut chosen: Vec<Option<u8>> = vec![None; n];

    for (name, &key) in root_to_key {
        // 仅处理属于动态/受限组的字根名；固定字根（fixed_roots）不在 root_to_group 中 → 跳过
        let Some(&gi) = ctx.root_to_group.get(name) else { continue; };
        // 约束 1：键位必须在该组 allowed_keys 内（需求 4.3）
        if !ctx.groups[gi].allowed_keys.contains(&key) {
            return Err(format!("字根 '{}' 的键位 {} 不在组 {} 的 allowed_keys 内", name, key, gi));
        }
        // 约束 2：组内一致（需求 4.4）
        match chosen[gi] {
            Some(prev) if prev != key =>
                return Err(format!("组 {} 内字根映射到不同键位({} vs {})，种子与当前方案不一致", gi, prev, key)),
            _ => chosen[gi] = Some(key),
        }
    }

    // 缺失组：随机合法填充（需求 5）
    let mut filled = 0usize;
    let mut assignment = vec![0u8; n];
    for gi in 0..n {
        match chosen[gi] {
            Some(k) => assignment[gi] = k,
            None => {
                let allowed = &ctx.groups[gi].allowed_keys;
                // allowed 恒非空（由 load_fixed/load_dynamic 保证）
                assignment[gi] = allowed[rng.gen_range(0..allowed.len())];
                filled += 1;
            }
        }
    }
    Ok((assignment, filled))
}
```

要点：

- `division_path` 取 `cfg.files.splits`（需求 4.1），与当前 `ctx` 同源，保证后缀解析一致。
- 校验全部基于当前 `ctx`（需求 4.5）；运行时量（`n = ctx.num_groups`、各组 `allowed_keys`）均取自 `ctx`，不写死（需求 4.6）。
- 缺失组用 `thread_rng()` 在 `allowed_keys` 内取（需求 5.1/5.3/5.4），返回 `filled` 计数供日志（需求 5.2/8.3）。

### 退火初始化接入（annealing.rs）

`simulated_annealing_resumable` 增加 `seed: Option<&[u8]>` 参数。fresh 分支：

```rust
} else {
    assignment = match seed {
        Some(s) => s.to_vec(),            // 播种：用种子分配作为起点（独立副本，需求 7.4）
        None => multi_start_init(ctx, cfg, thread_id),
    };
    start_step = 0;
    evaluator = Evaluator::new_full_only(ctx, &assignment);
}
```

- `resume` 与 `seed` 互斥语义：resume 续算时 `seed` 恒为 `None`（resume 走另一分支，不受影响）。
- 起始步 0、`new_full_only`、简码延迟激活与基线 fresh 完全一致（需求 6.1/6.4）。
- 现有薄包装 `simulated_annealing` 与 resume 调用点传 `seed = None`，行为不变。

### 并行调用编排（main.rs run_optimize）

```rust
let seeds: Vec<Vec<u8>> = if seed_paths.is_empty() {
    Vec::new()
} else {
    let mut rng = thread_rng();
    let mut v = Vec::with_capacity(seed_paths.len());
    for (idx, p) in seed_paths.iter().enumerate() {
        let map = loader::load_keymap(p, &cfg.files.splits);
        let (asg, filled) = match loader::keymap_to_assignment(&ctx, &map, &mut rng) {
            Ok(x) => x,
            Err(e) => { eprintln!("❌ 种子 {} 无效: {}", p, e); std::process::exit(1); }
        };
        if filled > 0 {
            println!("⚠️ 种子 {} 有 {} 个组未被覆盖，已随机合法填充", p, filled);
        }
        v.push(asg);
    }
    println!("🌱 播种来源: {} 个种子；{} 线程按 round-robin(i % K) 映射", v.len(), num_threads);
    v
};

let sa_results: Vec<SaResult> = (0..num_threads).into_par_iter().map(|i| {
    let seed_ref = if seeds.is_empty() { None } else { Some(seeds[i % seeds.len()].as_slice()) };
    simulated_annealing_resumable(&ctx, cfg, i, &stop_flag, None, Some(&ckpt_dir), interval, seed_ref)
}).collect();
```

- 种子在并行前一次性解析为 `Vec<Vec<u8>>`，并行闭包内只读 `seeds[i % K]`（需求 7.1/7.2/7.3）。
- `seeds.is_empty()` ⇒ 不播种，逐线程 `multi_start_init`（需求 3.2）。
- 日志覆盖需求 8.1/8.2/8.3。

> resume 子命令（`run_resume`）不涉及播种，其 `simulated_annealing_resumable` 调用传 `seed = None`。

## Components and Interfaces

本特性涉及的组件与对外接口（签名）：

- **`main.rs::resolve_seed_paths(seed_dir: Option<&str>, seed_keymap: Option<&str>) -> Result<Vec<String>, String>`**：解析两个互斥 CLI 选项为种子 keymap 路径序列（见上文「种子来源解析」）。空 `Vec` 表示不播种。
- **`main.rs::run_optimize(cfg, cli_config_path, output_dir_opt, seed_dir, seed_keymap)`**：在现有流程上增加种子解析、`keymap_to_assignment` 调用、并行映射，其余不变。
- **`loader::keymap_to_assignment(ctx: &OptContext, root_to_key: &HashMap<String,u8>, rng: &mut impl Rng) -> Result<(Vec<u8>, usize), String>`**：keymap→assignment + 校验 + 缺失组随机填充，返回 `(assignment, filled_count)`（见上文「keymap → assignment 转换与校验」）。
- **`loader::load_keymap(keymap_path, division_path) -> HashMap<String,u8>`**：复用既有函数，`division_path = cfg.files.splits`。
- **`annealing::simulated_annealing_resumable(ctx, cfg, thread_id, stop_flag, resume, ckpt_dir, checkpoint_interval, seed: Option<&[u8]>) -> SaResult`**：新增末位 `seed` 参数；fresh 分支用 `seed` 覆盖 `multi_start_init`（见上文「退火初始化接入」）。其余调用点（薄包装 `simulated_annealing`、`run_resume`）传 `seed = None`。

各组件协作的数据流见 Overview 的数据流图。

## Data Models

本特性不引入新的持久化数据结构，仅在内存中传递两类既有/简单类型：

- **种子路径序列 `Vec<String>`**：`resolve_seed_paths` 的产物。元素为 keymap 文件绝对/相对路径；`--seed-dir` 时按 `thread-NN` 编号升序，`--seed-keymap` 时仅一个元素。
- **「子字根名 → 键位」映射 `HashMap<String, u8>`**：`load_keymap` 产物，键为子字根名（如 `口`、`口.1`），值为键位索引（0 起）。
- **种子分配 `Vec<u8>`（Seed_Assignment）**：长度 `ctx.num_groups`，`assignment[gi] = key`，`key ∈ ctx.groups[gi].allowed_keys`；与退火搜索的决策变量同型。多线程复用时各持独立副本。
- **填充计数 `usize`（filled_count）**：`keymap_to_assignment` 返回，记录随机填充的缺失组数，仅用于日志。

复用的既有类型：`OptContext`（`num_groups`、`root_to_group`、`groups[*].allowed_keys`）、`RootGroup`、`SaResult`。无新增序列化派生、无新增配置项、无磁盘格式变更。

## Error Handling

| 情形 | 处理 | 需求 |
|------|------|------|
| `--seed-dir` 与 `--seed-keymap` 同时给 | `eprintln!` + exit(1) | 3.1 |
| `--seed-dir` 不存在/无 `thread-NN/output-keymap.txt` | exit(1) | 1.4 |
| `--seed-keymap` 文件不存在/无有效行 | exit(1) | 2.3 |
| keymap 键位不在 allowed_keys | exit(1)，错误含字根名与键位 | 4.3 |
| 组内不一致 | exit(1)，错误含组信息 | 4.4 |
| 缺失组 | 告警 + 随机合法填充（不退出） | 5.1/5.2 |

`--seed-keymap` 的「无有效行」判定：`load_keymap` 返回的 map 折叠后，若 `chosen` 全为 `None`（即没有任何组被种子覆盖），视为该种子对当前方案无效 → `Err`（与「文件无法解析出有效编码行」对齐，需求 2.3）。

## Correctness Properties

以下性质用 `proptest` 表达（每条 ≥100 次迭代），辅以单元测试。测试顶部标注 `// Feature: seed-optimize-from-result, Property N: ...`。

### Property 1: 合法种子转换得到合法且一致的分配

对任意当前 `ctx` 与任意「为每个组从其 `allowed_keys` 选一个键」构造出的 keymap（即完全覆盖、键位合法），`keymap_to_assignment` 返回 `Ok((assignment, 0))`，且对每个组 `gi` 有 `assignment[gi] ∈ ctx.groups[gi].allowed_keys` 并等于构造时选定的键，`filled == 0`。
**Validates: Requirements 4.2, 4.3, 5.3**

### Property 2: 非法键位被拒绝

对任意当前 `ctx`，若某组在 keymap 中被赋予一个不在其 `allowed_keys` 内的键位，则 `keymap_to_assignment` 返回 `Err`。
**Validates: Requirements 4.3**

### Property 3: 缺失组随机填充后分配合法且计数正确

对任意当前 `ctx` 与任意「仅覆盖部分组」的合法 keymap，`keymap_to_assignment` 返回 `Ok((assignment, filled))`，其中 `filled` 等于未被覆盖的组数；且最终每个组 `gi` 均满足 `assignment[gi] ∈ ctx.groups[gi].allowed_keys`（全部合法）。
**Validates: Requirements 5.1, 5.2, 5.3**

### Property 4: 线程→种子轮询映射

对任意 `K ≥ 1` 与 `T ≥ 1`，映射函数对线程 `i` 给出 `i % K`；当 `T < K` 时被使用的种子下标集合恰为 `{0, 1, ..., T-1}`；当 `K == 1` 时所有线程映射到下标 0。
**Validates: Requirements 7.1, 7.2, 7.3**

### Property 5: 播种起点不退化（种子被忠实采用）

对任意合法 Seed_Assignment，以其作为种子调用退火（fresh 路径、`total_steps` 极小）后返回的 `best_score ≤ 以该种子直接构建 Evaluator 得到的初始得分`（退火最优不差于起点），且返回的 assignment 长度等于 `ctx.num_groups`。
**Validates: Requirements 6.1, 6.4**

## Testing Strategy

- **属性测试（proptest，≥100 次）**：Property 1–5，置于 `loader.rs`（Property 1/2/3）、`main.rs` 或独立测试模块（Property 4 映射）、`annealing.rs`（Property 5）。构造小规模 `ctx`（沿用现有测试里 `make_*_ctx` 风格的小型 groups/splits）。
- **单元测试**：
  - `resolve_seed_paths`：互斥报错、空（不播种）、`--seed-keymap` 单文件、`--seed-dir` 收集并按 NN 升序、空目录报错。
  - `keymap_to_assignment`：组内不一致报错；固定字根名被跳过不报错。
- **回归测试**：现有 `cargo test` 全绿；确认无播种一次性运行最终产物与基线一致（需求 9.2）——通过既有退火测试与 `simulated_annealing`（seed=None）路径覆盖。
- **手动烟雾验证**：用一份真实小配置先正常 `optimize` 产出 `output-X/`，再 `optimize --seed-dir output-X`（线程数 > thread 目录数以触发 round-robin）与 `optimize --seed-keymap output-X/thread-00/output-keymap.txt`，确认日志播种信息正确、能正常完成并产出结果。

## 设计取舍与说明

- **为何 seed 走 fresh 而非 resume**：用户要的是「换个更好的起点重新退火微调」，需要完整的温度调度与简码延迟激活，从第 0 步开始；resume 是「接着上次的步数/温度续算」，语义不同。故 seed 复用 fresh 分支，仅替换初始 assignment。
- **为何种子在并行前解析**：keymap 解析与校验涉及 I/O 与可能的 `exit(1)`，放在并行闭包外可统一错误处理、避免多线程重复解析，并让 `seeds: Vec<Vec<u8>>` 在闭包内只读共享。
- **为何只需 `output-keymap.txt`**：assignment 由「当前 ctx 的组」+「种子给出的每组键位」唯一确定；当前 ctx 来自当前 `input-*.txt`/`config.toml`。种子目录的 `inputs/` 是那次运行的旧输入，与本次「用最新输入」的意图相悖，故不读取。
- **种子与当前方案不匹配的鲁棒性**：键位非法 → 明确报错（硬约束不能破坏）；组缺失 → 告警 + 随机合法填充（软性，能跑通）。这与用户确认的策略一致。
- **跨平台**：仅做文件读取与目录枚举，无符号链接、无平台特定能力，Windows/Unix 一致。
