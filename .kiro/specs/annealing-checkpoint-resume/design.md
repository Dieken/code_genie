# 技术设计文档：退火进度 checkpoint 与断点续算（annealing-checkpoint-resume）

## Overview

本特性为 code_genie 的模拟退火优化新增**断点续算**能力，参考 commit `9be44b1` 的思路，并针对当前更复杂的退火主循环（简码延迟激活、按分量存储最优解）与三点用户要求做调整：

1. **每线程一个 checkpoint 文件**（`{Output_Dir}/checkpoint/thread-{NN}.json`），各线程在自己的节奏独立写，无跨线程锁/协调——实现最简、对退火干扰最小。
2. **周期性写**：每 `Checkpoint_Interval` 步写一次（默认跟随汇报频率），而非仅 Ctrl-C 时写。
3. **写入 output 目录**而非当前目录，避免多实例互相覆盖。

外加：退火前把依赖输入文件备份到 `{Output_Dir}/inputs/`；Ctrl-C 优雅中断写出最新 checkpoint；`resume -d {Output_Dir}` 复用既有目录续算。

核心简化（沿用参考实现）：checkpoint 只存退火**控制状态 + `assignment`**，**不序列化 `Evaluator`**（含本仓库刚优化的稀疏桶等大结构）；resume 时由 `Evaluator::new(&assignment)` 重建。checkpoint 文件因此很小（≈ `num_groups` 字节 + 少量标量），写入开销可忽略。

改动范围（需求 12）：
- 新增 `src/checkpoint.rs`（数据结构 + 原子保存/加载）。
- `Cargo.toml`：新增 `ctrlc = "3.4"`。
- `src/types.rs`：为 `Metrics`/`SimpleMetrics`/`ScaleConfig` 加 `Serialize`/`Deserialize`。
- `src/config.rs`：新增 `[annealing].checkpoint_interval_ratio`（默认 0.05）+ 访问器；同步两个 toml。
- `src/annealing.rs`：`simulated_annealing` → `simulated_annealing_resumable`（周期写 + 停止检查 + resume 初始化），保留薄包装。
- `src/main.rs`：输入备份、写 meta、安装 ctrlc + stop_flag、传递 checkpoint 目录/间隔、新增 `resume` 子命令、resume 复用目录。

不改变正常（无中断、不 resume）一次性运行的最终产物（需求 12.4）。

---

## Architecture

### 模块依赖

```
config.rs   ← 新增 checkpoint_interval_ratio
types.rs    ← Metrics/SimpleMetrics/ScaleConfig 加 serde 派生
checkpoint.rs (新) ← ThreadCheckpoint / CheckpointMeta / 原子 save·load
   ↑ 依赖 types
annealing.rs ← simulated_annealing_resumable（周期写 thread-NN.json、停止检查、resume 初始化）
main.rs      ← 备份输入、写 meta、ctrlc + stop_flag、optimize/resume 编排
```

### 输出目录布局

```
output-{YYYYMMDD-HHMMSS}/
  inputs/                     # 退火前备份（需求 4）
    config.toml
    input-fixed.txt
    input-roots.txt
    input-division.txt
    pair_equivalence.txt
    key_distribution.txt
  checkpoint/                 # 断点续算（需求 1/2/13）
    meta.json                 # 校准后写一次（需求 6），运行期/ resume 期只读
    thread-00.json            # 规范文件：各线程最新（原子写，需求 1/3/8）
    thread-00-{TS1}.json      # 归档：历史版本，永不删除（需求 13）
    thread-00-{TS2}.json
    thread-01.json
    ...
  output-keymap.txt 等         # 完成时既有产物（不变）
  thread-XX/                  # 既有每线程结果子目录（不变）
  summary.txt
```

注意：既有的每线程结果子目录为 `thread-XX/`（目录），checkpoint 为 `checkpoint/thread-XX.json`（文件），二者路径不冲突。

### 数据流

