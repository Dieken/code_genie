# Implementation Plan: 稀疏桶内存优化

## Overview

本计划将设计文档拆解为增量编码步骤，严格遵循「最小改动约束」（需求 10）：仅修改与桶存储抽象引入、按 `code` 索引的密集数组替换、相关全量扫描去 code_space 化、相关快照/回滚适配直接相关的代码，不引入无关重构，不新增任何对外配置项。

实现语言为 Rust（与现有代码库一致）。改动集中在：新增 `BucketStore` 模块、`src/evaluator.rs`（主评估器与简码评估器）、`src/context.rs`（阈值常量与 `max_parts ≥ 7` 构建期检查）。

测试策略遵循设计：用 `proptest`（仓库已在测试中使用）实现 7 条正确性属性，每条 ≥100 次迭代，并在每个属性测试顶部标注：
`// Feature: sparse-bucket-memory-optimization, Property {number}: {property_text}`

每个实现任务完成后均运行 `cargo build` 与现有测试套件，保证逐步零回归。标记 `*` 的子任务为测试相关任务，可选。

## Tasks

- [x] 1. 引入 BucketStore 抽象与 FullBucket（新增模块）
  - [x] 1.1 实现 `FullBucket` 与 `BucketStore<B>`
    - 新增 `FullBucket { members: Vec<u32>, freq_sum: u64, max_freq: u64, first: u32 }`，手写 `Default` 使 `first = u32::MAX`、`max_freq = 0`、`freq_sum = 0`、`members = []`（空桶语义）
    - 实现 `Backend<B> { Dense(Vec<B>), Sparse(FxHashMap<u32,B>) }` 与 `BucketStore<B>`
    - 实现接口：`new(capacity)`（按 `SPARSE_THRESHOLD = 1<<21` 选后端）、`get`、`get_or_default`、`get_mut_or_insert`、`remove`、`iter_nonempty`、非空计数
    - 在 `src/context.rs` 定义并导出 `SPARSE_THRESHOLD` 常量（或置于 BucketStore 模块）
    - _Requirements: 1.1, 1.2, 1.3, 2.4, 5.1, 5.2, 5.3, 5.4, 5.5, 6.1, 6.2_

  - [x] 1.2* 编写 BucketStore 单元/属性测试
    - **Property 1: 空桶语义一致**、**Property 2: 稀疏有界性**、**Property 3: 双后端可观察等价**
    - 顶部标注 `// Feature: sparse-bucket-memory-optimization, Property N: ...`，proptest ≥100 次迭代
    - **Validates: Requirements 2.1, 2.2, 2.3, 2.4, 5.2, 5.5, 7.4**

- [x] 2. 主评估器全码桶迁移至 BucketStore（`src/evaluator.rs`）
  - [x] 2.1 替换 Evaluator 四数组为 `full_buckets: BucketStore<FullBucket>`
    - 删除 `code_to_chars`/`bucket_freq_sum`/`bucket_max_freq`/`bucket_first` 字段，统一由 `full_buckets` 承载
    - 改写 `new_impl` 填充逻辑（`get_mut_or_insert` + 聚合累加）
    - 改写 `update_char`：旧桶 swap_remove/聚合更新后若空则 `remove`；新桶 `get_mut_or_insert` + 聚合更新；`rescan_bucket_first`/`rescan_bucket_max` 改为对 `&[u32] members` 操作以避免借用冲突
    - 保持全码重码、当量、首选字选取规则与维护时机不变（需求 3.3/3.6）
    - _Requirements: 1.1, 1.2, 1.3, 1.4, 2.1, 3.1, 3.3, 3.6, 6.1_

  - [x] 2.2 全量扫描去 code_space 化（主评估器）
    - 将 `new_impl` 中碰撞统计与首选字初始化的 `for code in 0..cs` 改为 `full_buckets.iter_nonempty()`
    - 确认聚合结果与基线一致（仅改变遍历范围）
    - _Requirements: 7.1, 7.3, 7.4_

  - [x] 2.3 `max_parts ≥ 7`（code_space > u32::MAX）构建期检查
    - 在 `OptContext::new` 检测 `code_space > u32::MAX as usize` 并报错终止
    - _Requirements: 6.2, 6.3_

  - [x] 2.4* 编写全码指标等价属性测试
    - **Property 4: 全码指标等价**
    - **Validates: Requirements 3.1, 3.3**

- [x] 3. 简码评估器 bucket_collision_contrib 稀疏化（`src/evaluator.rs`）
  - [x] 3.1 改 `bucket_collision_contrib` 为 `FxHashMap<u32,(u32,u64)>`
    - 读取缺失键按 `(0,0)`；差量维护后为 `(0,0)` 则 `remove`，否则 `insert`
    - `recompute_collisions_full` 改为 `clear()` + 仅遍历非空全码桶
    - count 改用 `u32`
    - _Requirements: 4.1, 4.2, 7.2, 7.3_

- [x] 4. 简码每级 buckets 迁移 + bucket_gen 稀疏化（`src/evaluator.rs`）
  - [x] 4.1 `SimpleLevelTracker.buckets` 改为 `BucketStore<SimpleBucket>`
    - `SimpleBucket.members` 改 `Vec<u32>`；容量传 `simple_level_capacity[li]`，同阈值选后端
    - 桶变空时 `remove`；`simple_level_capacity` 仅保留为 `debug_assert` 上界校验
    - _Requirements: 4.3, 4.5, 6.1_

  - [x] 4.2 `bucket_gen` 替换为每级 `GenSet`（稀疏代际去重）
    - 实现 `GenSet { gen: FxHashMap<u32,u32>, cur: u32 }` 的 `bump`/`touch`
    - `touch_bucket` 与 `bump_generation` 改用 `GenSet`，保持 O(1) 去重与回滚语义
    - _Requirements: 4.4_

