# 技术设计文档：稀疏桶内存优化（sparse-bucket-memory-optimization）

## Overview

本特性把 `Evaluator` 与 `SimpleEvaluator` 中一批「按编码值 `code` 直接索引、大小 = `code_space`（或 `code_base^L`）」的密集数组，替换为一个统一的桶存储抽象 `BucketStore`。`BucketStore` 在构建时按 `code_space` 阈值自适应选择两种后端：

- **Dense（密集）**：`Vec` 直接索引，等价于现状，零性能回归，用于小码长（`max_parts ≤ 4`）。
- **Sparse（稀疏）**：`FxHashMap<u32, _>` 仅存非空桶，内存随非空桶数（≤ n_chars）增长，用于大码长（`max_parts = 5`）。

核心不变量：**任意时刻非空全码桶数 ≤ n_chars（11177）**，因此稀疏后端内存与 `code_space` 无关。

改动范围严格限定（需求 10）：
- 新增 `src/bucket_store.rs`（或置于 `evaluator.rs` 内的模块）——`BucketStore` 抽象。
- `src/evaluator.rs`——`Evaluator` 四数组合并入 `BucketStore`；`SimpleEvaluator` 的 `bucket_collision_contrib`、每级 `buckets`、`bucket_gen` 稀疏化；全量扫描去 code_space 化；快照/回滚适配。
- `src/context.rs`——仅在需要时新增「后端选择阈值」常量与 `max_parts ≥ 7`（`code_space > u32::MAX`）的构建期检查；在配置确认阶段输出后端选择日志（需求 12，全部规模数字运行时计算）；`simple_level_capacity` 语义保留为上界校验。

不改变任何配置项、对外指标、输出文件或优化质量（需求 3、需求 10.4）。

---

## Architecture

### 当前（基线）按 code 索引的密集结构

| 持有者 | 字段 | 大小 | 元素 |
|--------|------|------|------|
| `Evaluator` | `code_to_chars: Vec<Vec<usize>>` | `code_space` | 桶成员 |
| `Evaluator` | `bucket_freq_sum: Vec<u64>` | `code_space` | 桶频率和 |
| `Evaluator` | `bucket_max_freq: Vec<u64>` | `code_space` | 桶最大频率 |
| `Evaluator` | `bucket_first: Vec<usize>` | `code_space` | 桶首选字 |
| `SimpleEvaluator` | `bucket_collision_contrib: Vec<(usize,u64)>` | `code_space` | 全码桶简码重码贡献 |
| `SimpleLevelTracker` | `buckets: Vec<SimpleBucket>` | `code_base^L` | 简码桶 |
| `SimpleEvaluator` | `bucket_gen: Vec<Vec<u32>>` | 每级 `code_base^L` | 桶触碰代际标记 |

按 `ci`（汉字下标）索引的结构（`char_bucket_pos`、`current_codes`、`current_equiv_val`、`last_full_codes`、各级 `current_simple_code`/`selected`/`sel_*`）均为 n_chars 大小，**不在本特性改动范围**（需求 1.4）。

### 目标结构

```
Evaluator
  full_buckets: BucketStore<FullBucket>      ← 取代上面 4 个 Vec
SimpleEvaluator
  bucket_collision_contrib: FxHashMap<u32,(u32,u64)>   ← 稀疏，缺失=（0,0）
  levels[li].buckets: BucketStore<SimpleBucket>        ← 取代每级 Vec
  touch: GenSet（稀疏代际去重）                          ← 取代 bucket_gen
```

### 后端选择

```
const SPARSE_THRESHOLD: usize = 1 << 21;   // 2,097,152
// max_parts=4 → code_space=1,048,576 ≤ 阈值 → Dense（与基线逐字节等价）
// max_parts=5 → code_space=33,554,432 > 阈值 → Sparse
BucketStore::new(capacity) =
    if capacity <= SPARSE_THRESHOLD { Dense(vec![default; capacity]) }
    else { Sparse(FxHashMap::default()) }
```

阈值取 `2^21` 使 `max_parts ≤ 4` 全部走 Dense（满足需求 5.4）。简码每级以 `simple_level_capacity[li]` 做同样判断。

---

## Components and Interfaces

### 1. `FullBucket`：合并后的全码桶状态

把现 4 个数组中同一 `code` 的状态聚到一个 struct，一次查找拿全（需求 1.2）：