```
optimize:
  load config(-c) → 创建 output_dir → 备份 inputs/ → 校准 scale_config
    → 写 checkpoint/meta.json → 安装 ctrlc(stop_flag)
    → 并行 simulated_annealing_resumable(thread_id, stop_flag, resume=None, ckpt_dir, interval)
        每 interval 步：写 checkpoint/thread-NN.json（原子）
        每 K 步：若 stop_flag → 写最终 checkpoint 后 break
    → join → 若被中断：打印 resume 提示；否则正常收尾产出

resume -d output_dir:
  读 output_dir/inputs/config.toml + 输入 → 读 checkpoint/meta.json(scale_config)
    → 重建 ctx（复用 scale_config，不重校准）→ 读 checkpoint/thread-NN.json
    → 安装 ctrlc(stop_flag)
    → 并行 simulated_annealing_resumable(thread_id, stop_flag, resume=Some(tc), ckpt_dir, interval)
        从 tc.current_step 续算到 total_steps
    → join → 收尾产出到同一 output_dir
```

---

## Components and Interfaces

#### 决策逻辑：归档而非覆盖（不使用符号链接）

为支持「旧 checkpoint 不删除、可手动回滚」（需求 13）且跨平台可靠，**不使用符号链接**：Windows 创建 symlink 需管理员/开发者模式，`std::os::windows::fs::symlink_file` 在普通用户下会失败。改用「规范真实文件 + 时间戳归档」：

- `thread-{NN}.json` 恒为最新（真实文件，resume 稳定入口）。
- 每次写新版本前，把现有 `thread-{NN}.json` 按其自身 `timestamp` 重命名为 `thread-{NN}-{TIMESTAMP}.json`（归档，永不删除），再原子写新的 `thread-{NN}.json`。
- 仅用 `rename` + 原子写，Windows/Unix 一致；手动回滚 = 把某归档复制/重命名回 `thread-{NN}.json`。

### 1. `src/checkpoint.rs`（新增）

```rust
use serde::{Deserialize, Serialize};
use std::path::Path;
use crate::types::{Metrics, ScaleConfig, SimpleMetrics};

pub const CHECKPOINT_VERSION: u32 = 1;

/// 单个退火线程的可续算控制状态（需求 5）。不含 Evaluator——resume 时由 assignment 重建。
#[derive(Clone, Serialize, Deserialize)]
pub struct ThreadCheckpoint {
    pub thread_id: usize,
    /// 该检查点生成时间戳（毫秒级，如 "20260618-093015.123"），用于归档命名（需求 13.1/13.4）。
    pub timestamp: String,
    pub assignment: Vec<u8>,
    pub current_step: usize,
    // 按分量存储的最优解（需求 5.2）
    pub best_assignment: Vec<u8>,
    pub best_full_score: f64,
    pub best_simple_score: f64,
    pub best_score: f64,
    pub best_metrics: Metrics,
    pub best_simple_metrics: SimpleMetrics,
    // 退火控制状态（需求 5.3）
    pub temp_multiplier: f64,
    pub steps_since_improve: usize,
    pub last_best_score: f64,
    // 简码激活闩锁（需求 5.4）
    pub simple_activated: bool,
}

/// 本次运行全局元信息（需求 6），运行期不变。
#[derive(Clone, Serialize, Deserialize)]
pub struct CheckpointMeta {
    pub version: u32,
    pub timestamp: String,
    pub scale_config: ScaleConfig,
    pub total_steps: usize,
    pub num_threads: usize,
    // 实际温度参数（当前实现直接用配置值；预留以便将来自动校准温度时复用）
    pub temp_start: f64,
    pub temp_end: f64,
    pub comfort_temp: f64,
}

/// checkpoint 子目录路径与文件名（需求 2/13）。
pub fn checkpoint_dir(output_dir: &str) -> std::path::PathBuf;          // {output_dir}/checkpoint
pub fn thread_path(ckpt_dir: &Path, thread_id: usize) -> std::path::PathBuf;        // thread-{:02}.json（规范）
pub fn thread_archive_path(ckpt_dir: &Path, thread_id: usize, ts: &str) -> std::path::PathBuf; // thread-{:02}-{ts}.json（归档）
pub fn meta_path(ckpt_dir: &Path) -> std::path::PathBuf;               // meta.json

/// 原子写：先写 *.tmp 再 rename（需求 3）。泛型 over Serialize。
fn save_atomic<T: Serialize>(value: &T, path: &Path) -> std::io::Result<()>;

/// 写线程检查点：先归档现有规范文件（按 prev_ts），再原子写新规范文件（需求 13.1）。
/// `prev_ts`：上一次写出的（或 resume 恢复的）检查点时间戳；首次写且无现存规范文件时为 None。
/// 写成功后调用方应把 `prev_ts` 更新为本次 `tc.timestamp`。
pub fn save_thread_checkpoint(tc: &ThreadCheckpoint, ckpt_dir: &Path, prev_ts: Option<&str>)
    -> std::io::Result<()>;
pub fn save_meta(meta: &CheckpointMeta, ckpt_dir: &Path) -> std::io::Result<()>;

pub fn load_thread_checkpoint(path: &Path) -> Result<ThreadCheckpoint, String>;
pub fn load_meta(ckpt_dir: &Path) -> Result<CheckpointMeta, String>;    // 校验 version（需求 6.4/11.4）

/// 生成毫秒级时间戳字符串（如 "20260618-093015.123"），供 ThreadCheckpoint.timestamp 使用。
pub fn now_timestamp_ms() -> String;
```

