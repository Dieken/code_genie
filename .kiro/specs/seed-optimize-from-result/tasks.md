# Implementation Plan: 从既有结果给 optimize 播种（seed）

## Overview

将设计拆解为增量编码步骤，严格遵循最小改动约束（需求 9）：仅新增/修改与播种相关的代码——`optimize` 的 CLI 选项、种子来源解析与「线程→种子」映射、keymap→assignment 转换与校验、退火初始化处接入 Seed_Assignment 的钩子。实现语言 Rust，复用既有 `loader::load_keymap`，不引入新依赖。

测试策略：用 `proptest`（仓库已用）实现 5 条正确性属性，每条 ≥100 次迭代，测试顶部标注：
`// Feature: seed-optimize-from-result, Property {number}: {property_text}`
辅以 `resolve_seed_paths`/`keymap_to_assignment` 单元测试与无播种回归验证。

每个实现任务完成后运行 `cargo build` 与现有测试，保证逐步零回归。标记 `*` 的子任务为测试任务。

## Tasks

- [x] 1. keymap→assignment 转换与校验（`src/loader.rs`）
  - [x] 1.1 实现 `keymap_to_assignment`
    - 新增 `pub fn keymap_to_assignment(ctx: &OptContext, root_to_key: &HashMap<String, u8>, rng: &mut impl Rng) -> Result<(Vec<u8>, usize), String>`
    - 用 `ctx.root_to_group` 折叠：仅处理属于组的字根名（固定字根名不在 `root_to_group` 中 → 跳过）；校验键位 ∈ `ctx.groups[gi].allowed_keys`（不合法 → `Err`，含字根名与键位）；组内不一致 → `Err`（含组信息）
    - 缺失组（`chosen[gi] == None`）从 `allowed_keys` 用 `rng` 随机合法填充，统计 `filled`；返回 `(assignment, filled)`
    - 所有运行时量（`ctx.num_groups`、各组 `allowed_keys`）取自 `ctx`，不写死
    - _Requirements: 4.2, 4.3, 4.4, 4.5, 4.6, 5.1, 5.3, 5.4_

  - [x] 1.2* 编写转换/校验属性与单元测试
    - **Property 1: 合法种子转换得到合法且一致的分配**、**Property 2: 非法键位被拒绝**、**Property 3: 缺失组随机填充后分配合法且计数正确**（proptest，构造小规模 ctx）
    - 单元测试：组内不一致报错；固定字根名被跳过不报错
    - **Validates: Requirements 4.2, 4.3, 4.4, 5.1, 5.2, 5.3**

- [x] 2. 退火初始化接入种子钩子（`src/annealing.rs`）
  - [x] 2.1 给 `simulated_annealing_resumable` 增加 `seed` 参数
    - 函数签名末位增加 `seed: Option<&[u8]>`
    - fresh 分支：`assignment = match seed { Some(s) => s.to_vec(), None => multi_start_init(ctx, cfg, thread_id) }`；起始步 0、`Evaluator::new_full_only`、简码延迟激活保持不变
    - resume 分支不受影响（与 seed 互斥；resume 时 seed 恒为 None）
    - 更新薄包装 `simulated_annealing` 调用点传 `seed = None`（行为不变）
    - _Requirements: 6.1, 6.2, 6.3, 6.4, 6.5, 7.4_

  - [x] 2.2* 编写播种起点属性测试
    - **Property 5: 播种起点不退化**（构造合法 Seed_Assignment，极小 total_steps，断言返回 best_score ≤ 该种子初始得分，assignment 长度 == num_groups）
    - **Validates: Requirements 6.1, 6.4**

- [x] 3. CLI 选项与种子来源解析（`src/main.rs`）
  - [x] 3.1 扩展 `Optimize` 子命令与 `resolve_seed_paths`
    - `Commands::Optimize` 新增 `seed_dir: Option<String>`（`--seed-dir`）与 `seed_keymap: Option<String>`（`--seed-keymap`）
    - `main` 中 `Optimize`/`None` 分支调用 `run_optimize`，透传两个新参数（`None` 分支两者均为 `None`）
    - 新增 `fn resolve_seed_paths(seed_dir: Option<&str>, seed_keymap: Option<&str>) -> Result<Vec<String>, String>`：互斥校验（同时给 → Err）；均 None → 空 Vec；`--seed-keymap` 校验存在 → `vec![path]`；`--seed-dir` 枚举 `thread-{NN}/output-keymap.txt` 按 NN 升序收集，空则 Err，且不触碰 `inputs/`、`checkpoint/`
    - _Requirements: 1.1, 1.2, 1.3, 1.4, 1.5, 2.1, 2.2, 2.3, 3.1, 3.2_

  - [x] 3.2* 编写 `resolve_seed_paths` 单元与映射属性测试
    - 单元测试：互斥报错；均 None 返回空；`--seed-keymap` 单文件；`--seed-dir` 收集并按 NN 升序；空目录报错
    - **Property 4: 线程→种子轮询映射**（proptest：线程 i → i % K；T<K 用 {0..T-1}；K==1 全映射 0）
    - **Validates: Requirements 1.3, 1.4, 2.3, 3.1, 3.2, 7.1, 7.2, 7.3**