```rust
#[derive(Clone, Default)]
pub struct FullBucket {
    pub members: Vec<u32>,   // 原 code_to_chars[code]，ci 用 u32（需求 6.1）
    pub freq_sum: u64,       // 原 bucket_freq_sum[code]
    pub max_freq: u64,       // 原 bucket_max_freq[code]
    pub first: u32,          // 原 bucket_first[code]，空桶 = u32::MAX
}
```

`Default` 给出空桶语义 `{members:[], freq_sum:0, max_freq:0, first:u32::MAX}`（需求 2.4）。注意 `first` 默认须为 `u32::MAX`，故不能直接 `#[derive(Default)]`（默认 0），需手写 `Default` 或用构造函数。

### 2. `BucketStore<B>`：统一桶存储抽象

```rust
enum Backend<B> {
    Dense(Vec<B>),
    Sparse(FxHashMap<u32, B>),
}
pub struct BucketStore<B: Default + Clone> {
    backend: Backend<B>,
    nonempty: usize,   // 仅观测/防回归用
}
```

核心接口（对调用方屏蔽后端，需求 5.5）：

```rust
impl<B: Default + Clone> BucketStore<B> {
    fn new(capacity: usize) -> Self;             // 按阈值选后端（需求 5.1-5.4）

    // 只读：缺失桶按空桶语义处理（需求 2.4）。Dense 直接索引；Sparse get。
    fn get(&self, code: u32) -> Option<&B>;
    fn get_or_default<'a>(&'a self, code: u32, empty: &'a B) -> &'a B;

    // 可变：确保该 code 的桶存在（Sparse 用 entry().or_default()）（需求 1.1）。
    fn get_mut_or_insert(&mut self, code: u32) -> &mut B;

    // 移除空桶（Dense：原地置回 Default；Sparse：remove）（需求 2.1/2.2）。
    fn remove(&mut self, code: u32);

    // 仅遍历非空桶（需求 7）。Dense：filter 非空；Sparse：iter。
    fn iter_nonempty(&self) -> impl Iterator<Item = (u32, &B)>;
}
```

**有界性的关键**：调用方在桶成员降为 0 后必须调用 `remove`（Sparse 才能保持条目数 ≤ n_chars，需求 2）。Dense 后端的 `remove` 是把该槽位写回 `Default`（不缩容，行为与基线一致）。

判定一个桶是否「非空」统一为「`get(code)` 存在且 `members` 非空」。Sparse 下因空桶已 `remove`，「键存在」即「非空」，这正好支撑需求 9.1 的占用保护判定。

### 2.1 后端选择决策与日志（需求 12）

#### 决策逻辑

后端选择**只依据容量上界，不看实际占用**，因为 Dense 是按容量一次性预分配的（无论多稀疏都吃满 `capacity × sizeof(B)`）。容量完全由配置/输入在运行时决定，是确定性的：

```rust
// 唯一允许的编译期常量（需求 12.6）：后端选择阈值。
// 取 2^21 使 code_space ≤ 1,048,576 走 Dense、33,554,432 走 Sparse。
pub const SPARSE_THRESHOLD: usize = 1 << 21;   // 2,097,152

pub enum BackendKind { Dense, Sparse }

#[inline]
pub fn choose_backend(capacity: usize) -> BackendKind {
    if capacity <= SPARSE_THRESHOLD { BackendKind::Dense } else { BackendKind::Sparse }
}
```

- 全码桶容量 = `ctx.code_space`（运行时 = `code_base^max_parts`）。
- 简码每级容量 = `ctx.simple_level_capacity[li]`（运行时 = `code_base^L`）。

`BucketStore::new(capacity)` 内部调用 `choose_backend(capacity)` 选后端，**构造时定一次、之后不变**，且内部静默（不打日志，避免每线程/每次评估器构造刷屏）。

**所有规模数字均运行时计算（需求 12.6）**：`code_base`、`max_parts`、`code_space`、各级 `code_base^L`、`n_chars = ctx.char_infos.len()`、Dense 预估内存（`capacity × sizeof::<B>()`，`sizeof` 用 `std::mem::size_of`）一律从 `ctx` 字段与类型大小推导，代码中除 `SPARSE_THRESHOLD` 外不出现任何写死的规模常量。

#### 日志（在配置确认阶段、单线程、仅一次）

因决策是容量的纯函数，故在 `OptContext::new`（或紧随其后的「配置确认」打印处，单线程执行一次）统一计算并输出，而非在 `BucketStore::new` 内（需求 12.5）。提供一个纯函数生成日志，便于单元测试：