`save_thread_checkpoint` 流程（需求 13.1/13.6）：
```
let canonical = thread_path(ckpt_dir, tc.thread_id);
if let Some(ts) = prev_ts {
    if canonical.exists() {
        fs::rename(&canonical, thread_archive_path(ckpt_dir, tc.thread_id, ts))?; // 归档旧版
    }
}
save_atomic(tc, &canonical)?;   // .tmp + rename 写新版
```
`save_atomic`：`serde_json::to_string_pretty` → 写 `path.with_extension("json.tmp")` → `std::fs::rename` 到 `path`。归档与新写均仅用 `rename`/写文件，无符号链接（需求 13.3/13.6）。各线程文件名不同，互不干扰（需求 3.3）。

### 2. `src/types.rs`：serde 派生

为被持久化的类型加派生（仅加 trait，不改字段/语义，需求 12.3）：

```rust
#[derive(Clone, Copy, Default, Serialize, Deserialize)] pub struct Metrics { ... }
#[derive(Clone, Copy, Default, Serialize, Deserialize)] pub struct SimpleMetrics { ... }
#[derive(Clone, Copy, Serialize, Deserialize)]          pub struct ScaleConfig { ... }
```

### 3. `src/config.rs`：新增配置项

`AnnealingConfig` 新增（与既有 `*_ratio` 同风格）：

```rust
#[serde(default = "default_checkpoint_interval_ratio")]
pub checkpoint_interval_ratio: f64,   // 单位：占 total_steps 的比例
fn default_checkpoint_interval_ratio() -> f64 { 0.05 }
```

换算（与 `reconcile_interval` 同式）：`checkpoint_interval(total_steps, ratio) = max(1, floor(total_steps × ratio))`（需求 9.2）。`Config::default()` 后备同步补上该字段。

### 4. `src/annealing.rs`：可续算 SA

保留对外签名不变的薄包装，新增可续算实现：

```rust
pub struct SaResult {
    pub assignment: Vec<u8>,
    pub score: f64,
    pub metrics: Metrics,
    pub simple_metrics: SimpleMetrics,
    pub interrupted: bool,   // 被 stop_flag 提前中断
}

/// 向后兼容入口（测试/无 checkpoint 场景）：内部以 stop_flag=false、resume=None、不写盘调用。
pub fn simulated_annealing(ctx, cfg, thread_id) -> (Vec<u8>, f64, Metrics, SimpleMetrics);

/// 可续算主循环。
pub fn simulated_annealing_resumable(
    ctx: &OptContext,
    cfg: &Config,
    thread_id: usize,
    stop_flag: &Arc<AtomicBool>,
    resume: Option<&ThreadCheckpoint>,
    ckpt_dir: Option<&Path>,   // None = 不写 checkpoint（兼容入口）
    checkpoint_interval: usize,
) -> SaResult;
```

