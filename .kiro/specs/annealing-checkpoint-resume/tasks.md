# Implementation Plan: 退火进度 checkpoint 与断点续算

## Overview

将设计拆解为增量编码步骤，严格遵循最小改动约束（需求 12）：仅新增/修改与 checkpoint 保存恢复、输入备份、Ctrl-C 处理、resume 子命令、相关配置项直接相关的代码。实现语言 Rust。

测试策略：用 `proptest`（仓库已用）实现 5 条正确性属性，每条 ≥100 次迭代，测试顶部标注：
`// Feature: annealing-checkpoint-resume, Property {number}: {property_text}`
辅以 checkpoint 往返/原子写单元测试、输入备份单元测试、resume 初始化单元测试、配置文件烟雾测试、小规模端到端「中断→续算」测试。

每个实现任务完成后运行 `cargo build` 与现有测试，保证逐步零回归。标记 `*` 的子任务为测试任务。

## Tasks

- [x] 1. 依赖与可序列化类型（`Cargo.toml`、`src/types.rs`）
  - [x] 1.1 添加 ctrlc 依赖与 serde 派生
    - `Cargo.toml` 新增 `ctrlc = "3.4"`
    - 为 `Metrics`、`SimpleMetrics`、`ScaleConfig` 添加 `serde::{Serialize, Deserialize}` 派生（仅加 trait，不改字段/语义）
    - _Requirements: 12.2, 12.3_

- [x] 2. checkpoint 模块（新增 `src/checkpoint.rs`）
  - [x] 2.1 实现数据结构与原子保存/加载/归档
    - 定义 `CHECKPOINT_VERSION`、`ThreadCheckpoint`（含 `timestamp` 字段）、`CheckpointMeta`（字段见设计）
    - 路径助手 `checkpoint_dir`/`thread_path`(`thread-{:02}.json`)/`thread_archive_path`(`thread-{:02}-{ts}.json`)/`meta_path`；`now_timestamp_ms`（毫秒级时间戳）
    - `save_atomic`（`.tmp` + rename）、`save_thread_checkpoint`（先按 `prev_ts` 归档现存规范文件再原子写新版，不删旧、不用 symlink）、`save_meta`、`load_thread_checkpoint`、`load_meta`（含版本校验）
    - 在 `main.rs` 注册 `mod checkpoint;`
    - _Requirements: 3.1, 3.2, 3.3, 5.1, 5.2, 5.3, 5.4, 6.1, 6.2, 6.4, 13.1, 13.2, 13.3, 13.4, 13.6_

  - [x] 2.2* 编写 checkpoint 往返/原子写/版本校验/归档保留测试
    - **Property 2: ThreadCheckpoint 序列化往返恒等**、**Property 3: CheckpointMeta 往返恒等且版本校验**、**Property 4: 原子保存后可加载且与源相等（无 .tmp 残留）**、**Property 6: 归档保留旧版本且规范文件恒为最新（无 symlink）**
    - **Validates: Requirements 3.1, 3.2, 5.1, 5.2, 5.3, 5.4, 6.2, 6.4, 11.4, 13.1, 13.2, 13.3, 13.5**

- [x] 3. 配置项 checkpoint_interval_ratio（`src/config.rs` + 两个 toml）
  - [x] 3.1 新增配置项、默认值与换算函数
    - `AnnealingConfig` 新增 `checkpoint_interval_ratio`（`#[serde(default)]`，默认 0.05）；`Config::default()` 后备补齐
    - 提供 `checkpoint_interval(total_steps, ratio) = max(1, floor(total_steps × ratio))`（可置于 `config.rs` 或 `annealing.rs`，与 `reconcile_interval` 同处风格）
    - _Requirements: 9.1, 9.2, 9.3_

  - [x] 3.2 同步两个 toml 并加注释
    - `config.toml.example` 与 `moling/config.toml` 的 `[annealing]` 段新增 `checkpoint_interval_ratio = 0.05`，注释说明「单位=占 total_steps 的比例，默认 0.05≈每 5% 写一次」；example 取值等于代码默认
    - _Requirements: 9.4_

  - [x] 3.3* 编写间隔换算与配置烟雾测试
    - **Property 1: 检查点间隔换算**（proptest）
    - 配置文件烟雾测试：两个 toml 含该项且带注释，example 值=默认
    - **Validates: Requirements 9.2, 9.3, 9.4**