```rust
// 全部数字来自参数（运行时值），无硬编码（需求 12.6）。
fn log_backend_choice(label: &str, capacity: usize, elem_size: usize, n_chars: usize) {
    match choose_backend(capacity) {
        BackendKind::Dense => println!(
            "   · {label}: Dense (容量 {capacity} ≤ 阈值 {SPARSE_THRESHOLD}；预估 ~{} MB/线程)",
            (capacity * elem_size) / (1024 * 1024)),
        BackendKind::Sparse => println!(
            "   · {label}: Sparse (容量 {capacity} > 阈值 {SPARSE_THRESHOLD}；Dense 将需 ~{} MB/线程，\
             改用稀疏存储，内存随非空桶数 ≤ n_chars({n_chars}) 增长)",
            (capacity * elem_size) / (1024 * 1024)),
    }
}
```

调用点（需求 12.1/12.7）：
- 一行头：`📦 桶存储后端选择 (code_base={ctx.code_base}, max_parts={ctx.max_parts}, code_space={ctx.code_space}):`
- 全码桶：`log_backend_choice("全码桶", ctx.code_space, size_of::<FullBucket>(), n_chars)`。
- 简码各级（仅当 `enable_simple_code`，需求 12.7）：对每个 `li` 调 `log_backend_choice(&format!("简码级别 L={}", ...), ctx.simple_level_capacity[li], size_of::<SimpleBucket>(), n_chars)`。

注意 `size_of::<FullBucket>()` 仅为桶头部大小（含 `members: Vec` 的 24 字节指针，不含其堆元素）；预估值用于「Dense 预分配开销」的量级提示，与基线观测一致即可，无需精确到堆元素。

### 3. `Evaluator` 改造


字段替换：

```rust
// 删除：code_to_chars / bucket_freq_sum / bucket_max_freq / bucket_first
full_buckets: BucketStore<FullBucket>,
```

`new_impl`（evaluator.rs:1818 起）：
- `full_buckets = BucketStore::new(cs)`；逐字填充时 `let b = full_buckets.get_mut_or_insert(code); b.members.push(ci as u32); b.freq_sum += freq; ...`。
- 碰撞统计与首选字初始化循环由 `for code in 0..cs` 改为 `for (code, b) in full_buckets.iter_nonempty()`（需求 7.1）。

`update_char`（evaluator.rs:2009 起）逐句映射（语义不变，需求 3）：
- 旧桶：`let b = full_buckets.get_mut_or_insert(old_code)`（一定存在）→ swap_remove + `freq_sum -= freq` + `max_freq`/`first` 重扫；**若 `b.members.is_empty()` 则 `full_buckets.remove(old_code)`**（需求 2.1）。
- 新桶：`let b = full_buckets.get_mut_or_insert(new_code)`（`or_insert` 给空桶）→ push + 聚合更新。
- `rescan_bucket_first`/`rescan_bucket_max`（evaluator.rs:1961/1981）改为接收 `&[u32] members` 的自由函数或在取得 `&mut FullBucket` 后对 `&b.members` 操作，避免与 `get_mut` 借用冲突。

借用冲突处理：`update_char` 中需要先改旧桶、再改新桶。由于 Sparse 是 `FxHashMap`，不能同时持有两个 `get_mut`。采用「先完成旧桶全部操作（局部作用域结束释放借用），再处理新桶」的顺序（基线本就是先移除后插入，天然满足）。`char_bucket_pos[moved_ci]` 的写发生在旧桶作用域内，对 `self.char_bucket_pos` 的借用与 `full_buckets` 不冲突（不同字段）。

### 4. `SimpleEvaluator` 改造

**4a. `bucket_collision_contrib`**（需求 4.1/4.2）：

```rust
bucket_collision_contrib: FxHashMap<u32, (u32, u64)>,   // 缺失 = (0,0)
```
- 读：`*self.bucket_collision_contrib.get(&code).unwrap_or(&(0,0))`。
- 差量维护：算出 `(new_count,new_freq)`，若为 `(0,0)` 则 `remove(&code)`，否则 `insert`。
- `recompute_collisions_full`（evaluator.rs:694）：`clear()` 后仅遍历非空全码桶（由 `Evaluator` 传入 `full_buckets.iter_nonempty()` 的视图，见下「跨结构访问」），对每个非空桶算贡献并 `insert`（需求 7.2）。count 用 `u32`（桶成员 ≤ n_chars）。

**4b. 每级 `buckets`**（需求 4.3）：`SimpleLevelTracker.buckets: BucketStore<SimpleBucket>`，容量 `simple_level_capacity[li]`，同阈值选后端。`SimpleBucket { members: Vec<u32>, freq_sum: u64 }`（成员改 u32）。桶变空时 `remove`。