**初始化分支**（需求 11.1/11.2）：
- `resume = None`：现有流程（`multi_start_init` + `Evaluator::new_full_only` + 简码延迟激活初值），`start_step = 0`，`simple_activated = false`。
- `resume = Some(tc)`：
  - `assignment = tc.assignment`；`best_* = tc.best_*`；`start_step = tc.current_step`；`temp_multiplier/steps_since_improve/last_best_score = tc.*`；`simple_activated = tc.simple_activated`。
  - 重建评估器：`let mut evaluator = Evaluator::new(ctx, &assignment)`（含简码急切构建）。
  - 按闩锁设置激活态：若 `simple_activated`，置 `evaluator.simple_active = true` 并据 `w_simple_eff(p_at(start_step))` 设 `current_simple_weight`；否则 `simple_active=false`、`current_simple_weight=0.0`。`score_dirty/full_score_dirty=true`。
  - （T0 打印「从检查点恢复 | 步数 start/total | 最优 …」）

**主循环 `for step in start_step..steps`** 在现有逻辑上插入两处旁路：

```rust
// (a) 停止检查（每 STOP_CHECK_STRIDE 步，建议 10000）：
if step % STOP_CHECK_STRIDE == 0 && stop_flag.load(Relaxed) {
    write_thread_ckpt(step);      // 写最新 checkpoint（原子）
    return SaResult { interrupted: true, ... best_* };
}
// (b) 周期写（每 checkpoint_interval 步）：
if ckpt_dir.is_some() && step % checkpoint_interval == 0 && step > start_step {
    write_thread_ckpt(step);
}
```

`write_thread_ckpt(step)`：用当前 `assignment/best_*/temp_multiplier/steps_since_improve/last_best_score/simple_activated` 加上新生成的 `timestamp = now_timestamp_ms()` 组装 `ThreadCheckpoint`，调 `save_thread_checkpoint(&tc, ckpt_dir, prev_ts.as_deref())`，成功后 `prev_ts = Some(tc.timestamp)`。线程持有局部 `prev_ts: Option<String>`：fresh 运行初始为 `None`；resume 时初始化为 `Some(resumed_tc.timestamp)`，使续算首次写出前先归档上一轮遗留的规范文件（需求 13.7）。写失败仅 `eprintln!` 告警，不中断退火（需求 10.2）。

**收尾**：循环正常结束（`step==steps`）后照旧做最终精炼并返回 `interrupted=false`。`current_step >= steps` 的 resume 线程：`start_step..steps` 为空区间，自然跳过主循环直接收尾（需求 7.7）。

> 注：随机数仍用 `thread_rng()`，不序列化（需求 5.6）；resume 后随机轨迹不要求逐步一致，但 best 与控制状态忠实恢复。

### 5. `src/main.rs`：编排

**输入备份**（需求 4）——创建 output_dir 后、退火前：

```rust
fn backup_inputs(cfg: &Config, cli_config_path: &str, output_dir: &str) {
    let inputs_dir = format!("{output_dir}/inputs");
    fs::create_dir_all(&inputs_dir)...;
    for src in [cli_config_path, &cfg.files.fixed, &cfg.files.dynamic,
                &cfg.files.splits, &cfg.files.pair_equiv, &cfg.files.key_dist] {
        let dst = format!("{inputs_dir}/{}", basename(src));
        if let Err(e) = fs::copy(src, &dst) { eprintln!("⚠️ 备份输入 {src} 失败: {e}"); }
    }
}
```

**写 meta**（需求 6）——校准得 `scale_config` 后、退火前，写一次 `checkpoint/meta.json`。

**ctrl-c + stop_flag**（需求 8）：

```rust
let stop_flag = Arc::new(AtomicBool::new(false));
{ let sf = stop_flag.clone(); ctrlc::set_handler(move || sf.store(true, Relaxed)).expect(...); }
println!("💡 提示: Ctrl-C 暂停并保存检查点，之后 `resume -d {output_dir}` 继续");
```