- [x] 4. 可续算 SA 主循环（`src/annealing.rs`）
  - [x] 4.1 引入 SaResult 与 simulated_annealing_resumable
    - 定义 `SaResult { assignment, score, metrics, simple_metrics, interrupted }`
    - 新增 `simulated_annealing_resumable(ctx, cfg, thread_id, stop_flag, resume, ckpt_dir, checkpoint_interval) -> SaResult`，把现有 `simulated_annealing` 主体迁入；保留 `simulated_annealing` 为薄包装（stop_flag=false、resume=None、ckpt_dir=None）行为不变
    - resume 初始化分支：从 `ThreadCheckpoint` 恢复 assignment/best_*/step/控制状态/simple_activated；`Evaluator::new(&assignment)` 重建并按闩锁设 `simple_active`/`current_simple_weight`
    - _Requirements: 5.5, 7.7, 10.1, 11.1, 11.2, 11.3, 12.1_

  - [x] 4.2 主循环插入周期写与停止检查
    - 线程持有局部 `prev_ts: Option<String>`（fresh=None；resume=Some(resumed_tc.timestamp)）
    - 每 `checkpoint_interval` 步（`step > start_step` 且 `ckpt_dir` 非空）经 `save_thread_checkpoint(..., prev_ts)` 写 `thread-NN.json`（先归档旧版、原子写新版），成功后更新 `prev_ts`；写失败仅告警不中断
    - 每 `STOP_CHECK_STRIDE` 步检查 `stop_flag`，置位则写最终 checkpoint 并 `return SaResult{ interrupted: true, ... }`
    - 周期写/停止检查仅在边界发生，不进入单步热路径
    - _Requirements: 1.1, 1.2, 1.3, 1.4, 1.5, 8.2, 10.2, 10.3, 13.1, 13.7_

  - [x] 4.3* 编写 resume 初始化与续算单元测试
    - **Property 5: resume 续算不重置进度且最优不退化**（构造 ThreadCheckpoint 断言起始步=current_step、best 恢复一致；小规模续算断言 best 不退化）
    - **Validates: Requirements 5.5, 7.3, 7.7, 11.1, 11.3**

- [x] 5. 输入备份与 meta 写出（`src/main.rs`）
  - [x] 5.1 实现 backup_inputs 与 write meta
    - `backup_inputs(cfg, cli_config_path, output_dir)`：创建 `inputs/`，复制 config + 5 个输入文件（按 basename），失败告警不中止
    - 校准 `scale_config` 后写 `checkpoint/meta.json`（含 version/scale_config/total_steps/num_threads/温度参数/时间戳）
    - _Requirements: 4.1, 4.2, 4.3, 4.4, 4.5, 6.1, 6.2_

  - [x] 5.2* 编写输入备份单元测试
    - 临时 cfg/输入 → `backup_inputs` → 断言 `inputs/` 下 6 文件存在且内容与源一致
    - _Requirements: 4.1, 4.2, 4.3_

- [x] 6. Ctrl-C 与 optimize 集成（`src/main.rs`）
  - [x] 6.1 安装 ctrlc 并接入并行退火
    - 创建 `stop_flag: Arc<AtomicBool>`，`ctrlc::set_handler` 置位；打印 Ctrl-C/resume 提示
    - 并行调用改为 `simulated_annealing_resumable(..., &stop_flag, None, Some(&ckpt_dir), interval)`，收集 `SaResult`
    - `interrupted = stop_flag.load() || results.any(|r| r.interrupted)`；中断时打印 resume 提示并以当前 best 收尾（与正常收尾共用产出逻辑）
    - 抽出共享 `run_sa_phase(...)` 供 optimize/resume 复用（可选但推荐，减少重复）
    - _Requirements: 8.1, 8.2, 8.3, 8.4, 8.5, 10.1_