**4c. `bucket_gen` → 稀疏代际去重 `GenSet`**（需求 4.4）：

`bucket_gen` 现为每级 `code_base^L` 的 `Vec<u32>`，靠代际计数实现 O(1) 去重 + O(1) 复位。稀疏替代：

```rust
struct GenSet { gen: FxHashMap<u32, u32>, cur: u32 }   // 每级一个
impl GenSet {
    fn bump(&mut self) { self.cur = self.cur.wrapping_add(1);
        if self.cur == 0 { self.gen.clear(); self.cur = 1; } }
    fn touch(&mut self, code: u32) -> bool {            // true=首次触碰
        if self.gen.get(&code) == Some(&self.cur) { false }
        else { self.gen.insert(code, self.cur); true } }
}
```

`touch_bucket`（evaluator.rs:1001）改用 `GenSet::touch` 判首次触碰；`bump_generation`（983）改用各级 `GenSet::bump`。Dense 后端下也可继续用 `GenSet`（条目数 = 本次移动触碰桶数，极小），无需为 Dense 单独保留 `Vec` 版本——`GenSet` 的条目同样受「每步触碰桶数」界定，复位走 `clear`。`assigned_touch_gen`（n_chars 大小，按 ci 索引）不在改动范围，保持不变。

### 5. 跨结构访问：简码读全码桶

简码侧多处需要读 `full_code_to_chars`（主评估器全码桶）：`is_code_blocked`（427）、`recompute_collisions_full`（694）、`apply_move_incremental` 中对 `full_code_to_chars[code]` 的空/非空判定（885-890）等。基线通过 `&[Vec<usize>]` 切片传入。改造后主评估器持有的是 `BucketStore<FullBucket>`，故：

- 把这些函数的入参从 `full_code_to_chars: &[Vec<usize>]` 改为 `full_buckets: &BucketStore<FullBucket>`。
- 取成员：`full_buckets.get(code).map(|b| b.members.as_slice()).unwrap_or(&[])`。
- 空/非空判定：`full_buckets.get(code).map_or(true, |b| b.members.is_empty())`。
- `is_code_blocked` 的 N=0 分支（需求 9.1）：`full_buckets.get(code).map_or(false, |b| !b.members.is_empty())`，Sparse 下等价于 `contains_key`。

### 6. 快照与回滚（需求 8）

简码侧回滚机制（`SimpleSnapshot` 的 `bucket_snaps`/撤销日志）本质是「记录修改前值、逆序回放」，与底层是 Vec 还是 Map 无关，逻辑基本照搬。仅两点适配：
- `BucketSnap` 还原成员后，若还原结果为空桶，需经 `BucketStore::remove` 落实（需求 8.2）；若从空恢复为非空，`get_mut_or_insert` 自然重建（需求 8.3）。
- `bucket_collision_contrib` 回滚：现以 `collision_buckets: Vec<(code, old_count, old_freq)>` 逆序回放；还原为 `(0,0)` 时改为 `remove`，否则 `insert`。

主评估器 `Evaluator` 全码桶的回滚：现有全码回滚（`try_move`/`try_swap` 内对 `update_char` 的反向重放，本就经 `update_char` 逐字回放）天然适配——回放同样会触发空桶 `remove`/非空 `or_insert`，无需额外快照。

---

## Data Models

### 内存对比（n_chars = 11177，单线程，max_parts=5，code_space=33.5M）

| 结构 | 基线（Dense） | 本设计（Sparse） |
|------|--------------|------------------|
| 全码 4 数组（合并 FullBucket） | ~1.6 GB | ≈ 40B×11177 + Map 开销 ≈ **1~2 MB** |
| `bucket_collision_contrib` | ~537 MB | ≈ 12B×(非空桶) ≈ **0.2 MB** |
| 每级简码 `buckets` + `bucket_gen` | 每级 ~1.2 GB | 随该级非空简码桶 ≈ **MB 级** |

× 8~12 线程后，从 ~17 GB 量级降到几十 MB 量级（需求 11.5）。

max_parts=4（code_space=1.05M）走 Dense，内存与性能与基线完全一致（需求 5.2、11.4）。

### 整型表示（需求 6）

- 桶成员 `ci`：`u32`（≤ 11177）。
- 稀疏键 `code`：`u32`。`code_base^max_parts`：`32^6 ≈ 1.07e9 < u32::MAX`，`32^7 ≈ 3.4e10 > u32::MAX`。
- 构建期检查：`if code_space > u32::MAX as usize { panic/Err }`（即 `max_parts ≥ 7`，需求 6.3）。