- [x] 5. 跨结构访问签名调整（`src/evaluator.rs`）
  - [x] 5.1 将简码侧入参 `full_code_to_chars: &[Vec<usize>]` 改为 `&BucketStore<FullBucket>`
    - 调整 `is_code_blocked`、`recompute_collisions_full`、`apply_move_incremental` 等处的成员/空判定（`get(code).map(...).unwrap_or(&[])`）
    - `is_code_blocked` 的 N=0 分支用「桶存在且非空」，Sparse 下等价 `contains_key`
    - _Requirements: 4.6, 9.1, 9.2, 9.3_

  - [x] 5.2* 编写占用保护语义属性测试
    - **Property 7: 占用保护语义保持**
    - **Validates: Requirements 9.1, 9.3**

- [x] 6. 快照/回滚适配与一致性验证
  - [x] 6.1 适配快照/回滚
    - `BucketSnap` 还原为空桶时经 `BucketStore::remove` 落实；从空恢复经 `get_mut_or_insert` 重建
    - `bucket_collision_contrib` 回滚还原 `(0,0)` 时 `remove`，否则 `insert`
    - 确认全码 `update_char` 反向回放天然触发空桶 remove / 非空 or_insert
    - _Requirements: 8.1, 8.2, 8.3, 8.4_

  - [x] 6.2* 编写一致性与回滚属性测试
    - **Property 5: 增量 == 全量重建（稀疏后端）**、**Property 6: 回滚精确还原**
    - 强制 Sparse 后端（小阈值）执行；并保持现有「增量 == 全量重建」「对账 == 全量重建」属性测试通过
    - **Validates: Requirements 3.2, 3.4, 4.6, 8.1, 8.2, 8.3, 8.4, 11.1**

- [x] 7. 集成回归验证
  - [x] 7.1 max_parts=4 等价回归
    - 固定种子、相同步数运行，断言最终上报指标与基线一致；确认 Dense 后端无性能回归
    - _Requirements: 5.2, 11.3, 11.4_

  - [x] 7.2* max_parts=5 可运行性冒烟
    - 在 `max_parts=5` 配置（小步数）下构建并运行至产出输出，验证不发生 code_space 量级 OOM
    - _Requirements: 11.5_

- [x] 8. 后端选择日志（`src/context.rs`，需求 12）
  - [x] 8.1 实现并接入后端选择日志
    - 实现纯函数 `log_backend_choice(label, capacity, elem_size, n_chars)` 与一行头日志，全部数字取自运行时（`ctx.code_base`/`ctx.max_parts`/`ctx.code_space`/`ctx.simple_level_capacity`/`ctx.char_infos.len()`/`size_of::<_>()`），除 `SPARSE_THRESHOLD` 外不写死任何规模常量
    - 在配置确认阶段（单线程、一次）为全码桶与各简码级别桶输出后端选择；`enable_simple_code == false` 时省略简码级别行
    - 确认 `BucketStore::new` 内部静默、不重复打印
    - _Requirements: 12.1, 12.2, 12.3, 12.4, 12.5, 12.6, 12.7_

  - [x] 8.2* 编写日志单元测试
    - 断言 `choose_backend` 在容量 ≤/＞ 阈值时分别返回 Dense/Sparse；断言日志函数对给定运行时输入产生预期后端标记与含 n_chars 的提示文案
    - _Requirements: 12.2, 12.4, 12.6_

## Task Dependency Graph

```json
{
  "waves": [
    { "id": 0, "tasks": ["1.1"] },
    { "id": 1, "tasks": ["1.2", "2.1"] },
    { "id": 2, "tasks": ["2.2", "2.3", "2.4", "3.1", "4.1"] },
    { "id": 3, "tasks": ["4.2", "5.1", "8.1"] },
    { "id": 4, "tasks": ["5.2", "6.1", "8.2"] },
    { "id": 5, "tasks": ["6.2", "7.1"] },
    { "id": 6, "tasks": ["7.2"] }
  ]
}
```

关键路径：1.1 → 2.1 → {3.1, 4.1→4.2} → 5.1 → 6.1 → 7.1。
任务 3 与 4 在 2.1 完成后可并行。带 `*` 的测试任务可在其依赖实现完成后任意时点插入。

## Notes

- **逐步零回归**：每个非测试任务完成后运行 `cargo build` 与现有测试套件；尤其在 2.1（主评估器迁移）后应确认现有简码一致性属性测试全部通过。
- **后端切换便于测试**：`BucketStore::new` 的阈值用常量，测试中可通过一个仅测试可见的构造入口（如 `new_with_threshold`）强制 Sparse 后端，以在小规模下覆盖稀疏路径（Property 5）。
- **借用冲突**：`update_char` 必须先完成旧桶操作（作用域结束释放 `get_mut` 借用）再处理新桶，Sparse（FxHashMap）不能同时持有两个可变桶引用；基线「先移除后插入」顺序天然满足。
- **不改对外契约**：本特性不新增配置项、不改输出文件、不改指标语义（需求 3、10.4）。任何观察到的指标差异都应视为缺陷并修复，而非接受。
- **内存预期**：max_parts=5 单线程从 ~GB 级降至 ~MB 级；max_parts=4 走 Dense，内存与性能与基线一致。