- [x] 7. resume 子命令（`src/main.rs`）
  - [x] 7.1 实现 resume -d <output-dir>
    - 新增 `Commands::Resume { dir }`
    - `run_resume`：`load_meta`（只读 + 版本校验）→ 用 `inputs/config.toml` + `inputs/` 输入 + `meta.scale_config` 重建 ctx（跳过 calibrate，对 inputs/ 与 meta.json 只读、不修改）→ 校验线程数 → `load_thread_checkpoint` ×N
    - 复用传入目录为 output_dir（不新建），日志「输出目录」显示该目录；不调用 `backup_inputs`、不重写 meta.json
    - 并行 `simulated_annealing_resumable(resume=Some(&tc[i]), Some(&ckpt_dir), interval)` 续算（`prev_ts` 初始化为该线程 `tc.timestamp`，续算首写先归档遗留规范文件）→收尾产出到该目录
    - 错误处理：缺文件/版本不符/线程数不符 → 明确报错退出
    - _Requirements: 7.1, 7.2, 7.3, 7.4, 7.5, 7.6, 7.7, 7.8, 7.9, 11.4, 13.7_

- [x] 8. 回归与端到端验证
  - [x] 8.1* 小规模端到端中断→续算测试
    - 跑少量步后置位 stop_flag → 断言写出 thread-NN.json；resume 同目录 → 断言从 current_step 续算、最终达 total_steps、best 不退化
    - _Requirements: 1.1, 7.3, 7.6, 8.2_

  - [x] 8.2 构建与全部测试通过
    - `cargo build` 无新错误；`cargo test` 全绿；确认正常一次性运行最终产物不变（需求 12.4）；真实小配置跑一次确认 `inputs/` 与 `checkpoint/` 正常生成
    - _Requirements: 10.2, 12.4_

## Task Dependency Graph

```json
{
  "waves": [
    { "id": 0, "tasks": ["1.1"] },
    { "id": 1, "tasks": ["2.1", "3.1"] },
    { "id": 2, "tasks": ["2.2", "3.2", "3.3", "4.1"] },
    { "id": 3, "tasks": ["4.2", "5.1"] },
    { "id": 4, "tasks": ["4.3", "5.2", "6.1"] },
    { "id": 5, "tasks": ["7.1"] },
    { "id": 6, "tasks": ["8.1", "8.2"] }
  ]
}
```

关键路径：1.1 → 2.1 → 4.1 → 4.2 → 6.1 → 7.1 → 8.2。
任务 3（配置）与 2（checkpoint 模块）在 1.1 后可并行；输入备份/meta（5.1）与 SA 改造（4.x）相对独立。

## Notes

- **每线程一个文件**：各线程写 `checkpoint/thread-{:02}.json`（规范=最新），无共享锁；meta.json 在退火前单线程写一次。
- **旧版本保留、无 symlink**：写新版前先把旧规范文件按其 `timestamp` 归档为 `thread-{:02}-{TS}.json`（永不删除），规范文件始终是真实文件（非符号链接），Windows/Unix 一致；手动回滚 = 把某归档复制回规范名。
- **resume 只读 inputs/ 与 meta.json**：resume 不调用 `backup_inputs`、不重写 meta.json，仅读取重建 ctx 并续写 thread-NN.json。
- **不序列化 Evaluator**：checkpoint 仅存控制状态 + assignment，resume 时 `Evaluator::new(&assignment)` 重建；checkpoint 文件数 KB。
- **RNG 不持久化**：沿用 `thread_rng()`；resume 后随机轨迹不要求逐步一致，但 best 与控制状态忠实恢复。
- **scale_config 复用**：resume 从 meta.json 取，跳过 calibrate，避免评分口径漂移。
- **resume 复用 output 目录**：不新建时间戳目录，日志「输出目录」显示传入目录。
- **不改对外契约**：正常一次性运行的最终产物不变；checkpoint/inputs 为新增旁路产物。
- **原子写**：所有 checkpoint 文件 `.tmp` + rename；写失败仅告警不影响退火。