- [x] 4. run_optimize 编排与并行映射（`src/main.rs`）
  - [x] 4.1 接入种子解析、转换与并行播种
    - `run_optimize` 签名扩展为 `(cfg, cli_config_path, output_dir_opt, seed_dir, seed_keymap)`
    - 入口调用 `resolve_seed_paths`，`Err` → `eprintln!` + `exit(1)`
    - 校准/加载 ctx 后（即 ctx 就绪后）：对每个 seed 路径 `load_keymap(p, &cfg.files.splits)` → `keymap_to_assignment(&ctx, &map, &mut rng)`，`Err` → `eprintln!` + `exit(1)`；`filled > 0` 打印告警（含组数）；收集 `seeds: Vec<Vec<u8>>`
    - 播种启用时打印播种来源、种子个数 K、线程数 T 与 round-robin 映射说明
    - 并行调用：线程 `i` 用 `seed_ref = (!seeds.is_empty()).then(|| seeds[i % seeds.len()].as_slice())`，传入 `simulated_annealing_resumable(..., seed_ref)`
    - `run_resume` 的 `simulated_annealing_resumable` 调用点传 `seed = None`
    - _Requirements: 1.5, 3.2, 4.1, 5.2, 6.2, 7.1, 7.2, 7.3, 7.4, 8.1, 8.2, 8.3_

- [x] 5. 回归与验证
  - [x] 5.1 构建与全部测试通过
    - `cargo build` 无新错误/警告（必要时 `#[allow(dead_code)]`）；`cargo test` 全绿
    - 确认无播种（未给任一 seed 选项）一次性运行行为与基线一致（需求 9.2）：由 seed=None 路径与既有退火测试覆盖
    - _Requirements: 9.1, 9.2, 9.3, 9.4_

  - [x] 5.2 手动烟雾验证（小配置）
    - 用真实小配置先正常 `optimize -d output-seed-smoke` 产出 `thread-NN/output-keymap.txt`
    - 再 `optimize --seed-dir output-seed-smoke`（令线程数 > thread 目录数以触发 round-robin）与 `optimize --seed-keymap output-seed-smoke/thread-00/output-keymap.txt`，确认播种日志正确、能正常完成并产出结果
    - 验证后清理烟雾测试产生的输出目录
    - _Requirements: 1.2, 7.1, 8.1, 8.2_

## Task Dependency Graph

```json
{
  "waves": [
    { "id": 0, "tasks": ["1.1"] },
    { "id": 1, "tasks": ["1.2", "2.1", "3.1"] },
    { "id": 2, "tasks": ["2.2", "3.2", "4.1"] },
    { "id": 3, "tasks": ["5.1"] },
    { "id": 4, "tasks": ["5.2"] }
  ]
}
```

关键路径：1.1 → 2.1 → 4.1 → 5.1 → 5.2。
转换函数（1.1）是基础；退火钩子（2.1）、CLI 解析（3.1）在其后可并行；编排（4.1）汇合三者。

## Notes

- **最小改动**：仅改 `loader.rs`（新增 1 个函数）、`annealing.rs`（加 1 个参数 + fresh 分支分支判断）、`main.rs`（CLI 字段 + 解析函数 + run_optimize 编排）。不动校准、主循环、refine、checkpoint、resume 续算逻辑。
- **种子走 fresh 路径**：seed 仅替换初始 assignment，起始步 0、`new_full_only`、简码延迟激活与基线 fresh 一致；不是 resume 续算。
- **种子在并行前解析**：keymap I/O 与校验（可能 `exit(1)`）统一在并行闭包外；`seeds: Vec<Vec<u8>>` 闭包内只读，线程复用同种子时各自 `to_vec()` 独立副本。
- **只用 output-keymap.txt**：输入文件与配置一律用当前 `optimize` 运行的最新版本（当前 `cfg`/`ctx`）；不读种子目录的 `inputs/`、`checkpoint/`。
- **运行时量不写死**：`num_groups`、各组 `allowed_keys` 等均取自 `ctx`。
- **硬/软约束**：键位非法或组内不一致 → 报错退出（硬约束）；缺失组 → 告警 + 随机合法填充（软约束，能跑通）。
- **无播种不变性**：未给任一 seed 选项时，seed=None，逐线程 `multi_start_init`，最终产物与基线一致。
- **无新依赖**：复用 `loader::load_keymap`、`rand`、`rayon`、`clap`。