**并行调用**：把 `simulated_annealing(&ctx, cfg, i)` 改为 `simulated_annealing_resumable(&ctx, cfg, i, &stop_flag, None, Some(&ckpt_dir), interval)`，收集 `SaResult`；`interrupted = stop_flag.load() || results.any(|r| r.interrupted)`。中断时打印 resume 提示并以当前 best 收尾（与正常收尾共用产出逻辑）。

**resume 子命令**：

```rust
#[derive(Subcommand)] enum Commands {
    ...
    Resume { #[arg(short='d', long)] dir: String },
}

fn run_resume(dir: &str) {
    let ckpt_dir = checkpoint_dir(dir);
    let meta = load_meta(&ckpt_dir)?;                          // 只读 + 校验 version（需求 7.4/11.4）
    // 从备份重建 ctx：只读 {dir}/inputs/config.toml 与 inputs 下各文件（需求 7.2/7.3，不修改 inputs）
    let cfg = Config::load_from_path(&format!("{dir}/inputs/config.toml"));
    let (ctx, ...) = build_ctx_from_inputs(dir, &cfg, meta.scale_config); // 复用 meta.scale_config，不重校准（需求 6.3/7.4）
    // 校验线程数（需求 11.4）
    let tcs: Vec<ThreadCheckpoint> = (0..meta.num_threads)
        .map(|i| load_thread_checkpoint(&thread_path(&ckpt_dir, i))).collect::<Result<_,_>>()?;
    let output_dir = dir.to_string();   // 复用既有目录，不新建（需求 7.6）
    println!("输出目录: {output_dir}");
    // 不调用 backup_inputs、不重写 meta.json（resume 对 inputs/ 与 meta.json 只读，需求 7.3/7.4）
    // 安装 ctrlc + 并行 resumable(resume=Some(&tcs[i]))，收尾产出到 output_dir（需求 7.8）
}
```

`build_ctx_from_inputs`：用 `inputs/` 下的备份输入与 `meta.scale_config` 走与 optimize 相同的 ctx 构建路径，但跳过 `calibrate_scales`；全程只读 inputs。`total_steps`/`num_threads`/温度参数以 `meta` 为权威。

> 实现提示：optimize 与 resume 的「加载输入 → 构建 ctx → 安装 ctrlc → 并行退火 → 收尾产出」高度重合，应抽出共享函数（如 `run_sa_phase(ctx, cfg, output_dir, scale_config, resume_ckpts: Option<Vec<ThreadCheckpoint>>)`），减少重复并保证两路径行为一致。

---

## Data Models

### checkpoint 文件大小

`ThreadCheckpoint` ≈ `assignment`(num_groups B) + `best_assignment`(num_groups B) + ~8 个 f64/usize + 两个 Metrics（各 5 标量）。num_groups 量级数百~数千 → 单文件 JSON 数 KB。每 interval（默认全程 20 次）× num_threads 写，I/O 可忽略（需求 10.3）。

### 版本与兼容

`CHECKPOINT_VERSION = 1`。`load_meta` 校验 `meta.version == CHECKPOINT_VERSION`，不一致则报错退出（需求 6.4/11.4）。线程检查点数与 `meta.num_threads` 不一致亦报错（需求 11.4）。

---

## Error Handling

| 情况 | 处理 |
|------|------|
| 备份某输入文件失败（需求 4.4） | `eprintln!` 告警，继续备份其余；不中止 |
| checkpoint 写失败（需求 10.2） | `eprintln!` 告警，退火继续（旁路持久化失败不影响优化） |
| 写入在 rename 前中断（需求 3.2） | 目标文件保持上次完整内容；残留 `.tmp` 无害 |
| resume 缺 meta.json / 版本不符 / 线程数不符（需求 11.4） | 明确错误信息并安全退出 |
| resume 缺某 `thread-NN.json` | 明确错误并退出（不静默以默认状态续算） |
| 某线程 `current_step >= total_steps`（需求 7.9） | 主循环空区间，跳过直接收尾，不报错 |
| resume 误改 inputs/ 或 meta.json | 设计上 resume 路径不调用 `backup_inputs`、不调用 `save_meta`，仅只读（需求 7.3/7.4）|
| 归档目标名碰撞（同毫秒两次） | `timestamp` 取毫秒级；极端碰撞时后者覆盖前者归档（两版近乎相同，无害，需求 13.4）|
| `ctrlc::set_handler` 重复设置 | 单进程仅设置一次；optimize 与 resume 各为独立进程 |