---

## Error Handling

- `max_parts ≥ 7`（`code_space > u32::MAX`）：在 `OptContext::new` 构建期检测并以明确错误信息终止（需求 6.3），不静默截断。
- 简码编码越界（`code >= simple_level_capacity[li]`）：保留现有 `debug_assert!`（需求 4.5），release 下行为不变。
- 稀疏后端 `get` 缺失键：按空桶语义返回，不 panic（需求 2.4）。

---

## Correctness Properties

以下性质用 `proptest` 表达，每条 ≥100 次迭代，作为回归防护与正确性证明（需求 11）。

### Property 1: 空桶语义一致
对任意 `code`，未写入或已移除的桶经 `get` 读取等价于 `{members:[], freq_sum:0, max_freq:0, first:u32::MAX}`，且 Dense 与 Sparse 两后端逐字段一致。
**Validates: Requirements 2.4, 5.5**

### Property 2: 稀疏有界性
对任意 push/pop/remove 操作序列，Sparse 后端的条目数恒等于当前非空桶数，且 ≤ n_chars。
**Validates: Requirements 2.1, 2.2, 2.3**

### Property 3: 双后端可观察等价
对同一随机操作序列，Dense 与 Sparse 的 `iter_nonempty` 结果集合及各桶聚合（members/freq_sum/max_freq/first）完全一致。
**Validates: Requirements 5.2, 5.5, 7.4**

### Property 4: 全码指标等价
对任意分配，经 `BucketStore` 计算的 `total_collisions`/`collision_frequency`/`total_equiv_weighted` 与基线公式逐字段一致。
**Validates: Requirements 3.1, 3.3**

### Property 5: 增量 == 全量重建（稀疏后端）
在强制 Sparse 后端（小阈值）下，增量维护的简码指标与对同一分配全量重建的结果一致。
**Validates: Requirements 3.2, 3.4, 4.6**

### Property 6: 回滚精确还原
对任意被拒绝/回滚的移动，全码与简码桶（含空桶移除/重建）还原至移动前状态，指标与移动前逐字段一致。
**Validates: Requirements 8.1, 8.2, 8.3, 8.4**

### Property 7: 占用保护语义保持
N=0 与 N>0 下 `is_code_blocked(code)` 在 `BucketStore` 上的结果与基线一致。
**Validates: Requirements 9.1, 9.3**

## Testing Strategy

遵循仓库既有做法（`proptest`，每属性 ≥100 次迭代，测试顶部标注 `// Feature: sparse-bucket-memory-optimization, Property N: ...`）。

1. **BucketStore 单元测试**（需求 11.2）：
   - 空桶语义：`get` 缺失键返回 `None`/空桶默认值。
   - 有界性：随机 push/pop 操作序列后，Sparse 条目数 == 非空桶数，且空桶被 `remove`。
   - 双后端等价：对同一随机操作序列，Dense 与 Sparse 产生一致的 `iter_nonempty` 结果集合与各桶聚合。
2. **增量 == 全量重建一致性**（需求 11.1）：复用并保持现有简码一致性属性测试通过（这些测试不感知后端，天然覆盖稀疏化正确性）。可补充一条强制 Sparse 后端（小阈值）下的「增量 == 全量」属性测试。
3. **全码指标等价**（需求 3.1）：对随机分配，断言经 `BucketStore` 的 `total_collisions`/`collision_frequency`/当量与基线公式一致。
4. **占用保护语义**（需求 9）：N=0 与 N>0 下 `is_code_blocked` 结果与基线一致。
5. **集成回归**（需求 11.3/11.5）：
   - `max_parts=4` 配置固定种子运行，最终指标与基线一致。
   - `max_parts=5` 配置（如 `code_genie2/moling`）能构建并运行至产出输出，不 OOM（冒烟级，小步数）。

---

## 实施顺序（与 tasks 对应）

1. 引入 `BucketStore` + `FullBucket` + 单元测试（独立、可先行验证）。
2. `Evaluator` 四数组迁移 + 全量扫描去 code_space 化 + `max_parts≥7` 检查。
3. `SimpleEvaluator`：`bucket_collision_contrib` 稀疏化。
4. `SimpleEvaluator`：每级 `buckets` 迁移 + `bucket_gen` → `GenSet`。
5. 跨结构访问签名调整（`&[Vec<usize>]` → `&BucketStore`）。
6. 快照/回滚适配 + 一致性属性测试 + 集成回归。

每步完成后跑 `cargo build` 与现有测试，保证逐步零回归。