---

## Correctness Properties

以下性质用 `proptest` 表达（每条 ≥100 次迭代），辅以单元/集成测试。测试顶部标注 `// Feature: annealing-checkpoint-resume, Property N: ...`。

### Property 1: 检查点间隔换算
对任意 `total_steps ≥ 1` 与 `ratio ∈ [0,1]`，`checkpoint_interval(total_steps, ratio) == max(1, floor(total_steps × ratio))`，且恒 ≥ 1。
**Validates: Requirements 9.2, 9.3**

### Property 2: ThreadCheckpoint 序列化往返恒等
对任意 `ThreadCheckpoint`，`from_json(to_json(tc))` 与 `tc` 逐字段相等（含 best 分量、assignment、控制状态、simple_activated）。
**Validates: Requirements 5.1, 5.2, 5.3, 5.4**

### Property 3: CheckpointMeta 序列化往返恒等且版本校验
对任意 `CheckpointMeta`，往返恒等；且 `load_meta` 对版本不符的元信息返回错误。
**Validates: Requirements 6.2, 6.4, 11.4**

### Property 4: 原子保存后可加载且与源相等
对任意 `ThreadCheckpoint`/`CheckpointMeta`，`save_*` 后从目标路径 `load_*` 得到与源逐字段相等的值，且保存后无残留 `.tmp` 文件。
**Validates: Requirements 3.1, 3.2**

### Property 5: resume 续算不重置进度且最优不退化
对任意 `ThreadCheckpoint`（`current_step < total_steps`），`simulated_annealing_resumable(resume=Some(tc))` 的起始步等于 `tc.current_step`，恢复后的 `best_*` 等于 `tc.best_*`，且续算返回的 `best_score ≤ tc.best_score`（最优单调不退化）。
**Validates: Requirements 5.5, 7.5, 7.9, 11.1, 11.3**

### Property 6: 归档保留旧版本且规范文件恒为最新
对任意一串依次写出的 `ThreadCheckpoint`（时间戳各异），连续调用 `save_thread_checkpoint`（按约定传递 `prev_ts`）后：规范文件 `thread-{NN}.json` 内容等于最后一次写入；每个被覆盖的历史版本都以 `thread-{NN}-{TIMESTAMP}.json` 存在且内容与当时写入相等；无历史文件被删除；过程不创建符号链接。
**Validates: Requirements 13.1, 13.2, 13.3, 13.5**

## Testing Strategy

1. **checkpoint 单元/属性测试**（`src/checkpoint.rs`）：Property 2/3/4/6（往返、原子 save→load、版本校验、无 .tmp 残留、归档保留旧版且规范恒为最新、无 symlink）。
2. **间隔换算属性测试**（`src/config.rs` 或 `annealing.rs`）：Property 1（镜像既有 `reconcile_interval` 测试）。
3. **resume 初始化单元测试**（`src/annealing.rs`）：构造 `ThreadCheckpoint`，断言 resumable 的起始步与 best 恢复一致（Property 5 的非随机部分）；小规模端到端「跑 N 步→中断→resume 续算」断言 best 不退化、最终步数达 `total_steps`。
4. **输入备份单元测试**（`main.rs` 或集成测试）：构造临时 cfg/输入，调用 `backup_inputs`，断言 `inputs/` 下出现全部 6 个文件且内容与源一致。
5. **配置文件烟雾测试**：`config.toml.example` 与 `moling/config.toml` 含 `checkpoint_interval_ratio` 且带注释；example 取值等于代码默认（0.05）。
6. **回归**：现有全部单元/属性/集成测试保持通过（`simulated_annealing` 薄包装行为不变）；正常一次性运行的最终产物不变（需求 12.4）。
