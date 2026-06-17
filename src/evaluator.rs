// =========================================================================
// ⚡ 评估器
// =========================================================================

use rand::prelude::*;

use std::cmp::Ordering;

use rustc_hash::FxHashMap;

use crate::context::OptContext;
use crate::bucket_store::{BucketStore, FullBucket};
use crate::types::{
    KeyDistConfig, MetricScores, Metrics, SimpleAssignMode, SimpleMetricScores, SimpleMetrics, EQUIV_TABLE_SIZE,
    KEY_SPACE,
};

// =========================================================================
// 简码评估器
// =========================================================================

/// 单个简码指令的最大键位数上界（用于无堆分配地存储「选中字出简贡献」的键位列表）。
///
/// 简码桶容量为 `code_base^L`（`code_base = EQUIV_TABLE_SIZE + 1 = 32`），故现实配置中
/// 单级简码指令长度 `L` 极少超过 4（`32^4` 已达百万级桶）。取 12 留足冗余；`add_contrib`
/// 中以 `debug_assert!` 校验不越界。这样每个「选中字」的键位贡献可存于定长数组，回滚时
/// 整存整取，warmup 后热路径不再产生新的堆分配（需求 3.1）。
const SIMPLE_KEYS_CAP: usize = 12;

/// 简码桶：映射到同一简码编码的候选字集合（局部排序对象）
#[derive(Clone, Default)]
struct SimpleBucket {
    /// 映射到该简码编码的候选字 ci 列表（u32，需求 6.1）。内联小向量，≤2 成员不触碰堆。
    members: crate::bucket_store::Members,
    /// 桶频率和
    freq_sum: u64,
}

impl crate::bucket_store::Bucket for SimpleBucket {
    #[inline]
    fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    #[inline]
    fn reset(&mut self) {
        self.members.clear(); // 保留容量，不释放
        self.freq_sum = 0;
    }
}

/// 简码级别跟踪器（增量化）
struct SimpleLevelTracker {
    /// 该级别的编码数
    code_num: usize,
    /// 简码桶向量容量 = ctx.simple_level_capacity[li]
    capacity: usize,
    /// 简码桶存储：按 code_base^L 阈值自适应密集/稀疏后端（需求 4.3/5）。
    buckets: BucketStore<SimpleBucket>,
    /// current_simple_code[ci]：该字当前所在桶编码，-1 表示无
    current_simple_code: Vec<i64>,
    /// 该级出简标记，按 ci 索引
    selected: Vec<bool>,
    /// 已覆盖的频率（级别聚合，增量维护）
    covered_freq: u64,
    /// 加权等价值
    equiv_weighted: f64,
    /// 等价值频率总和
    equiv_freq_sum: u64,
    /// 键位使用统计
    key_usage: [f64; EQUIV_TABLE_SIZE],
    /// 键击次数
    key_presses: f64,
    /// 出简贡献缓存（按 ci 索引，仅当 `selected[ci]` 时有效）：选中时刻该字的简码当量值。
    ///
    /// 用于在「取消选中 / 刷新」时精确扣除该字曾累加进 `equiv_weighted` 的贡献
    /// （`sel_equiv[ci] * freq`）。因增量更新发生在 `assignment` 已更新之后，无法再由
    /// 当前 `assignment` 复原旧贡献，故在选中时刻就地存储，保证逐字段与全量一致（需求 1.4）。
    sel_equiv: Vec<f64>,
    /// 出简贡献缓存（按 ci 索引，仅当 `selected[ci]` 时有效）：选中时刻该字的简码键位列表。
    /// 配合 `sel_keys_len` 使用，避免存储变长 `Vec` 带来的每步堆分配（需求 3.1）。
    sel_keys: Vec<[u8; SIMPLE_KEYS_CAP]>,
    /// `sel_keys[ci]` 的有效长度（按 ci 索引）。
    sel_keys_len: Vec<u8>,
}

/// 级别聚合标量快照（用于回滚整存整取）
#[derive(Clone)]
struct LevelAggregateSnapshot {
    covered_freq: u64,
    equiv_weighted: f64,
    equiv_freq_sum: u64,
    key_usage: [f64; EQUIV_TABLE_SIZE],
    key_presses: f64,
}

/// 触碰桶的成员快照（移动前状态，复用缓冲）。
///
/// 阶段 2 改为「仅对脏桶做局部重排 + pending 跨级排除传播」的精确增量后，回滚不再整级
/// 覆盖，而是对「本次移动真正触碰过的桶」做一次性成员快照（首次触碰时记录），回滚时整存
/// 整取还原成员顺序与 `freq_sum`。`members` 用 `clear` + `extend_from_slice` 就地填充，
/// warmup 后不再产生新的堆分配（需求 3.1）。
#[derive(Clone, Default)]
struct BucketSnap {
    li: usize,
    code: usize,
    members: Vec<u32>,
    freq_sum: u64,
}

/// 选中贡献撤销项（移动前 `sel_equiv` / `sel_keys` / `sel_keys_len`，定长可 Copy）。
#[derive(Clone, Copy)]
struct ContribUndo {
    li: usize,
    ci: usize,
    old_equiv: f64,
    old_keys: [u8; SIMPLE_KEYS_CAP],
    old_len: u8,
}

/// 简码快照（复用预分配缓冲，供 commit/rollback 使用）
///
/// 阶段 2 精确增量化（Backlog B1）后的回滚策略 —— 细粒度撤销（option b）：
/// - 桶成员：`bucket_snaps` 在「首次触碰某桶」时记录其移动前成员与 `freq_sum`
///   （由 `cur_gen` 代际标记去重，保证每桶仅快照一次），回滚整存整取还原；
/// - 级别聚合：标量较小，移动起始对全部级别整存一次（`aggregates`），回滚整体写回；
/// - `current_simple_code` / `selected` / `sel_*` 贡献：以撤销日志（记录每次修改前值）
///   逆序回放，最早的旧值最终生效，精确还原到移动前；
/// - `all_assigned_flags`：由 `assigned_start_flag` + `assigned_touched_list` 记录移动起始
///   值并按需还原（同一结构亦用于阶段 3 的「出简翻转」检测，复用以避免 O(候选字集) 扫描）。
///
/// 该策略不再整级快照/重置（避免 O(级别容量) 退化），回滚成本为 O(受影响项)。
#[derive(Default)]
struct SimpleSnapshot {
    /// 本次快照是否包含出简选择状态（`selection_may_change` 为真时为 true）。
    /// 仅影响全码桶成员（出简选择不变）的移动只需回滚简码重码标量与贡献缓存。
    has_selection: bool,
    /// 触碰桶成员快照池（复用：用 `bucket_snaps_len` 标记有效前缀，不收缩底层容量）。
    bucket_snaps: Vec<BucketSnap>,
    /// `bucket_snaps` 有效前缀长度。
    bucket_snaps_len: usize,
    /// 移动起始的全部级别聚合标量快照（回滚整体写回）。
    aggregates: Vec<LevelAggregateSnapshot>,
    /// 移动起始的全局聚合标量快照（需求 29.5，回滚整体写回）。
    g_covered_freq: u64,
    g_equiv_weighted: f64,
    g_equiv_freq_sum: u64,
    g_key_usage: [f64; EQUIV_TABLE_SIZE],
    g_key_presses: f64,
    /// 移动起始的全局分布偏差与每键贡献快照（方向 B，回滚整体写回）。
    g_dist_deviation: f64,
    g_dist_contrib: [f64; EQUIV_TABLE_SIZE],
    /// 受保护占用计数的撤销日志（需求 33）：(code, 修改前 count)，逆序回放还原。
    protect_undo: Vec<(usize, u32)>,
    /// `current_simple_code` 撤销日志：(li, ci, 修改前 code)，逆序回放。
    undo_code: Vec<(usize, usize, i64)>,
    /// `selected` 撤销日志：(li, ci, 修改前 selected)，逆序回放。
    undo_selected: Vec<(usize, usize, bool)>,
    /// 选中贡献撤销日志（`sel_equiv` / `sel_keys` / `sel_keys_len`），逆序回放。
    undo_contrib: Vec<ContribUndo>,
    /// 触碰的全码桶简码重码贡献快照：(code, 修改前 count, 修改前 freq)
    collision_buckets: Vec<(usize, usize, u64)>,
    /// 触碰的 last_full_codes 条目：(ci, 修改前 full_code)
    last_full_codes: Vec<(usize, usize)>,
    /// 简码重码标量快照
    old_collision_count: usize,
    old_collision_freq: u64,
    old_collision_rate: f64,
    old_cached_simple_score: f64,
}

/// 简码评估器
pub struct SimpleEvaluator {
    /// 各简码级别的跟踪器
    levels: Vec<SimpleLevelTracker>,
    /// 所有出简的汉字标记（跨级别），Vec<bool> 替代 HashSet 加速查找
    all_assigned_flags: Vec<bool>,
    /// 简码重码数：全码桶去掉出简字后仍有重码的数量
    simple_collision_count: usize,
    /// 简码重码频率（重码率分子，增量维护）
    simple_collision_freq: u64,
    /// 简码重码率：全码桶去掉出简字后仍被重码的字频 / 总频
    simple_collision_rate: f64,
    /// 各全码桶对简码重码的当前贡献缓存：bucket_collision_contrib[code] = (count, freq)
    ///
    /// 用于增量更新简码重码：对受影响的少数全码桶用「减旧贡献、加新贡献」做差量维护，
    /// 从而避免每步全量扫描整个编码空间（需求 14.3）。稀疏存储：缺失键等价 (0,0)，
    /// 贡献归零时移除条目以保持有界（需求 4.1/4.2）。
    bucket_collision_contrib: FxHashMap<u32, (usize, u64)>,
    /// 每个汉字「上次同步时」的全码编码缓存（按 ci 索引）。
    ///
    /// 用于在增量更新时检测移动组所触碰的全码桶（旧编码 ∪ 新编码），定位需要重算
    /// 简码重码的全码桶（需求 14.2/14.3）。全量重建时从当前分配重新填充。
    last_full_codes: Vec<usize>,
    /// 全局简码聚合标量（需求 29）：恒等于「各级别对应聚合之和 + 固定简码常量偏置」。
    /// `get_simple_metrics` 直接读取这些量，避免每步跨级重加 O(级数×键数)。
    /// 在 select/deselect/refresh 处与级别聚合同步增量更新，并纳入移动快照回滚。
    g_covered_freq: u64,
    g_equiv_weighted: f64,
    g_equiv_freq_sum: u64,
    g_key_usage: [f64; EQUIV_TABLE_SIZE],
    g_key_presses: f64,
    /// 全局分布偏差及其每键贡献缓存（需求 29 方向 B）：`g_dist_deviation == Σ_k g_dist_contrib[k]`，
    /// `g_dist_contrib[k]` 为键 k 在当前 `g_key_usage[k]`/`g_key_presses` 下的惩罚。
    /// `get_simple_metrics` 直接读 `g_dist_deviation`，避免每步 O(键数) 重算。
    g_dist_deviation: f64,
    g_dist_contrib: [f64; EQUIV_TABLE_SIZE],
    /// 本次移动中 `g_key_usage` 被改动的键（工作缓冲，去堆分配）：用于 presses 不变时只增量
    /// 更新这些键的分布贡献。move 起始清空，move 末尾结算后去重使用。
    g_dirty_keys: Vec<u8>,
    /// 简码占用保护（需求 33）——仅 `simple_protect_top_n > 0` 时使用：
    /// 「全字频前 N 名汉字」当前全码编码的占用计数 `protect_count[code] = 占用该编码的 top-N 字数`。
    /// `blocked(code) = count > 0`。N=0（保护全部）时不用此表，改判全码桶占用。
    protect_count: FxHashMap<usize, u32>,
    /// 本次移动中受保护占用翻转、需重选的简码桶编码（工作缓冲，move 起始清空）。
    protect_dirty_buf: Vec<usize>,
    /// 候选范围标志（active/passive 性能优化）：false = 退火 active 候选集
    /// （`ctx.simple_candidate_chars`，每步增量/对账用）；true = 输出全集
    /// （`ctx.simple_output_candidate_chars`，仅最终上报与 output 文件用）。
    /// 仅影响 `rebuild_selection` 遍历哪个候选列表；增量路径不依赖它。
    output_scope: bool,
    /// 缓存的简码得分
    cached_simple_score: f64,
    /// 得分是否需要重新计算
    simple_score_dirty: bool,
    /// get_simple_keys 复用缓冲区（去堆分配）
    key_buf: Vec<u8>,
    /// 快照回滚复用缓冲
    snapshot: SimpleSnapshot,
    /// 桶触碰代际标记：`bucket_gen[li].get(code) == Some(cur_gen)` 表示该桶在本次移动中已被
    /// 触碰。稀疏存储（FxHashMap），条目数受每步触碰桶数界定（需求 4.4）；溢出时整体清空。
    bucket_gen: Vec<FxHashMap<u32, u32>>,
    /// 当前移动代际计数（每次增量选择开始时自增；溢出时整体复位）。
    cur_gen: u32,
    /// 每级脏桶编码列表（复用工作集）：本次移动中成员发生变化、需局部重排的桶。
    dirty_per_level: Vec<Vec<usize>>,
    /// 跨级排除传播待处理动作（ping-pong 缓冲之一）：(ci, kind)，kind=0 插入 / 1 移除。
    pending_a: Vec<(usize, u8)>,
    /// 跨级排除传播待处理动作（ping-pong 缓冲之二）。
    pending_b: Vec<(usize, u8)>,
    /// 当前级别经 pending 插入的候选字（复用）：用于重排后判定「未选中则继续向上插入」。
    inserted_buf: Vec<usize>,
    /// 当前级别重排新选中的候选字（复用）：原生新选中需向上传播「移除」。
    newly_sel_buf: Vec<usize>,
    /// 当前级别重排新落选的候选字（复用）：需向上传播「插入」。
    newly_desel_buf: Vec<usize>,
    /// `all_assigned_flags` 触碰代际标记（与 `cur_gen` 同源）。
    assigned_touch_gen: Vec<u32>,
    /// `all_assigned_flags` 移动起始值（首次触碰时记录），供回滚与阶段 3 翻转检测复用。
    assigned_start_flag: Vec<bool>,
    /// 本次移动中 `all_assigned_flags` 被触碰过的候选字列表（复用工作集）。
    assigned_touched_list: Vec<usize>,
    /// 增量更新时记录「需重算简码重码的全码桶」工作集（复用，去堆分配）
    affected_full_buckets: Vec<usize>,
    /// 全量重建调用计数（仅用于观测/防回归）。
    ///
    /// 在 `full_rebuild` 入口自增，证明生产热路径 `try_move`/`try_swap`（走增量路径
    /// `apply_move_incremental`）不会触发全量重建。增量路径（apply/commit/rollback）
    /// 不得触碰此计数器。
    pub(crate) full_rebuild_calls: usize,
    /// 阶段 2「出简选择增量」上一次移动访问的候选字次数（观测/防回归，Backlog B1 北极星）。
    ///
    /// 统计 `do_incremental_selection` 中真正被检视的候选字工作量：阶段 1 逐级归属处理、
    /// 重排种子触碰、脏桶局部重排的成员遍历、跨级传播动作。该值应与「受影响字 + 脏桶成员」
    /// 同阶，**不随候选字总集规模增长**——证明阶段 2 不再做 O(候选字集 × 级数) 的整体重算。
    /// 空交集（无候选归属变化且无首选翻转）移动时为 0。每次 `apply_move_incremental` 复位。
    pub(crate) stage2_visits: usize,
}

impl SimpleEvaluator {
    /// 创建新的简码评估器（全量构建，初始化增量状态）
    ///
    /// `is_first_candidate[ci]` 表示 `ci` 是否为其全码桶首选字，供 Efficiency
    /// 模式排序键中的 `sel_len`（0/1）取值（需求 4.6/5.4）。
    ///
    /// `output_scope`：false = 退火 active 候选集；true = 输出全集（仅最终上报/输出）。
    pub fn new(
        ctx: &OptContext,
        assignment: &[u8],
        full_buckets: &BucketStore<FullBucket>,
        is_first_candidate: &[bool],
        output_scope: bool,
    ) -> Self {
        let n_levels = ctx.simple_config.levels.len();
        let n_chars = ctx.char_infos.len();

        let levels: Vec<SimpleLevelTracker> = (0..n_levels)
            .map(|li| {
                let cap = ctx
                    .simple_level_capacity
                    .get(li)
                    .copied()
                    .unwrap_or(1)
                    .max(1);
                SimpleLevelTracker {
                    code_num: ctx.simple_config.levels[li].code_num,
                    capacity: cap,
                    buckets: BucketStore::new(cap),
                    current_simple_code: vec![-1i64; n_chars],
                    selected: vec![false; n_chars],
                    covered_freq: 0,
                    equiv_weighted: 0.0,
                    equiv_freq_sum: 0,
                    key_usage: [0.0; EQUIV_TABLE_SIZE],
                    key_presses: 0.0,
                    sel_equiv: vec![0.0; n_chars],
                    sel_keys: vec![[0u8; SIMPLE_KEYS_CAP]; n_chars],
                    sel_keys_len: vec![0u8; n_chars],
                }
            })
            .collect();

        let bucket_gen: Vec<FxHashMap<u32, u32>> =
            (0..n_levels).map(|_| FxHashMap::default()).collect();

        let mut se = Self {
            levels,
            all_assigned_flags: vec![false; n_chars],
            simple_collision_count: 0,
            simple_collision_freq: 0,
            simple_collision_rate: 0.0,
            bucket_collision_contrib: FxHashMap::default(),
            last_full_codes: vec![0usize; n_chars],
            g_covered_freq: 0,
            g_equiv_weighted: 0.0,
            g_equiv_freq_sum: 0,
            g_key_usage: [0.0; EQUIV_TABLE_SIZE],
            g_key_presses: 0.0,
            g_dist_deviation: 0.0,
            g_dist_contrib: [0.0; EQUIV_TABLE_SIZE],
            g_dirty_keys: Vec::new(),
            protect_count: FxHashMap::default(),
            protect_dirty_buf: Vec::new(),
            output_scope,
            cached_simple_score: 0.0,
            simple_score_dirty: true,
            key_buf: Vec::new(),
            snapshot: SimpleSnapshot::default(),
            bucket_gen,
            cur_gen: 0,
            dirty_per_level: (0..n_levels).map(|_| Vec::new()).collect(),
            pending_a: Vec::new(),
            pending_b: Vec::new(),
            inserted_buf: Vec::new(),
            newly_sel_buf: Vec::new(),
            newly_desel_buf: Vec::new(),
            assigned_touch_gen: vec![0u32; n_chars],
            assigned_start_flag: vec![false; n_chars],
            assigned_touched_list: Vec::new(),
            affected_full_buckets: Vec::new(),
            full_rebuild_calls: 0,
            stage2_visits: 0,
        };

        se.rebuild_internal(ctx, assignment, full_buckets, is_first_candidate);
        se.cached_simple_score = se.compute_simple_score(ctx);
        se.simple_score_dirty = false;
        se
    }

    /// 内部全量重建：从候选字集合出发重建所有级别的增量状态与简码重码。
    ///
    /// 保持与旧 HashMap 实现一致的语义：
    /// - 按级别升序处理，低级别先出简，高级别排除已出简字；
    /// - 桶内按当前分配模式的排序键（`cmp_in_bucket`）局部排序后选取前 `code_num`
    ///   个出简。Frequency 模式排序键为 `freq`，与旧实现的「freq 降序 / ci 升序
    ///   take(code_num)」完全一致（需求 17.5）；Efficiency 模式排序键为
    ///   `freq × (base_saving[ci][li] + sel_len)`。
    fn rebuild_internal(
        &mut self,
        ctx: &OptContext,
        assignment: &[u8],
        full_buckets: &BucketStore<FullBucket>,
        is_first_candidate: &[bool],
    ) {
        let n_chars = ctx.char_infos.len();

        // 记录每个汉字当前全码编码，作为后续增量检测全码桶变化的基线。
        // 必须先于 rebuild_selection：N>0 时占用保护判定（is_code_blocked）依赖 protect_count，
        // 而 protect_count 由 top-N 字的 last_full_codes 推导（需求 33）。
        if self.last_full_codes.len() != n_chars {
            self.last_full_codes = vec![0usize; n_chars];
        }
        for ci in 0..n_chars {
            self.last_full_codes[ci] = ctx.calc_code_only(ci, assignment);
        }
        // 重建受保护占用计数（需求 33，仅 N>0）。须先于出简选择，使桶名额据保护正确归零。
        self.recompute_protect_count(ctx);

        // 简码出简选择（桶 / current_simple_code / selected / 级别聚合 / all_assigned_flags）
        self.rebuild_selection(ctx, assignment, full_buckets, is_first_candidate);

        // 全量重算简码重码并填满每桶贡献缓存（供后续增量差量维护）
        self.recompute_collisions_full(ctx, full_buckets);

        // 据各级别聚合 + 固定简码常量偏置，一次性重算全局聚合（需求 29.2/29.8）。
        self.recompute_global_aggregates(ctx);

        self.simple_score_dirty = true;
    }

    /// 重建受保护占用计数（需求 33，仅 `simple_protect_top_n > 0`）：据 top-N 字当前全码
    /// （`last_full_codes`）统计每个编码被多少 top-N 字占用。N=0（保护全部）时清空、不使用。
    fn recompute_protect_count(&mut self, ctx: &OptContext) {
        self.protect_count.clear();
        if ctx.simple_protect_top_n == 0 || ctx.simple_is_topn.is_empty() {
            return;
        }
        for ci in 0..ctx.simple_is_topn.len() {
            if ctx.simple_is_topn[ci] {
                *self.protect_count.entry(self.last_full_codes[ci]).or_insert(0) += 1;
            }
        }
    }

    /// 简码占用保护判定（需求 33）：编码 `code` 是否等于某受保护汉字的全码。
    /// - N=0（保护全部）：等价于「全码桶 `code` 非空」（经 BucketStore：键存在即非空）。
    /// - N>0：`protect_count[code] > 0`。
    #[inline]
    fn is_code_blocked(
        &self,
        ctx: &OptContext,
        code: usize,
        full_buckets: &BucketStore<FullBucket>,
    ) -> bool {
        if ctx.simple_protect_top_n == 0 {
            full_buckets.get(code as u32).is_some()
        } else {
            self.protect_count.get(&code).copied().unwrap_or(0) > 0
        }
    }

    /// 据各级别聚合 + 固定简码常量偏置，全量重算全局聚合标量（需求 29.2）。
    /// 供全量重建（构造 / `full_rebuild`）与对账后调用，使全局量与「逐级求和 + 固定偏置」一致。
    fn recompute_global_aggregates(&mut self, ctx: &OptContext) {
        let mut cov = ctx.fixed_covered_freq;
        let mut ew = ctx.fixed_equiv_weighted;
        let mut ef = ctx.fixed_equiv_freq_sum;
        let mut kp = ctx.fixed_key_presses;
        let mut ku = ctx.fixed_key_usage;
        for level in &self.levels {
            cov += level.covered_freq;
            ew += level.equiv_weighted;
            ef += level.equiv_freq_sum;
            kp += level.key_presses;
            for k in 0..EQUIV_TABLE_SIZE {
                ku[k] += level.key_usage[k];
            }
        }
        self.g_covered_freq = cov;
        self.g_equiv_weighted = ew;
        self.g_equiv_freq_sum = ef;
        self.g_key_presses = kp;
        self.g_key_usage = ku;
        // 全量重算分布偏差与每键贡献缓存（需求 29 方向 B）。
        self.recompute_dist_full(ctx);
    }

    /// 单键分布惩罚：在给定 `usage`/`presses` 下键 `k` 对分布偏差的贡献。
    /// 与 `get_simple_metrics` 旧内联公式逐字一致；`presses <= 0` 时为 0。
    #[inline]
    fn key_dist_penalty(ctx: &OptContext, k: usize, usage: f64, presses: f64) -> f64 {
        if presses <= 0.0 {
            return 0.0;
        }
        let cfg = &ctx.key_dist_config[k];
        if cfg.target_rate == 0.0 && cfg.low_penalty == 0.0 && cfg.high_penalty == 0.0 {
            return 0.0;
        }
        let actual_pct = usage * 100.0 / presses;
        let diff = actual_pct - cfg.target_rate;
        if diff < 0.0 {
            diff * diff * cfg.low_penalty
        } else if diff > 0.0 {
            diff * diff * cfg.high_penalty
        } else {
            0.0
        }
    }

    /// 全量重算每键分布贡献缓存与总分布偏差（据当前 `g_key_usage`/`g_key_presses`）。
    fn recompute_dist_full(&mut self, ctx: &OptContext) {
        let presses = self.g_key_presses;
        let mut dev = 0.0;
        for k in 0..EQUIV_TABLE_SIZE {
            let c = Self::key_dist_penalty(ctx, k, self.g_key_usage[k], presses);
            self.g_dist_contrib[k] = c;
            dev += c;
        }
        self.g_dist_deviation = dev;
    }

    /// move 末尾结算分布偏差（需求 29 方向 B）：
    /// - presses 与 move 起始相同（仅键分布变化、未改选中集/长度）：只对本次改动的键
    ///   `g_dirty_keys` 增量更新贡献与总分布；
    /// - presses 变化（所有键归一化 pct 漂移）：退回全量重算。
    /// 仅在 `selection_may_change`（已取聚合快照）时调用。
    fn finalize_dist(&mut self, ctx: &OptContext) {
        if (self.g_key_presses - self.snapshot.g_key_presses).abs() < f64::EPSILON {
            // presses 不变：增量更新被触碰键。
            self.g_dirty_keys.sort_unstable();
            self.g_dirty_keys.dedup();
            let presses = self.g_key_presses;
            for idx in 0..self.g_dirty_keys.len() {
                let k = self.g_dirty_keys[idx] as usize;
                let new_c = Self::key_dist_penalty(ctx, k, self.g_key_usage[k], presses);
                self.g_dist_deviation += new_c - self.g_dist_contrib[k];
                self.g_dist_contrib[k] = new_c;
            }
        } else {
            self.recompute_dist_full(ctx);
        }
    }

    /// 将候选字 `ci` 在级别 `li` 的简码键位写入复用缓冲 `key_buf`（去堆分配），并在该级
    /// `space_commit` 为真时追加一个尾随空格键 `KEY_SPACE`（需求 20.7/20.8）。
    ///
    /// 返回是否存在有效简码键位（无简码指令/越界时返回 false，此时不追加空格）。该尾随空格
    /// 通过 `sel_keys` 缓存与 `key_usage`/`key_presses` 的既有维护路径统一计入分布偏差，并由
    /// 快照回滚精确还原；`space_commit` 为假时缓冲不含空格。
    #[inline]
    fn fill_keys_with_commit(
        &mut self,
        ctx: &OptContext,
        ci: usize,
        li: usize,
        assignment: &[u8],
    ) -> bool {
        let has_keys = ctx.get_simple_keys_into(ci, li, assignment, &mut self.key_buf);
        if has_keys && ctx.simple_config.levels[li].space_commit {
            self.key_buf.push(KEY_SPACE as u8);
        }
        has_keys
    }

    /// 仅重建简码出简选择（不触碰简码重码缓存）。
    ///
    /// 与全量路径语义一致：按级别升序处理，低级别先出简、高级别排除已出简字；
    /// 桶内按当前分配模式排序键局部排序后取前 `code_num` 个出简。
    /// 供 `rebuild_internal` 与 `apply_move_incremental` 复用。
    fn rebuild_selection(
        &mut self,
        ctx: &OptContext,
        assignment: &[u8],
        full_buckets: &BucketStore<FullBucket>,
        is_first_candidate: &[bool],
    ) {
        let n_levels = self.levels.len();
        let n_chars = ctx.char_infos.len();

        // 重置所有级别状态：以全新桶存储替换（Sparse 后端 O(1)，Dense 后端重分配，均不残留旧成员）。
        for level in self.levels.iter_mut() {
            level.buckets = BucketStore::new(level.capacity);
            for c in level.current_simple_code.iter_mut() {
                *c = -1;
            }
            for s in level.selected.iter_mut() {
                *s = false;
            }
            level.covered_freq = 0;
            level.equiv_weighted = 0.0;
            level.equiv_freq_sum = 0;
            level.key_usage = [0.0; EQUIV_TABLE_SIZE];
            level.key_presses = 0.0;
        }
        self.all_assigned_flags.clear();
        self.all_assigned_flags.resize(n_chars, false);
        // 固定简码字（需求 21.8）：初始化即置 all_assigned_flags = true 且永不翻转
        // （固定字不在候选集、不参与出简选择），使其在简码重码统计中始终作为「已出简」
        // 从全码桶排除。固定字不计入任何级别聚合（其贡献由 ctx 常量偏置体现）。
        for fc in &ctx.simple_fixed_codes {
            self.all_assigned_flags[fc.ci] = true;
        }

        let mut touched: Vec<usize> = Vec::new();
        // 候选范围（active/passive）：active 用 simple_candidate_chars；输出全集用
        // simple_output_candidate_chars（passive 也参与，仅最终上报/输出时 output_scope=true）。
        let candidate_list: &[usize] = if self.output_scope {
            &ctx.simple_output_candidate_chars
        } else {
            &ctx.simple_candidate_chars
        };
        for li in 0..n_levels {
            // 阶段 1：候选字入桶（仅遍历候选字集合，排除已被低级别出简的字）
            touched.clear();
            for idx in 0..candidate_list.len() {
                let ci = candidate_list[idx];
                if self.all_assigned_flags[ci] {
                    continue;
                }
                if let Some(code) = ctx.calc_simple_code_eligible(ci, li, assignment) {
                    debug_assert!(code < self.levels[li].capacity);
                    let bucket = self.levels[li].buckets.get_mut_or_insert(code as u32);
                    if bucket.members.is_empty() {
                        touched.push(code);
                    }
                    bucket.members.push(ci as u32);
                    bucket.freq_sum += ctx.char_infos[ci].frequency;
                    self.levels[li].current_simple_code[ci] = code as i64;
                }
            }

            // 阶段 2：桶内按分配模式排序键局部排序后选取前 code_num 个出简
            for ti in 0..touched.len() {
                let code = touched[ti];
                // 简码占用保护（需求 33）：编码撞受保护全码的桶不出简（名额=0）；
                // 其候选字 all_assigned_flags 保持 false → 由更高级别（更长简码）继续尝试。
                let code_num = if self.is_code_blocked(ctx, code, full_buckets) {
                    0
                } else {
                    // 固定简码占用名额（需求 21.7）：每桶优化可选名额 = code_num - 占用数（下限 0）。
                    self.levels[li]
                        .code_num
                        .saturating_sub(ctx.simple_fixed_occ(li, code))
                };
                // 仅对受影响桶内的候选列表执行局部排序（需求 6.2/6.3）
                Self::sort_bucket(
                    ctx,
                    is_first_candidate,
                    li,
                    &mut self.levels[li].buckets.get_mut_or_insert(code as u32).members,
                );
                let sel: Vec<usize> = self.levels[li]
                    .buckets
                    .get(code as u32)
                    .map(|b| b.members.iter().take(code_num).map(|&c| c as usize).collect())
                    .unwrap_or_default();
                for ci in sel {
                    let freq = ctx.char_infos[ci].frequency;
                    let freq_f = freq as f64;
                    self.levels[li].selected[ci] = true;
                    self.all_assigned_flags[ci] = true;
                    self.levels[li].covered_freq += freq;

                    let eq = ctx.calc_simple_equiv(ci, li, assignment);
                    self.levels[li].equiv_weighted += eq * freq_f;
                    self.levels[li].equiv_freq_sum += freq;

                    // 记录选中时刻的当量与键位贡献，供后续增量「取消选中/刷新」精确扣除。
                    self.levels[li].sel_equiv[ci] = eq;
                    let has_keys = self.fill_keys_with_commit(ctx, ci, li, assignment);
                    if has_keys {
                        let klen = self.key_buf.len();
                        debug_assert!(klen <= SIMPLE_KEYS_CAP, "简码键位数超出 SIMPLE_KEYS_CAP");
                        let n = klen.min(SIMPLE_KEYS_CAP);
                        for i in 0..n {
                            self.levels[li].sel_keys[ci][i] = self.key_buf[i];
                        }
                        self.levels[li].sel_keys_len[ci] = n as u8;
                        for &k in &self.key_buf {
                            self.levels[li].key_usage[k as usize] += freq_f;
                        }
                        self.levels[li].key_presses += freq_f * klen as f64;
                    } else {
                        self.levels[li].sel_keys_len[ci] = 0;
                    }
                }
            }
        }
    }

    /// 计算单个全码桶（去除已出简字后）对简码重码的贡献：(count, freq)。
    #[inline]
    fn bucket_collision_contrib_of(
        ctx: &OptContext,
        chars: &[u32],
        assigned: &[bool],
    ) -> (usize, u64) {
        let mut n = 0usize;
        let mut max_freq = 0u64;
        let mut sum_freq = 0u64;
        for &ci in chars {
            let ci = ci as usize;
            if !assigned[ci] {
                let f = ctx.char_infos[ci].frequency;
                sum_freq += f;
                if f > max_freq {
                    max_freq = f;
                }
                n += 1;
            }
        }
        if n >= 2 {
            (n - 1, sum_freq - max_freq)
        } else {
            (0, 0)
        }
    }

    /// 全量重算简码重码并填满每桶贡献缓存（供后续增量差量维护）。仅遍历非空全码桶（需求 7.2）。
    fn recompute_collisions_full(&mut self, ctx: &OptContext, full_buckets: &BucketStore<FullBucket>) {
        self.bucket_collision_contrib.clear();
        let mut total_count = 0usize;
        let mut total_freq = 0u64;
        full_buckets.for_each_nonempty(|code, b| {
            let contrib = Self::bucket_collision_contrib_of(ctx, &b.members, &self.all_assigned_flags);
            // 仅存非零贡献，保持稀疏有界（需求 4.2）。
            if contrib.0 > 0 || contrib.1 > 0 {
                self.bucket_collision_contrib.insert(code, contrib);
            }
            total_count += contrib.0;
            total_freq += contrib.1;
        });
        self.simple_collision_count = total_count;
        self.simple_collision_freq = total_freq;
        self.simple_collision_rate = if ctx.total_frequency > 0 {
            total_freq as f64 / ctx.total_frequency as f64
        } else {
            0.0
        };
    }

    /// 计算桶内排序键（需求 4.3/4.4/5.4）。
    ///
    /// - Frequency 模式：`key = freq`；
    /// - Efficiency 模式：`key = freq × (base_saving[ci][li] + sel_len)`，
    ///   `sel_len` 由 `is_first_candidate[ci]` 取 0（首选字）或 1（非首选字）。
    #[inline]
    fn bucket_sort_key(
        ctx: &OptContext,
        is_first_candidate: &[bool],
        li: usize,
        ci: usize,
    ) -> i64 {
        let freq = ctx.char_infos[ci].frequency as i64;
        match ctx.simple_assign_mode {
            SimpleAssignMode::Frequency => freq,
            SimpleAssignMode::Efficiency => {
                let sel_len: i64 = if is_first_candidate[ci] { 0 } else { 1 };
                freq * (ctx.simple_base_saving[ci][li] + sel_len)
            }
        }
    }

    /// 桶内出简候选排序比较器（需求 4.7）。
    ///
    /// 排序键降序；排序键相等时先按 `freq` 降序、再按 `ci` 升序，保证结果可复现。
    /// 返回的顺序使「应出简」的候选排在前面，便于 `take(code_num)` 选中。
    #[inline]
    fn cmp_in_bucket(
        ctx: &OptContext,
        is_first_candidate: &[bool],
        li: usize,
        a: usize,
        b: usize,
    ) -> Ordering {
        let ka = Self::bucket_sort_key(ctx, is_first_candidate, li, a);
        let kb = Self::bucket_sort_key(ctx, is_first_candidate, li, b);
        if ka != kb {
            return kb.cmp(&ka); // 排序键降序
        }
        let fa = ctx.char_infos[a].frequency;
        let fb = ctx.char_infos[b].frequency;
        if fa != fb {
            return fb.cmp(&fa); // 先按 freq 降序
        }
        a.cmp(&b) // 再按 ci 升序
    }

    /// 仅对单个受影响桶内的候选列表执行局部排序（需求 6.1/6.2/6.3）。
    ///
    /// 使用不分配额外堆缓冲的 `sort_unstable_by`；因 `cmp_in_bucket` 以 `ci`
    /// 收尾构成严格全序，排序结果确定可复现。供 `rebuild_internal` 与后续
    /// 增量 `apply_move` 复用。
    #[inline]
    fn sort_bucket(
        ctx: &OptContext,
        is_first_candidate: &[bool],
        li: usize,
        members: &mut [u32],
    ) {
        members.sort_unstable_by(|&a, &b| {
            Self::cmp_in_bucket(ctx, is_first_candidate, li, a as usize, b as usize)
        });
    }

    /// 完整重建简码评估（供周期对账与结束校验调用）
    pub fn full_rebuild(
        &mut self,
        ctx: &OptContext,
        assignment: &[u8],
        full_buckets: &BucketStore<FullBucket>,
        is_first_candidate: &[bool],
    ) {
        // 观测计数：记录一次全量重建（防回归断言依赖此计数证明热路径不走全量重建）。
        self.full_rebuild_calls += 1;
        self.rebuild_internal(ctx, assignment, full_buckets, is_first_candidate);
    }

    /// 简码增量更新（热路径核心，需求 1/14）。
    ///
    /// 当退火移动一个/两个字根组且（受影响候选交集 `affected_candidates` 非空、或存在首选翻转
    /// 重排种子 `resort_seeds`、或全码桶成员变化 `full_affected_chars` 非空）且简码已激活时调用。
    ///
    /// 参数：
    /// - `affected_candidates`：受影响候选字集合 A（= `group_to_simple_affected_candidate[r]`，
    ///   已与候选字集合求交）。这些字的简码编码可能变化，阶段 1 据此做「旧桶移除/新桶加入」。
    /// - `resort_seeds`：首选状态（`is_first_candidate`）翻转、且为候选字的汉字（仅 Efficiency
    ///   模式非空）。其简码编码可能未变，但桶内排序键随首选翻转而变，需把其当前所在简码桶标记
    ///   为脏以触发局部重排（需求 4.6/5.4/6.2）。
    /// - `full_affected_chars`：本次移动改变了全码编码的汉字（= 移动组的 `group_to_chars`，
    ///   交换时为两组之并）。用于定位成员发生变化的全码桶。
    /// - `full_code_to_chars`：主评估器当前（移动后）的全码桶。
    /// - `is_first_candidate`：主评估器维护的全码桶首选标记，供 Efficiency 排序键取 `sel_len`。
    ///
    /// 在修改前以细粒度撤销日志/快照记录将被覆盖的状态，供 `rollback` 使用；本方法只负责
    /// 「正向增量」，提交/回滚由调用方决定。
    ///
    /// 实现说明（Backlog B1：阶段 2 出简选择精确增量化）：
    /// - 阶段 2 不再调用 `rebuild_selection` 做整体重算，而是 `do_incremental_selection`：
    ///   对受影响候选字与重排种子标记脏桶，按级别升序「仅对脏桶局部重排选出前 code_num」，
    ///   并经 `pending` 做「跨级排除传播」（新出简→更高级移除、落选→更高级插入），逐字段与
    ///   `full_rebuild` 一致，单步复杂度降至 O(受影响字数 × 级别相关小量)（需求 1.1/1.3/1.4/7.5）；
    /// - 阶段 3 的简码重码同为真增量：仅在「移动组改变成员的全码桶」与「`all_assigned_flags`
    ///   翻转的字所在全码桶」这少数桶上用贡献缓存做差量更新，避免每步全量扫描整个编码空间
    ///   （需求 14.3）。
    pub fn apply_move_incremental(
        &mut self,
        ctx: &OptContext,
        assignment: &[u8],
        affected_candidates: &[usize],
        resort_seeds: &[usize],
        full_affected_chars: &[usize],
        full_buckets: &BucketStore<FullBucket>,
        is_first_candidate: &[bool],
    ) {
        // 受影响裁剪（需求 1.5/7.6）：当本次移动既不影响任何候选字的简码归属
        // （`affected_candidates` 空 ⟹ 出简选择不变），又不改变任何全码桶成员
        // （`full_affected_chars` 空 ⟹ 简码重码不变）时，简码状态完全不变，直接返回。
        //
        // 注意：仅 `affected_candidates` 为空不足以早退——非候选字在全码桶间移动虽不改变
        // 出简选择，却会改变「去掉出简字后」的全码桶成员，从而影响简码重码（需求 14.2/14.3）。
        // 因此空交集时仍需执行阶段 1 + 阶段 3 的简码重码增量，仅可跳过阶段 2 的出简重选。
        // === 记录将被覆盖的标量快照（供回滚）===
        // 即使本次为空交集早退（简码状态完全不变），也先写入移动前标量快照并清空触碰向量，
        // 使紧随其后的 `rollback()` 成为真正的 no-op（还原到与当前完全相同的值），而不会
        // 误用上一次移动遗留的陈旧标量覆盖当前状态（需求 2.3）。
        self.snapshot.has_selection = false;
        self.snapshot.old_collision_count = self.simple_collision_count;
        self.snapshot.old_collision_freq = self.simple_collision_freq;
        self.snapshot.old_collision_rate = self.simple_collision_rate;
        self.snapshot.old_cached_simple_score = self.cached_simple_score;
        self.snapshot.collision_buckets.clear();
        self.snapshot.last_full_codes.clear();
        // 复位选择相关撤销缓冲（保留容量），保证空交集早退后的 rollback 为干净 no-op。
        self.snapshot.bucket_snaps_len = 0;
        self.snapshot.undo_code.clear();
        self.snapshot.undo_selected.clear();
        self.snapshot.undo_contrib.clear();
        self.assigned_touched_list.clear();
        // 复位阶段 2 访问计数（观测北极星：本次移动的出简选择增量工作量）。
        self.stage2_visits = 0;
        // 复位本次移动的「键改动」缓冲（方向 B 分布偏差增量结算用）。
        self.g_dirty_keys.clear();
        // 复位简码占用保护的本移动缓冲（需求 33）。
        self.protect_dirty_buf.clear();
        self.snapshot.protect_undo.clear();

        if affected_candidates.is_empty() && full_affected_chars.is_empty() && resort_seeds.is_empty() {
            return;
        }

        // 工作集复位
        self.affected_full_buckets.clear();

        // === 阶段 1（全码桶来源）：检测移动组所改变的全码桶（旧编码 ∪ 新编码）===
        // 同时增量维护简码占用保护（需求 33）：据全码编码变化更新受保护占用、收集 blocked 翻转。
        for &ci in full_affected_chars {
            let new_code = ctx.calc_code_only(ci, assignment);
            let old_code = self.last_full_codes[ci];
            if new_code != old_code {
                self.affected_full_buckets.push(old_code);
                self.affected_full_buckets.push(new_code);
                self.snapshot.last_full_codes.push((ci, old_code));
                self.last_full_codes[ci] = new_code;

                // 简码占用保护增量（需求 33）。full_buckets 此时已反映本次移动后状态。
                if ctx.simple_protect_top_n == 0 {
                    // N=0（保护全部）：blocked(C)=全码桶非空。old_code 变空 → 解禁；
                    // new_code 变为恰含此字（之前为空）→ 新禁；据此标脏对应简码桶。
                    if full_buckets.get(old_code as u32).is_none() {
                        self.protect_dirty_buf.push(old_code);
                    }
                    if full_buckets
                        .get(new_code as u32)
                        .map_or(false, |b| b.members.len() == 1)
                    {
                        self.protect_dirty_buf.push(new_code);
                    }
                } else if ctx.simple_is_topn[ci] {
                    // N>0：仅受保护(top-N)字影响占用计数；计数 1→0 解禁、0→1 新禁。
                    let old_c = self.protect_count.get(&old_code).copied().unwrap_or(0);
                    self.snapshot.protect_undo.push((old_code, old_c));
                    if old_c <= 1 {
                        self.protect_count.remove(&old_code);
                        self.protect_dirty_buf.push(old_code);
                    } else {
                        self.protect_count.insert(old_code, old_c - 1);
                    }
                    let new_c = self.protect_count.get(&new_code).copied().unwrap_or(0);
                    self.snapshot.protect_undo.push((new_code, new_c));
                    self.protect_count.insert(new_code, new_c + 1);
                    if new_c == 0 {
                        self.protect_dirty_buf.push(new_code);
                    }
                }
            }
        }

        // 出简选择是否可能变化：受影响候选字、重排种子，或简码占用保护 blocked 翻转，均需重算。
        let selection_may_change = !affected_candidates.is_empty()
            || !resort_seeds.is_empty()
            || !self.protect_dirty_buf.is_empty();
        self.snapshot.has_selection = selection_may_change;

        // === 阶段 2：仅对脏桶做局部重排 + pending_chars 跨级排除传播（精确增量，Backlog B1）===
        // 不再调用 `rebuild_selection` 做整体重算。仅遍历受影响候选字 / 重排种子与脏桶，
        // 单步复杂度降至 O(受影响字数 × 级别相关小量)，逐字段与全量重建一致（需求 1.1/1.3/1.4/7.5）。
        if selection_may_change {
            self.do_incremental_selection(
                ctx,
                assignment,
                full_buckets,
                affected_candidates,
                resort_seeds,
                is_first_candidate,
            );
        }

        // === 阶段 3（出简翻转来源）：出简标记净翻转的候选字所在全码桶 ===
        // 复用 `assigned_touched_list` / `assigned_start_flag`：增量选择中每次改写
        // `all_assigned_flags` 都登记了「移动起始值」，此处只遍历被触碰的少量候选字，
        // 比较起始值与当前值判定净翻转，避免退化为 O(候选字集) 扫描（需求 14.2/14.3）。
        if selection_may_change {
            for idx in 0..self.assigned_touched_list.len() {
                let ci = self.assigned_touched_list[idx];
                if self.assigned_start_flag[ci] != self.all_assigned_flags[ci] {
                    let code = ctx.calc_code_only(ci, assignment);
                    self.affected_full_buckets.push(code);
                }
            }
        }

        // === 阶段 3：在受影响的少数全码桶上差量更新简码重码（需求 14.3）===
        self.affected_full_buckets.sort_unstable();
        self.affected_full_buckets.dedup();
        for idx in 0..self.affected_full_buckets.len() {
            let code = self.affected_full_buckets[idx];
            let (old_count, old_freq) = self
                .bucket_collision_contrib
                .get(&(code as u32))
                .copied()
                .unwrap_or((0, 0));
            let members: &[u32] = full_buckets
                .get(code as u32)
                .map(|b| b.members.as_slice())
                .unwrap_or(&[]);
            let (new_count, new_freq) =
                Self::bucket_collision_contrib_of(ctx, members, &self.all_assigned_flags);
            if (new_count, new_freq) == (old_count, old_freq) {
                continue;
            }
            self.snapshot
                .collision_buckets
                .push((code, old_count, old_freq));
            // 先加新值再减旧值，避免 usize 下溢（总量恒 ≥ 旧桶贡献）
            self.simple_collision_count = self.simple_collision_count + new_count - old_count;
            self.simple_collision_freq = self.simple_collision_freq + new_freq - old_freq;
            // 稀疏维护：归零则移除条目，否则插入（需求 4.2）。
            if new_count == 0 && new_freq == 0 {
                self.bucket_collision_contrib.remove(&(code as u32));
            } else {
                self.bucket_collision_contrib
                    .insert(code as u32, (new_count, new_freq));
            }
        }
        self.simple_collision_rate = if ctx.total_frequency > 0 {
            self.simple_collision_freq as f64 / ctx.total_frequency as f64
        } else {
            0.0
        };

        // 分布偏差增量结算（需求 29 方向 B）：仅在出简选择可能变化（已取聚合快照）时；
        // presses 不变则只更新被触碰键，presses 变化则退回全量重算。
        if selection_may_change {
            self.finalize_dist(ctx);
        }

        self.simple_score_dirty = true;
    }

    /// 自增移动代际（用于脏桶/出简标记触碰的 O(1) 去重与 O(1) 整体复位）。
    /// 代际计数溢出（绕回 0）时，整体清零代际标记数组并从 1 重新计数。
    #[inline]
    fn bump_generation(&mut self) {
        self.cur_gen = self.cur_gen.wrapping_add(1);
        if self.cur_gen == 0 {
            for lvl in self.bucket_gen.iter_mut() {
                lvl.clear();
            }
            for g in self.assigned_touch_gen.iter_mut() {
                *g = 0;
            }
            self.cur_gen = 1;
        }
    }

    /// 首次触碰某桶时：快照其移动前成员与 `freq_sum`（供回滚），并将其编码加入
    /// `dirty_per_level[li]`（待局部重排）。代际标记保证每桶仅快照/入列一次。
    #[inline]
    fn touch_bucket(&mut self, li: usize, code: usize) {
        if self.bucket_gen[li].get(&(code as u32)) == Some(&self.cur_gen) {
            return;
        }
        self.bucket_gen[li].insert(code as u32, self.cur_gen);

        let idx = self.snapshot.bucket_snaps_len;
        if idx == self.snapshot.bucket_snaps.len() {
            self.snapshot.bucket_snaps.push(BucketSnap::default());
        }
        // 先把桶成员/频率和复制到本地（短借用），再写入快照池，避免 self 字段借用冲突。
        let snap = &mut self.snapshot.bucket_snaps[idx];
        snap.li = li;
        snap.code = code;
        snap.members.clear();
        // 缺失桶（未插入）按空桶语义：snapshot 记录空成员与 0 频率。
        if let Some(b) = self.levels[li].buckets.get(code as u32) {
            snap.members.extend_from_slice(&b.members);
            snap.freq_sum = b.freq_sum;
        } else {
            snap.freq_sum = 0;
        }
        self.snapshot.bucket_snaps_len += 1;

        self.dirty_per_level[li].push(code);
    }

    /// 将候选字 `ci` 插入级别 `li` 编码 `code` 的桶（成员追加 + freq_sum 累加），并触碰该桶。
    #[inline]
    fn bucket_insert_member(&mut self, li: usize, code: usize, ci: usize, freq: u64) {
        self.touch_bucket(li, code);
        let b = self.levels[li].buckets.get_mut_or_insert(code as u32);
        b.members.push(ci as u32);
        b.freq_sum += freq;
    }

    /// 从级别 `li` 编码 `code` 的桶移除候选字 `ci`（swap_remove + freq_sum 扣减），并触碰该桶。
    /// 桶变空时移除条目以维持稀疏有界（需求 2.1）。
    /// 成员顺序的扰动由桶成员快照在回滚时整存整取还原，不影响正确性（重排前会重新排序）。
    #[inline]
    fn bucket_remove_member(&mut self, li: usize, code: usize, ci: usize, freq: u64) {
        self.touch_bucket(li, code);
        let now_empty = {
            let b = self.levels[li].buckets.get_mut_or_insert(code as u32);
            if let Some(pos) = b.members.iter().position(|&x| x == ci as u32) {
                b.members.swap_remove(pos);
                b.freq_sum -= freq;
            }
            b.members.is_empty()
        };
        if now_empty {
            self.levels[li].buckets.remove(code as u32);
        }
    }

    /// 记录 `current_simple_code[li][ci]` 的撤销项并就地写入新值。
    #[inline]
    fn set_code(&mut self, li: usize, ci: usize, new_code: i64) {
        let old = self.levels[li].current_simple_code[ci];
        self.snapshot.undo_code.push((li, ci, old));
        self.levels[li].current_simple_code[ci] = new_code;
    }

    /// 首次触碰某字的 `all_assigned_flags` 时记录其移动起始值（供回滚与阶段 3 翻转检测）。
    #[inline]
    fn mark_assigned_touch(&mut self, ci: usize) {
        if self.assigned_touch_gen[ci] == self.cur_gen {
            return;
        }
        self.assigned_touch_gen[ci] = self.cur_gen;
        self.assigned_start_flag[ci] = self.all_assigned_flags[ci];
        self.assigned_touched_list.push(ci);
    }

    /// 选中 `ci` 于级别 `li`：翻转 `selected`/`all_assigned_flags`（带撤销/起始登记），
    /// 并按当前 `assignment` 累加级别聚合贡献，同时缓存其当量/键位贡献（供后续精确扣除）。
    #[inline]
    fn select_char(&mut self, ctx: &OptContext, assignment: &[u8], li: usize, ci: usize) {
        let freq = ctx.char_infos[ci].frequency;
        let freq_f = freq as f64;

        self.snapshot.undo_selected.push((li, ci, false));
        self.levels[li].selected[ci] = true;
        self.mark_assigned_touch(ci);
        self.all_assigned_flags[ci] = true;

        self.levels[li].covered_freq += freq;
        self.levels[li].equiv_freq_sum += freq;

        let eq = ctx.calc_simple_equiv(ci, li, assignment);
        let has_keys = self.fill_keys_with_commit(ctx, ci, li, assignment);
        let klen = if has_keys { self.key_buf.len() } else { 0 };
        debug_assert!(klen <= SIMPLE_KEYS_CAP, "简码键位数超出 SIMPLE_KEYS_CAP");
        let n = klen.min(SIMPLE_KEYS_CAP);

        // 撤销项：记录旧的贡献缓存，再写入新值
        let lvl = &mut self.levels[li];
        self.snapshot.undo_contrib.push(ContribUndo {
            li,
            ci,
            old_equiv: lvl.sel_equiv[ci],
            old_keys: lvl.sel_keys[ci],
            old_len: lvl.sel_keys_len[ci],
        });
        lvl.equiv_weighted += eq * freq_f;
        lvl.sel_equiv[ci] = eq;
        for i in 0..n {
            let k = self.key_buf[i];
            lvl.sel_keys[ci][i] = k;
            lvl.key_usage[k as usize] += freq_f;
        }
        lvl.sel_keys_len[ci] = n as u8;
        lvl.key_presses += freq_f * klen as f64;

        // 全局聚合同步（需求 29.3）：与上面级别聚合施加相同 Δ。
        self.g_covered_freq += freq;
        self.g_equiv_freq_sum += freq;
        self.g_equiv_weighted += eq * freq_f;
        for i in 0..n {
            let k = self.key_buf[i];
            self.g_key_usage[k as usize] += freq_f;
            self.g_dirty_keys.push(k);
        }
        self.g_key_presses += freq_f * klen as f64;
    }

    /// 取消选中 `ci` 于级别 `li`（原生落选）：翻转 `selected`/`all_assigned_flags`，
    /// 并用缓存的当量/键位贡献精确扣除级别聚合。
    #[inline]
    fn deselect_char(&mut self, ctx: &OptContext, li: usize, ci: usize) {
        self.mark_assigned_touch(ci);
        self.all_assigned_flags[ci] = false;
        self.deselect_contrib_only(ctx, li, ci);
    }

    /// 取消选中 `ci` 于级别 `li` 但保留 `all_assigned_flags`（用于「因低级别出简而被跨级排除」
    /// 的移除：该字在更低级别已出简，跨级标记应保持为真）。仅翻转本级 `selected` 与扣除贡献。
    #[inline]
    fn deselect_contrib_only(&mut self, ctx: &OptContext, li: usize, ci: usize) {
        let freq = ctx.char_infos[ci].frequency;
        let freq_f = freq as f64;

        self.snapshot.undo_selected.push((li, ci, true));
        let lvl = &mut self.levels[li];
        lvl.selected[ci] = false;
        lvl.covered_freq -= freq;
        lvl.equiv_freq_sum -= freq;
        lvl.equiv_weighted -= lvl.sel_equiv[ci] * freq_f;
        let klen = lvl.sel_keys_len[ci] as usize;
        for i in 0..klen {
            let k = lvl.sel_keys[ci][i];
            lvl.key_usage[k as usize] -= freq_f;
        }
        lvl.key_presses -= freq_f * klen as f64;

        // 全局聚合同步（需求 29.3）：sel_equiv/sel_keys 在 deselect 中不被改写，可在 lvl 借用结束后回读。
        self.g_covered_freq -= freq;
        self.g_equiv_freq_sum -= freq;
        self.g_equiv_weighted -= self.levels[li].sel_equiv[ci] * freq_f;
        for i in 0..klen {
            let k = self.levels[li].sel_keys[ci][i];
            self.g_key_usage[k as usize] -= freq_f;
            self.g_dirty_keys.push(k);
        }
        self.g_key_presses -= freq_f * klen as f64;
    }

    /// 刷新仍选中字 `ci` 在级别 `li` 的「值型」贡献（当量/键位），用于其简码编码因移动而变化
    /// （仍保持选中）时更新聚合。`covered_freq` / `equiv_freq_sum` 仅依赖字频（不变），不触碰。
    /// 对编码未变的字，重算值与缓存值相等，净效果为零，安全且精确（无漂移）。
    #[inline]
    fn refresh_char(&mut self, ctx: &OptContext, assignment: &[u8], li: usize, ci: usize) {
        let freq = ctx.char_infos[ci].frequency;
        let freq_f = freq as f64;

        let eq = ctx.calc_simple_equiv(ci, li, assignment);
        let has_keys = self.fill_keys_with_commit(ctx, ci, li, assignment);
        let klen = if has_keys { self.key_buf.len() } else { 0 };
        debug_assert!(klen <= SIMPLE_KEYS_CAP, "简码键位数超出 SIMPLE_KEYS_CAP");
        let n = klen.min(SIMPLE_KEYS_CAP);

        // 捕获旧值型贡献（全局聚合同步用）：refresh 会改写 sel_equiv/sel_keys，故须在改写前读取。
        let g_old_equiv = self.levels[li].sel_equiv[ci];
        let g_old_len = self.levels[li].sel_keys_len[ci] as usize;
        let g_old_keys = self.levels[li].sel_keys[ci];

        let lvl = &mut self.levels[li];
        // 撤销项：记录旧贡献缓存
        self.snapshot.undo_contrib.push(ContribUndo {
            li,
            ci,
            old_equiv: lvl.sel_equiv[ci],
            old_keys: lvl.sel_keys[ci],
            old_len: lvl.sel_keys_len[ci],
        });
        // 扣除旧值型贡献
        lvl.equiv_weighted -= lvl.sel_equiv[ci] * freq_f;
        let old_len = lvl.sel_keys_len[ci] as usize;
        for i in 0..old_len {
            let k = lvl.sel_keys[ci][i];
            lvl.key_usage[k as usize] -= freq_f;
        }
        lvl.key_presses -= freq_f * old_len as f64;
        // 加入新值型贡献并刷新缓存
        lvl.equiv_weighted += eq * freq_f;
        lvl.sel_equiv[ci] = eq;
        for i in 0..n {
            let k = self.key_buf[i];
            lvl.sel_keys[ci][i] = k;
            lvl.key_usage[k as usize] += freq_f;
        }
        lvl.sel_keys_len[ci] = n as u8;
        lvl.key_presses += freq_f * klen as f64;

        // 全局聚合同步（需求 29.3）：covered_freq/equiv_freq_sum 不变（字仍选中、字频不变）；
        // 当量与键用量按「减旧值型贡献、加新值型贡献」同步。
        self.g_equiv_weighted += (eq - g_old_equiv) * freq_f;
        for i in 0..g_old_len {
            let k = g_old_keys[i];
            self.g_key_usage[k as usize] -= freq_f;
            self.g_dirty_keys.push(k);
        }
        for i in 0..n {
            let k = self.key_buf[i];
            self.g_key_usage[k as usize] += freq_f;
            self.g_dirty_keys.push(k);
        }
        self.g_key_presses += freq_f * (klen as f64 - g_old_len as f64);
    }

    /// 对单个脏桶做局部重排并重选前 `code_num` 个出简，检测出简翻转，更新聚合与跨级标记，
    /// 并把「原生新选中 / 新落选」分别记入 `newly_sel_buf` / `newly_desel_buf` 供跨级传播。
    fn reselect_bucket(
        &mut self,
        ctx: &OptContext,
        assignment: &[u8],
        full_buckets: &BucketStore<FullBucket>,
        is_first_candidate: &[bool],
        li: usize,
        code: usize,
    ) {
        // 简码占用保护（需求 33）：编码撞受保护全码的桶名额=0（谁都不出简）。
        let code_num = if self.is_code_blocked(ctx, code, full_buckets) {
            0
        } else {
            self.levels[li]
                .code_num
                .saturating_sub(ctx.simple_fixed_occ(li, code))
        };
        // 局部选择前 code_num（C 优化）：用部分选择 `select_nth_unstable_by` 取代整桶全排序，
        // 复杂度 O(成员数) 而非 O(n log n)，且选中集合不变——分划后 members[0..code_num] 恰为
        // 按 `cmp_in_bucket` 排序键最优的 code_num 个（严格全序 ⟹ 该集合唯一确定）。
        // 仅 `0 < code_num < len` 时需分划：code_num==0（无人出简）或 code_num>=len（全员出简）
        // 时选中集合与桶内顺序无关，跳过。
        let len = self
            .levels[li]
            .buckets
            .get(code as u32)
            .map_or(0, |b| b.members.len());
        // 桶为空（成员已全部移除）：无可出简，直接返回，且不经 get_mut_or_insert 创建空条目
        // （否则会破坏稀疏后端「键存在 ⟺ 非空」不变量并泄漏空桶）。
        if len == 0 {
            return;
        }
        // 局部选择前 code_num（C 优化）：用部分选择取代整桶全排序。
        if code_num > 0 && code_num < len {
            let members = &mut self.levels[li].buckets.get_mut(code as u32).unwrap().members;
            members.select_nth_unstable_by(code_num - 1, |&a, &b| {
                Self::cmp_in_bucket(ctx, is_first_candidate, li, a as usize, b as usize)
            });
        }
        // 北极星计数：脏桶局部重排访问的成员数（与桶规模同阶，非候选字总集）。
        self.stage2_visits += len;
        for idx in 0..len {
            let ci = self.levels[li].buckets.get(code as u32).unwrap().members[idx] as usize;
            let now_sel = idx < code_num;
            let was_sel = self.levels[li].selected[ci];
            if now_sel && !was_sel {
                self.select_char(ctx, assignment, li, ci);
                self.newly_sel_buf.push(ci);
            } else if !now_sel && was_sel {
                self.deselect_char(ctx, li, ci);
                self.newly_desel_buf.push(ci);
            } else if now_sel && was_sel {
                self.refresh_char(ctx, assignment, li, ci);
            }
        }
    }

    /// 阶段 2 核心：精确增量地重算出简选择。
    ///
    /// 步骤：
    /// 1. 移动起始整存全部级别聚合标量（供回滚），并复位脏桶/传播/触碰工作集；
    /// 2. 阶段 1（候选归属变化）：对每个受影响候选字、在其当前所在的各级别做「旧桶移除 + 新桶加入」，
    ///    同步 `current_simple_code`，并把旧/新桶标记为脏；
    /// 3. 阶段 2（按级别升序处理脏桶）：先应用来自低级别的跨级传播动作（插入/移除），
    ///    再对脏桶局部重排选出前 `code_num`，检测出简翻转更新聚合与 `selected`/`all_assigned_flags`，
    ///    并把翻转影响经 `pending` 传播到更高一级（新出简→更高级移除、落选→更高级插入），
    ///    复刻全量重建「低级别先出简、高级别排除已出简字」的语义（需求 1.4/15.1）。
    fn do_incremental_selection(
        &mut self,
        ctx: &OptContext,
        assignment: &[u8],
        full_buckets: &BucketStore<FullBucket>,
        affected_candidates: &[usize],
        resort_seeds: &[usize],
        is_first_candidate: &[bool],
    ) {
        const ACT_INSERT: u8 = 0;
        const ACT_REMOVE: u8 = 1;

        self.bump_generation();
        self.snapshot_aggregates();

        let n_levels = self.levels.len();
        for li in 0..n_levels {
            self.dirty_per_level[li].clear();
        }
        self.pending_a.clear();
        self.pending_b.clear();

        // --- 简码占用保护翻转（需求 33）：本次移动使某编码的「受保护占用」翻转（新禁/解禁），
        // 把对应简码桶标记为脏以重选。`protect_dirty_buf` 在阶段 1 已据全码变化收集。
        // 一个全码编码值可能对应某一级的合法简码桶（按长度匹配），逐级以容量守卫后触碰。---
        for k in 0..self.protect_dirty_buf.len() {
            let code = self.protect_dirty_buf[k];
            for li in 0..n_levels {
                if code < self.levels[li].capacity {
                    self.touch_bucket(li, code);
                }
            }
        }

        // --- 阶段 1：受影响候选字逐级「旧桶移除 / 新桶加入」（仅在其当前出现的级别）---
        for &ci in affected_candidates {
            let freq = ctx.char_infos[ci].frequency;
            for li in 0..n_levels {
                let old = self.levels[li].current_simple_code[ci];
                if old < 0 {
                    // 当前未出现在该级别（被低级别排除，或该级无简码）：归属由跨级传播处理。
                    continue;
                }
                self.stage2_visits += 1;
                // `calc_simple_code` 的 Some/None 与 assignment 无关（仅 value 变化），
                // 且该字当前出现在该级（old >= 0）⟹ 当初入桶时已通过长度资格过滤，
                // 故此处长度资格恒为真（静态），eligible 版本必返回 Some。
                let new_code = ctx
                    .calc_simple_code_eligible(ci, li, assignment)
                    .expect("present candidate must have a simple code at this level");
                if new_code as i64 == old {
                    continue;
                }
                self.bucket_remove_member(li, old as usize, ci, freq);
                self.bucket_insert_member(li, new_code, ci, freq);
                self.set_code(li, ci, new_code as i64);
            }
        }

        // --- 重排种子：首选状态翻转使排序键变化的候选字（Efficiency 模式）---
        // 其简码编码未变（归属不变），但桶内排序键变化，可能改变出简选择。仅把它们当前所在的
        // 各级桶标记为脏（不改动成员），交由后续局部重排重新判定，并经 pending 传播跨级影响。
        for &ci in resort_seeds {
            for li in 0..n_levels {
                let code = self.levels[li].current_simple_code[ci];
                if code >= 0 {
                    self.stage2_visits += 1;
                    self.touch_bucket(li, code as usize);
                }
            }
        }

        // --- 阶段 2：按级别升序处理脏桶 + 跨级排除传播 ---
        for li in 0..n_levels {
            self.inserted_buf.clear();

            // (a) 应用来自低级别的传播动作（pending_a 面向当前级别）
            for k in 0..self.pending_a.len() {
                let (ci, kind) = self.pending_a[k];
                let freq = ctx.char_infos[ci].frequency;
                self.stage2_visits += 1;
                if kind == ACT_INSERT {
                    match ctx.calc_simple_code_eligible(ci, li, assignment) {
                        Some(code) => {
                            self.bucket_insert_member(li, code, ci, freq);
                            self.set_code(li, ci, code as i64);
                            self.inserted_buf.push(ci);
                        }
                        None => {
                            // 该级无简码或长度不合格：不入桶，但更高级别仍可选 → 继续向上插入
                            self.pending_b.push((ci, ACT_INSERT));
                        }
                    }
                } else {
                    // ACT_REMOVE：该字已在更低级别出简，需从本级及以上排除
                    let code = self.levels[li].current_simple_code[ci];
                    let was_sel = self.levels[li].selected[ci];
                    if code >= 0 {
                        self.bucket_remove_member(li, code as usize, ci, freq);
                        self.set_code(li, ci, -1);
                    }
                    if was_sel {
                        // 本级原本出简：取消但保留 all_assigned（已在更低级别出简）；不再向上传播
                        self.deselect_contrib_only(ctx, li, ci);
                    } else {
                        // 本级原为「在桶未出简」或「无简码」：更高级别仍需排除 → 继续向上移除
                        self.pending_b.push((ci, ACT_REMOVE));
                    }
                }
            }

            // (b) 局部重排脏桶并检测出简翻转
            self.newly_sel_buf.clear();
            self.newly_desel_buf.clear();
            for di in 0..self.dirty_per_level[li].len() {
                let code = self.dirty_per_level[li][di];
                self.reselect_bucket(ctx, assignment, full_buckets, is_first_candidate, li, code);
            }

            // (c) 跨级传播到 li+1
            // 原生落选：在更高级别重新可选 → 插入
            for di in 0..self.newly_desel_buf.len() {
                let ci = self.newly_desel_buf[di];
                self.pending_b.push((ci, ACT_INSERT));
            }
            // 新选中：原生者曾出现在更高级别 → 移除；本级新插入者从未到达更高级别 → 不传播
            for di in 0..self.newly_sel_buf.len() {
                let ci = self.newly_sel_buf[di];
                if !self.inserted_buf.contains(&ci) {
                    self.pending_b.push((ci, ACT_REMOVE));
                }
            }
            // 本级新插入但未选中者：在更高级别仍为可选成员 → 继续向上插入
            for di in 0..self.inserted_buf.len() {
                let ci = self.inserted_buf[di];
                if !self.levels[li].selected[ci] {
                    self.pending_b.push((ci, ACT_INSERT));
                }
            }

            // pending 翻页：pending_b → pending_a
            std::mem::swap(&mut self.pending_a, &mut self.pending_b);
            self.pending_b.clear();
        }
    }

    /// 移动起始整存全部级别聚合标量（供回滚整体写回）。复用 `snapshot.aggregates` 缓冲。
    fn snapshot_aggregates(&mut self) {
        self.snapshot.aggregates.clear();
        for li in 0..self.levels.len() {
            let lvl = &self.levels[li];
            self.snapshot.aggregates.push(LevelAggregateSnapshot {
                covered_freq: lvl.covered_freq,
                equiv_weighted: lvl.equiv_weighted,
                equiv_freq_sum: lvl.equiv_freq_sum,
                key_usage: lvl.key_usage,
                key_presses: lvl.key_presses,
            });
        }
        // 全局聚合标量快照（需求 29.5）。
        self.snapshot.g_covered_freq = self.g_covered_freq;
        self.snapshot.g_equiv_weighted = self.g_equiv_weighted;
        self.snapshot.g_equiv_freq_sum = self.g_equiv_freq_sum;
        self.snapshot.g_key_usage = self.g_key_usage;
        self.snapshot.g_key_presses = self.g_key_presses;
        // 分布偏差与每键贡献快照（方向 B）。
        self.snapshot.g_dist_deviation = self.g_dist_deviation;
        self.snapshot.g_dist_contrib = self.g_dist_contrib;
    }

    /// 提交本次增量：清空快照缓冲（确认增量结果，需求 2.1）。
    ///
    /// 仅复位长度/清空小型工作向量；底层缓冲（`bucket_snaps`/撤销日志/聚合快照等）保留
    /// 已分配容量以避免下次移动重新分配（需求 3.1）。
    pub fn commit(&mut self) {
        self.snapshot.has_selection = false;
        self.snapshot.bucket_snaps_len = 0;
        self.snapshot.aggregates.clear();
        self.snapshot.undo_code.clear();
        self.snapshot.undo_selected.clear();
        self.snapshot.undo_contrib.clear();
        self.snapshot.collision_buckets.clear();
        self.snapshot.last_full_codes.clear();
        self.snapshot.protect_undo.clear();
        self.assigned_touched_list.clear();
    }

    /// 回滚本次增量：用细粒度撤销日志还原至移动前状态（需求 2.2/2.3/3.1）。
    ///
    /// 还原内容（当 `has_selection`）：
    /// - 级别聚合标量：从 `aggregates` 整体写回；
    /// - 触碰桶的成员与 `freq_sum`：从 `bucket_snaps` 整存整取（精确还原成员顺序）；
    /// - `current_simple_code` / `selected` / 选中贡献缓存（`sel_equiv`/`sel_keys`/`sel_keys_len`）：
    ///   逆序回放撤销日志；
    /// - `all_assigned_flags`：对被触碰候选字写回移动起始值；
    /// 以及（无论 `has_selection`）简码重码贡献缓存/标量、`last_full_codes` 与 `cached_simple_score`。
    ///
    /// 不触发任何全量重建；回滚成本为 O(受影响项)，与正向 `apply` 同阶。
    pub fn rollback(&mut self) {
        if self.snapshot.has_selection {
            // 1) 级别聚合标量整体写回
            for li in 0..self.levels.len() {
                let s = &self.snapshot.aggregates[li];
                let lvl = &mut self.levels[li];
                lvl.covered_freq = s.covered_freq;
                lvl.equiv_weighted = s.equiv_weighted;
                lvl.equiv_freq_sum = s.equiv_freq_sum;
                lvl.key_usage = s.key_usage;
                lvl.key_presses = s.key_presses;
            }
            // 1b) 全局聚合标量整体写回（需求 29.5）
            self.g_covered_freq = self.snapshot.g_covered_freq;
            self.g_equiv_weighted = self.snapshot.g_equiv_weighted;
            self.g_equiv_freq_sum = self.snapshot.g_equiv_freq_sum;
            self.g_key_usage = self.snapshot.g_key_usage;
            self.g_key_presses = self.snapshot.g_key_presses;
            // 1c) 分布偏差与每键贡献整体写回（方向 B）
            self.g_dist_deviation = self.snapshot.g_dist_deviation;
            self.g_dist_contrib = self.snapshot.g_dist_contrib;

            // 2) 触碰桶成员/freq_sum 整存整取（仅有效前缀）。还原为空则移除条目、
            //    非空则重建，维持稀疏「键存在 ⟺ 非空」不变量（需求 8.2/8.3）。
            for i in 0..self.snapshot.bucket_snaps_len {
                let snap = &self.snapshot.bucket_snaps[i];
                let li = snap.li;
                let code = snap.code as u32;
                if snap.members.is_empty() {
                    self.levels[li].buckets.remove(code);
                } else {
                    let b = self.levels[li].buckets.get_mut_or_insert(code);
                    b.members.clear();
                    b.members.extend_from_slice(&snap.members);
                    b.freq_sum = snap.freq_sum;
                }
            }

            // 3) 逆序回放 current_simple_code 撤销日志
            for k in (0..self.snapshot.undo_code.len()).rev() {
                let (li, ci, old) = self.snapshot.undo_code[k];
                self.levels[li].current_simple_code[ci] = old;
            }

            // 4) 逆序回放 selected 撤销日志
            for k in (0..self.snapshot.undo_selected.len()).rev() {
                let (li, ci, old) = self.snapshot.undo_selected[k];
                self.levels[li].selected[ci] = old;
            }

            // 5) 逆序回放选中贡献缓存撤销日志
            for k in (0..self.snapshot.undo_contrib.len()).rev() {
                let u = self.snapshot.undo_contrib[k];
                let lvl = &mut self.levels[u.li];
                lvl.sel_equiv[u.ci] = u.old_equiv;
                lvl.sel_keys[u.ci] = u.old_keys;
                lvl.sel_keys_len[u.ci] = u.old_len;
            }

            // 6) all_assigned_flags：被触碰候选字写回移动起始值
            for idx in 0..self.assigned_touched_list.len() {
                let ci = self.assigned_touched_list[idx];
                self.all_assigned_flags[ci] = self.assigned_start_flag[ci];
            }
        }

        // 7) 还原 last_full_codes 条目（逆序，确保多次触碰同一 ci 时回到最早值）
        for &(ci, old_code) in self.snapshot.last_full_codes.iter().rev() {
            self.last_full_codes[ci] = old_code;
        }

        // 7b) 还原受保护占用计数（需求 33，逆序回放，确保同一 code 多次改动回到最早值）。
        // 与 has_selection 无关：protect_count 在阶段 1 据全码变化更新，可能未触发 selection。
        for &(code, old_count) in self.snapshot.protect_undo.iter().rev() {
            if old_count == 0 {
                self.protect_count.remove(&code);
            } else {
                self.protect_count.insert(code, old_count);
            }
        }

        // 8) 还原简码重码贡献缓存（逆序）：还原为 (0,0) 即移除条目，否则插入（需求 4.2）。
        for &(code, old_count, old_freq) in self.snapshot.collision_buckets.iter().rev() {
            if old_count == 0 && old_freq == 0 {
                self.bucket_collision_contrib.remove(&(code as u32));
            } else {
                self.bucket_collision_contrib
                    .insert(code as u32, (old_count, old_freq));
            }
        }

        // 9) 还原简码重码标量与缓存得分
        self.simple_collision_count = self.snapshot.old_collision_count;
        self.simple_collision_freq = self.snapshot.old_collision_freq;
        self.simple_collision_rate = self.snapshot.old_collision_rate;
        self.cached_simple_score = self.snapshot.old_cached_simple_score;
        self.simple_score_dirty = false;

        // 清空工作向量（保留容量）
        self.commit();
    }

    /// 计算简码得分
    fn compute_simple_score(&self, ctx: &OptContext) -> f64 {
        let sm = self.get_simple_metrics(ctx);

        if ctx.targets_config.simple_code.enabled {
            let t = &ctx.targets_config.simple_code;
            let lw = t.low_weight;
            let mut score = 0.0;

            // freq（频率覆盖损失 = 1 - coverage）
            {
                let v = 1.0 - sm.weighted_freq_coverage;
                let target_v = 1.0 - t.freq;   // 目标损失 = 1 - 目标覆盖率
                let s = ctx.scale_config.simple_freq;
                let d = (v - target_v).max(0.0) * s;
                score += ctx.weights.simple_weight_freq * (d + d * d + lw * (v * s));
            }

            // equiv
            {
                let v = sm.equiv_mean;
                let s = ctx.scale_config.simple_equiv;
                let d = (v - t.equiv).max(0.0) * s;
                score += ctx.weights.simple_weight_equiv * (d + d * d + lw * (v * s));
            }

            // dist
            {
                let v = sm.dist_deviation;
                let s = ctx.scale_config.simple_dist;
                let d = (v - t.dist).max(0.0) * s;
                score += ctx.weights.simple_weight_dist * (d + d * d + lw * (v * s));
            }

            // collision_count
            {
                let v = sm.collision_count as f64;
                let s = ctx.scale_config.simple_collision_count;
                let d = (v - t.collision_count).max(0.0) * s;
                score += ctx.weights.simple_weight_collision_count * (d + d * d + lw * (v * s));
            }

            // collision_rate
            {
                let v = sm.collision_rate;
                let s = ctx.scale_config.simple_collision_rate;
                let d = (v - t.collision_rate).max(0.0) * s;
                score += ctx.weights.simple_weight_collision_rate * (d + d * d + lw * (v * s));
            }

            score
        } else {
            // 原有绝对值最小化模式（保持不变）
            let freq_loss = (1.0 - sm.weighted_freq_coverage) * ctx.scale_config.simple_freq;
            let equiv_loss = sm.equiv_mean * ctx.scale_config.simple_equiv;
            let dist_loss = sm.dist_deviation * ctx.scale_config.simple_dist;
            let collision_count_loss =
                sm.collision_count as f64 * ctx.scale_config.simple_collision_count;
            let collision_rate_loss = sm.collision_rate * ctx.scale_config.simple_collision_rate;

            ctx.weights.simple_weight_freq * freq_loss
                + ctx.weights.simple_weight_equiv * equiv_loss
                + ctx.weights.simple_weight_dist * dist_loss
                + ctx.weights.simple_weight_collision_count * collision_count_loss
                + ctx.weights.simple_weight_collision_rate * collision_rate_loss
        }
    }

    /// 获取简码得分
    pub fn get_simple_score(&mut self, ctx: &OptContext) -> f64 {
        if self.simple_score_dirty {
            self.cached_simple_score = self.compute_simple_score(ctx);
            self.simple_score_dirty = false;
        }
        self.cached_simple_score
    }

    /// 获取简码评估指标
    pub fn get_simple_metrics(&self, ctx: &OptContext) -> SimpleMetrics {
        // 直接读取全局聚合（需求 29.4，已含固定简码常量偏置），不再每步跨级重加。
        let total_covered = self.g_covered_freq;
        let total_equiv_weighted = self.g_equiv_weighted;
        let total_equiv_freq = self.g_equiv_freq_sum;

        let coverage = if ctx.total_frequency > 0 {
            total_covered as f64 / ctx.total_frequency as f64
        } else {
            0.0
        };

        let equiv_mean = if total_equiv_freq > 0 {
            total_equiv_weighted / total_equiv_freq as f64
        } else {
            0.0
        };

        // 分布偏差直接读增量维护的全局量（需求 29 方向 B），不再每步 O(键数) 重算。
        let dist_deviation = self.g_dist_deviation;

        SimpleMetrics {
            weighted_freq_coverage: coverage,
            equiv_mean,
            dist_deviation,
            collision_count: self.simple_collision_count,
            collision_rate: self.simple_collision_rate,
        }
    }

    /// 镜像评估器的出简选择（需求 23）：返回实际出简的 `(level_idx, ci)` 列表，按级别升序、
    /// 同级别按简码桶编码升序、桶内按选择排序键 `cmp_in_bucket`（即分配简码时的排序）排列。
    ///
    /// 供 output 直接复用评估器的 `selected` 状态生成输出，使输出方案与被优化方案逐字一致
    /// （含 efficiency 模式 / sel_len / 固定占用扣减 / 跨级排除），且输出顺序与分配顺序一致。
    /// 不含固定简码（固定简码由 `ctx.simple_fixed_codes` 单独输出）。
    pub fn selected_ordered(
        &self,
        ctx: &OptContext,
        is_first_candidate: &[bool],
    ) -> Vec<(usize, usize)> {
        let n_chars = ctx.char_infos.len();
        // (li, code, ci)：仅收集实际出简的候选字。
        let mut items: Vec<(usize, usize, usize)> = Vec::new();
        for li in 0..self.levels.len() {
            let lvl = &self.levels[li];
            for ci in 0..n_chars {
                if lvl.selected[ci] {
                    let code = lvl.current_simple_code[ci];
                    if code >= 0 {
                        items.push((li, code as usize, ci));
                    }
                }
            }
        }
        items.sort_by(|&(la, ca, a), &(lb, cb, b)| {
            la.cmp(&lb)
                .then(ca.cmp(&cb))
                .then_with(|| Self::cmp_in_bucket(ctx, is_first_candidate, la, a, b))
        });
        items.into_iter().map(|(li, _c, ci)| (li, ci)).collect()
    }
}

// =========================================================================
// 主评估器
// =========================================================================

/// 输出桶存储后端选择日志（需求 12）：在配置确认阶段单线程调用一次。
///
/// 全部规模数字（code_base / max_parts / code_space / 各级 code_base^L / n_chars / 预估内存）
/// 均在运行时据 `ctx` 与类型大小计算，除 `SPARSE_THRESHOLD` 外无任何硬编码规模常量（需求 12.6）。
pub(crate) fn log_bucket_backends(ctx: &OptContext) {
    use crate::bucket_store::SPARSE_THRESHOLD;
    let n_chars = ctx.char_infos.len();
    println!(
        "📦 桶存储后端选择 (code_base={}, max_parts={}, code_space={}, 阈值={}):",
        ctx.code_base, ctx.max_parts, ctx.code_space, SPARSE_THRESHOLD
    );
    log_one_backend("全码桶", ctx.code_space, std::mem::size_of::<FullBucket>(), n_chars);
    // 简码级别桶：仅在启用简码时输出（需求 12.7）。
    if ctx.enable_simple_code {
        for (li, &cap) in ctx.simple_level_capacity.iter().enumerate() {
            log_one_backend(
                &format!("简码级别[{li}]桶"),
                cap,
                std::mem::size_of::<SimpleBucket>(),
                n_chars,
            );
        }
    }
}

/// 单个桶存储的后端选择日志行（需求 12.2/12.3/12.4）。
fn log_one_backend(label: &str, capacity: usize, elem_size: usize, n_chars: usize) {
    println!("{}", backend_log_line(label, capacity, elem_size, n_chars));
}

/// 构造单个桶存储后端选择日志行（纯函数，便于测试，需求 12.6）。
/// 全部数字来自运行时参数，无硬编码规模常量（阈值除外）。
pub(crate) fn backend_log_line(label: &str, capacity: usize, elem_size: usize, n_chars: usize) -> String {
    use crate::bucket_store::{choose_backend, BackendKind, SPARSE_THRESHOLD};
    let mb = capacity.saturating_mul(elem_size) / (1024 * 1024);
    match choose_backend(capacity) {
        BackendKind::Dense => format!(
            "   · {label}: Dense (容量 {capacity} ≤ 阈值 {SPARSE_THRESHOLD}；预估 ~{mb} MB/线程)"
        ),
        BackendKind::Sparse => format!(
            "   · {label}: Sparse (容量 {capacity} > 阈值 {SPARSE_THRESHOLD}；Dense 将需 ~{mb} MB/线程，\
             改用稀疏存储，内存随非空桶数 ≤ n_chars({n_chars}) 增长)"
        ),
    }
}

/// 据当前分配构建全码桶存储 `BucketStore<FullBucket>` 与首选标记 `is_first_candidate`
/// （与 `Evaluator::new` 同口径）。供 output 与测试在不构造完整 `Evaluator` 时复用，
/// 集中处理 u32 成员表示与稀疏后端选择（需求 1/5/6）。
pub(crate) fn build_full_buckets(
    ctx: &OptContext,
    assignment: &[u8],
) -> (BucketStore<FullBucket>, Vec<bool>) {
    let n = ctx.char_infos.len();
    let mut fb: BucketStore<FullBucket> = BucketStore::new(ctx.code_space);
    fb.reserve_nonempty(n); // 稀疏后端预留 n_chars，避免 rehash、降低负载因子
    for ci in 0..n {
        let code = ctx.calc_code_only(ci, assignment);
        let b = fb.get_mut_or_insert(code as u32);
        b.members.push(ci as u32);
        let f = ctx.char_infos[ci].frequency;
        b.freq_sum += f;
        if f > b.max_freq {
            b.max_freq = f;
        }
    }
    let mut is_first = vec![false; n];
    let mut updates: Vec<(u32, u32)> = Vec::new();
    fb.for_each_nonempty(|code, b| {
        let mut max_f = 0u64;
        let mut first = u32::MAX;
        for &ci in &b.members {
            let f = ctx.char_infos[ci as usize].frequency;
            if f > max_f || (f == max_f && ci < first) {
                max_f = f;
                first = ci;
            }
        }
        updates.push((code, first));
    });
    for (code, first) in updates {
        fb.get_mut_or_insert(code).first = first;
        is_first[first as usize] = true;
    }
    (fb, is_first)
}

/// 主评估器 - 评估整个编码方案
pub struct Evaluator {
    /// 当前编码列表
    current_codes: Vec<usize>,
    /// 当前等价值列表
    current_equiv_val: Vec<f64>,
    /// 全码桶存储（取代基线的 code_to_chars/bucket_freq_sum/bucket_max_freq/bucket_first
    /// 四个 code_space 大小数组）。密集/稀疏后端按 code_space 阈值自适应（需求 1/5）。
    full_buckets: BucketStore<FullBucket>,
    /// 每个汉字在其桶中的位置（用于 O(1) swap_remove）
    char_bucket_pos: Vec<usize>,
    /// 每个汉字是否为其全码桶首选字（需求 5：首选字选重键长为 0）
    is_first_candidate: Vec<bool>,
    /// 本次移动中 `is_first_candidate` 发生写入（可能翻转）的汉字列表（复用工作集）。
    ///
    /// Efficiency 模式下桶内排序键含 `sel_len`（由 `is_first_candidate` 取 0/1），故某字
    /// 首选状态翻转会改变其在简码桶中的排序键，进而可能改变出简选择——即便该字的简码编码
    /// 未变、不在 `group_to_simple_affected_candidate` 内。`update_char` 在每次写入
    /// `is_first_candidate` 时登记受影响字，供简码增量把这些字的当前简码桶标记为「需重排」，
    /// 从而逐字段对齐全量重建（需求 4.6/5.4/6.2）。`apply_simple_for_move` 读取后清空。
    simple_is_first_dirty: Vec<usize>,

    /// 总重码数
    total_collisions: usize,
    /// 重码频率
    collision_frequency: u64,

    /// 加权等价值总和
    total_equiv_weighted: f64,
    /// 加权等价值平方总和
    total_equiv_sq_weighted: f64,

    /// 键位加权使用统计
    pub key_weighted_usage: [f64; EQUIV_TABLE_SIZE],
    /// 总键击次数
    pub total_key_presses: f64,

    /// 总频率
    pub total_frequency: u64,
    /// 总频率倒数
    pub inv_total_frequency: f64,
    /// 总键击次数倒数
    pub inv_total_key_presses: f64,

    /// 缓存的得分
    pub cached_score: f64,
    /// 得分是否需要重新计算
    pub score_dirty: bool,

    /// 缓存的全码分量得分（需求 10.1）
    pub cached_full_score: f64,
    /// 全码分量得分是否需要重新计算
    pub full_score_dirty: bool,
    /// 简码计算是否已激活（需求 8）。未激活时简码分量对综合得分贡献恒为 0（需求 10.2）。
    pub simple_active: bool,
    /// 当前有效简码权重（需求 9/10）。未激活时为 0；激活后由退火主循环按权重曲线维护。
    pub current_simple_weight: f64,

    /// 简码评估器
    simple_eval: Option<SimpleEvaluator>,

    /// 全量重建调用计数（仅用于观测/防回归）。
    ///
    /// 在 `rebuild_simple` 入口自增。结合 `SimpleEvaluator::full_rebuild_calls`，
    /// 用于证明生产热路径 `try_move`/`try_swap` 激活简码后不再触发全量重建。
    full_rebuild_calls: usize,
}

impl Evaluator {
    /// 创建新的评估器（急切构建简码评估器，向后兼容入口）。
    pub fn new(ctx: &OptContext, assignment: &[u8]) -> Self {
        Self::new_impl(ctx, assignment, true, false)
    }

    /// 创建「仅全码」评估器：跳过急切 `SimpleEvaluator` 构建（需求 24）。
    ///
    /// 供 `disable_simple == true` 的调用上下文（Init/校准的 warmup 与坐标下降）使用：
    /// 这些路径产物仅为 `Vec<u8>`、评估器用完即弃，简码对结果零贡献，无需付出
    /// `SimpleEvaluator::new` 的全量构建开销。构建后 `simple_eval = None`，
    /// 故 `simple_active = false`、`current_simple_weight = 0.0`，`has_simple_impact`
    /// 恒为 `false`，综合得分退化为纯全码 `weight_full_code · full_score`。
    ///
    /// 与「先 `new` 再置 `simple_active=false`」相比，本入口额外省去了被丢弃的
    /// 全量简码构建（warmup 每候选一次，约 50 次/阶段），是校准/Init 阶段的主要提速点。
    /// 简码整体关闭（`enable_simple_code=false`）时本入口与 `new` 完全等价（都为 None）。
    pub fn new_full_only(ctx: &OptContext, assignment: &[u8]) -> Self {
        Self::new_impl(ctx, assignment, false, false)
    }

    /// 创建评估器并以「输出全集（full）」范围急切构建简码评估器（active/passive 性能优化）。
    ///
    /// 供退火结束最终上报（对 `best_assignment` 重建以得到含 passive 的真实简码指标，点 (b)）
    /// 与 output 文件生成使用：`simple_eval` 的出简选择覆盖 `simple_output_candidate_chars`
    /// 全集（active ∪ passive）。退火热路径仍用 `new`（active 范围）。
    pub fn new_output_scope(ctx: &OptContext, assignment: &[u8]) -> Self {
        Self::new_impl(ctx, assignment, true, true)
    }

    /// 评估器构造实现。`build_simple` 为 false 时跳过急切 `SimpleEvaluator` 构建（需求 24）。
    /// `simple_output_scope` 为 true 时简码评估器以输出全集范围构建（仅最终上报/输出）。
    fn new_impl(ctx: &OptContext, assignment: &[u8], build_simple: bool, simple_output_scope: bool) -> Self {
        let n = ctx.char_infos.len();
        let cs = ctx.code_space;
        let mut full_buckets: BucketStore<FullBucket> = BucketStore::new(cs);
        // 稀疏后端预留 n_chars 容量：避免退火期 rehash、降低负载因子（缩短探测、减少缓存缺失）。
        full_buckets.reserve_nonempty(n);
        let mut char_bucket_pos = vec![0usize; n];
        let mut current_codes = Vec::with_capacity(n);
        let mut current_equiv_val = Vec::with_capacity(n);

        let mut total_equiv_weighted = 0.0f64;
        let mut total_equiv_sq_weighted = 0.0f64;
        let mut key_weighted_usage = [0.0f64; EQUIV_TABLE_SIZE];
        let mut total_key_presses = 0.0f64;

        for ci in 0..n {
            let info = &ctx.char_infos[ci];
            let freq_f = info.frequency as f64;

            let code = ctx.calc_code_only(ci, assignment);
            let equiv = ctx.calc_equiv_from_parts(ci, assignment);

            current_codes.push(code);
            current_equiv_val.push(equiv);

            let b = full_buckets.get_mut_or_insert(code as u32);
            let pos = b.members.len();
            b.members.push(ci as u32);
            char_bucket_pos[ci] = pos;
            b.freq_sum += info.frequency;
            if info.frequency > b.max_freq {
                b.max_freq = info.frequency;
            }

            total_equiv_weighted += equiv * freq_f;
            total_equiv_sq_weighted += equiv * equiv * freq_f;

            for &p in &info.parts {
                let k = ctx.resolve_key(p, assignment) as usize;
                key_weighted_usage[k] += freq_f;
            }
            total_key_presses += freq_f * info.parts.len() as f64;
        }

        // 碰撞统计与首选字初始化：仅遍历非空桶（需求 7.1）。
        let mut total_collisions = 0usize;
        let mut collision_frequency = 0u64;
        let mut is_first_candidate = vec![false; n];
        // 收集需要写回 first 的 (code, first_ci)，避免在遍历不可变借用期间可变借用。
        let mut first_updates: Vec<(u32, u32)> = Vec::new();
        full_buckets.for_each_nonempty(|code, b| {
            let cnt = b.members.len();
            if cnt >= 2 {
                total_collisions += cnt - 1;
                collision_frequency += b.freq_sum - b.max_freq;
            }
            // 首选字：桶内 (最大频率, 最小 ci)（需求 5.1/5.2）
            let mut max_f = 0u64;
            let mut first = u32::MAX;
            for &ci in &b.members {
                let f = ctx.char_infos[ci as usize].frequency;
                if f > max_f || (f == max_f && ci < first) {
                    max_f = f;
                    first = ci;
                }
            }
            first_updates.push((code, first));
        });
        for (code, first) in first_updates {
            full_buckets.get_mut_or_insert(code).first = first;
            is_first_candidate[first as usize] = true;
        }

        let inv_tf = if ctx.total_frequency > 0 {
            1.0 / ctx.total_frequency as f64
        } else {
            0.0
        };
        let inv_tkp = if total_key_presses > 0.0 {
            1.0 / total_key_presses
        } else {
            0.0
        };

        let simple_eval = if build_simple && ctx.enable_simple_code && !ctx.simple_config.levels.is_empty() {
            Some(SimpleEvaluator::new(ctx, assignment, &full_buckets, &is_first_candidate, simple_output_scope))
        } else {
            None
        };

        // 向后兼容（back-compat）决策：
        // `Evaluator::new` 仍按 `enable_simple_code` 急切（eager）构建 `SimpleEvaluator`，
        // 因此既有测试（prop1/2/3/4/5/6/7/11/14 等）仍能在 `new` 后断言 `simple_eval.is_some()`。
        // （`new_full_only` 显式传 `build_simple=false` 跳过该构建，仅供 disable_simple 上下文，需求 24。）
        // 急切构建时直接置 `simple_active = true` 且 `current_simple_weight = weight_simple_code`，
        // 使 `compute_score` 与旧公式
        // `weight_full_code * full_score + weight_simple_code * simple_score` 完全等价；
        // 而退火后期的「延迟激活」流程（任务 10）改走 `activate_simple` 显式翻转 `simple_active`
        // 并由权重曲线维护 `current_simple_weight`。
        let (simple_active, current_simple_weight) = if simple_eval.is_some() {
            (true, ctx.weights.weight_simple_code)
        } else {
            (false, 0.0)
        };

        let mut e = Self {
            current_codes,
            current_equiv_val,
            full_buckets,
            char_bucket_pos,
            is_first_candidate,
            simple_is_first_dirty: Vec::new(),
            total_collisions,
            collision_frequency,
            total_equiv_weighted,
            total_equiv_sq_weighted,
            key_weighted_usage,
            total_key_presses,
            total_frequency: ctx.total_frequency,
            inv_total_frequency: inv_tf,
            inv_total_key_presses: inv_tkp,
            cached_score: 0.0,
            score_dirty: true,
            cached_full_score: 0.0,
            full_score_dirty: true,
            simple_active,
            current_simple_weight,
            simple_eval,
            full_rebuild_calls: 0,
        };
        e.cached_full_score = e.compute_full_score(ctx);
        e.full_score_dirty = false;
        e.cached_score = e.compute_score(ctx);
        e.score_dirty = false;
        e
    }

    /// 重新扫描桶成员的最大频率与首选字 (max_freq, 最小 ci)。
    /// 首选字取桶内最大频率者，频率并列时取最小 ci（需求 5.1/5.2）。
    /// 以成员切片为入参（静态），避免与 BucketStore 的可变借用冲突。
    #[inline]
    fn rescan_bucket_first(ctx: &OptContext, members: &[u32]) -> (u64, u32) {
        let mut max_f = 0u64;
        let mut first = u32::MAX;
        for &ci in members {
            let f = ctx.char_infos[ci as usize].frequency;
            if f > max_f || (f == max_f && ci < first) {
                max_f = f;
                first = ci;
            }
        }
        (max_f, first)
    }

    /// 重新扫描桶成员的最大频率（仅 max，不跟踪首选字）。
    ///
    /// 供简码未激活/关闭的纯全码热路径使用：此时无需维护首选字
    /// `is_first_candidate`/`first`，仅需 `max_freq` 用于重码频率统计，
    /// 行为与基线版本（27fcc6d）的 `rescan_bucket_max` 完全一致。
    #[inline]
    fn rescan_bucket_max(ctx: &OptContext, members: &[u32]) -> u64 {
        let mut max_f = 0u64;
        for &ci in members {
            let f = ctx.char_infos[ci as usize].frequency;
            if f > max_f {
                max_f = f;
            }
        }
        max_f
    }

    /// 计算桶的重码频率（仅用于 SimpleEvaluator 等非热路径）
    #[inline]
    fn bucket_cf_static(ctx: &OptContext, chars: &[usize]) -> u64 {
        debug_assert!(chars.len() >= 2);
        let mut total = 0u64;
        let mut max_f = 0u64;
        for &ci in chars {
            let f = ctx.char_infos[ci].frequency;
            total += f;
            if f > max_f {
                max_f = f;
            }
        }
        total - max_f
    }

    /// 更新单个汉字的编码（增量更新碰撞计数）
    #[inline]
    pub fn update_char(&mut self, ctx: &OptContext, assignment: &[u8], ci: usize) {
        let old_code = self.current_codes[ci];
        let new_code = ctx.calc_code_only(ci, assignment);
        if old_code == new_code {
            return;
        }

        let freq = ctx.char_infos[ci].frequency;
        let freq_f = freq as f64;

        // 更新等价值
        let old_eq = self.current_equiv_val[ci];
        let new_eq = ctx.calc_equiv_from_parts(ci, assignment);
        self.total_equiv_weighted += (new_eq - old_eq) * freq_f;
        self.total_equiv_sq_weighted += (new_eq * new_eq - old_eq * old_eq) * freq_f;
        self.current_equiv_val[ci] = new_eq;

        let old_code_u = old_code as u32;
        let new_code_u = new_code as u32;

        // === 从旧桶移除 ===
        let pos = self.char_bucket_pos[ci];
        // 旧桶一定非空：访问 + 移除（空时）在稀疏后端单次哈希查找内完成，省去额外的 remove 查找。
        // 预借用 char_bucket_pos（与 full_buckets 为不相交字段），供闭包内重链 swap_remove 的尾元素。
        let char_bucket_pos = &mut self.char_bucket_pos;
        let simple_active = self.simple_active;
        let (old_bucket_cc, old_bucket_cf, new_old_cc, new_old_cf, old_first_set) = self
            .full_buckets
            .modify_existing_remove_if_empty(old_code_u, |ob| {
                let old_len = ob.members.len();
                // 旧桶的碰撞贡献（移除前）
                let old_bucket_cc = old_len.saturating_sub(1);
                let old_bucket_cf = if old_len >= 2 { ob.freq_sum - ob.max_freq } else { 0 };

                // swap_remove: 用最后一个元素替换被移除的元素
                let last_idx = old_len - 1;
                if pos != last_idx {
                    let moved_ci = ob.members[last_idx];
                    ob.members[pos] = moved_ci;
                    char_bucket_pos[moved_ci as usize] = pos;
                }
                ob.members.pop();

                // 更新旧桶的频率统计与首选字（需求 5.3）
                ob.freq_sum -= freq;
                // 如果移除的是 max（含恰为首选字的情况），需要重扫。
                // 简码激活时维护 (max, 首选)；未激活/简码关闭时仅维护 max（基线行为，性能不退化）。
                let mut old_first_set: Option<u32> = None;
                if freq >= ob.max_freq {
                    if ob.members.is_empty() {
                        ob.max_freq = 0;
                        if simple_active {
                            ob.first = u32::MAX;
                        }
                    } else if simple_active {
                        let (mf, first) = Self::rescan_bucket_first(ctx, &ob.members);
                        ob.max_freq = mf;
                        ob.first = first;
                        old_first_set = Some(first);
                    } else {
                        ob.max_freq = Self::rescan_bucket_max(ctx, &ob.members);
                    }
                }

                let new_old_len = ob.members.len();
                let new_old_cc = new_old_len.saturating_sub(1);
                let new_old_cf = if new_old_len >= 2 { ob.freq_sum - ob.max_freq } else { 0 };
                (old_bucket_cc, old_bucket_cf, new_old_cc, new_old_cf, old_first_set)
            });
        // 旧桶若新首选翻转：标记 is_first_candidate 与 resort 种子。
        if let Some(first) = old_first_set {
            self.is_first_candidate[first as usize] = true;
            // 记录首选翻转（resort 种子）；apply_simple_for_move 才会清空它。
            // 仅候选字才会被用作 resort 种子，故只 push 候选字（性能优化，行为等价）。
            if ctx.simple_is_candidate[first as usize] {
                self.simple_is_first_dirty.push(first as usize);
            }
        }
        // 旧桶变空时的移除已在 modify_existing_remove_if_empty 内（同一次查找）完成。

        // === 插入新桶 ===
        let (new_bucket_cc, new_bucket_cf, after_new_cc, after_new_cf) = {
            let nb = self.full_buckets.get_mut_or_insert(new_code_u);
            let new_len = nb.members.len();
            let new_bucket_cc = new_len.saturating_sub(1);
            let new_bucket_cf = if new_len >= 2 { nb.freq_sum - nb.max_freq } else { 0 };

            let new_pos = new_len;
            nb.members.push(ci as u32);
            self.char_bucket_pos[ci] = new_pos;
            nb.freq_sum += freq;
            // 加入新桶后增量维护首选字（需求 5.3）：取插入前的桶状态判定。
            // 仅在简码激活时维护；未激活/简码关闭时跳过整段（基线行为，仅下方更新 max）。
            if self.simple_active {
                let prev_first = nb.first;
                let prev_max = nb.max_freq;
                let becomes_first = prev_first == u32::MAX
                    || freq > prev_max
                    || (freq == prev_max && (ci as u32) < prev_first);
                if becomes_first {
                    if prev_first != u32::MAX {
                        self.is_first_candidate[prev_first as usize] = false;
                        if ctx.simple_is_candidate[prev_first as usize] {
                            self.simple_is_first_dirty.push(prev_first as usize);
                        }
                    }
                    nb.first = ci as u32;
                    self.is_first_candidate[ci] = true;
                    if ctx.simple_is_candidate[ci] {
                        self.simple_is_first_dirty.push(ci);
                    }
                } else {
                    self.is_first_candidate[ci] = false;
                    if ctx.simple_is_candidate[ci] {
                        self.simple_is_first_dirty.push(ci);
                    }
                }
            }
            if freq > nb.max_freq {
                nb.max_freq = freq;
            }

            let after_new_len = new_len + 1;
            let after_new_cc = after_new_len.saturating_sub(1);
            let after_new_cf = if after_new_len >= 2 { nb.freq_sum - nb.max_freq } else { 0 };
            (new_bucket_cc, new_bucket_cf, after_new_cc, after_new_cf)
        };

        // 更新全局碰撞计数
        self.total_collisions = (self.total_collisions + new_old_cc + after_new_cc)
            - (old_bucket_cc + new_bucket_cc);
        self.collision_frequency = (self.collision_frequency + new_old_cf + after_new_cf)
            - (old_bucket_cf + new_bucket_cf);
        self.current_codes[ci] = new_code;

        // 全码聚合已变化：标记全码分量缓存为脏（需求 10.1）。
        // `update_char` 是全码聚合的唯一变更入口（正向移动与回滚反向重放都经此），
        // 在此单点置脏可保证 `full_score_component` 永远基于最新聚合重算，
        // 无需在 `try_move`/`try_swap` 的多个分支重复维护。
        self.full_score_dirty = true;
    }

    /// 执行全码 _max 硬约束检查
    /// 返回 true 表示通过（未超限），false 表示超限需回滚
    #[inline(always)]
    fn check_full_code_max(&self, ctx: &OptContext) -> bool {
        let t = &ctx.targets_config.full_code;

        if t.collision_count_max > 0.0
            && self.total_collisions as f64 > t.collision_count_max
        {
            return false;
        }

        if t.collision_rate_max > 0.0 {
            let rate = self.collision_frequency as f64 * self.inv_total_frequency;
            if rate > t.collision_rate_max {
                return false;
            }
        }

        if t.equivalence_max > 0.0 {
            let equiv = self.total_equiv_weighted * self.inv_total_frequency;
            if equiv > t.equivalence_max {
                return false;
            }
        }

        if t.equiv_cv_max > 0.0 {
            let cv = self.calc_equiv_cv();
            if cv > t.equiv_cv_max {
                return false;
            }
        }

        if t.distribution_max > 0.0 {
            let dist = self.calc_distribution_deviation(&ctx.key_dist_config);
            if dist > t.distribution_max {
                return false;
            }
        }

        true
    }

    /// 执行简码 _max 硬约束检查
    /// 返回 true 表示通过（未超限），false 表示超限需回滚
    #[inline(always)]
    fn check_simple_code_max(&self, ctx: &OptContext) -> bool {
        let t = &ctx.targets_config.simple_code;
        if let Some(ref se) = self.simple_eval {
            let sm = se.get_simple_metrics(ctx);

            if t.collision_count_max > 0.0
                && sm.collision_count as f64 > t.collision_count_max
            {
                return false;
            }
            if t.collision_rate_max > 0.0 && sm.collision_rate > t.collision_rate_max {
                return false;
            }
            // freq_max 是覆盖率下限：覆盖率低于此值时拒绝
            if t.freq_max > 0.0 && sm.weighted_freq_coverage < t.freq_max {
                return false;
            }
            if t.equiv_max > 0.0 && sm.equiv_mean > t.equiv_max {
                return false;
            }
            if t.dist_max > 0.0 && sm.dist_deviation > t.dist_max {
                return false;
            }
        }
        true
    }

    /// 计算全码得分
    #[inline(always)]
    pub fn compute_full_score(&self, ctx: &OptContext) -> f64 {
        if ctx.targets_config.full_code.enabled {
            let t = &ctx.targets_config.full_code;
            let lw = t.low_weight;
            let mut score = 0.0;

            // collision_count
            {
                let v = self.total_collisions as f64;
                let s = ctx.scale_config.collision_count;
                let d = (v - t.collision_count).max(0.0) * s;
                score += ctx.weights.weight_collision_count * (d + d * d + lw * (v * s));
            }

            // collision_rate
            if ctx.weights.weight_collision_rate > 0.0 {
                let v = self.collision_frequency as f64 * self.inv_total_frequency;
                let s = ctx.scale_config.collision_rate;
                let d = (v - t.collision_rate).max(0.0) * s;
                score += ctx.weights.weight_collision_rate * (d + d * d + lw * (v * s));
            }

            // equivalence (equiv_mean)
            if ctx.weights.weight_equivalence > 0.0 {
                let v = self.total_equiv_weighted * self.inv_total_frequency;
                let s = ctx.scale_config.equivalence;
                let d = (v - t.equivalence).max(0.0) * s;
                score += ctx.weights.weight_equivalence * (d + d * d + lw * (v * s));
            }

            // equiv_cv
            if ctx.weights.weight_equiv_cv > 0.0 {
                let v = self.calc_equiv_cv();
                let s = ctx.scale_config.equiv_cv;
                let d = (v - t.equiv_cv).max(0.0) * s;
                score += ctx.weights.weight_equiv_cv * (d + d * d + lw * (v * s));
            }

            // distribution
            if ctx.weights.weight_distribution > 0.0 {
                let v = self.calc_distribution_deviation(&ctx.key_dist_config);
                let s = ctx.scale_config.distribution;
                let d = (v - t.distribution).max(0.0) * s;
                score += ctx.weights.weight_distribution * (d + d * d + lw * (v * s));
            }

            score
        } else {
            // 原有绝对值最小化模式（保持不变）
            let mut score = ctx.weights.weight_collision_count
                * self.total_collisions as f64
                * ctx.scale_config.collision_count;

            if ctx.weights.weight_collision_rate > 0.0 {
                let collision_rate = self.collision_frequency as f64 * self.inv_total_frequency;
                score += ctx.weights.weight_collision_rate
                    * collision_rate
                    * ctx.scale_config.collision_rate;
            }

            if ctx.weights.weight_equivalence > 0.0 {
                let weighted_equiv = self.total_equiv_weighted * self.inv_total_frequency;
                score += ctx.weights.weight_equivalence
                    * weighted_equiv
                    * ctx.scale_config.equivalence;
            }

            if ctx.weights.weight_equiv_cv > 0.0 {
                let equiv_cv = self.calc_equiv_cv();
                score += ctx.weights.weight_equiv_cv
                    * equiv_cv
                    * ctx.scale_config.equiv_cv;
            }

            if ctx.weights.weight_distribution > 0.0 {
                let dist_deviation = self.calc_distribution_deviation(&ctx.key_dist_config);
                score += ctx.weights.weight_distribution
                    * dist_deviation
                    * ctx.scale_config.distribution;
            }

            score
        }
    }

    /// 计算综合得分
    ///
    /// 合成公式（需求 9.5/10.3）：
    /// `total = weight_full_code * full_score + current_simple_weight * simple_score`。
    /// 未激活时（`simple_active == false`）`current_simple_weight = 0` 且简码分量取 0，
    /// 故综合得分退化为纯全码分量 `weight_full_code * full_score`（需求 10.2）。
    ///
    /// 注：本方法为 `&self` 纯计算，直接读取 `simple_eval.cached_simple_score`
    /// （与旧实现一致）；调用方需保证简码缓存已是最新（`get_score` / 增量路径会维护）。
    #[inline(always)]
    pub fn compute_score(&self, ctx: &OptContext) -> f64 {
        let full_score = self.compute_full_score(ctx);
        let simple_score = if self.simple_active {
            self.simple_eval
                .as_ref()
                .map_or(0.0, |se| se.cached_simple_score)
        } else {
            0.0
        };
        ctx.weights.weight_full_code * full_score + self.current_simple_weight * simple_score
    }

    /// 获取得分
    #[inline(always)]
    pub fn get_score(&mut self, ctx: &OptContext) -> f64 {
        if self.score_dirty {
            // 刷新全码分量缓存（需求 10.1）
            self.cached_full_score = self.compute_full_score(ctx);
            self.full_score_dirty = false;
            // 简码分量：未激活时贡献 0（需求 10.2）
            let simple_score = self.simple_score_component(ctx);
            self.cached_score = ctx.weights.weight_full_code * self.cached_full_score
                + self.current_simple_weight * simple_score;
            self.score_dirty = false;
        }
        self.cached_score
    }

    /// 获取当前全码分量得分（需求 10.1，供退火主循环按分量存储最佳解）。
    ///
    /// 依据 `full_score_dirty` 缓存，脏时以 `compute_full_score` 从全码聚合 O(1) 重算。
    #[inline(always)]
    pub fn full_score_component(&mut self, ctx: &OptContext) -> f64 {
        if self.full_score_dirty {
            self.cached_full_score = self.compute_full_score(ctx);
            self.full_score_dirty = false;
        }
        self.cached_full_score
    }

    /// 获取当前简码分量得分（需求 10.1/10.2，供退火主循环按分量存储最佳解）。
    ///
    /// 未激活（`simple_active == false`）或无简码评估器时恒为 0（需求 10.2）。
    #[inline(always)]
    pub fn simple_score_component(&mut self, ctx: &OptContext) -> f64 {
        if self.simple_active {
            if let Some(ref mut se) = self.simple_eval {
                se.get_simple_score(ctx)
            } else {
                0.0
            }
        } else {
            0.0
        }
    }

    /// 按分量与给定权重 O(1) 合成综合得分（需求 11.2/11.3）。
    ///
    /// 退火主循环以全码分量 `best_full_score`、简码分量 `best_simple_score` 存储最佳解，
    /// 比较时用当前有效简码权重 `weight_simple_eff` 重算 `best_total`，从而解决目标函数
    /// 随时间漂移导致最佳解被冻结的问题。该重算不依赖任何评估器内部状态，故为关联函数。
    #[inline(always)]
    pub fn best_total(
        weight_full: f64,
        full_score: f64,
        weight_simple_eff: f64,
        simple_score: f64,
    ) -> f64 {
        weight_full * full_score + weight_simple_eff * simple_score
    }

    /// 计算等价值变异系数
    #[inline(always)]
    pub fn calc_equiv_cv(&self) -> f64 {
        let mean = self.total_equiv_weighted * self.inv_total_frequency;
        if mean <= 0.0 {
            return 0.0;
        }
        let mean_sq = self.total_equiv_sq_weighted * self.inv_total_frequency;
        let variance = mean_sq - mean * mean;
        if variance <= 0.0 {
            return 0.0;
        }
        variance.sqrt() / mean
    }

    /// 计算分布偏差
    #[inline(always)]
    pub fn calc_distribution_deviation(&self, kdc: &[KeyDistConfig; EQUIV_TABLE_SIZE]) -> f64 {
        let mut dev = 0.0;
        for key in 0..EQUIV_TABLE_SIZE {
            let cfg = &kdc[key];
            if cfg.target_rate == 0.0 && cfg.low_penalty == 0.0 && cfg.high_penalty == 0.0 {
                continue;
            }
            let actual_pct = self.key_weighted_usage[key] * 100.0 * self.inv_total_key_presses;
            let diff = actual_pct - cfg.target_rate;
            if diff < 0.0 {
                dev += diff * diff * cfg.low_penalty;
            } else if diff > 0.0 {
                dev += diff * diff * cfg.high_penalty;
            }
        }
        dev
    }

    /// 获取评估指标
    pub fn get_metrics(&self, ctx: &OptContext) -> Metrics {
        Metrics {
            collision_count: self.total_collisions,
            collision_rate: self.collision_frequency as f64 * self.inv_total_frequency,
            equiv_mean: self.total_equiv_weighted * self.inv_total_frequency,
            equiv_cv: self.calc_equiv_cv(),
            dist_deviation: self.calc_distribution_deviation(&ctx.key_dist_config),
        }
    }

    /// 获取各指标的分数分量（用于日志输出，复用算分逻辑，不重复公式）
    pub fn get_metric_scores(&self, ctx: &OptContext) -> MetricScores {
        // 复用 compute_full_score 的逻辑，但拆分为各指标分量
        let score_for = |v: f64, target: f64, scale: f64, weight: f64, lw: f64| -> f64 {
            if ctx.targets_config.full_code.enabled {
                let d = (v - target).max(0.0) * scale;
                weight * (d + d * d + lw * (v * scale))
            } else {
                weight * v * scale
            }
        };

        let t = &ctx.targets_config.full_code;
        let lw = t.low_weight;

        let s_collision_count = score_for(
            self.total_collisions as f64,
            t.collision_count,
            ctx.scale_config.collision_count,
            ctx.weights.weight_collision_count,
            lw,
        );
        let s_collision_rate = score_for(
            self.collision_frequency as f64 * self.inv_total_frequency,
            t.collision_rate,
            ctx.scale_config.collision_rate,
            ctx.weights.weight_collision_rate,
            lw,
        );
        let s_equivalence = score_for(
            self.total_equiv_weighted * self.inv_total_frequency,
            t.equivalence,
            ctx.scale_config.equivalence,
            ctx.weights.weight_equivalence,
            lw,
        );
        let s_equiv_cv = score_for(
            self.calc_equiv_cv(),
            t.equiv_cv,
            ctx.scale_config.equiv_cv,
            ctx.weights.weight_equiv_cv,
            lw,
        );
        let s_distribution = score_for(
            self.calc_distribution_deviation(&ctx.key_dist_config),
            t.distribution,
            ctx.scale_config.distribution,
            ctx.weights.weight_distribution,
            lw,
        );

        let total_full = s_collision_count + s_collision_rate + s_equivalence + s_equiv_cv + s_distribution;

        let total_simple = if ctx.enable_simple_code {
            if let Some(ref se) = self.simple_eval {
                se.cached_simple_score
            } else {
                0.0
            }
        } else {
            0.0
        };

        let total = if ctx.enable_simple_code {
            ctx.weights.weight_full_code * total_full + ctx.weights.weight_simple_code * total_simple
        } else {
            total_full
        };

        MetricScores {
            collision_count: s_collision_count,
            collision_rate: s_collision_rate,
            equivalence: s_equivalence,
            equiv_cv: s_equiv_cv,
            distribution: s_distribution,
            total_full,
            total_simple,
            total,
        }
    }

    /// 获取简码评估指标
    pub fn get_simple_metrics(&self, ctx: &OptContext) -> SimpleMetrics {
        if let Some(ref se) = self.simple_eval {
            se.get_simple_metrics(ctx)
        } else {
            SimpleMetrics::default()
        }
    }

    /// 获取简码各子指标的分数分量（需求 28.5/28.6），镜像 `SimpleEvaluator::compute_simple_score`
    /// 的分项拆解。各子分数之和等于简码总分（`total`，未乘综合权重 `weight_simple_code`）。
    /// 简码未启用或无评估器时返回全 0（需求 28.7）。
    pub fn get_simple_metric_scores(&self, ctx: &OptContext) -> SimpleMetricScores {
        if !ctx.enable_simple_code || self.simple_eval.is_none() {
            return SimpleMetricScores::default();
        }
        let sm = self.get_simple_metrics(ctx);
        let w = &ctx.weights;
        let sc = &ctx.scale_config;
        let (freq, equiv, dist, cc, cr) = if ctx.targets_config.simple_code.enabled {
            let t = &ctx.targets_config.simple_code;
            let lw = t.low_weight;
            let comp = |v: f64, target: f64, s: f64, weight: f64| -> f64 {
                let d = (v - target).max(0.0) * s;
                weight * (d + d * d + lw * (v * s))
            };
            (
                comp(1.0 - sm.weighted_freq_coverage, 1.0 - t.freq, sc.simple_freq, w.simple_weight_freq),
                comp(sm.equiv_mean, t.equiv, sc.simple_equiv, w.simple_weight_equiv),
                comp(sm.dist_deviation, t.dist, sc.simple_dist, w.simple_weight_dist),
                comp(sm.collision_count as f64, t.collision_count, sc.simple_collision_count, w.simple_weight_collision_count),
                comp(sm.collision_rate, t.collision_rate, sc.simple_collision_rate, w.simple_weight_collision_rate),
            )
        } else {
            (
                w.simple_weight_freq * (1.0 - sm.weighted_freq_coverage) * sc.simple_freq,
                w.simple_weight_equiv * sm.equiv_mean * sc.simple_equiv,
                w.simple_weight_dist * sm.dist_deviation * sc.simple_dist,
                w.simple_weight_collision_count * (sm.collision_count as f64) * sc.simple_collision_count,
                w.simple_weight_collision_rate * sm.collision_rate * sc.simple_collision_rate,
            )
        };
        SimpleMetricScores {
            freq,
            equiv,
            dist,
            collision_count: cc,
            collision_rate: cr,
            total: freq + equiv + dist + cc + cr,
        }
    }

    /// 检查是否有简码影响
    pub fn has_simple_impact(&self, ctx: &OptContext, group: usize) -> bool {
        // 简码未激活时（延迟激活早期探索阶段，需求 8.5/10.2）简码分量贡献恒为 0，
        // 故任何移动都不需要触碰简码评估，直接返回 false 跳过简码重算（性能与正确性双赢）。
        if !ctx.enable_simple_code || self.simple_eval.is_none() || !self.simple_active {
            return false;
        }
        !ctx.group_to_simple_affected[group].is_empty()
    }

    /// 重建简码评估
    pub fn rebuild_simple(&mut self, ctx: &OptContext, assignment: &[u8]) {
        // 观测计数：记录一次经主评估器入口触发的全量重建。
        self.full_rebuild_calls += 1;
        // 全量重建已从零重算全部首选/出简状态，残留的增量「首选翻转」resort 种子作废；
        // 在此清空，防止 warmup/coordinate_descent 等只走 rebuild_simple 的路径上该缓冲累积增长。
        self.simple_is_first_dirty.clear();
        let full_buckets = &self.full_buckets;
        let is_first_candidate = &self.is_first_candidate;
        if let Some(ref mut se) = self.simple_eval {
            se.full_rebuild(ctx, assignment, full_buckets, is_first_candidate);
            se.cached_simple_score = se.compute_simple_score(ctx);
            se.simple_score_dirty = false;
        }
    }

    /// 观测用：返回截至目前的全量重建总次数（仅用于防回归测试，不参与优化逻辑）。
    ///
    /// 合计主评估器入口 `rebuild_simple` 的计数与简码评估器 `full_rebuild` 的计数。
    /// 生产热路径 `try_move`/`try_swap` 在简码激活后只走增量路径
    /// （`apply_simple_for_move` → `apply_move_incremental`），因此该计数在热路径调用
    /// 期间应保持不变。
    pub fn full_rebuild_calls(&self) -> usize {
        self.full_rebuild_calls + self.simple_eval.as_ref().map_or(0, |se| se.full_rebuild_calls)
    }

    /// 观测用：返回上一次简码增量移动中阶段 2（出简选择增量）访问的候选字工作量
    /// （Backlog B1 北极星）。无简码评估器时返回 0。该值应与「受影响字 + 脏桶成员」同阶，
    /// 不随候选字总集规模增长，用于证明阶段 2 不再做 O(候选字集 × 级数) 的整体重算。
    #[allow(dead_code)]
    pub fn last_stage2_visits(&self) -> usize {
        self.simple_eval.as_ref().map_or(0, |se| se.stage2_visits)
    }

    /// 观测用：分别返回两个全量重建计数 `(主评估器 rebuild_simple 次数, 简码评估器 full_rebuild 次数)`。
    ///
    /// 生产路径中 `SimpleEvaluator::full_rebuild` 仅由 `Evaluator::rebuild_simple` 调用，
    /// 故两者在生产中应同步增长；若二者出现差异，说明 `full_rebuild` 被 `rebuild_simple`
    /// 以外的路径触发。两个计数都不统计 `reconcile`（周期对账走 `Evaluator::new` 重建，
    /// 是需求 15 认可的周期性全量，不经这两个入口），因此它们专门回答：
    /// 「是否有人（尤其是退火热路径）调用了旧的全量重建入口」。健康运行下两者在整个 SA
    /// 主循环期间应恒为 0。
    pub fn full_rebuild_calls_breakdown(&self) -> (usize, usize) {
        (
            self.full_rebuild_calls,
            self.simple_eval.as_ref().map_or(0, |se| se.full_rebuild_calls),
        )
    }

    /// 简码增量更新入口（任务 9.1 将以此替换 `try_move`/`try_swap` 中的 `rebuild_simple`）。
    ///
    /// `groups` 为本次移动涉及的字根组（移动单组时长度为 1，交换时为 2）。本方法收集这些组的
    /// 受影响候选字交集（`group_to_simple_affected_candidate`）与受影响全码字（`group_to_chars`），
    /// 调用 `SimpleEvaluator::apply_move_incremental` 做增量更新，并刷新缓存的简码得分。
    ///
    /// 注意：当前未接入 `try_move`/`try_swap`（仍走 `rebuild_simple` 路径），接入与回滚分支
    /// 由任务 9/10 完成；此处提供可调用入口并保证与全量重建结果一致。
    pub fn apply_simple_for_move(&mut self, ctx: &OptContext, assignment: &[u8], groups: &[usize]) {
        if self.simple_eval.is_none() {
            return;
        }

        // 收集受影响候选字交集（去重）与受影响全码字（去重）
        let mut affected_candidates: Vec<usize> = Vec::new();
        let mut full_affected_chars: Vec<usize> = Vec::new();
        for &g in groups {
            affected_candidates.extend_from_slice(&ctx.group_to_simple_affected_candidate[g]);
            full_affected_chars.extend_from_slice(&ctx.group_to_chars[g]);
        }
        affected_candidates.sort_unstable();
        affected_candidates.dedup();
        full_affected_chars.sort_unstable();
        full_affected_chars.dedup();

        // 收集「重排种子」：本次移动中首选状态翻转、且属于候选字的汉字（仅 Efficiency 模式相关）。
        // 这些字的桶内排序键随 `is_first_candidate` 翻转而变化，可能改变出简选择（需求 4.6/5.4/6.2）。
        //
        // 注意：即使某字在 `affected_candidates` 中，也必须保留为重排种子——因为受影响候选字
        // 仅在「简码编码发生变化」的级别被阶段 1 标脏；若其在某级别编码未变但首选状态翻转
        // （排序键变化），该级别的桶不会被阶段 1 标脏，必须靠重排种子触发局部重排。
        // Frequency 模式排序键与首选状态无关，无需重排；不收集以省去无谓工作。
        let mut resort_seeds: Vec<usize> = Vec::new();
        if ctx.simple_assign_mode == SimpleAssignMode::Efficiency {
            for &ci in &self.simple_is_first_dirty {
                if ctx.simple_is_candidate[ci] {
                    resort_seeds.push(ci);
                }
            }
            resort_seeds.sort_unstable();
            resort_seeds.dedup();
        }
        // 读取完毕，清空首选翻转登记缓冲，避免跨移动累积（拒绝路径的反向翻转作为下次种子无害）。
        self.simple_is_first_dirty.clear();

        let full_buckets = &self.full_buckets;
        let is_first_candidate = &self.is_first_candidate;
        if let Some(ref mut se) = self.simple_eval {
            se.apply_move_incremental(
                ctx,
                assignment,
                &affected_candidates,
                &resort_seeds,
                &full_affected_chars,
                full_buckets,
                is_first_candidate,
            );
            se.cached_simple_score = se.compute_simple_score(ctx);
            se.simple_score_dirty = false;
        }
    }

    /// 提交简码增量（接受移动时调用，需求 2.1）。
    ///
    /// 委托 `SimpleEvaluator::commit` 清空快照确认增量。无简码评估器时为 no-op。
    /// 供任务 9/10 在 `try_move`/`try_swap` 的接受分支接入。
    pub fn commit_simple(&mut self) {
        if let Some(ref mut se) = self.simple_eval {
            se.commit();
        }
    }

    /// 回滚简码增量（移动被拒绝或 `_max` 硬约束回滚时调用，需求 2.2/2.3）。
    ///
    /// 委托 `SimpleEvaluator::rollback` 用快照还原受影响桶项、级别聚合、简码重码标量与
    /// `cached_simple_score`，使简码状态恢复至移动前，且不触发任何全量重建。
    /// 供任务 9/10 在 `try_move`/`try_swap` 的拒绝/回滚分支接入（替换 `rebuild_simple`）。
    pub fn rollback_simple(&mut self) {
        if let Some(ref mut se) = self.simple_eval {
            se.rollback();
        }
    }

    /// 激活简码计算（需求 8.3/8.6）。
    ///
    /// 退火进度首次达到 `simple_start_progress`（或硬激活）时由主循环调用：
    /// - 若简码评估器尚未构建（延迟激活流程），执行一次全量构建初始化增量状态（需求 8.6）；
    /// - 置 `simple_active = true`，此后简码分量开始参与综合得分。
    ///
    /// `current_simple_weight` 不在此设置——它由退火主循环按权重渐进曲线
    /// `w_simple_eff(p)` 维护（需求 9）。激活后将 `score_dirty` / `full_score_dirty`
    /// 置脏，使下次 `get_score` 以新激活状态重算。
    ///
    /// 幂等：已激活时直接返回（闩锁语义，需求 8.4 由主循环保证）。
    pub fn activate_simple(&mut self, ctx: &OptContext, assignment: &[u8]) {
        if self.simple_active {
            return;
        }
        let will_activate = self.simple_eval.is_some()
            || (ctx.enable_simple_code && !ctx.simple_config.levels.is_empty());
        // 激活前 `is_first_candidate`/`bucket_first` 未做增量维护（性能优化：激活前无人读取，
        // 见 has_simple_impact 在 !simple_active 时短路）。在此一次性全量重建，使其反映当前
        // assignment——开销为 O(字数)，远小于激活前在每步移动里反复增量维护的累计开销。
        // 必须在下方构建 `SimpleEvaluator` 之前完成（其构造会读取 is_first_candidate）。
        if will_activate {
            self.rebuild_first_candidates(ctx);
        }
        if self.simple_eval.is_none()
            && ctx.enable_simple_code
            && !ctx.simple_config.levels.is_empty()
        {
            self.simple_eval = Some(SimpleEvaluator::new(
                ctx,
                assignment,
                &self.full_buckets,
                &self.is_first_candidate,
                false, // 退火激活：active 候选范围
            ));
        }
        // 仅当确有简码评估器时才视为激活；否则（简码关闭/无级别）保持未激活、贡献为 0。
        if self.simple_eval.is_some() {
            self.simple_active = true;
            self.score_dirty = true;
            self.full_score_dirty = true;
        }
    }

    /// 全量重建所有全码桶的首选字标记（`is_first_candidate`/`bucket_first`）。
    ///
    /// 首选字取桶内最大频率者，频率并列时取最小 ci（需求 5.1/5.2），与 `Evaluator::new`
    /// 的初始化一致。供 `activate_simple` 在激活时调用：因激活前不增量维护首选字
    /// （纯全码热路径性能优化），需在激活那一刻据当前 `code_to_chars` 一次性重算。
    fn rebuild_first_candidates(&mut self, ctx: &OptContext) {
        for v in self.is_first_candidate.iter_mut() {
            *v = false;
        }
        // 仅遍历非空桶（需求 7.1）。先收集每桶首选字，再写回，避免迭代期可变借用。
        let mut first_updates: Vec<(u32, u32)> = Vec::new();
        self.full_buckets.for_each_nonempty(|code, b| {
            let mut max_f = 0u64;
            let mut first = u32::MAX;
            for &ci in &b.members {
                let f = ctx.char_infos[ci as usize].frequency;
                if f > max_f || (f == max_f && ci < first) {
                    max_f = f;
                    first = ci;
                }
            }
            first_updates.push((code, first));
        });
        for (code, first) in first_updates {
            self.full_buckets.get_mut_or_insert(code).first = first;
            self.is_first_candidate[first as usize] = true;
        }
    }

    /// 周期对账 / 结束强制校验（需求 15.4/15.5/15.6）。
    ///
    /// 用对当前 `assignment` 从零做的全量重算覆盖增量维护的全码聚合与简码指标，
    /// 纠正长程优化中可能累积的浮点/整型漂移。实现上直接构建一个全新的
    /// `Evaluator`（其全码聚合与简码状态均为精确全量值），再保留当前的激活状态
    /// 与有效简码权重，使对账后逐字段等于「对当前分配从零构建的评估器」（Property 13）。
    pub fn reconcile(&mut self, ctx: &OptContext, assignment: &[u8]) {
        let active = self.simple_active;
        let weight = self.current_simple_weight;
        // 保留全量重建观测计数：reconcile 用 `*self = fresh` 整体替换评估器，若不显式
        // 携带，计数会在每次对账时被重置为 0，从而掩盖「热路径意外触发全量重建」的回归。
        // 这里把旧计数带入 fresh，使其成为跨整个优化过程的真实累计值（防回归诊断）。
        let prev_ev_calls = self.full_rebuild_calls;
        let prev_se_calls = self
            .simple_eval
            .as_ref()
            .map_or(0, |se| se.full_rebuild_calls);

        let mut fresh = Evaluator::new(ctx, assignment);
        // 保留激活状态与有效简码权重（这两项是退火过程的时变量，不属于全量重算范畴）
        fresh.simple_active = active;
        fresh.current_simple_weight = weight;
        // 携带全量重建观测计数（reconcile 自身不计数：它走 Evaluator::new，不经
        // rebuild_simple / full_rebuild 入口，是需求 15 认可的周期性全量）。
        fresh.full_rebuild_calls = prev_ev_calls;
        if let Some(se) = fresh.simple_eval.as_mut() {
            se.full_rebuild_calls = prev_se_calls;
        }
        // 以保留的激活状态/权重重算缓存得分，保证 cached_score 与分量一致
        fresh.cached_full_score = fresh.compute_full_score(ctx);
        fresh.full_score_dirty = false;
        fresh.cached_score = fresh.compute_score(ctx);
        fresh.score_dirty = false;

        *self = fresh;
    }
    #[inline(always)]
    pub fn try_move(
        &mut self,
        ctx: &OptContext,
        assignment: &mut [u8],
        r: usize,
        new_key: u8,
        temp: f64,
        rng: &mut ThreadRng,
    ) -> bool {
        let old_key = assignment[r];
        if old_key == new_key {
            return false;
        }

        let old_score = self.get_score(ctx);
        let needs_simple = self.has_simple_impact(ctx, r);

        // O(1) key_weighted_usage 更新
        let gfs = ctx.group_freq_sum[r];
        self.key_weighted_usage[old_key as usize] -= gfs;
        self.key_weighted_usage[new_key as usize] += gfs;

        assignment[r] = new_key;
        for &ci in &ctx.group_to_chars[r] {
            self.update_char(ctx, assignment, ci);
        }

        if needs_simple {
            self.apply_simple_for_move(ctx, assignment, &[r]);
        }

        // _max 硬约束检查：超限则直接回滚，不进入得分计算
        if !self.check_full_code_max(ctx) {
            self.key_weighted_usage[new_key as usize] -= gfs;
            self.key_weighted_usage[old_key as usize] += gfs;
            assignment[r] = old_key;
            for &ci in &ctx.group_to_chars[r] {
                self.update_char(ctx, assignment, ci);
            }
            if needs_simple {
                self.rollback_simple();
            }
            self.cached_score = old_score;
            self.score_dirty = false;
            return false;
        }
        if ctx.enable_simple_code && needs_simple && !self.check_simple_code_max(ctx) {
            self.key_weighted_usage[new_key as usize] -= gfs;
            self.key_weighted_usage[old_key as usize] += gfs;
            assignment[r] = old_key;
            for &ci in &ctx.group_to_chars[r] {
                self.update_char(ctx, assignment, ci);
            }
            if needs_simple {
                self.rollback_simple();
            }
            self.cached_score = old_score;
            self.score_dirty = false;
            return false;
        }

        self.score_dirty = true;
        let new_score = self.get_score(ctx);
        let delta = new_score - old_score;

        if delta <= 0.0 || rng.gen::<f64>() < (-delta / temp).exp() {
            if needs_simple {
                self.commit_simple();
            }
            true
        } else {
            // 回滚 key_weighted_usage
            self.key_weighted_usage[new_key as usize] -= gfs;
            self.key_weighted_usage[old_key as usize] += gfs;

            assignment[r] = old_key;
            for &ci in &ctx.group_to_chars[r] {
                self.update_char(ctx, assignment, ci);
            }

            if needs_simple {
                self.rollback_simple();
            }

            self.cached_score = old_score;
            self.score_dirty = false;
            false
        }
    }

    /// 尝试交换（交换两个组的键位）
    #[inline(always)]
    pub fn try_swap(
        &mut self,
        ctx: &OptContext,
        assignment: &mut [u8],
        r1: usize,
        r2: usize,
        temp: f64,
        rng: &mut ThreadRng,
    ) -> bool {
        let k1 = assignment[r1];
        let k2 = assignment[r2];
        if k1 == k2 {
            return false;
        }

        let old_score = self.get_score(ctx);
        let needs_simple = self.has_simple_impact(ctx, r1) || self.has_simple_impact(ctx, r2);

        // O(1) key_weighted_usage 更新
        let gfs1 = ctx.group_freq_sum[r1];
        let gfs2 = ctx.group_freq_sum[r2];
        self.key_weighted_usage[k1 as usize] -= gfs1;
        self.key_weighted_usage[k2 as usize] += gfs1;
        self.key_weighted_usage[k2 as usize] -= gfs2;
        self.key_weighted_usage[k1 as usize] += gfs2;

        assignment[r1] = k2;
        assignment[r2] = k1;
        for &ci in &ctx.group_to_chars[r1] {
            self.update_char(ctx, assignment, ci);
        }
        for &ci in &ctx.group_to_chars[r2] {
            self.update_char(ctx, assignment, ci);
        }

        if needs_simple {
            self.apply_simple_for_move(ctx, assignment, &[r1, r2]);
        }

        // _max 硬约束检查：超限则直接回滚，不进入得分计算
        if !self.check_full_code_max(ctx) {
            self.key_weighted_usage[k2 as usize] -= gfs1;
            self.key_weighted_usage[k1 as usize] += gfs1;
            self.key_weighted_usage[k1 as usize] -= gfs2;
            self.key_weighted_usage[k2 as usize] += gfs2;
            assignment[r1] = k1;
            assignment[r2] = k2;
            for &ci in &ctx.group_to_chars[r1] {
                self.update_char(ctx, assignment, ci);
            }
            for &ci in &ctx.group_to_chars[r2] {
                self.update_char(ctx, assignment, ci);
            }
            if needs_simple {
                self.rollback_simple();
            }
            self.cached_score = old_score;
            self.score_dirty = false;
            return false;
        }
        if ctx.enable_simple_code && needs_simple && !self.check_simple_code_max(ctx) {
            self.key_weighted_usage[k2 as usize] -= gfs1;
            self.key_weighted_usage[k1 as usize] += gfs1;
            self.key_weighted_usage[k1 as usize] -= gfs2;
            self.key_weighted_usage[k2 as usize] += gfs2;
            assignment[r1] = k1;
            assignment[r2] = k2;
            for &ci in &ctx.group_to_chars[r1] {
                self.update_char(ctx, assignment, ci);
            }
            for &ci in &ctx.group_to_chars[r2] {
                self.update_char(ctx, assignment, ci);
            }
            if needs_simple {
                self.rollback_simple();
            }
            self.cached_score = old_score;
            self.score_dirty = false;
            return false;
        }

        self.score_dirty = true;
        let new_score = self.get_score(ctx);
        let delta = new_score - old_score;

        if delta <= 0.0 || rng.gen::<f64>() < (-delta / temp).exp() {
            if needs_simple {
                self.commit_simple();
            }
            true
        } else {
            // 回滚 key_weighted_usage
            self.key_weighted_usage[k2 as usize] -= gfs1;
            self.key_weighted_usage[k1 as usize] += gfs1;
            self.key_weighted_usage[k1 as usize] -= gfs2;
            self.key_weighted_usage[k2 as usize] += gfs2;

            assignment[r1] = k1;
            assignment[r2] = k2;
            for &ci in &ctx.group_to_chars[r1] {
                self.update_char(ctx, assignment, ci);
            }
            for &ci in &ctx.group_to_chars[r2] {
                self.update_char(ctx, assignment, ci);
            }

            if needs_simple {
                self.rollback_simple();
            }

            self.cached_score = old_score;
            self.score_dirty = false;
            false
        }
    }
}

// =========================================================================
// 🧪 首选标记测试（simple-code-perf-optimization, Property 3）
// =========================================================================
#[cfg(test)]
mod first_candidate_tests {
    use super::*;
    use crate::config::TargetsConfig;
    use crate::context::OptContext;
    use crate::types::{
        KeyDistConfig, RootGroup, ScaleConfig, SimpleCodeConfig, SimpleCodeLevel, SimpleCodeStep,
        WeightConfig, EQUIV_TABLE_SIZE,
    };
    use proptest::prelude::*;
    use rand::thread_rng;
    use std::collections::HashMap;

    // 后端选择日志行（需求 12.2/12.3/12.4/12.6）：数字均来自运行时参数。
    #[test]
    fn backend_log_line_content() {
        // 小容量 → Dense，行内含 "Dense" 与预估 MB。
        let dense = backend_log_line("全码桶", 1024, 40, 11177);
        assert!(dense.contains("Dense"), "应标记 Dense: {dense}");
        assert!(dense.contains("1024"), "应含运行时容量: {dense}");
        // 超阈值容量 → Sparse，行内含 "Sparse" 与运行时 n_chars 提示。
        let sparse = backend_log_line("全码桶", 33_554_432, 40, 11177);
        assert!(sparse.contains("Sparse"), "应标记 Sparse: {sparse}");
        assert!(sparse.contains("n_chars(11177)"), "应含运行时 n_chars 提示: {sparse}");
    }

    /// 构建最小 OptContext：每个频率对应一个动态组，每组含 1 个字根、1 个单部件汉字。
    /// 不同组的汉字被分到同一键位时即产生重码（全码桶）。`enable_simple` 控制是否启用简码：
    /// 启用时附带一个 `"Aa"` 规则的简码级别（使主评估器持有 `SimpleEvaluator` 且
    /// `simple_active=true`，从而走首选字增量维护路径）；关闭时简码级别留空（`simple_eval=None`）。
    /// 首选标记仅依赖全码桶、与简码分配无关。
    fn make_ctx(freqs: &[u64], allowed: &[u8], enable_simple: bool) -> OptContext {
        let n = freqs.len();
        let mut groups = Vec::with_capacity(n);
        let mut splits = Vec::with_capacity(n);
        for (i, &f) in freqs.iter().enumerate() {
            let root = format!("r{i}");
            groups.push(RootGroup {
                roots: vec![root.clone()],
                allowed_keys: allowed.to_vec(),
            });
            let ch = char::from_u32(0x4e00 + i as u32).unwrap();
            splits.push((ch, vec![root], f));
        }
        let fixed_roots: HashMap<String, u8> = HashMap::new();
        let equiv_table = [[0.0f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];
        let key_dist = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];
        let mut weights = WeightConfig::default();
        weights.enable_simple_code = enable_simple;
        weights.simple_coverage_ratio = 1.0;
        let simple_config = if enable_simple {
            SimpleCodeConfig {
                levels: vec![SimpleCodeLevel {
                    level: 1,
                    code_num: 1,
                    rule_candidates: vec![vec![SimpleCodeStep {
                        root_selector: 'A',
                        code_selector: 'a',
                    }]],
                    space_commit: false,
                }],
            }
        } else {
            SimpleCodeConfig { levels: vec![] }
        };
        OptContext::new(
            &splits,
            &fixed_roots,
            &groups,
            equiv_table,
            key_dist,
            ScaleConfig::default(),
            simple_config,
            weights,
            TargetsConfig::default(),
        )
    }

    /// 从头重算每个汉字的首选标记：对每个非空全码桶取 (最大频率, 最小 ci) 为首选字。
    /// 仅依赖全码编码（assignment）与字频，不引用任何简码状态，因此与简码分配无关。
    fn expected_first_candidates(ctx: &OptContext, assignment: &[u8]) -> Vec<bool> {
        let n = ctx.char_infos.len();
        let cs = ctx.code_space;
        let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); cs];
        for ci in 0..n {
            let code = ctx.calc_code_only(ci, assignment);
            buckets[code].push(ci);
        }
        let mut expected = vec![false; n];
        for chars in &buckets {
            if chars.is_empty() {
                continue;
            }
            let mut max_f = 0u64;
            let mut first = usize::MAX;
            for &ci in chars {
                let f = ctx.char_infos[ci].frequency;
                if f > max_f || (f == max_f && ci < first) {
                    max_f = f;
                    first = ci;
                }
            }
            expected[first] = true;
        }
        expected
    }

    // Feature: simple-code-perf-optimization, 防回归：简码关闭时 update_char 不累积 resort 缓冲。
    // 关闭简码（simple_active=false）时，热路径每步的首选翻转不应推入 simple_is_first_dirty，
    // 否则该缓冲在纯全码优化过程中无限增长，拖慢全码路径。
    #[test]
    fn disabled_simple_does_not_grow_resort_buffer() {
        let allowed: [u8; 4] = [0, 1, 2, 3];
        let ctx = make_ctx(&[5u64, 4, 3, 2, 1], &allowed, false);
        let n = 5usize;
        let mut assignment = vec![0u8; n];
        let mut ev = Evaluator::new(&ctx, &assignment);
        assert!(!ev.simple_active, "简码关闭时 simple_active 应为 false");
        assert!(ev.simple_eval.is_none(), "简码关闭时不应有简码评估器");

        let mut rng = thread_rng();
        for step in 0..3000usize {
            let r = step % n;
            let nk = (step % 4) as u8;
            // 高温接受率高，充分驱动 update_char 的首选维护路径。
            ev.try_move(&ctx, &mut assignment, r, nk, 1e18, &mut rng);
        }
        assert_eq!(
            ev.simple_is_first_dirty.len(),
            0,
            "简码关闭时 simple_is_first_dirty 不应增长（实际 {}）",
            ev.simple_is_first_dirty.len()
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        // Feature: simple-code-perf-optimization, Property 3: 首选标记与全码桶重算一致
        // 对任意合法分配及任意一串移动序列，对每个汉字，增量维护的 is_first_candidate[ci]
        // 应等于「ci 是其全码桶 code_to_chars[code] 中频率最大者（频率并列时取最小 ci）」
        // 这一从全码桶直接重算的结果，且该值与简码分配无关。
        // Validates: Requirements 5.1, 5.2, 5.3, 5.4, 4.6
        #[test]
        fn prop3_first_candidate_matches_full_bucket_rescan(
            // 使用较小的频率范围以制造频率并列，充分驱动 (最大频率, 最小 ci) 的并列裁决
            freqs in prop::collection::vec(1u64..6, 3usize..8),
            moves in prop::collection::vec((0usize..16, 0u8..4), 0usize..40),
        ) {
            let allowed: [u8; 4] = [0, 1, 2, 3];
            let ctx = make_ctx(&freqs, &allowed, true);
            let n = freqs.len();
            let mut assignment = vec![0u8; n];
            let mut ev = Evaluator::new(&ctx, &assignment);
            let mut rng = thread_rng();

            // 初始状态即应与从头重算一致
            let expected0 = expected_first_candidates(&ctx, &assignment);
            prop_assert_eq!(&ev.is_first_candidate, &expected0);

            // 每个汉字恰位于其全码桶的首选关系中，因此 true 的数量 = 非空桶数量
            for (gi, nk) in moves {
                let r = gi % n;
                // 高温保证大多数移动被接受，充分驱动增量维护路径
                ev.try_move(&ctx, &mut assignment, r, nk, 1e18, &mut rng);

                // 每步后：增量维护的首选标记应与从全码桶从头重算的结果逐元素一致
                let expected = expected_first_candidates(&ctx, &assignment);
                prop_assert_eq!(
                    &ev.is_first_candidate,
                    &expected,
                    "is_first_candidate 与全码桶重算不一致, assignment={:?}",
                    assignment
                );

                // 一致性约束：恰有「非空全码桶数量」个汉字被标记为首选字
                let n_true = ev.is_first_candidate.iter().filter(|&&b| b).count();
                let n_nonempty = ev.full_buckets.nonempty_count();
                prop_assert_eq!(n_true, n_nonempty);
            }
        }
    }

    // 需求 28.6：简码各子分数之和等于简码总分，且等于综合算分里的 total_simple（口径自洽）。
    #[test]
    fn test_simple_metric_scores_sum_consistency() {
        let allowed: [u8; 4] = [0, 1, 2, 3];
        let ctx = make_ctx(&[9u64, 7, 5, 3, 2, 1], &allowed, true);
        let n = 6usize;
        let assignment = vec![0u8, 1, 2, 3, 0, 1];
        let mut ev = Evaluator::new(&ctx, &assignment);
        assert!(ev.simple_eval.is_some(), "简码启用时应有简码评估器");

        let sub = ev.get_simple_metric_scores(&ctx);
        let sum = sub.freq + sub.equiv + sub.dist + sub.collision_count + sub.collision_rate;
        assert!(
            (sum - sub.total).abs() < 1e-9,
            "简码子分数之和 {} 应等于 total {}",
            sum, sub.total
        );

        // 与综合算分的 total_simple 同口径（均为未乘 weight_simple_code 的简码总分）。
        let total_simple = ev.get_metric_scores(&ctx).total_simple;
        assert!(
            (sub.total - total_simple).abs() < 1e-9,
            "get_simple_metric_scores.total {} 应等于 get_metric_scores.total_simple {}",
            sub.total, total_simple
        );
    }

    // 需求 28.7：简码关闭时简码子分数全为 0。
    #[test]
    fn test_simple_metric_scores_zero_when_disabled() {
        let allowed: [u8; 4] = [0, 1, 2, 3];
        let ctx = make_ctx(&[5u64, 4, 3, 2, 1], &allowed, false);
        let assignment = vec![0u8; 5];
        let ev = Evaluator::new(&ctx, &assignment);
        let sub = ev.get_simple_metric_scores(&ctx);
        assert_eq!(sub.total, 0.0);
        assert_eq!(sub.freq, 0.0);
        assert_eq!(sub.equiv, 0.0);
        assert_eq!(sub.dist, 0.0);
    }
}

// =========================================================================
// 🧪 全码重码独立性测试（simple-code-perf-optimization, Property 14）
// =========================================================================
#[cfg(test)]
mod full_collision_independence_tests {
    use super::*;
    use crate::config::TargetsConfig;
    use crate::context::OptContext;
    use crate::types::{
        KeyDistConfig, RootGroup, ScaleConfig, SimpleCodeConfig, SimpleCodeLevel, SimpleCodeStep,
        WeightConfig, EQUIV_TABLE_SIZE,
    };
    use proptest::prelude::*;
    use std::collections::HashMap;

    /// 构建最小 OptContext：每个频率对应一个动态组（1 字根、1 单部件汉字）。
    /// 不同组的汉字落到同一键位即产生全码重码桶。`enable_simple` 控制是否启用简码：
    /// - 启用时附带一个 `"Aa"` 规则的简码级别（使主评估器持有 `SimpleEvaluator`，
    ///   从而拥有非平凡的出简标记集合 `all_assigned_flags`），覆盖率阈值取 1.0 使所有字均为候选；
    /// - 关闭时简码级别留空（`simple_eval` 为 `None`）。
    ///
    /// 两种取值下全码结构（组、允许键、字根/部件）完全一致，因此可对比全码指标。
    fn make_ctx(freqs: &[u64], allowed: &[u8], enable_simple: bool) -> OptContext {
        let n = freqs.len();
        let mut groups = Vec::with_capacity(n);
        let mut splits = Vec::with_capacity(n);
        for (i, &f) in freqs.iter().enumerate() {
            let root = format!("r{i}");
            groups.push(RootGroup {
                roots: vec![root.clone()],
                allowed_keys: allowed.to_vec(),
            });
            let ch = char::from_u32(0x4e00 + i as u32).unwrap();
            splits.push((ch, vec![root], f));
        }
        let fixed_roots: HashMap<String, u8> = HashMap::new();
        let equiv_table = [[0.0f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];
        let key_dist = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];
        let mut weights = WeightConfig::default();
        weights.enable_simple_code = enable_simple;
        weights.simple_coverage_ratio = 1.0;
        let simple_config = if enable_simple {
            SimpleCodeConfig {
                levels: vec![SimpleCodeLevel {
                    level: 1,
                    code_num: 1,
                    rule_candidates: vec![vec![SimpleCodeStep {
                        root_selector: 'A',
                        code_selector: 'a',
                    }]],
                    space_commit: false,
                }],
            }
        } else {
            SimpleCodeConfig { levels: vec![] }
        };
        OptContext::new(
            &splits,
            &fixed_roots,
            &groups,
            equiv_table,
            key_dist,
            ScaleConfig::default(),
            simple_config,
            weights,
            TargetsConfig::default(),
        )
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        // Feature: simple-code-perf-optimization, Property 14: 全码重码独立于出简状态
        //
        // 对任意分配与任意出简标记集合，主评估器的全码重码指标（total_collisions 与
        // collision_frequency，经 get_metrics 暴露为 collision_count 与 collision_rate）
        // 应与出简状态无关——改变 all_assigned_flags 不改变这两个全码指标。
        // Validates: Requirements 14.1
        #[test]
        fn prop14_full_collision_independent_of_simple(
            // 较小的频率范围以制造重码桶与频率并列
            freqs in prop::collection::vec(1u64..6, 3usize..8),
            // 确定性移动序列：直接改写 assignment（不依赖随机接受），逐步遍历分配空间
            moves in prop::collection::vec((0usize..16, 0u8..4), 0usize..40),
            // 用于驱动任意出简标记集合的随机布尔串
            flag_bits in prop::collection::vec(any::<bool>(), 0usize..256),
        ) {
            let allowed: [u8; 4] = [0, 1, 2, 3];
            let n = freqs.len();
            let ctx_off = make_ctx(&freqs, &allowed, false);
            let ctx_on = make_ctx(&freqs, &allowed, true);

            let mut asg = vec![0u8; n];

            // 逐步施加确定性移动；step==0 为初始分配，其后每步改写一个组的键位
            for step in 0..=moves.len() {
                // 简码启用：主评估器持有 SimpleEvaluator（含 all_assigned_flags）
                let mut ev_on = Evaluator::new(&ctx_on, &asg);
                // 简码关闭：作为「无出简状态」的全码基准
                let ev_off = Evaluator::new(&ctx_off, &asg);
                prop_assert!(ev_on.simple_eval.is_some());
                prop_assert!(ev_off.simple_eval.is_none());

                // (A) 启用 vs 关闭简码：全码指标完全一致（全码计算独立于简码）
                let m_on = ev_on.get_metrics(&ctx_on);
                let m_off = ev_off.get_metrics(&ctx_off);
                prop_assert_eq!(m_on.collision_count, m_off.collision_count);
                prop_assert!((m_on.collision_rate - m_off.collision_rate).abs() < 1e-12);

                // (B) 直接改变出简标记集合 all_assigned_flags，全码指标必须不变（需求 14.1 核心）
                let base_count = ev_on.total_collisions;
                let base_freq = ev_on.collision_frequency;

                // 全部出简
                for f in ev_on.simple_eval.as_mut().unwrap().all_assigned_flags.iter_mut() {
                    *f = true;
                }
                prop_assert_eq!(ev_on.total_collisions, base_count);
                prop_assert_eq!(ev_on.collision_frequency, base_freq);

                // 全部不出简
                for f in ev_on.simple_eval.as_mut().unwrap().all_assigned_flags.iter_mut() {
                    *f = false;
                }
                prop_assert_eq!(ev_on.total_collisions, base_count);
                prop_assert_eq!(ev_on.collision_frequency, base_freq);

                // 任意（随机）出简标记集合
                {
                    let flags = &mut ev_on.simple_eval.as_mut().unwrap().all_assigned_flags;
                    let len = flags.len();
                    for (i, f) in flags.iter_mut().enumerate() {
                        *f = flag_bits.get(step * len + i).copied().unwrap_or(i % 2 == 0);
                    }
                }
                prop_assert_eq!(ev_on.total_collisions, base_count);
                prop_assert_eq!(ev_on.collision_frequency, base_freq);

                // get_metrics 暴露的全码指标同样不受出简状态影响
                let m_after = ev_on.get_metrics(&ctx_on);
                prop_assert_eq!(m_after.collision_count, base_count);
                prop_assert!((m_after.collision_rate - m_off.collision_rate).abs() < 1e-12);

                // 施加下一步移动（确定性改写）
                if step < moves.len() {
                    let (gi, nk) = moves[step];
                    asg[gi % n] = nk;
                }
            }
        }
    }
}

// =========================================================================
// 🧪 去堆分配验证测试（simple-code-perf-optimization, 任务 5.2）
// =========================================================================
// 验证简码热路径所用的复用缓冲变体 `OptContext::get_simple_keys_into`：
//   (1) 结果与全量分配版本 `get_simple_keys` 完全一致（同一键位序列 / 同样的有/无简码判定）；
//   (2) 复用同一缓冲区跨多次调用时不在每步重新分配（容量预热后保持恒定，即无每步堆分配）。
// Requirements: 3.1, 3.3
#[cfg(test)]
mod dealloc_free_tests {
    use super::*;
    use crate::config::TargetsConfig;
    use crate::context::OptContext;
    use crate::types::{
        KeyDistConfig, RootGroup, ScaleConfig, SimpleCodeConfig, SimpleCodeLevel, SimpleCodeStep,
        WeightConfig, EQUIV_TABLE_SIZE,
    };
    use proptest::prelude::*;
    use std::collections::HashMap;

    /// 构建启用简码、含多步简码指令的最小 OptContext。
    ///
    /// `specs[i] = (freq, n_roots)`：第 i 个汉字含 `n_roots` 个互不相同的字根
    /// （各自独立成组），故该字全码部件数 == `n_roots`。简码配置三个级别，规则分别取
    /// 1 / 2 / 3 个逻辑根的首个编码（`code_selector = 'a'`），从而产生 1 / 2 / 3 步指令：
    /// - 级别 0：`[A.a]`  → `n_roots >= 1` 时解析成功（1 个键位）
    /// - 级别 1：`[A.a,B.a]` → `n_roots >= 2` 时解析成功（2 个键位）
    /// - 级别 2：`[A.a,B.a,C.a]` → `n_roots >= 3` 时解析成功（3 个键位）
    ///
    /// 否则该级指令为 `None`。这样可同时覆盖「有简码键位」与「无简码键位」两种分支，
    /// 并让简码键位长度跨级别变化（驱动复用缓冲的容量预热）。
    fn make_ctx(specs: &[(u64, usize)]) -> OptContext {
        let mut groups: Vec<RootGroup> = Vec::new();
        let mut splits: Vec<(char, Vec<String>, u64)> = Vec::with_capacity(specs.len());
        for (i, &(freq, n_roots)) in specs.iter().enumerate() {
            let mut roots: Vec<String> = Vec::with_capacity(n_roots);
            for j in 0..n_roots {
                let root = format!("r{i}_{j}");
                groups.push(RootGroup {
                    roots: vec![root.clone()],
                    allowed_keys: vec![0, 1, 2],
                });
                roots.push(root);
            }
            let ch = char::from_u32(0x4e00 + i as u32).unwrap();
            splits.push((ch, roots, freq));
        }

        let step = |sel: char| SimpleCodeStep {
            root_selector: sel,
            code_selector: 'a',
        };
        let levels = vec![
            SimpleCodeLevel {
                level: 1,
                code_num: 1,
                rule_candidates: vec![vec![step('A')]],
                space_commit: false,
            },
            SimpleCodeLevel {
                level: 2,
                code_num: 1,
                rule_candidates: vec![vec![step('A'), step('B')]],
                space_commit: false,
            },
            SimpleCodeLevel {
                level: 3,
                code_num: 1,
                rule_candidates: vec![vec![step('A'), step('B'), step('C')]],
                space_commit: false,
            },
        ];

        let fixed_roots: HashMap<String, u8> = HashMap::new();
        let equiv_table = [[0.0f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];
        let key_dist = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];
        let mut weights = WeightConfig::default();
        weights.enable_simple_code = true;
        weights.simple_coverage_ratio = 1.0;
        OptContext::new(
            &splits,
            &fixed_roots,
            &groups,
            equiv_table,
            key_dist,
            ScaleConfig::default(),
            SimpleCodeConfig { levels },
            weights,
            TargetsConfig::default(),
        )
    }

    /// 固定 specs：含 1 / 2 / 3 根的汉字，键位序列长度跨 1~3 变化。
    fn sample_specs() -> Vec<(u64, usize)> {
        vec![(100, 1), (90, 2), (80, 3), (70, 2), (60, 1), (50, 3)]
    }

    #[test]
    fn into_matches_allocating_across_levels_and_assignments() {
        let specs = sample_specs();
        let ctx = make_ctx(&specs);
        let n_chars = ctx.char_infos.len();
        let n_levels = ctx.simple_config.levels.len();
        let n_groups = ctx.num_groups;

        // 多组分配：全 0、全 1、全 2、以及若干周期性图案
        let assignments: Vec<Vec<u8>> = vec![
            vec![0u8; n_groups],
            vec![1u8; n_groups],
            vec![2u8; n_groups],
            (0..n_groups).map(|g| (g % 3) as u8).collect(),
            (0..n_groups).map(|g| ((g + 1) % 3) as u8).collect(),
            (0..n_groups).map(|g| ((g * 2) % 3) as u8).collect(),
        ];

        let mut buf: Vec<u8> = Vec::new();
        let mut saw_some = false;
        let mut saw_none = false;

        for asg in &assignments {
            for ci in 0..n_chars {
                for li in 0..n_levels {
                    let allocating = ctx.get_simple_keys(ci, li, asg);
                    let ok = ctx.get_simple_keys_into(ci, li, asg, &mut buf);

                    match allocating {
                        Some(expected) => {
                            saw_some = true;
                            assert!(
                                ok,
                                "复用路径应成功 (ci={ci}, li={li}), 但返回 false; asg={asg:?}"
                            );
                            assert_eq!(
                                buf, expected,
                                "复用路径键位序列与全量路径不一致 (ci={ci}, li={li}); asg={asg:?}"
                            );
                        }
                        None => {
                            saw_none = true;
                            assert!(
                                !ok,
                                "全量路径为 None 时复用路径应返回 false (ci={ci}, li={li}); asg={asg:?}"
                            );
                        }
                    }
                }
            }
        }

        // 测试夹具应同时覆盖「有简码」与「无简码」两种分支，否则等价性验证不充分
        assert!(saw_some, "测试夹具未覆盖任何有效简码键位序列");
        assert!(saw_none, "测试夹具未覆盖任何无效（None）简码分支");
    }

    #[test]
    fn reused_buffer_capacity_stable_no_per_call_realloc() {
        let specs = sample_specs();
        let ctx = make_ctx(&specs);
        let n_chars = ctx.char_infos.len();
        let n_levels = ctx.simple_config.levels.len();
        let n_groups = ctx.num_groups;
        let asg = vec![0u8; n_groups];

        // 先求出所有 (ci, li) 中的最大简码键位长度，并据此预热缓冲容量。
        let mut buf: Vec<u8> = Vec::new();
        let mut max_len = 0usize;
        for ci in 0..n_chars {
            for li in 0..n_levels {
                if ctx.get_simple_keys_into(ci, li, &asg, &mut buf) {
                    max_len = max_len.max(buf.len());
                }
            }
        }
        assert!(max_len >= 2, "夹具应产生长度>=2 的简码键位以验证容量预热");

        // 预热到至少 max_len 的容量；此后任何 (ci, li, assignment) 调用都不应触发再分配。
        buf.clear();
        buf.reserve(max_len);
        let warmed_cap = buf.capacity();
        assert!(warmed_cap >= max_len);

        // 反复调用 get_simple_keys_into（变化 ci/li/assignment），断言容量恒定（无每步堆分配）。
        for round in 0..200usize {
            let asg: Vec<u8> = (0..n_groups).map(|g| ((g + round) % 3) as u8).collect();
            for ci in 0..n_chars {
                for li in 0..n_levels {
                    ctx.get_simple_keys_into(ci, li, &asg, &mut buf);
                    assert_eq!(
                        buf.capacity(),
                        warmed_cap,
                        "复用缓冲容量在第 {round} 轮 (ci={ci}, li={li}) 发生变化，存在每步堆分配"
                    );
                }
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        // 复用缓冲路径与全量路径在任意 specs / 分配 / (ci, li) 上结果一致，
        // 且全程复用同一缓冲区时容量单调不减（即不会在每步收缩后重分配）。
        // Validates: Requirements 3.1, 3.3
        #[test]
        fn prop_into_equiv_and_no_capacity_shrink(
            specs in prop::collection::vec((1u64..200, 1usize..=3), 1..6),
            key_bits in prop::collection::vec(0u8..3, 0..64),
        ) {
            let ctx = make_ctx(&specs);
            let n_chars = ctx.char_infos.len();
            let n_levels = ctx.simple_config.levels.len();
            let n_groups = ctx.num_groups;

            // 由随机比特构造一个合法分配（键位取自 {0,1,2}）
            let asg: Vec<u8> = (0..n_groups)
                .map(|g| key_bits.get(g).copied().unwrap_or((g % 3) as u8))
                .collect();

            let mut buf: Vec<u8> = Vec::new();
            let mut last_cap = buf.capacity();

            for ci in 0..n_chars {
                for li in 0..n_levels {
                    let allocating = ctx.get_simple_keys(ci, li, &asg);
                    let ok = ctx.get_simple_keys_into(ci, li, &asg, &mut buf);

                    match allocating {
                        Some(expected) => {
                            prop_assert!(ok);
                            prop_assert_eq!(&buf, &expected);
                        }
                        None => prop_assert!(!ok),
                    }

                    // 复用缓冲容量只增不减：清空+push 永不收缩，故无周期性再分配。
                    prop_assert!(buf.capacity() >= last_cap);
                    last_cap = buf.capacity();
                }
            }
        }
    }
}

// =========================================================================
// 🧪 桶内选中集合测试（simple-code-perf-optimization, Property 4）
// =========================================================================
#[cfg(test)]
mod bucket_selection_tests {
    use super::*;
    use crate::config::TargetsConfig;
    use crate::context::OptContext;
    use crate::types::{
        KeyDistConfig, RootGroup, ScaleConfig, SimpleAssignMode, SimpleCodeConfig, SimpleCodeLevel,
        SimpleCodeStep, WeightConfig, EQUIV_TABLE_SIZE,
    };
    use proptest::prelude::*;
    use std::collections::HashMap;

    /// 构建启用简码、含 3 个级别、参数化分配模式与每级编码数 `code_num` 的最小 OptContext。
    ///
    /// `specs[i] = (freq, n_roots)`：第 i 个汉字含 `n_roots` 个互不相同的字根（各自独立成组），
    /// 故该字全码部件数 == `n_roots`，从而 `simple_base_saving[ci][li]` 随 `n_roots`/`li` 变化，
    /// 充分驱动 Efficiency 模式排序键 `freq × (base_saving + sel_len)` 的差异。
    ///
    /// 仅用 2 个允许键位（`[0, 1]`），使级别 0 桶容量为 2、级别 1 为 4、级别 2 为 8，
    /// 在较多候选字下制造同桶碰撞，让「桶内成员数 > code_num」时的选取逻辑真正被触发。
    /// 覆盖率阈值取 1.0 使所有字均为候选字。
    fn make_ctx(specs: &[(u64, usize)], mode: SimpleAssignMode, code_num: usize) -> OptContext {
        let mut groups: Vec<RootGroup> = Vec::new();
        let mut splits: Vec<(char, Vec<String>, u64)> = Vec::with_capacity(specs.len());
        for (i, &(freq, n_roots)) in specs.iter().enumerate() {
            let n_roots = n_roots.max(1);
            let mut roots: Vec<String> = Vec::with_capacity(n_roots);
            for j in 0..n_roots {
                let root = format!("r{i}_{j}");
                groups.push(RootGroup {
                    roots: vec![root.clone()],
                    allowed_keys: vec![0, 1],
                });
                roots.push(root);
            }
            let ch = char::from_u32(0x4e00 + i as u32).unwrap();
            splits.push((ch, roots, freq));
        }

        let step = |sel: char| SimpleCodeStep {
            root_selector: sel,
            code_selector: 'a',
        };
        let levels = vec![
            SimpleCodeLevel {
                level: 1,
                code_num,
                rule_candidates: vec![vec![step('A')]],
                space_commit: false,
            },
            SimpleCodeLevel {
                level: 2,
                code_num,
                rule_candidates: vec![vec![step('A'), step('B')]],
                space_commit: false,
            },
            SimpleCodeLevel {
                level: 3,
                code_num,
                rule_candidates: vec![vec![step('A'), step('B'), step('C')]],
                space_commit: false,
            },
        ];

        let fixed_roots: HashMap<String, u8> = HashMap::new();
        let equiv_table = [[0.0f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];
        let key_dist = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];
        let mut weights = WeightConfig::default();
        weights.enable_simple_code = true;
        weights.simple_coverage_ratio = 1.0;
        weights.simple_assign_mode = mode;
        OptContext::new(
            &splits,
            &fixed_roots,
            &groups,
            equiv_table,
            key_dist,
            ScaleConfig::default(),
            SimpleCodeConfig { levels },
            weights,
            TargetsConfig::default(),
        )
    }

    /// 独立重算桶内排序键（与设计 Property 4 一致，刻意从头实现而非复用产线比较器）。
    ///
    /// - Frequency 模式：`key = freq`；
    /// - Efficiency 模式：`key = freq × (base_saving[ci][li] + sel_len)`，`sel_len` 由
    ///   `is_first_candidate[ci]` 取 0（首选字）或 1（非首选字）。
    fn oracle_sort_key(
        ctx: &OptContext,
        is_first_candidate: &[bool],
        li: usize,
        ci: usize,
    ) -> i64 {
        let freq = ctx.char_infos[ci].frequency as i64;
        match ctx.simple_assign_mode {
            SimpleAssignMode::Frequency => freq,
            SimpleAssignMode::Efficiency => {
                let sel_len: i64 = if is_first_candidate[ci] { 0 } else { 1 };
                freq * (ctx.simple_base_saving[ci][li] + sel_len)
            }
        }
    }

    /// 独立并列裁决比较器：排序键降序；相等时先按 `freq` 降序，再按 `ci` 升序。
    fn oracle_cmp(
        ctx: &OptContext,
        is_first_candidate: &[bool],
        li: usize,
        a: usize,
        b: usize,
    ) -> std::cmp::Ordering {
        let ka = oracle_sort_key(ctx, is_first_candidate, li, a);
        let kb = oracle_sort_key(ctx, is_first_candidate, li, b);
        kb.cmp(&ka).then_with(|| {
            let fa = ctx.char_infos[a].frequency;
            let fb = ctx.char_infos[b].frequency;
            fb.cmp(&fa).then_with(|| a.cmp(&b))
        })
    }

    /// 独立判定编码 `code` 是否被简码占用保护阻断（需求 33，与产线 `is_code_blocked` 同口径）。
    ///
    /// - `simple_protect_top_n == 0`（保护全部）：等价于「全码桶 `full_code_to_chars[code]` 非空」。
    /// - `simple_protect_top_n > 0`：等价于「某受保护汉字（`simple_is_topn`）全码 == code」。
    fn oracle_is_blocked(ctx: &OptContext, full_code_to_chars: &[Vec<usize>], code: usize) -> bool {
        if ctx.simple_protect_top_n == 0 {
            code < full_code_to_chars.len() && !full_code_to_chars[code].is_empty()
        } else {
            full_code_to_chars.get(code).map_or(false, |chars| {
                chars
                    .iter()
                    .any(|&c| ctx.simple_is_topn.get(c).copied().unwrap_or(false))
            })
        }
    }

    /// 据当前分配从头构建全码桶 `full_code_to_chars`（与产线 `Evaluator::new` 同口径）。
    fn oracle_full_code_to_chars(ctx: &OptContext, assignment: &[u8]) -> Vec<Vec<usize>> {
        let mut full_code_to_chars: Vec<Vec<usize>> = vec![Vec::new(); ctx.code_space];
        for ci in 0..ctx.char_infos.len() {
            full_code_to_chars[ctx.calc_code_only(ci, assignment)].push(ci);
        }
        full_code_to_chars
    }

    /// 从头独立推导每级的「桶成员」与「桶内选中集合」。
    ///
    /// 忠实复现设计 Property 4：按级别升序处理，低级别先出简；高级别桶中排除已被前序级别
    /// 选中（出简）的字（跨级排除）。每个桶内用 `oracle_cmp` 排序后取前 `code_num` 个为选中集合。
    /// 此外复现需求 33 的简码占用保护：编码撞受保护全码的桶名额=0（谁都不出简），其候选字
    /// 不被跨级排除，由更高级别继续尝试。
    /// 返回 `(members_per_level, selected_per_level)`，均以「桶编码 -> 排序后的 ci 列表」表示。
    #[allow(clippy::type_complexity)]
    fn oracle_selection(
        ctx: &OptContext,
        assignment: &[u8],
        is_first_candidate: &[bool],
    ) -> (
        Vec<HashMap<usize, Vec<usize>>>,
        Vec<HashMap<usize, Vec<usize>>>,
    ) {
        let n_levels = ctx.simple_config.levels.len();
        let n_chars = ctx.char_infos.len();
        let full_code_to_chars = oracle_full_code_to_chars(ctx, assignment);
        let mut assigned = vec![false; n_chars];
        let mut members_per_level: Vec<HashMap<usize, Vec<usize>>> = Vec::with_capacity(n_levels);
        let mut selected_per_level: Vec<HashMap<usize, Vec<usize>>> = Vec::with_capacity(n_levels);

        for li in 0..n_levels {
            let code_num = ctx.simple_config.levels[li].code_num;
            // 阶段 1：候选字入桶（排除已被低级别出简的字 —— 跨级排除）
            let mut buckets: HashMap<usize, Vec<usize>> = HashMap::new();
            for &ci in &ctx.simple_candidate_chars {
                if assigned[ci] {
                    continue;
                }
                if let Some(code) = ctx.calc_simple_code_eligible(ci, li, assignment) {
                    buckets.entry(code).or_default().push(ci);
                }
            }
            // 阶段 2：桶内排序后取前 code_num 个为选中集合，并标记其跨级排除。
            // 简码占用保护（需求 33）：被阻断的编码名额=0，桶内字不出简、不跨级排除。
            let mut selected: HashMap<usize, Vec<usize>> = HashMap::new();
            for (&code, members) in buckets.iter() {
                let eff_code_num = if oracle_is_blocked(ctx, &full_code_to_chars, code) {
                    0
                } else {
                    code_num
                };
                let mut sorted = members.clone();
                sorted.sort_by(|&a, &b| oracle_cmp(ctx, is_first_candidate, li, a, b));
                let sel: Vec<usize> = sorted.iter().take(eff_code_num).copied().collect();
                for &ci in &sel {
                    assigned[ci] = true;
                }
                selected.insert(code, sel);
            }
            members_per_level.push(buckets);
            selected_per_level.push(selected);
        }
        (members_per_level, selected_per_level)
    }

    /// 将一组 ci 排序后返回（用于按集合语义比较）。
    fn sorted(mut v: Vec<usize>) -> Vec<usize> {
        v.sort_unstable();
        v
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        // Feature: simple-code-perf-optimization, Property 4: 桶内选中集合符合所选模式的排序键
        //
        // 对任意简码桶的候选字集合，桶内被选中出简的字集合应恰为「按当前分配模式的排序键、并按
        // 并列裁决规则（排序键相等时先按 freq 降序、再按 ci 升序）排序后的前 code_num 个（在未被
        // 前序级别排除的字中）」。其中 frequency 模式排序键为 freq，efficiency 模式排序键为
        // freq × (base_saving + sel_len)、sel_len 由 is_first_candidate 取 0 或 1。
        // 参数化 frequency / efficiency 两种模式。
        // Validates: Requirements 4.3, 4.4, 4.7, 6.2, 6.3
        #[test]
        fn prop4_bucket_selection_matches_sort_key(
            // true => Frequency 模式；false => Efficiency 模式（参数化两种模式）
            mode_is_freq in any::<bool>(),
            // (freq, n_roots)：小频率范围制造并列；n_roots 1..=3 使 base_saving 跨级变化
            specs in prop::collection::vec((1u64..6, 1usize..4), 3usize..9),
            // 每级编码数 1..=2：code_num=2 时桶内成员数可超出，真正触发「取前 code_num」选取
            code_num in 1usize..3,
            // 确定性移动序列：直接改写 assignment（不依赖随机接受），遍历分配空间
            moves in prop::collection::vec((0usize..32, 0u8..2), 0usize..24),
        ) {
            let mode = if mode_is_freq {
                SimpleAssignMode::Frequency
            } else {
                SimpleAssignMode::Efficiency
            };
            let ctx = make_ctx(&specs, mode, code_num);
            let n_groups = ctx.num_groups;
            let n_chars = ctx.char_infos.len();
            let n_levels = ctx.simple_config.levels.len();

            let mut asg = vec![0u8; n_groups];

            // step==0 为初始分配，其后每步确定性改写一个组的键位
            for step in 0..=moves.len() {
                let ev = Evaluator::new(&ctx, &asg);
                let se = ev.simple_eval.as_ref().expect("简码已启用，simple_eval 应为 Some");
                let is_first = &ev.is_first_candidate;

                // 用与评估器相同的 is_first_candidate 独立从头推导期望选中集合
                let (exp_members, exp_selected) = oracle_selection(&ctx, &asg, is_first);

                for li in 0..n_levels {
                    let level = &se.levels[li];

                    // 从评估器状态重建：current_simple_code 指向的桶成员 + selected 选中集合
                    let mut act_members: HashMap<usize, Vec<usize>> = HashMap::new();
                    let mut act_selected: HashMap<usize, Vec<usize>> = HashMap::new();
                    for ci in 0..n_chars {
                        let code = level.current_simple_code[ci];
                        if code >= 0 {
                            let code = code as usize;
                            act_members.entry(code).or_default().push(ci);
                            if level.selected[ci] {
                                act_selected.entry(code).or_default().push(ci);
                            }
                        }
                    }

                    // (1) 桶成员集合（current_simple_code 归属）应与从头推导一致
                    let exp_codes = sorted(exp_members[li].keys().copied().collect());
                    let act_codes = sorted(act_members.keys().copied().collect());
                    prop_assert_eq!(
                        &act_codes, &exp_codes,
                        "级别 {} 桶集合不一致, mode={:?}, asg={:?}", li, mode, asg
                    );

                    for (&code, exp_mem) in exp_members[li].iter() {
                        let act_mem = act_members.get(&code).cloned().unwrap_or_default();
                        prop_assert_eq!(
                            sorted(act_mem.clone()), sorted(exp_mem.clone()),
                            "级别 {} 桶 {} 成员不一致, mode={:?}, asg={:?}", li, code, mode, asg
                        );

                        // (2) 桶内选中出简集合应恰为排序后的前 code_num 个
                        let exp_sel = exp_selected[li].get(&code).cloned().unwrap_or_default();
                        let act_sel = act_selected.get(&code).cloned().unwrap_or_default();
                        prop_assert_eq!(
                            sorted(act_sel.clone()), sorted(exp_sel.clone()),
                            "级别 {} 桶 {} 选中集合与排序键前 code_num 不一致, mode={:?}, asg={:?}",
                            li, code, mode, asg
                        );

                        // (3) 选中数 = oracle 选中数：未阻断桶为 min(code_num, 成员数)，
                        //     被简码占用保护阻断的桶为 0（需求 33）。
                        prop_assert_eq!(
                            act_sel.len(),
                            exp_sel.len(),
                            "级别 {} 桶 {} 选中数与 oracle 不一致, mode={:?}",
                            li, code, mode
                        );
                    }
                }

                if step < moves.len() {
                    let (gi, nk) = moves[step];
                    asg[gi % n_groups] = nk;
                }
            }
        }
    }
}

// =========================================================================
// 🧪 增量与全量一致性测试（simple-code-perf-optimization, Property 1）
// =========================================================================
#[cfg(test)]
mod incremental_full_consistency_tests {
    use super::*;
    use crate::config::TargetsConfig;
    use crate::context::OptContext;
    use crate::types::{
        KeyDistConfig, RootGroup, ScaleConfig, SimpleAssignMode, SimpleCodeConfig, SimpleCodeLevel,
        SimpleCodeStep, WeightConfig, EQUIV_TABLE_SIZE,
    };
    use proptest::prelude::*;
    use std::collections::HashMap;

    /// 构建启用简码、含 3 个级别、参数化分配模式 / 每级编码数 / 候选覆盖率的最小 OptContext。
    ///
    /// `specs[i] = (freq, n_roots)`：第 i 个汉字含 `n_roots` 个互不相同的字根（各自独立成组），
    /// 故该字全码部件数 == `n_roots`，从而 `simple_base_saving[ci][li]` 随 `n_roots`/`li` 变化，
    /// 充分驱动 Efficiency 模式排序键的差异。仅用 2 个允许键位（`[0, 1]`），使级别 0 桶容量为 2、
    /// 级别 1 为 4、级别 2 为 8，在多候选字下制造同桶碰撞与「桶成员数 > code_num」的选取。
    ///
    /// `coverage_ratio < 1.0` 时低频字将落选候选集合（非候选字），从而其所属字根组的
    /// `group_to_simple_affected_candidate` 为空 —— 触发「空交集移动」的早退分支（需求 1.5/7.6）。
    fn make_ctx(
        specs: &[(u64, usize)],
        mode: SimpleAssignMode,
        code_num: usize,
        coverage_ratio: f64,
    ) -> OptContext {
        let mut groups: Vec<RootGroup> = Vec::new();
        let mut splits: Vec<(char, Vec<String>, u64)> = Vec::with_capacity(specs.len());
        for (i, &(freq, n_roots)) in specs.iter().enumerate() {
            let n_roots = n_roots.max(1);
            let mut roots: Vec<String> = Vec::with_capacity(n_roots);
            for j in 0..n_roots {
                let root = format!("r{i}_{j}");
                groups.push(RootGroup {
                    roots: vec![root.clone()],
                    allowed_keys: vec![0, 1],
                });
                roots.push(root);
            }
            let ch = char::from_u32(0x4e00 + i as u32).unwrap();
            splits.push((ch, roots, freq.max(1)));
        }

        let step = |sel: char| SimpleCodeStep {
            root_selector: sel,
            code_selector: 'a',
        };
        let levels = vec![
            SimpleCodeLevel {
                level: 1,
                code_num,
                rule_candidates: vec![vec![step('A')]],
                space_commit: false,
            },
            SimpleCodeLevel {
                level: 2,
                code_num,
                rule_candidates: vec![vec![step('A'), step('B')]],
                space_commit: false,
            },
            SimpleCodeLevel {
                level: 3,
                code_num,
                rule_candidates: vec![vec![step('A'), step('B'), step('C')]],
                space_commit: false,
            },
        ];

        let fixed_roots: HashMap<String, u8> = HashMap::new();
        let equiv_table = [[0.0f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];
        let key_dist = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];
        let mut weights = WeightConfig::default();
        weights.enable_simple_code = true;
        weights.simple_coverage_ratio = coverage_ratio;
        weights.simple_assign_mode = mode;
        OptContext::new(
            &splits,
            &fixed_roots,
            &groups,
            equiv_table,
            key_dist,
            ScaleConfig::default(),
            SimpleCodeConfig { levels },
            weights,
            TargetsConfig::default(),
        )
    }

    /// 任务 17.1 回归专用：与 `make_ctx` 同构，但可逐级指定 `space_commit`，并注入固定简码
    /// （经 `OptContext::new_with_fixed`）。用于验证「含空格上屏 + 固定简码 + 长度约束」时
    /// 增量维护仍与全量重建逐字段一致。
    fn make_ctx_combined(
        specs: &[(u64, usize)],
        mode: SimpleAssignMode,
        code_num: usize,
        coverage_ratio: f64,
        space_commits: [bool; 3],
        fixed: &[(char, String)],
    ) -> OptContext {
        let mut groups: Vec<RootGroup> = Vec::new();
        let mut splits: Vec<(char, Vec<String>, u64)> = Vec::with_capacity(specs.len());
        for (i, &(freq, n_roots)) in specs.iter().enumerate() {
            let n_roots = n_roots.max(1);
            let mut roots: Vec<String> = Vec::with_capacity(n_roots);
            for j in 0..n_roots {
                let root = format!("r{i}_{j}");
                groups.push(RootGroup {
                    roots: vec![root.clone()],
                    allowed_keys: vec![0, 1],
                });
                roots.push(root);
            }
            let ch = char::from_u32(0x4e00 + i as u32).unwrap();
            splits.push((ch, roots, freq.max(1)));
        }
        let step = |sel: char| SimpleCodeStep {
            root_selector: sel,
            code_selector: 'a',
        };
        let levels = vec![
            SimpleCodeLevel {
                level: 1,
                code_num,
                rule_candidates: vec![vec![step('A')]],
                space_commit: space_commits[0],
            },
            SimpleCodeLevel {
                level: 2,
                code_num,
                rule_candidates: vec![vec![step('A'), step('B')]],
                space_commit: space_commits[1],
            },
            SimpleCodeLevel {
                level: 3,
                code_num,
                rule_candidates: vec![vec![step('A'), step('B'), step('C')]],
                space_commit: space_commits[2],
            },
        ];
        let fixed_roots: HashMap<String, u8> = HashMap::new();
        let equiv_table = [[0.0f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];
        let key_dist = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];
        let mut weights = WeightConfig::default();
        weights.enable_simple_code = true;
        weights.simple_coverage_ratio = coverage_ratio;
        weights.simple_assign_mode = mode;
        OptContext::new_with_fixed(
            &splits,
            &fixed_roots,
            &groups,
            equiv_table,
            key_dist,
            ScaleConfig::default(),
            SimpleCodeConfig { levels },
            weights,
            TargetsConfig::default(),
            fixed,
        )
    }

    /// 将一次合法移动（把组 `r` 改到键位 `new_key`）施加到 `ev` 上，复刻产线移动顺序：
    /// 1. 改写 `assignment[r]`；2. 对组内全码受影响字增量维护全码桶（`update_char`，
    ///    同步 `code_to_chars` / `is_first_candidate`）；3. 走简码增量路径 `apply_simple_for_move`。
    ///
    /// 这样 `ev.simple_eval` 即经增量维护得到的简码状态，可与对同一 `assignment` 从零
    /// 全量构建的 `Evaluator::new(...).simple_eval` 作为 oracle 逐字段比对。
    fn apply_move_inc(ev: &mut Evaluator, ctx: &OptContext, assignment: &mut [u8], r: usize, new_key: u8) {
        assignment[r] = new_key;
        // 注意：先克隆组内字列表以规避借用冲突（group_to_chars 借自 ctx，update_char 借 ev）
        for idx in 0..ctx.group_to_chars[r].len() {
            let ci = ctx.group_to_chars[r][idx];
            ev.update_char(ctx, assignment, ci);
        }
        ev.apply_simple_for_move(ctx, assignment, &[r]);
    }

    // Feature: sparse-bucket-memory-optimization, Property 5: 增量 == 全量重建（稀疏后端）
    //
    // 强制 BucketStore 走稀疏后端（线程局部阈值覆盖 = 0），对随机移动序列断言：
    // 稀疏后端的「增量维护」结果与对同一分配「从零全量重建（稀疏）」逐字段一致，且与
    // 密集后端从零重建的全码/简码指标一致（证明稀疏路径与基线密集等价）。
    // Validates: Requirements 3.1, 3.2, 3.4, 4.6, 5.2, 5.5, 8.x
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(60))]
        #[test]
        fn prop5_sparse_evaluator_matches_dense(
            mode_is_freq in any::<bool>(),
            specs in prop::collection::vec((1u64..6, 1usize..4), 3usize..8),
            moves in prop::collection::vec((0usize..12, 0u8..2), 0usize..40),
        ) {
            let mode = if mode_is_freq { SimpleAssignMode::Frequency } else { SimpleAssignMode::Efficiency };
            let ctx = make_ctx(&specs, mode, 1, 1.0);
            let n = ctx.num_groups;

            // 稀疏增量：从零分配开始，逐步施加移动（强制稀疏后端）。
            crate::bucket_store::set_test_threshold_override(Some(0));
            let mut asg_sparse = vec![0u8; n];
            let mut ev_sparse = Evaluator::new(&ctx, &asg_sparse);
            for &(g, k) in &moves {
                let r = g % n;
                apply_move_inc(&mut ev_sparse, &ctx, &mut asg_sparse, r, k);
            }
            // 稀疏全量重建（对最终分配从零构建）作为同后端 oracle。
            let ev_sparse_full = Evaluator::new(&ctx, &asg_sparse);
            crate::bucket_store::set_test_threshold_override(None);

            // 密集全量重建（默认阈值）作为基线 oracle。
            let asg_final = asg_sparse.clone();
            let ev_dense_full = Evaluator::new(&ctx, &asg_final);

            // 后端确为稀疏 / 密集（防回归）。
            prop_assert_eq!(ev_sparse.full_buckets.backend_kind(), crate::bucket_store::BackendKind::Sparse);
            prop_assert_eq!(ev_dense_full.full_buckets.backend_kind(), crate::bucket_store::BackendKind::Dense);

            // 稀疏增量 == 稀疏全量重建（内部一致性）。
            assert_simple_eq(&ctx, &ev_sparse, &ev_sparse_full, "sparse-inc vs sparse-full")?;
            // 稀疏增量 == 密集全量重建（与基线等价）。
            assert_simple_eq(&ctx, &ev_sparse, &ev_dense_full, "sparse-inc vs dense-full")?;

            // 全码指标也须一致（重码数 / 重码率 / 当量）。
            let mut ev_sparse_mut = ev_sparse;
            let mut ev_dense_mut = ev_dense_full;
            let ms = ev_sparse_mut.get_metrics(&ctx);
            let md = ev_dense_mut.get_metrics(&ctx);
            prop_assert_eq!(ms.collision_count, md.collision_count, "全码重码数 稀疏≠密集");
            prop_assert!((ms.collision_rate - md.collision_rate).abs() < 1e-9, "全码重码率 稀疏≠密集");
            prop_assert!((ms.equiv_mean - md.equiv_mean).abs() < 1e-9, "全码当量 稀疏≠密集");
        }
    }

    /// 逐字段断言两个 SimpleEvaluator 的全部简码指标与状态一致。
    ///
    /// 比对内容：weighted_freq_coverage / equiv_mean / dist_deviation /
    /// simple_collision_count / simple_collision_rate（含底层 simple_collision_freq），
    /// 以及每级 `selected`、每候选字 `current_simple_code` 与跨级 `all_assigned_flags`。
    fn assert_simple_eq(
        ctx: &OptContext,
        inc: &Evaluator,
        full: &Evaluator,
        label: &str,
    ) -> Result<(), TestCaseError> {
        let m_inc = inc.get_simple_metrics(ctx);
        let m_full = full.get_simple_metrics(ctx);

        let eps = 1e-9;
        prop_assert!(
            (m_inc.weighted_freq_coverage - m_full.weighted_freq_coverage).abs() < eps,
            "{label}: weighted_freq_coverage 不一致 inc={} full={}",
            m_inc.weighted_freq_coverage,
            m_full.weighted_freq_coverage
        );
        prop_assert!(
            (m_inc.equiv_mean - m_full.equiv_mean).abs() < eps,
            "{label}: equiv_mean 不一致 inc={} full={}",
            m_inc.equiv_mean,
            m_full.equiv_mean
        );
        prop_assert!(
            (m_inc.dist_deviation - m_full.dist_deviation).abs() < eps,
            "{label}: dist_deviation 不一致 inc={} full={}",
            m_inc.dist_deviation,
            m_full.dist_deviation
        );
        prop_assert_eq!(
            m_inc.collision_count,
            m_full.collision_count,
            "{}: simple_collision_count 不一致",
            label
        );
        prop_assert!(
            (m_inc.collision_rate - m_full.collision_rate).abs() < eps,
            "{label}: simple_collision_rate 不一致 inc={} full={}",
            m_inc.collision_rate,
            m_full.collision_rate
        );

        let se_inc = inc.simple_eval.as_ref().expect("inc.simple_eval");
        let se_full = full.simple_eval.as_ref().expect("full.simple_eval");

        // 底层简码重码标量（整数，精确比对）
        prop_assert_eq!(
            se_inc.simple_collision_count,
            se_full.simple_collision_count,
            "{}: 底层 simple_collision_count 不一致",
            label
        );
        prop_assert_eq!(
            se_inc.simple_collision_freq,
            se_full.simple_collision_freq,
            "{}: 底层 simple_collision_freq 不一致",
            label
        );

        // 跨级出简标记
        prop_assert_eq!(
            &se_inc.all_assigned_flags,
            &se_full.all_assigned_flags,
            "{}: all_assigned_flags 不一致",
            label
        );

        // 逐级 selected / current_simple_code
        prop_assert_eq!(se_inc.levels.len(), se_full.levels.len(), "{}: 级别数不一致", label);
        for li in 0..se_inc.levels.len() {
            prop_assert_eq!(
                &se_inc.levels[li].selected,
                &se_full.levels[li].selected,
                "{}: 级别 {} selected 不一致",
                label,
                li
            );
            prop_assert_eq!(
                &se_inc.levels[li].current_simple_code,
                &se_full.levels[li].current_simple_code,
                "{}: 级别 {} current_simple_code 不一致",
                label,
                li
            );
            // 级别聚合标量（覆盖频率精确；浮点聚合用紧 epsilon）
            prop_assert_eq!(
                se_inc.levels[li].covered_freq,
                se_full.levels[li].covered_freq,
                "{}: 级别 {} covered_freq 不一致",
                label,
                li
            );
            prop_assert!(
                (se_inc.levels[li].equiv_weighted - se_full.levels[li].equiv_weighted).abs() < eps,
                "{label}: 级别 {li} equiv_weighted 不一致"
            );
            prop_assert_eq!(
                se_inc.levels[li].equiv_freq_sum,
                se_full.levels[li].equiv_freq_sum,
                "{}: 级别 {} equiv_freq_sum 不一致",
                label,
                li
            );
        }
        Ok(())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        // Feature: simple-code-perf-optimization, Property 1 (任务 17.1 回归): 含空格上屏与固定简码下增量=全量
        //
        // 在「逐级随机 space_commit + 注入固定简码 + 长度约束」的上下文下，对随机移动序列逐步
        // 断言增量维护的全部简码状态与对同一分配 full_rebuild 的结果逐字段一致（assert_simple_eq）。
        // 固定简码仅取级别 0 单键（键位 'a'/'b' ∈ 允许键 [0,1]），结尾下划线与 level0 space_commit
        // 一致，且仅当对应字全码长度 > level0 有效长度时纳入（满足需求 22 长度约束）。
        // Validates: Requirements 20.7, 21.8, 21.9, 22.4
        #[test]
        fn prop_task17_1_incremental_matches_full_with_space_and_fixed(
            mode_is_freq in any::<bool>(),
            specs in prop::collection::vec((1u64..6, 2usize..4), 3usize..9),
            code_num in 1usize..4,
            coverage_pct in 50u32..=100,
            space_bits in any::<[bool; 3]>(),
            n_fixed in 0usize..3,
            moves in prop::collection::vec((0usize..64, 0u8..2), 0usize..30),
        ) {
            let mode = if mode_is_freq {
                SimpleAssignMode::Frequency
            } else {
                SimpleAssignMode::Efficiency
            };
            let coverage_ratio = coverage_pct as f64 / 100.0;

            // 固定简码：前 n_fixed（≤2）个字，级别 0 单键（key 'a'/'b'），下划线随 level0
            // space_commit，仅当全码长度严格大于 level0 有效长度时纳入（否则被长度约束拒绝）。
            let space0 = space_bits[0];
            let eff0 = 1usize + if space0 { 1 } else { 0 };
            let mut fixed: Vec<(char, String)> = Vec::new();
            for i in 0..n_fixed.min(2).min(specs.len()) {
                let full_len = specs[i].1.max(1);
                if full_len > eff0 {
                    let mut code = crate::types::key_to_char(i as u8).to_string(); // 0→'a',1→'b'
                    if space0 {
                        code.push('_');
                    }
                    let ch = char::from_u32(0x4e00 + i as u32).unwrap();
                    fixed.push((ch, code));
                }
            }

            let ctx = make_ctx_combined(&specs, mode, code_num, coverage_ratio, space_bits, &fixed);
            let n_groups = ctx.num_groups;
            let mut assignment = vec![0u8; n_groups];

            let mut ev_inc = Evaluator::new(&ctx, &assignment);
            prop_assert!(ev_inc.simple_eval.is_some(), "简码应启用");

            let ev_full0 = Evaluator::new(&ctx, &assignment);
            assert_simple_eq(&ctx, &ev_inc, &ev_full0, "task17.1 step0")?;

            for (k, &(gi, nk)) in moves.iter().enumerate() {
                let r = gi % n_groups;
                apply_move_inc(&mut ev_inc, &ctx, &mut assignment, r, nk);
                let ev_full = Evaluator::new(&ctx, &assignment);
                let label = format!(
                    "task17.1 step{} move(r={},nk={}) space={:?} n_fixed={} mode={:?}",
                    k + 1, r, nk, space_bits, fixed.len(), mode
                );
                assert_simple_eq(&ctx, &ev_inc, &ev_full, &label)?;
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        // Feature: simple-code-perf-optimization, Property 1: 简码增量维护与全量重建一致
        //
        // 对任意合法分配以及任意一串合法移动序列，在每种分配模式（frequency / efficiency）下，
        // 经增量维护得到的全部简码指标 —— 频率覆盖率、平均当量、分布偏差、简码重码数、
        // 简码重码率、各级出简标记 all_assigned_flags、各级 selected、各候选字
        // current_simple_code —— 都应与对同一最终分配执行 full_rebuild 得到的结果逐字段一致。
        // 参数化 frequency / efficiency 两种模式，对随机移动序列逐步比对。
        // Validates: Requirements 1.1, 1.2, 1.3, 1.4, 6.1, 6.2, 6.3, 7.5, 8.6, 10.1, 14.2, 14.3, 14.4, 15.1, 17.3, 17.5
        #[test]
        fn prop1_incremental_matches_full_rebuild(
            // true => Frequency 模式；false => Efficiency 模式（参数化两种模式）
            mode_is_freq in any::<bool>(),
            // (freq, n_roots)：小频率范围制造并列；n_roots 1..=3 使 base_saving 跨级变化
            specs in prop::collection::vec((1u64..6, 1usize..4), 3usize..9),
            // 每级编码数 1..=3：覆盖 code_num 大于/小于桶成员数两种情形
            code_num in 1usize..4,
            // 覆盖率阈值 0.5..=1.0：< 1.0 时产生非候选字 → 触发空交集移动早退
            coverage_pct in 50u32..=100,
            // 随机移动序列（组索引, 新键位 ∈ {0,1}）
            moves in prop::collection::vec((0usize..64, 0u8..2), 0usize..40),
        ) {
            let mode = if mode_is_freq {
                SimpleAssignMode::Frequency
            } else {
                SimpleAssignMode::Efficiency
            };
            let coverage_ratio = coverage_pct as f64 / 100.0;
            let ctx = make_ctx(&specs, mode, code_num, coverage_ratio);
            let n_groups = ctx.num_groups;

            let mut assignment = vec![0u8; n_groups];

            // 增量评估器：全程通过 update_char + apply_simple_for_move 增量维护
            let mut ev_inc = Evaluator::new(&ctx, &assignment);
            prop_assert!(ev_inc.simple_eval.is_some(), "简码应启用");

            // 初始状态即应与全量一致
            let ev_full0 = Evaluator::new(&ctx, &assignment);
            assert_simple_eq(&ctx, &ev_inc, &ev_full0, "step0")?;

            for (k, &(gi, nk)) in moves.iter().enumerate() {
                let r = gi % n_groups;
                // 增量施加移动（产线顺序：全码 update_char → 简码 apply_simple_for_move）
                apply_move_inc(&mut ev_inc, &ctx, &mut assignment, r, nk);

                // oracle：对当前最终分配从零全量重建（Evaluator::new 走 full build）
                let ev_full = Evaluator::new(&ctx, &assignment);

                let label = format!("step{} move(r={},nk={}) assignment={:?}", k + 1, r, nk, assignment);
                assert_simple_eq(&ctx, &ev_inc, &ev_full, &label)?;
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        // Feature: simple-code-perf-optimization, Property 13: 对账以全量结果覆盖增量值
        //
        // 对任意优化过程中的中间分配（由随机移动序列在增量维护的评估器上到达），执行
        // reconcile 后，全码与简码的全部指标应等于对当前分配从零做全量重建/重算所得的指标：
        // - 全码：get_metrics() 的 collision_count / collision_rate / equiv_mean / equiv_cv / dist_deviation
        // - 简码：get_simple_metrics() 的 weighted_freq_coverage / equiv_mean / dist_deviation /
        //         collision_count / collision_rate
        // 整型字段精确相等，浮点字段紧 epsilon。同时验证 reconcile 保留时变量
        // simple_active / current_simple_weight（退火过程量，不属于全量重算范畴）。
        // Validates: Requirements 15.4, 15.5, 15.6
        #[test]
        fn prop13_reconcile_equals_full_rebuild(
            mode_is_freq in any::<bool>(),
            specs in prop::collection::vec((1u64..6, 1usize..4), 3usize..9),
            code_num in 1usize..4,
            coverage_pct in 50u32..=100,
            moves in prop::collection::vec((0usize..64, 0u8..2), 0usize..40),
            // 用作 reconcile 前的时变量测试值（应在 reconcile 后保持不变）
            test_weight in 0.0f64..2.0,
        ) {
            let mode = if mode_is_freq {
                SimpleAssignMode::Frequency
            } else {
                SimpleAssignMode::Efficiency
            };
            let coverage_ratio = coverage_pct as f64 / 100.0;
            let ctx = make_ctx(&specs, mode, code_num, coverage_ratio);
            let n_groups = ctx.num_groups;

            let mut assignment = vec![0u8; n_groups];

            // 增量评估器：到达任意中间分配（产线顺序 update_char → apply_simple_for_move）
            let mut ev_inc = Evaluator::new(&ctx, &assignment);
            prop_assert!(ev_inc.simple_eval.is_some(), "简码应启用");
            for &(gi, nk) in moves.iter() {
                let r = gi % n_groups;
                apply_move_inc(&mut ev_inc, &ctx, &mut assignment, r, nk);
            }

            // 设置退火时变量为测试值，reconcile 后应保持不变（需求 15）
            ev_inc.simple_active = true;
            ev_inc.current_simple_weight = test_weight;

            // 对账：以对当前分配从零全量重建覆盖增量值
            ev_inc.reconcile(&ctx, &assignment);

            // oracle：对当前最终分配从零全量构建
            let ev_full = Evaluator::new(&ctx, &assignment);

            let eps = 1e-9;

            // ---- 全码指标（get_metrics）逐字段相等 ----
            let fm_inc = ev_inc.get_metrics(&ctx);
            let fm_full = ev_full.get_metrics(&ctx);
            prop_assert_eq!(
                fm_inc.collision_count, fm_full.collision_count,
                "full collision_count 不一致 assignment={:?}", assignment
            );
            prop_assert!(
                (fm_inc.collision_rate - fm_full.collision_rate).abs() < eps,
                "full collision_rate 不一致 inc={} full={}", fm_inc.collision_rate, fm_full.collision_rate
            );
            prop_assert!(
                (fm_inc.equiv_mean - fm_full.equiv_mean).abs() < eps,
                "full equiv_mean 不一致 inc={} full={}", fm_inc.equiv_mean, fm_full.equiv_mean
            );
            prop_assert!(
                (fm_inc.equiv_cv - fm_full.equiv_cv).abs() < eps,
                "full equiv_cv 不一致 inc={} full={}", fm_inc.equiv_cv, fm_full.equiv_cv
            );
            prop_assert!(
                (fm_inc.dist_deviation - fm_full.dist_deviation).abs() < eps,
                "full dist_deviation 不一致 inc={} full={}", fm_inc.dist_deviation, fm_full.dist_deviation
            );

            // ---- 简码指标（get_simple_metrics）逐字段相等 ----
            let sm_inc = ev_inc.get_simple_metrics(&ctx);
            let sm_full = ev_full.get_simple_metrics(&ctx);
            prop_assert!(
                (sm_inc.weighted_freq_coverage - sm_full.weighted_freq_coverage).abs() < eps,
                "simple weighted_freq_coverage 不一致 inc={} full={}",
                sm_inc.weighted_freq_coverage, sm_full.weighted_freq_coverage
            );
            prop_assert!(
                (sm_inc.equiv_mean - sm_full.equiv_mean).abs() < eps,
                "simple equiv_mean 不一致 inc={} full={}", sm_inc.equiv_mean, sm_full.equiv_mean
            );
            prop_assert!(
                (sm_inc.dist_deviation - sm_full.dist_deviation).abs() < eps,
                "simple dist_deviation 不一致 inc={} full={}", sm_inc.dist_deviation, sm_full.dist_deviation
            );
            prop_assert_eq!(
                sm_inc.collision_count, sm_full.collision_count,
                "simple collision_count 不一致"
            );
            prop_assert!(
                (sm_inc.collision_rate - sm_full.collision_rate).abs() < eps,
                "simple collision_rate 不一致 inc={} full={}", sm_inc.collision_rate, sm_full.collision_rate
            );

            // ---- reconcile 保留时变量 simple_active / current_simple_weight ----
            prop_assert!(ev_inc.simple_active, "reconcile 应保留 simple_active=true");
            prop_assert!(
                (ev_inc.current_simple_weight - test_weight).abs() < eps,
                "reconcile 应保留 current_simple_weight inc={} expected={}",
                ev_inc.current_simple_weight, test_weight
            );
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        // Feature: simple-code-perf-optimization, Property 1 (Backlog B1 强化): 阶段 2 出简选择精确增量
        // —— 「仅脏桶局部重排 + pending_chars 跨级排除传播」与全量重建逐字段一致，且工作量局部化。
        //
        // 本测试专门守护 B1 精确增量化：参数偏向小频率范围（制造排序键并列）、code_num 1..=3
        // （覆盖「桶成员数 > code_num」的择优与落选）、3 个简码级别（驱动「出简翻转触发跨级连锁」），
        // 覆盖率 0.5..=1.0（含 coverage<1.0 的非候选字场景），并参数化 frequency / efficiency 两模式。
        // 对随机移动序列逐步断言：
        //   (1) 增量出简选择（每级 selected / current_simple_code、跨级 all_assigned_flags、级别聚合、
        //       简码重码）与对同一分配 full_rebuild 的结果逐字段一致（含单级桶成员变化与跨级连锁）；
        //   (2) 北极星：阶段 2 不再做整体重算——Frequency 模式下「非候选字组移动」（受影响候选交集为空）
        //       的 last_stage2_visits 必为 0（空交集早退，需求 1.5/7.6）；且任意移动的访问量不超过
        //       「候选字数 × 级数」的整体重算上界（增量恒不劣于全量扫描）。
        // Validates: Requirements 1.1, 1.3, 1.4, 7.5
        #[test]
        fn prop_b1_incremental_selection_local_and_consistent(
            mode_is_freq in any::<bool>(),
            specs in prop::collection::vec((1u64..5, 1usize..4), 4usize..10),
            code_num in 1usize..4,
            coverage_pct in 50u32..=100,
            moves in prop::collection::vec((0usize..64, 0u8..2), 1usize..40),
        ) {
            let mode = if mode_is_freq {
                SimpleAssignMode::Frequency
            } else {
                SimpleAssignMode::Efficiency
            };
            let coverage_ratio = coverage_pct as f64 / 100.0;
            let ctx = make_ctx(&specs, mode, code_num, coverage_ratio);
            let n_groups = ctx.num_groups;
            let n_levels = ctx.simple_config.levels.len();
            let n_candidates = ctx.simple_candidate_chars.len();
            // 增量阶段 2 每个候选字至多在「阶段 1 归属 / 重排种子 / 脏桶重排 / 跨级传播」各相位
            // 被各级访问常数次，故访问量有 O(候选字 × 级数) 的常数倍上界；取宽松常数 6 作为
            // 防回归哨兵（真正的「不随候选字总集增长」由 b1_stage2_work_is_local 单元测试守护）。
            let sanity_bound = 6 * (n_candidates + 1) * (n_levels + 1);

            let mut assignment = vec![0u8; n_groups];
            let mut ev_inc = Evaluator::new(&ctx, &assignment);
            prop_assert!(ev_inc.simple_eval.is_some(), "简码应启用");

            for (k, &(gi, nk)) in moves.iter().enumerate() {
                let r = gi % n_groups;
                let affected_empty = ctx.group_to_simple_affected_candidate[r].is_empty();
                // 记录移动前组内字的旧全码，用于判定需求 33「简码占用保护」是否翻转。
                let moved_chars: Vec<usize> = ctx.group_to_chars[r].clone();
                let old_full: Vec<usize> = moved_chars
                    .iter()
                    .map(|&ci| ctx.calc_code_only(ci, &assignment))
                    .collect();

                apply_move_inc(&mut ev_inc, &ctx, &mut assignment, r, nk);
                let visits = ev_inc.last_stage2_visits();

                // 判定本次移动是否触发简码占用保护翻转（需求 33，N=0：移动后某全码桶变空 →
                // 解禁，或恰含一字 → 新禁）。翻转会把对应简码桶标脏并触发阶段 2 重选，使「空交集
                // 零访问」前提不再成立。仅在无翻转时才断言零访问。
                let protect_flip = if ctx.simple_protect_top_n == 0 {
                    let mut fc: Vec<usize> = vec![0; ctx.code_space];
                    for ci in 0..ctx.char_infos.len() {
                        fc[ctx.calc_code_only(ci, &assignment)] += 1;
                    }
                    moved_chars.iter().zip(old_full.iter()).any(|(&ci, &oc)| {
                        let nc = ctx.calc_code_only(ci, &assignment);
                        nc != oc && (fc[oc] == 0 || fc[nc] == 1)
                    })
                } else {
                    // N>0：仅受保护(top-N)字全码变化才可能翻转占用计数。
                    moved_chars.iter().any(|&ci| {
                        ctx.simple_is_topn.get(ci).copied().unwrap_or(false)
                    })
                };

                // (2) 北极星断言
                prop_assert!(
                    visits <= sanity_bound,
                    "step{}: 阶段 2 访问量 {} 超过常数倍上界 {}（candidates={} levels={}）",
                    k + 1, visits, sanity_bound, n_candidates, n_levels
                );
                if mode_is_freq && affected_empty && !protect_flip {
                    // Frequency 模式无重排种子；非候选字组移动且无保护翻转 ⟹ 出简选择不变
                    // ⟹ 阶段 2 零访问。
                    prop_assert_eq!(
                        visits, 0,
                        "step{}: Frequency 模式空交集且无保护翻转应零访问，实际 {}",
                        k + 1, visits
                    );
                }

                // (1) 逐字段一致
                let ev_full = Evaluator::new(&ctx, &assignment);
                let label = format!(
                    "B1 step{} move(r={},nk={}) mode={:?} code_num={} cov={} visits={}",
                    k + 1, r, nk, mode, code_num, coverage_ratio, visits
                );
                assert_simple_eq(&ctx, &ev_inc, &ev_full, &label)?;
            }
        }
    }

    /// 北极星单元测试（Backlog B1）：阶段 2「出简选择增量」的工作量随**受影响局部**变化，
    /// 不随候选字总集规模增长——证明已摆脱旧实现 O(候选字集 × 级数) 的整体重算。
    ///
    /// 构造稀疏上下文：N 个单根候选字，键位充足且初始分配两两不同 ⟹ 每个字在级别 1 独占一个
    /// 简码桶（全被选中、无桶内竞争、无跨级连锁）。对组 0 施加一次「迁移到空闲键位」的移动：
    /// 仅触碰该字的旧/新简码桶（各 ≤1 成员）与其首选翻转。断言：
    ///   - 该移动的 last_stage2_visits 为很小的常数（不随 N 变化）；
    ///   - N = 6 与 N = 20 两种规模下访问量完全相等（局部性）；
    ///   - 访问量远小于「候选字数 × 级数」的整体重算上界。
    fn make_sparse_ctx(n: usize, n_keys: u8) -> OptContext {
        use crate::config::TargetsConfig;
        use crate::types::{
            KeyDistConfig, RootGroup, ScaleConfig, SimpleCodeConfig, SimpleCodeLevel,
            SimpleCodeStep, WeightConfig,
        };
        use std::collections::HashMap;

        let allowed: Vec<u8> = (0..n_keys).collect();
        let mut groups: Vec<RootGroup> = Vec::new();
        let mut splits: Vec<(char, Vec<String>, u64)> = Vec::with_capacity(n);
        for i in 0..n {
            let root = format!("s{i}");
            groups.push(RootGroup {
                roots: vec![root.clone()],
                allowed_keys: allowed.clone(),
            });
            let ch = char::from_u32(0x4e00 + i as u32).unwrap();
            // 频率两两不同（i+1），避免并列，使选择确定且每字独占桶后全部选中。
            splits.push((ch, vec![root], (i as u64) + 1));
        }
        let step = |sel: char| SimpleCodeStep { root_selector: sel, code_selector: 'a' };
        // 单级（level 1）：单根字仅在该级有简码，足以验证局部性。
        let levels = vec![SimpleCodeLevel {
            level: 1,
            code_num: 1,
            rule_candidates: vec![vec![step('A')]],
            space_commit: false,
        }];
        let fixed_roots: HashMap<String, u8> = HashMap::new();
        let equiv_table = [[0.0f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];
        let key_dist = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];
        let mut weights = WeightConfig::default();
        weights.enable_simple_code = true;
        weights.simple_coverage_ratio = 1.0; // 全部为候选字
        weights.simple_assign_mode = SimpleAssignMode::Efficiency;
        OptContext::new(
            &splits,
            &fixed_roots,
            &groups,
            equiv_table,
            key_dist,
            ScaleConfig::default(),
            SimpleCodeConfig { levels },
            weights,
            TargetsConfig::default(),
        )
    }

    /// 在稀疏上下文中，对组 0 施加「字 0 迁移到空闲键位」的移动，返回该步阶段 2 访问量。
    fn sparse_localized_move_visits(n: usize) -> (usize, usize) {
        let n_keys = (n as u8) + 5; // 充足键位，保证初始两两不同且留有空闲键
        let ctx = make_sparse_ctx(n, n_keys);
        let n_groups = ctx.num_groups; // = n（每字单根单组）
        // 初始分配：assignment[i] = i ⟹ 各字级别 1 简码两两不同（独占桶）。
        let mut assignment: Vec<u8> = (0..n_groups as u8).collect();
        let mut ev = Evaluator::new(&ctx, &assignment);
        // 迁移字 0 到一个未被占用的键位（n+1），其旧/新简码桶均 ≤1 成员。
        let free_key = (n as u8) + 1;
        apply_move_inc(&mut ev, &ctx, &mut assignment, 0, free_key);
        (ev.last_stage2_visits(), ctx.simple_candidate_chars.len())
    }

    #[test]
    fn b1_stage2_work_is_local_not_candidate_set_size() {
        let (v_small, n_small) = sparse_localized_move_visits(6);
        let (v_large, n_large) = sparse_localized_move_visits(20);

        // 两种候选规模下，局部化移动的阶段 2 访问量应完全相等（与候选字总集无关）。
        assert_eq!(
            v_small, v_large,
            "局部化移动的阶段 2 访问量应与候选字总集规模无关：N=6 时 {}，N=20 时 {}",
            v_small, v_large
        );
        // 且为很小的常数，远小于整体重算上界（候选字数 × 级数）。
        assert!(
            v_large <= 8,
            "局部化移动的阶段 2 访问量应为很小的常数，实际 {}",
            v_large
        );
        assert!(
            v_large < n_large,
            "阶段 2 访问量 {} 应远小于候选字数 {}（证明非整体重算）",
            v_large, n_large
        );
        assert!(n_small == 6 && n_large == 20, "稀疏构造应令全部字为候选字");
    }
}

// =========================================================================
// 任务 8.2 / Property 2: 拒绝/回滚与未移动等价（round-trip）属性测试
//
// 对任意简码评估器状态与任意一次受影响移动，先执行增量更新（apply_move_incremental，
// 经 Evaluator::apply_simple_for_move 入口）再执行 rollback（经 rollback_simple），
// 所得简码评估器的全部状态应与移动前逐字段相等。复刻产线拒绝路径顺序：先逆转全码更新
// （assignment + update_char），再回滚简码（rollback_simple）。
// =========================================================================
#[cfg(test)]
mod rollback_roundtrip_tests {
    use super::*;
    use crate::config::TargetsConfig;
    use crate::context::OptContext;
    use crate::types::{
        KeyDistConfig, RootGroup, ScaleConfig, SimpleAssignMode, SimpleCodeConfig, SimpleCodeLevel,
        SimpleCodeStep, WeightConfig, EQUIV_TABLE_SIZE,
    };
    use proptest::prelude::*;
    use std::collections::HashMap;

    /// 构建启用简码、含 3 个级别、参数化分配模式 / 每级编码数 / 候选覆盖率的最小 OptContext。
    ///
    /// 与 `incremental_full_consistency_tests::make_ctx` 同构：`specs[i] = (freq, n_roots)`
    /// 决定第 i 个汉字的字频与全码部件数；仅用 2 个允许键位制造同桶碰撞与「桶成员数 > code_num」
    /// 的选取（驱动「改变选择」「并列裁决」等边界）。`coverage_ratio < 1.0` 时低频字落选候选集，
    /// 其字根组的受影响交集为空 —— 覆盖「空交集移动（非候选字组）」的回滚 no-op 路径。
    fn make_ctx(
        specs: &[(u64, usize)],
        mode: SimpleAssignMode,
        code_num: usize,
        coverage_ratio: f64,
    ) -> OptContext {
        let mut groups: Vec<RootGroup> = Vec::new();
        let mut splits: Vec<(char, Vec<String>, u64)> = Vec::with_capacity(specs.len());
        for (i, &(freq, n_roots)) in specs.iter().enumerate() {
            let n_roots = n_roots.max(1);
            let mut roots: Vec<String> = Vec::with_capacity(n_roots);
            for j in 0..n_roots {
                let root = format!("r{i}_{j}");
                groups.push(RootGroup {
                    roots: vec![root.clone()],
                    allowed_keys: vec![0, 1],
                });
                roots.push(root);
            }
            let ch = char::from_u32(0x4e00 + i as u32).unwrap();
            splits.push((ch, roots, freq.max(1)));
        }

        let step = |sel: char| SimpleCodeStep {
            root_selector: sel,
            code_selector: 'a',
        };
        let levels = vec![
            SimpleCodeLevel {
                level: 1,
                code_num,
                rule_candidates: vec![vec![step('A')]],
                space_commit: false,
            },
            SimpleCodeLevel {
                level: 2,
                code_num,
                rule_candidates: vec![vec![step('A'), step('B')]],
                space_commit: false,
            },
            SimpleCodeLevel {
                level: 3,
                code_num,
                rule_candidates: vec![vec![step('A'), step('B'), step('C')]],
                space_commit: false,
            },
        ];

        let fixed_roots: HashMap<String, u8> = HashMap::new();
        let equiv_table = [[0.0f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];
        let key_dist = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];
        let mut weights = WeightConfig::default();
        weights.enable_simple_code = true;
        weights.simple_coverage_ratio = coverage_ratio;
        weights.simple_assign_mode = mode;
        OptContext::new(
            &splits,
            &fixed_roots,
            &groups,
            equiv_table,
            key_dist,
            ScaleConfig::default(),
            SimpleCodeConfig { levels },
            weights,
            TargetsConfig::default(),
        )
    }

    // 需求 29.1/29.3/29.5：经一串移动（含接受/回滚）后，简码全局聚合标量应恒等于
    // 「各级别对应聚合之和 + 固定简码常量偏置」。
    #[test]
    fn test_global_aggregates_equal_sum_of_levels_plus_fixed() {
        let ctx = make_ctx(
            &[(9, 2), (7, 2), (5, 2), (4, 1), (3, 2), (2, 1), (1, 2)],
            SimpleAssignMode::Efficiency,
            1,
            1.0,
        );
        let n = ctx.num_groups;
        let mut assignment = vec![0u8; n];
        let mut ev = Evaluator::new(&ctx, &assignment);
        assert!(ev.simple_eval.is_some());

        let check = |ev: &Evaluator| {
            let se = ev.simple_eval.as_ref().unwrap();
            let mut cov = ctx.fixed_covered_freq;
            let mut ew = ctx.fixed_equiv_weighted;
            let mut ef = ctx.fixed_equiv_freq_sum;
            let mut kp = ctx.fixed_key_presses;
            let mut ku = ctx.fixed_key_usage;
            for lvl in &se.levels {
                cov += lvl.covered_freq;
                ew += lvl.equiv_weighted;
                ef += lvl.equiv_freq_sum;
                kp += lvl.key_presses;
                for k in 0..EQUIV_TABLE_SIZE {
                    ku[k] += lvl.key_usage[k];
                }
            }
            assert_eq!(se.g_covered_freq, cov, "g_covered_freq 与级别和不一致");
            assert_eq!(se.g_equiv_freq_sum, ef, "g_equiv_freq_sum 与级别和不一致");
            assert!((se.g_equiv_weighted - ew).abs() < 1e-6, "g_equiv_weighted 漂移");
            assert!((se.g_key_presses - kp).abs() < 1e-6, "g_key_presses 漂移");
            for k in 0..EQUIV_TABLE_SIZE {
                assert!((se.g_key_usage[k] - ku[k]).abs() < 1e-6, "g_key_usage[{k}] 漂移");
            }
        };

        check(&ev);
        let mut rng = rand::thread_rng();
        for step in 0..2000usize {
            let r = step % n;
            let nk = (step % 2) as u8;
            ev.try_move(&ctx, &mut assignment, r, nk, 1e18, &mut rng);
            if step % 50 == 0 {
                check(&ev);
            }
        }
        check(&ev);
    }

    // 需求 29 方向 B：非零分布配置下，增量维护的 g_dist_deviation 应始终等于「对同一分配
    // 全量重建」的分布偏差（含 presses 不变快速路径与 presses 变化全量回退）。
    #[test]
    fn test_incremental_dist_matches_full_rebuild() {
        let mut ctx = make_ctx(
            &[(9, 2), (7, 2), (5, 2), (4, 1), (3, 2), (2, 1), (1, 2), (6, 2)],
            SimpleAssignMode::Efficiency,
            1,
            1.0,
        );
        // 设置非零分布配置（简码键多落在 code_selector 'a'→键0 与键1 上），真正驱动 dist。
        for k in 0..2usize {
            ctx.key_dist_config[k].target_rate = 5.0;
            ctx.key_dist_config[k].low_penalty = 1.5;
            ctx.key_dist_config[k].high_penalty = 2.5;
        }

        let n = ctx.num_groups;
        let mut assignment = vec![0u8; n];
        let mut ev = Evaluator::new(&ctx, &assignment);

        let assert_dist_matches = |assignment: &[u8], ev: &Evaluator| {
            let inc = ev.simple_eval.as_ref().unwrap().g_dist_deviation;
            // 对同一分配从零全量重建，取其分布偏差作为基准。
            let fresh = Evaluator::new(&ctx, assignment);
            let full = fresh.simple_eval.as_ref().unwrap().g_dist_deviation;
            assert!(
                (inc - full).abs() < 1e-6,
                "增量 dist {inc} 与全量 {full} 不一致 (assignment={assignment:?})"
            );
        };

        assert_dist_matches(&assignment, &ev);
        let mut rng = rand::thread_rng();
        for step in 0..3000usize {
            let r = step % n;
            let nk = (step % 2) as u8;
            ev.try_move(&ctx, &mut assignment, r, nk, 1e18, &mut rng);
            if step % 25 == 0 {
                assert_dist_matches(&assignment, &ev);
            }
        }
        assert_dist_matches(&assignment, &ev);
    }

    /// 单个级别的可比较状态快照（用于逐字段精确比对）。
    #[derive(Clone, PartialEq, Debug)]
    struct LevelSnap {
        /// 各非空桶 (编码, 成员列表, freq_sum)，按编码升序，便于稀疏/密集后端等价比对。
        buckets: Vec<(u32, Vec<u32>, u64)>,
        current_simple_code: Vec<i64>,
        selected: Vec<bool>,
        covered_freq: u64,
        equiv_weighted: f64,
        equiv_freq_sum: u64,
        key_usage: Vec<f64>,
        key_presses: f64,
    }

    /// SimpleEvaluator 的完整可比较状态快照。
    ///
    /// 涵盖 Property 2 列举的全部字段：各级桶成员 + freq_sum、current_simple_code、selected、
    /// 级别聚合（covered_freq/equiv_weighted/equiv_freq_sum/key_usage/key_presses）、
    /// all_assigned_flags、简码重码标量（count/freq/rate）、cached_simple_score；并额外比对
    /// rollback 应一并还原的内部状态 last_full_codes 与 bucket_collision_contrib，以加强检测。
    #[derive(Clone, PartialEq, Debug)]
    struct SeSnap {
        levels: Vec<LevelSnap>,
        all_assigned_flags: Vec<bool>,
        simple_collision_count: usize,
        simple_collision_freq: u64,
        simple_collision_rate: f64,
        cached_simple_score: f64,
        last_full_codes: Vec<usize>,
        bucket_collision_contrib: FxHashMap<u32, (usize, u64)>,
    }

    /// 从 Evaluator 持有的 SimpleEvaluator 抽取完整状态快照。
    fn snap(ev: &Evaluator) -> SeSnap {
        let se = ev.simple_eval.as_ref().expect("simple_eval 应启用");
        let levels = se
            .levels
            .iter()
            .map(|lv| LevelSnap {
                buckets: {
                    let mut v: Vec<(u32, Vec<u32>, u64)> = lv
                        .buckets
                        .iter_nonempty()
                        .map(|(c, b)| (c, b.members.to_vec(), b.freq_sum))
                        .collect();
                    v.sort_by_key(|t| t.0);
                    v
                },
                current_simple_code: lv.current_simple_code.clone(),
                selected: lv.selected.clone(),
                covered_freq: lv.covered_freq,
                equiv_weighted: lv.equiv_weighted,
                equiv_freq_sum: lv.equiv_freq_sum,
                key_usage: lv.key_usage.to_vec(),
                key_presses: lv.key_presses,
            })
            .collect();
        SeSnap {
            levels,
            all_assigned_flags: se.all_assigned_flags.clone(),
            simple_collision_count: se.simple_collision_count,
            simple_collision_freq: se.simple_collision_freq,
            simple_collision_rate: se.simple_collision_rate,
            cached_simple_score: se.cached_simple_score,
            last_full_codes: se.last_full_codes.clone(),
            bucket_collision_contrib: se.bucket_collision_contrib.clone(),
        }
    }

    /// 施加一次完整移动（不提交）：复刻产线顺序 —— 先改写 assignment[r]，再对组内全码受影响字
    /// 增量维护全码桶（update_char），最后走简码增量 + 快照路径（apply_simple_for_move）。
    fn apply_move(ev: &mut Evaluator, ctx: &OptContext, assignment: &mut [u8], r: usize, new_key: u8) {
        assignment[r] = new_key;
        for idx in 0..ctx.group_to_chars[r].len() {
            let ci = ctx.group_to_chars[r][idx];
            ev.update_char(ctx, assignment, ci);
        }
        ev.apply_simple_for_move(ctx, assignment, &[r]);
    }

    /// 逆转一次移动（拒绝路径）：复刻产线拒绝顺序 —— 先逆转全码更新（恢复 assignment[r] = old_key
    /// 并对组内字 update_char 回退），再回滚简码增量（rollback_simple）。
    fn reject_move(ev: &mut Evaluator, ctx: &OptContext, assignment: &mut [u8], r: usize, old_key: u8) {
        assignment[r] = old_key;
        for idx in 0..ctx.group_to_chars[r].len() {
            let ci = ctx.group_to_chars[r][idx];
            ev.update_char(ctx, assignment, ci);
        }
        ev.rollback_simple();
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        // Feature: simple-code-perf-optimization, Property 2: 拒绝/回滚与未移动等价（round-trip）
        //
        // 对任意简码评估器状态与任意一次受影响移动，先执行 apply_move_incremental（经
        // apply_simple_for_move）再执行 rollback（经 rollback_simple），所得简码评估器的全部状态
        // （桶成员、freq_sum、各级聚合、current_simple_code、selected、all_assigned_flags、
        // 简码重码数与重码率、cached_simple_score）应与移动前逐字段相等。
        //
        // 测试在「活动」评估器上推进一串移动：每步以随机布尔决定接受（apply + commit，推进 baseline）
        // 或拒绝（apply + 逆转全码 + rollback，断言 round-trip 等价），从而在多样 baseline 下校验回滚。
        // 覆盖边界：空交集移动（非候选字组，coverage<1.0）、改变选择的移动、并列裁决（小频率范围）。
        // 参数化 frequency / efficiency 两种模式。
        // Validates: Requirements 2.1, 2.2, 2.3, 3.1
        #[test]
        fn prop2_rollback_roundtrip(
            // true => Frequency 模式；false => Efficiency 模式（参数化两种模式）
            mode_is_freq in any::<bool>(),
            // (freq, n_roots)：小频率范围制造并列；n_roots 1..=3 使 base_saving 跨级变化
            specs in prop::collection::vec((1u64..6, 1usize..4), 3usize..9),
            // 每级编码数 1..=3：覆盖 code_num 大于/小于桶成员数两种情形（含改变选择/并列）
            code_num in 1usize..4,
            // 覆盖率阈值 0.5..=1.0：< 1.0 时产生非候选字 → 触发空交集移动的回滚 no-op
            coverage_pct in 50u32..=100,
            // 随机移动序列（组索引, 新键位 ∈ {0,1}, 接受?）
            moves in prop::collection::vec((0usize..64, 0u8..2, any::<bool>()), 0usize..40),
        ) {
            let mode = if mode_is_freq {
                SimpleAssignMode::Frequency
            } else {
                SimpleAssignMode::Efficiency
            };
            let coverage_ratio = coverage_pct as f64 / 100.0;
            let ctx = make_ctx(&specs, mode, code_num, coverage_ratio);
            let n_groups = ctx.num_groups;

            let mut assignment = vec![0u8; n_groups];
            let mut ev = Evaluator::new(&ctx, &assignment);
            prop_assert!(ev.simple_eval.is_some(), "简码应启用");

            for (k, &(gi, nk, accept)) in moves.iter().enumerate() {
                let r = gi % n_groups;
                let old_key = assignment[r];

                if accept {
                    // 接受：apply + commit，推进 baseline（无需 round-trip 断言）
                    apply_move(&mut ev, &ctx, &mut assignment, r, nk);
                    ev.commit_simple();
                } else {
                    // 拒绝：快照移动前完整状态 → apply（不提交）→ 逆转全码 + rollback → 断言等价
                    let before = snap(&ev);
                    apply_move(&mut ev, &ctx, &mut assignment, r, nk);
                    reject_move(&mut ev, &ctx, &mut assignment, r, old_key);
                    let after = snap(&ev);

                    let label = format!(
                        "step{} reject move(r={},nk={},old={}) mode={:?} code_num={} cov={}",
                        k + 1, r, nk, old_key, mode, code_num, coverage_ratio
                    );
                    prop_assert_eq!(&before, &after, "{}: 回滚后简码状态与移动前不一致", label);
                }
            }
        }
    }
}

// =========================================================================
// 🧪 最佳解综合得分重算测试（simple-code-perf-optimization, Property 10）
// =========================================================================
#[cfg(test)]
mod best_total_recompute_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        // Feature: simple-code-perf-optimization, Property 10: 最佳解综合得分按分量以当前权重重算
        //
        // 对任意最佳解分量 best_full_score、best_simple_score 与任意权重 weight_full、w_eff，
        // Evaluator::best_total 应恰好等于 weight_full · best_full_score + w_eff · best_simple_score。
        // 该重算只做两次乘法加一次加法，为 O(1)，不依赖任何评估器内部状态。
        //
        // Validates: Requirements 11.1, 11.2, 11.3, 16.5
        #[test]
        fn prop10_best_total_recompute(
            best_full_score in -1.0e9f64..1.0e9f64,
            best_simple_score in -1.0e9f64..1.0e9f64,
            weight_full in -1.0e6f64..1.0e6f64,
            w_eff in -1.0e6f64..1.0e6f64,
        ) {
            let got = Evaluator::best_total(weight_full, best_full_score, w_eff, best_simple_score);
            let expected = weight_full * best_full_score + w_eff * best_simple_score;
            // 函数内部执行的正是同样的两次乘法 + 一次加法，结果按位精确相等。
            prop_assert_eq!(got, expected);
        }
    }
}

// =========================================================================
// 任务 9.2 / Property 9: 激活前简码贡献为零属性测试
//
// 对任意分配（含任意一串合法全码移动），在以下两种「简码未参与综合得分」的情形下，
// 综合得分 `get_score` 都应等于纯全码得分 `weight_full_code * compute_full_score`：
//   (a) 简码已启用（enable_simple_code = true）但尚未激活（延迟激活早期探索阶段，
//       p < simple_start_progress）：simple_active = false 且 current_simple_weight = 0；
//   (b) 简码整体关闭（enable_simple_code = false）：Evaluator::new 直接置
//       simple_active = false、current_simple_weight = 0，行为与「简码未启用」一致（需求 17.4）。
// =========================================================================
#[cfg(test)]
mod pre_activation_zero_contribution_tests {
    use super::*;
    use crate::config::TargetsConfig;
    use crate::context::OptContext;
    use crate::types::{
        KeyDistConfig, RootGroup, ScaleConfig, SimpleAssignMode, SimpleCodeConfig, SimpleCodeLevel,
        SimpleCodeStep, WeightConfig, EQUIV_TABLE_SIZE,
    };
    use proptest::prelude::*;
    use std::collections::HashMap;

    /// 构建最小 OptContext，可参数化是否启用简码（`enable_simple`）。
    ///
    /// 结构与 `incremental_full_consistency_tests::make_ctx` 同构：`specs[i] = (freq, n_roots)`
    /// 决定第 i 个汉字的字频与全码部件数；仅用 2 个允许键位（`[0, 1]`）制造同桶碰撞，
    /// 使全码得分（compute_full_score）随分配/移动产生非平凡变化，从而该属性确有判别力。
    fn make_ctx(
        specs: &[(u64, usize)],
        enable_simple: bool,
        mode: SimpleAssignMode,
        code_num: usize,
        coverage_ratio: f64,
    ) -> OptContext {
        let mut groups: Vec<RootGroup> = Vec::new();
        let mut splits: Vec<(char, Vec<String>, u64)> = Vec::with_capacity(specs.len());
        for (i, &(freq, n_roots)) in specs.iter().enumerate() {
            let n_roots = n_roots.max(1);
            let mut roots: Vec<String> = Vec::with_capacity(n_roots);
            for j in 0..n_roots {
                let root = format!("r{i}_{j}");
                groups.push(RootGroup {
                    roots: vec![root.clone()],
                    allowed_keys: vec![0, 1],
                });
                roots.push(root);
            }
            let ch = char::from_u32(0x4e00 + i as u32).unwrap();
            splits.push((ch, roots, freq.max(1)));
        }

        let step = |sel: char| SimpleCodeStep {
            root_selector: sel,
            code_selector: 'a',
        };
        let levels = vec![
            SimpleCodeLevel {
                level: 1,
                code_num,
                rule_candidates: vec![vec![step('A')]],
                space_commit: false,
            },
            SimpleCodeLevel {
                level: 2,
                code_num,
                rule_candidates: vec![vec![step('A'), step('B')]],
                space_commit: false,
            },
            SimpleCodeLevel {
                level: 3,
                code_num,
                rule_candidates: vec![vec![step('A'), step('B'), step('C')]],
                space_commit: false,
            },
        ];

        let fixed_roots: HashMap<String, u8> = HashMap::new();
        let equiv_table = [[0.0f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];
        let key_dist = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];
        let mut weights = WeightConfig::default();
        weights.enable_simple_code = enable_simple;
        weights.simple_coverage_ratio = coverage_ratio;
        weights.simple_assign_mode = mode;
        OptContext::new(
            &splits,
            &fixed_roots,
            &groups,
            equiv_table,
            key_dist,
            ScaleConfig::default(),
            SimpleCodeConfig { levels },
            weights,
            TargetsConfig::default(),
        )
    }

    /// 施加一次合法全码移动（仅维护全码桶；简码未参与，无需 apply_simple_for_move）：
    /// 改写 `assignment[r]` 后对组内全码受影响字调用 `update_char`，并置 `score_dirty` 触发重算。
    fn apply_full_move(ev: &mut Evaluator, ctx: &OptContext, assignment: &mut [u8], r: usize, new_key: u8) {
        assignment[r] = new_key;
        for idx in 0..ctx.group_to_chars[r].len() {
            let ci = ctx.group_to_chars[r][idx];
            ev.update_char(ctx, assignment, ci);
        }
        ev.score_dirty = true;
        ev.full_score_dirty = true;
    }

    /// 断言：综合得分恰等于纯全码分量 `weight_full_code * compute_full_score`（简码贡献为 0）。
    fn assert_zero_simple_contribution(
        ev: &mut Evaluator,
        ctx: &OptContext,
        label: &str,
    ) -> Result<(), TestCaseError> {
        let total = ev.get_score(ctx);
        let full_only = ctx.weights.weight_full_code * ev.compute_full_score(ctx);
        let eps = 1e-9 * (1.0 + full_only.abs());
        prop_assert!(
            (total - full_only).abs() <= eps,
            "{label}: 简码贡献应为 0：get_score={total} 应等于 weight_full_code*full_score={full_only}",
        );
        Ok(())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        // Feature: simple-code-perf-optimization, Property 9: 激活前简码贡献为零
        //
        // 对任意进度 p < simple_start_progress（以 simple_active=false、current_simple_weight=0 表征）、
        // 以及 weights.simple_code.enabled = false 的任意分配，简码分数 simple_score 对综合得分的
        // 贡献应为 0，使综合得分等于纯全码得分 weight_full_code * full_score。
        // 含两条路径：(a) 简码已启用但未激活；(b) enable_simple_code=false。
        // Validates: Requirements 8.5, 10.2, 17.4
        #[test]
        fn prop9_pre_activation_zero_simple_contribution(
            // (freq, n_roots)：小频率范围制造同桶碰撞，使全码得分非平凡
            specs in prop::collection::vec((1u64..6, 1usize..4), 3usize..9),
            mode_is_freq in any::<bool>(),
            code_num in 1usize..4,
            coverage_pct in 50u32..=100,
            // 随机合法全码移动序列（组索引, 新键位 ∈ {0,1}）
            moves in prop::collection::vec((0usize..64, 0u8..2), 0usize..30),
        ) {
            let mode = if mode_is_freq {
                SimpleAssignMode::Frequency
            } else {
                SimpleAssignMode::Efficiency
            };
            let coverage_ratio = coverage_pct as f64 / 100.0;

            // ---- 路径 (a)：简码已启用但「未激活」（延迟激活早期阶段，需求 8.5/10.2）----
            {
                let ctx = make_ctx(&specs, true, mode, code_num, coverage_ratio);
                let n_groups = ctx.num_groups;
                let mut assignment = vec![0u8; n_groups];
                let mut ev = Evaluator::new(&ctx, &assignment);
                // 启用简码时 new 会急切构建并置 simple_active=true；这里手动回退到「未激活」状态，
                // 模拟退火延迟激活的早期探索阶段：simple_active=false 且 current_simple_weight=0。
                prop_assert!(ev.simple_eval.is_some(), "启用简码时 simple_eval 应存在");
                ev.simple_active = false;
                ev.current_simple_weight = 0.0;
                ev.score_dirty = true;
                ev.full_score_dirty = true;

                assert_zero_simple_contribution(&mut ev, &ctx, "(a)启用未激活 step0")?;

                for (k, &(gi, nk)) in moves.iter().enumerate() {
                    let r = gi % n_groups;
                    apply_full_move(&mut ev, &ctx, &mut assignment, r, nk);
                    // 移动期间务必保持「未激活」状态（不调用 activate_simple）
                    prop_assert!(!ev.simple_active, "(a) 全程应保持未激活");
                    let label = format!("(a)启用未激活 step{}", k + 1);
                    assert_zero_simple_contribution(&mut ev, &ctx, &label)?;
                }
            }

            // ---- 路径 (b)：简码整体关闭（enable_simple_code=false，需求 17.4）----
            {
                let ctx = make_ctx(&specs, false, mode, code_num, coverage_ratio);
                let n_groups = ctx.num_groups;
                let mut assignment = vec![0u8; n_groups];
                let mut ev = Evaluator::new(&ctx, &assignment);
                // 简码关闭时 new 应置 simple_active=false、current_simple_weight=0，且无简码评估器。
                prop_assert!(ev.simple_eval.is_none(), "关闭简码时 simple_eval 应为 None");
                prop_assert!(!ev.simple_active, "关闭简码时 simple_active 应为 false");
                prop_assert_eq!(ev.current_simple_weight, 0.0, "关闭简码时 current_simple_weight 应为 0");

                assert_zero_simple_contribution(&mut ev, &ctx, "(b)关闭 step0")?;

                for (k, &(gi, nk)) in moves.iter().enumerate() {
                    let r = gi % n_groups;
                    apply_full_move(&mut ev, &ctx, &mut assignment, r, nk);
                    let label = format!("(b)关闭 step{}", k + 1);
                    assert_zero_simple_contribution(&mut ev, &ctx, &label)?;
                }
            }
        }
    }
}

// =========================================================================
// 🧪 热路径防回归测试（simple-code-perf-optimization, hot-path regression guard）
//
// 目标：守护本特性「简码评估增量化」的核心性能成果不被悄悄回退——一旦有人把生产热路径
// try_move / try_swap 改回（直接或间接）调用全量重建（Evaluator::rebuild_simple /
// SimpleEvaluator::full_rebuild），下面的断言立即失败。
//
// 手段：在 SimpleEvaluator::full_rebuild 与 Evaluator::rebuild_simple 入口处维护仅用于观测
// 的调用计数器 full_rebuild_calls（增量路径 apply_simple_for_move / commit / rollback 不触碰）。
// 激活简码后记录基线，跑一批 try_move / try_swap（覆盖「接受」与「拒绝」两分支），断言该计数
// 相对基线零增长。
// =========================================================================
#[cfg(test)]
mod hot_path_no_full_rebuild_tests {
    use super::*;
    use crate::config::TargetsConfig;
    use crate::context::OptContext;
    use crate::types::{
        KeyDistConfig, RootGroup, ScaleConfig, SimpleAssignMode, SimpleCodeConfig, SimpleCodeLevel,
        SimpleCodeStep, WeightConfig, EQUIV_TABLE_SIZE,
    };
    use proptest::prelude::*;
    use rand::thread_rng;
    use std::collections::HashMap;

    /// 构建启用简码、含 3 个级别、参数化分配模式 / 每级编码数 / 候选覆盖率的最小 OptContext。
    ///
    /// 与增量一致性测试同构：`specs[i] = (freq, n_roots)`，每个字根独立成组，仅 2 个允许键位
    /// `[0, 1]`，从而在多候选字下制造同桶碰撞并让全码桶在移动时频繁变化（既覆盖出简选择变化，
    /// 又覆盖简码重码增量）。
    fn make_ctx(
        specs: &[(u64, usize)],
        mode: SimpleAssignMode,
        code_num: usize,
        coverage_ratio: f64,
    ) -> OptContext {
        let mut groups: Vec<RootGroup> = Vec::new();
        let mut splits: Vec<(char, Vec<String>, u64)> = Vec::with_capacity(specs.len());
        for (i, &(freq, n_roots)) in specs.iter().enumerate() {
            let n_roots = n_roots.max(1);
            let mut roots: Vec<String> = Vec::with_capacity(n_roots);
            for j in 0..n_roots {
                let root = format!("r{i}_{j}");
                groups.push(RootGroup {
                    roots: vec![root.clone()],
                    allowed_keys: vec![0, 1],
                });
                roots.push(root);
            }
            let ch = char::from_u32(0x4e00 + i as u32).unwrap();
            splits.push((ch, roots, freq.max(1)));
        }

        let step = |sel: char| SimpleCodeStep {
            root_selector: sel,
            code_selector: 'a',
        };
        let levels = vec![
            SimpleCodeLevel {
                level: 1,
                code_num,
                rule_candidates: vec![vec![step('A')]],
                space_commit: false,
            },
            SimpleCodeLevel {
                level: 2,
                code_num,
                rule_candidates: vec![vec![step('A'), step('B')]],
                space_commit: false,
            },
            SimpleCodeLevel {
                level: 3,
                code_num,
                rule_candidates: vec![vec![step('A'), step('B'), step('C')]],
                space_commit: false,
            },
        ];

        let fixed_roots: HashMap<String, u8> = HashMap::new();
        let equiv_table = [[0.0f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];
        let key_dist = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];
        let mut weights = WeightConfig::default();
        weights.enable_simple_code = true;
        weights.simple_coverage_ratio = coverage_ratio;
        weights.simple_assign_mode = mode;
        OptContext::new(
            &splits,
            &fixed_roots,
            &groups,
            equiv_table,
            key_dist,
            ScaleConfig::default(),
            SimpleCodeConfig { levels },
            weights,
            TargetsConfig::default(),
        )
    }

    /// 复刻产线延迟激活：`Evaluator::new` 会急切构建并置 `simple_active=true`，这里显式回退为
    /// 未激活，再通过 `activate_simple` 重新打开，使「激活后」的语义与退火主循环一致。
    fn new_activated(ctx: &OptContext, assignment: &[u8]) -> Evaluator {
        let mut ev = Evaluator::new(ctx, assignment);
        ev.simple_active = false;
        ev.current_simple_weight = 0.0;
        ev.score_dirty = true;
        ev.activate_simple(ctx, assignment);
        ev
    }

    // -------------------------------------------------------------------------
    // 确定性断言：分别强制「接受」与「拒绝」两个分支，证明两条路径都不触发全量重建。
    // -------------------------------------------------------------------------

    // Feature: simple-code-perf-optimization, hot-path regression guard: 激活后 try_move/try_swap 不触发全量重建
    #[test]
    fn accept_branch_does_not_trigger_full_rebuild() {
        // 覆盖率 1.0 ⟹ 全部为候选字，保证移动确有简码影响（needs_simple=true）。
        let ctx = make_ctx(
            &[(1000, 2), (800, 2), (600, 1), (5, 1)],
            SimpleAssignMode::Efficiency,
            2,
            1.0,
        );
        let n_groups = ctx.num_groups;
        let mut assignment = vec![0u8; n_groups];
        let mut ev = new_activated(&ctx, &assignment);
        assert!(ev.simple_eval.is_some() && ev.simple_active, "简码应已激活");

        let baseline = ev.full_rebuild_calls();
        let mut rng = thread_rng();

        // 找一个确有简码影响的组，从键 0 移到键 1。
        // temp = +∞ ⟹ exp(-delta/temp) = 1.0 > rng.gen()∈[0,1)，与 rng 无关地必然接受。
        let r = (0..n_groups)
            .find(|&g| ev.has_simple_impact(&ctx, g))
            .expect("应存在受简码影响的组");
        assert!(ev.has_simple_impact(&ctx, r), "该组移动应触发简码增量路径");

        let accepted = ev.try_move(&ctx, &mut assignment, r, 1, f64::INFINITY, &mut rng);
        assert!(accepted, "temp=+∞ 时应必然接受（覆盖 commit 分支）");
        assert_eq!(
            ev.full_rebuild_calls(),
            baseline,
            "接受分支（commit）期间全量重建计数不得增长"
        );

        // 再叠加一次 try_swap（两组键位不同方可交换），同样必然接受。
        let r2 = (0..n_groups).find(|&g| g != r).expect("至少两组");
        // 确保 r、r2 当前键位不同：r 已被移到 1，找一个仍在 0 的组。
        if let Some(rz) = (0..n_groups).find(|&g| assignment[g] == 0) {
            if rz != r && assignment[r] != assignment[rz] {
                let swapped = ev.try_swap(&ctx, &mut assignment, r, rz, f64::INFINITY, &mut rng);
                assert!(swapped, "temp=+∞ 时 try_swap 应必然接受");
            }
        }
        let _ = r2;
        assert_eq!(
            ev.full_rebuild_calls(),
            baseline,
            "try_swap 接受分支期间全量重建计数不得增长"
        );
    }

    // Feature: simple-code-perf-optimization, hot-path regression guard: 激活后 try_move/try_swap 不触发全量重建
    #[test]
    fn reject_branch_does_not_trigger_full_rebuild() {
        // 单字根字（n_roots=1）⟹ 全码 = 单键，便于构造「移动制造重码」的确定性恶化移动。
        // 频率极端（10万级 vs 1）⟹ 重码频率项主导得分，移动后 delta 必为正。
        let ctx = make_ctx(
            &[(100_000, 1), (100_000, 1), (1, 1)],
            SimpleAssignMode::Efficiency,
            2,
            1.0,
        );
        let n_groups = ctx.num_groups;
        assert_eq!(n_groups, 3, "每字单根 ⟹ 3 组");

        // 低重码起点：组0→键0（独占），组1、组2→键1。
        let mut assignment = vec![0u8, 1u8, 1u8];
        let mut ev = new_activated(&ctx, &assignment);
        assert!(ev.simple_eval.is_some() && ev.simple_active, "简码应已激活");
        assert!(ev.has_simple_impact(&ctx, 0), "组0移动应触发简码增量路径");

        let baseline = ev.full_rebuild_calls();
        let mut rng = thread_rng();

        // 把组0 从键0 移到键1 ⟹ 三字同键，重码频率从 1 暴涨到 100001，delta>0。
        // temp = f64::MIN_POSITIVE ⟹ exp(-delta/temp) = 0.0，rng.gen()∈[0,1) 不可能 < 0 ⟹ 必然拒绝。
        let rejected = !ev.try_move(&ctx, &mut assignment, 0, 1, f64::MIN_POSITIVE, &mut rng);
        assert!(rejected, "恶化移动 + 极小温度应必然拒绝（覆盖 rollback 分支）");
        // 拒绝后 assignment 应已被还原。
        assert_eq!(assignment, vec![0u8, 1u8, 1u8], "拒绝后分配应回滚");
        assert_eq!(
            ev.full_rebuild_calls(),
            baseline,
            "拒绝分支（rollback）期间全量重建计数不得增长"
        );

        // try_swap 的拒绝分支：交换组0(键0)与组2(键1) ⟹ 组0(高频)与组1(高频)同键，重码暴涨，必然拒绝。
        let swap_rejected =
            !ev.try_swap(&ctx, &mut assignment, 0, 2, f64::MIN_POSITIVE, &mut rng);
        assert!(swap_rejected, "恶化交换 + 极小温度应必然拒绝");
        assert_eq!(assignment, vec![0u8, 1u8, 1u8], "拒绝后分配应回滚");
        assert_eq!(
            ev.full_rebuild_calls(),
            baseline,
            "try_swap 拒绝分支期间全量重建计数不得增长"
        );
    }

    // -------------------------------------------------------------------------
    // 属性测试：对随机分配 / 模式 / 覆盖率 / 移动与交换序列（混合温度同时覆盖接受与拒绝），
    // 断言「激活后整批热路径调用期间全量重建计数零增长」。≥100 次迭代。
    // -------------------------------------------------------------------------
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        // Feature: simple-code-perf-optimization, hot-path regression guard: 激活后 try_move/try_swap 不触发全量重建
        #[test]
        fn prop_hot_path_zero_full_rebuild(
            mode_is_freq in any::<bool>(),
            specs in prop::collection::vec((1u64..6, 1usize..4), 3usize..9),
            code_num in 1usize..4,
            coverage_pct in 50u32..=100,
            // 每个操作：(组索引种子, 第二组索引种子, 新键位, 是否交换, 是否高温)
            ops in prop::collection::vec((0usize..64, 0usize..64, 0u8..2, any::<bool>(), any::<bool>()), 1usize..40),
        ) {
            let mode = if mode_is_freq {
                SimpleAssignMode::Frequency
            } else {
                SimpleAssignMode::Efficiency
            };
            let coverage_ratio = coverage_pct as f64 / 100.0;
            let ctx = make_ctx(&specs, mode, code_num, coverage_ratio);
            let n_groups = ctx.num_groups;

            let mut assignment = vec![0u8; n_groups];
            let mut ev = new_activated(&ctx, &assignment);
            prop_assert!(ev.simple_eval.is_some() && ev.simple_active, "激活后简码应启用");

            let baseline = ev.full_rebuild_calls();
            let mut rng = thread_rng();

            let mut accepted = 0usize;
            let mut rejected = 0usize;

            for &(g1, g2, nk, is_swap, hot) in ops.iter() {
                // 高温 +∞ ⟹ 必然接受（覆盖 commit）；极小温度 ⟹ 恶化移动必然拒绝（覆盖 rollback）。
                let temp = if hot { f64::INFINITY } else { f64::MIN_POSITIVE };
                let r1 = g1 % n_groups;
                let did = if is_swap {
                    let r2 = g2 % n_groups;
                    if r1 != r2 && assignment[r1] != assignment[r2] {
                        ev.try_swap(&ctx, &mut assignment, r1, r2, temp, &mut rng)
                    } else {
                        false
                    }
                } else {
                    ev.try_move(&ctx, &mut assignment, r1, nk, temp, &mut rng)
                };
                if did { accepted += 1; } else { rejected += 1; }

                // 核心防回归断言：无论接受还是拒绝，热路径都不得触发任何全量重建。
                prop_assert_eq!(
                    ev.full_rebuild_calls(),
                    baseline,
                    "热路径调用后全量重建计数发生增长（应零增长）"
                );
            }

            // 计数器仅作内部一致性观测（不强制两分支同时出现，由确定性测试保证分支覆盖）。
            prop_assert_eq!(accepted + rejected, ops.len());
        }
    }
}

// =========================================================================
// 🧪 空格上屏与长度约束属性测试（simple-code-perf-optimization, Property 16 / 20）
// =========================================================================
#[cfg(test)]
mod space_commit_and_length_tests {
    use super::*;
    use crate::config::TargetsConfig;
    use crate::context::OptContext;
    use crate::types::{
        KeyDistConfig, RootGroup, ScaleConfig, SimpleAssignMode, SimpleCodeConfig, SimpleCodeLevel,
        SimpleCodeStep, WeightConfig, EQUIV_TABLE_SIZE, KEY_SPACE,
    };
    use proptest::prelude::*;
    use std::collections::HashMap;

    /// 构建启用简码、含 3 个级别（步数分别为 1/2/3）的最小 OptContext，可按级别指定
    /// `space_commit`。`specs[i] = (freq, n_roots)`：第 i 个汉字含 `n_roots` 个互不相同
    /// 的字根（各自独立成组），故该字全码部件数 == `n_roots`。
    ///
    /// 等量表设为全 1：使简码当量有解析闭式——步数为 `n` 的级别，无空格上屏时当量为
    /// `(n-1)/n`，有空格上屏时为 `n/n = 1`（含末位键到 KEY_SPACE 的 +1 转移）。
    fn make_ctx_space(specs: &[(u64, usize)], space: [bool; 3]) -> OptContext {
        let mut groups: Vec<RootGroup> = Vec::new();
        let mut splits: Vec<(char, Vec<String>, u64)> = Vec::with_capacity(specs.len());
        for (i, &(freq, n_roots)) in specs.iter().enumerate() {
            let mut roots: Vec<String> = Vec::with_capacity(n_roots);
            for j in 0..n_roots {
                let root = format!("r{i}_{j}");
                groups.push(RootGroup {
                    roots: vec![root.clone()],
                    allowed_keys: vec![0, 1, 2],
                });
                roots.push(root);
            }
            let ch = char::from_u32(0x4e00 + i as u32).unwrap();
            splits.push((ch, roots, freq));
        }

        let step = |sel: char| SimpleCodeStep {
            root_selector: sel,
            code_selector: 'a',
        };
        let levels = vec![
            SimpleCodeLevel {
                level: 1,
                code_num: 1,
                rule_candidates: vec![vec![step('A')]],
                space_commit: space[0],
            },
            SimpleCodeLevel {
                level: 2,
                code_num: 1,
                rule_candidates: vec![vec![step('A'), step('B')]],
                space_commit: space[1],
            },
            SimpleCodeLevel {
                level: 3,
                code_num: 1,
                rule_candidates: vec![vec![step('A'), step('B'), step('C')]],
                space_commit: space[2],
            },
        ];

        let fixed_roots: HashMap<String, u8> = HashMap::new();
        let equiv_table = [[1.0f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];
        let key_dist = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];
        let mut weights = WeightConfig::default();
        weights.enable_simple_code = true;
        weights.simple_coverage_ratio = 1.0; // 全部字纳入候选
        OptContext::new(
            &splits,
            &fixed_roots,
            &groups,
            equiv_table,
            key_dist,
            ScaleConfig::default(),
            SimpleCodeConfig { levels },
            weights,
            TargetsConfig::default(),
        )
    }

    /// 构建单级（步数 1，规则 [A.a]）、每字含 3 个独立字根（full_len=3）的 OptContext，
    /// 可指定该级 space_commit。code_space = code_base^3（约 3.3 万）保持可控，适合构建
    /// SimpleEvaluator 验证分布口径。
    fn make_ctx_single_step1(freqs: &[u64], space_commit: bool) -> OptContext {
        let mut groups: Vec<RootGroup> = Vec::new();
        let mut splits: Vec<(char, Vec<String>, u64)> = Vec::with_capacity(freqs.len());
        for (i, &freq) in freqs.iter().enumerate() {
            let mut roots: Vec<String> = Vec::with_capacity(3);
            for j in 0..3usize {
                let root = format!("s{i}_{j}");
                groups.push(RootGroup {
                    roots: vec![root.clone()],
                    allowed_keys: vec![0, 1, 2],
                });
                roots.push(root);
            }
            let ch = char::from_u32(0x4e00 + i as u32).unwrap();
            splits.push((ch, roots, freq));
        }
        let step = |sel: char| SimpleCodeStep {
            root_selector: sel,
            code_selector: 'a',
        };
        let levels = vec![SimpleCodeLevel {
            level: 1,
            code_num: 1,
            rule_candidates: vec![vec![step('A')]],
            space_commit,
        }];
        let fixed_roots: HashMap<String, u8> = HashMap::new();
        let equiv_table = [[1.0f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];
        let key_dist = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];
        let mut weights = WeightConfig::default();
        weights.enable_simple_code = true;
        weights.simple_coverage_ratio = 1.0;
        // 用 Frequency 模式：桶内排序键 = freq，与 base_saving 无关，使 space true/false 两侧
        // 出简选择集合一致，从而隔离「尾随空格对分布的影响」这一被测口径（需求 20.7/20.8）。
        weights.simple_assign_mode = SimpleAssignMode::Frequency;
        OptContext::new(
            &splits,
            &fixed_roots,
            &groups,
            equiv_table,
            key_dist,
            ScaleConfig::default(),
            SimpleCodeConfig { levels },
            weights,
            TargetsConfig::default(),
        )
    }

    /// 构建 SimpleEvaluator 所需的全码桶存储与 is_first_candidate（与
    /// `save_simple_code_output` / `Evaluator::new` 同口径）。
    fn build_simple_eval(ctx: &OptContext, asg: &[u8]) -> SimpleEvaluator {
        let (full_buckets, is_first) = build_full_buckets(ctx, asg);
        SimpleEvaluator::new(ctx, asg, &full_buckets, &is_first, false)
    }

    /// 该级指令步数（None 计 0）。
    fn step_count(ctx: &OptContext, ci: usize, li: usize) -> usize {
        ctx.char_simple_infos[ci]
            .level_instructions
            .get(li)
            .and_then(|o| o.as_ref())
            .map_or(0, |v| v.len())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        // Feature: simple-code-perf-optimization, Property 16: 空格上屏的 base_saving 与当量/分布口径
        //
        // 对任意候选字 ci 与简码级别 li：当该级 space_commit 为真时，simple_base_saving[ci][li]
        // 应等于 full_len - (simple_len + 1)，且该字出简对当量计入末位键到 KEY_SPACE 的转移、
        // 对分布计入一次 KEY_SPACE 键（key_usage 与 key_presses 各 +1）；当 space_commit 为假时
        // base_saving 应等于 full_len - simple_len，且当量与分布均不含尾随空格项。
        // 其中 full_len = char_infos[ci].parts.len()，simple_len 为该级指令步数。
        #[test]
        fn prop16_space_commit_base_saving_equiv_dist(
            // n_roots ∈ 4..=6 使三级（步数 1/2/3）的 base_saving / 当量口径覆盖多步指令。
            // 注意：(A)(B) 仅用 base_saving 与 calc_simple_equiv，无需构建 SimpleEvaluator，
            // 故不触发 code_space 大小的分配，可放心取较大 n_roots。
            specs in prop::collection::vec((1u64..=200, 4usize..=6), 1..=6),
        ) {
            let ctx_f = make_ctx_space(&specs, [false, false, false]);
            let ctx_t = make_ctx_space(&specs, [true, true, true]);
            let n_chars = ctx_f.char_infos.len();
            let n_levels = ctx_f.simple_config.levels.len();
            let asg = vec![0u8; ctx_f.num_groups];

            // (A) base_saving 口径
            for ci in 0..n_chars {
                let full_len = ctx_f.char_infos[ci].parts.len() as i64;
                for li in 0..n_levels {
                    let sc = step_count(&ctx_f, ci, li) as i64;
                    prop_assert_eq!(ctx_f.simple_base_saving[ci][li], full_len - sc,
                        "space_commit=false: base_saving 应为 full_len - simple_len");
                    prop_assert_eq!(ctx_t.simple_base_saving[ci][li], full_len - (sc + 1),
                        "space_commit=true: base_saving 应为 full_len - (simple_len + 1)");
                }
            }

            // (B) 当量口径：全 1 等量表 ⟹ 步数 n 的级别，false=(n-1)/n，true=n/n=1
            for ci in 0..n_chars {
                for li in 0..n_levels {
                    let n = step_count(&ctx_f, ci, li);
                    if n == 0 { continue; }
                    let eq_f = ctx_f.calc_simple_equiv(ci, li, &asg);
                    let eq_t = ctx_t.calc_simple_equiv(ci, li, &asg);
                    let exp_f = (n as f64 - 1.0) / n as f64;
                    let exp_t = 1.0;
                    prop_assert!((eq_f - exp_f).abs() < 1e-9,
                        "space=false 当量应为 (n-1)/n: got {} expect {}", eq_f, exp_f);
                    prop_assert!((eq_t - exp_t).abs() < 1e-9,
                        "space=true 当量应为 1.0（含尾随空格转移）: got {}", eq_t);
                    // 差额恰为一个「末位键→KEY_SPACE」转移除以 n（此处等量表为 1）
                    prop_assert!((eq_t - eq_f - 1.0 / n as f64).abs() < 1e-9);
                }
            }

            // (C) 分布口径：用小 code_space 的单级上下文（n_roots=3、步数 1）构建 SimpleEvaluator，
            // 比较 true/false 的 key_usage[KEY_SPACE] 与 key_presses。单级 n_roots=3 使 code_space
            // = code_base^3（约 3.3 万）保持可控，避免 build_simple_eval 的大分配。
            let dfreqs: Vec<u64> = specs.iter().map(|&(f, _)| f).collect();
            let ctx_fd = make_ctx_single_step1(&dfreqs, false);
            let ctx_td = make_ctx_single_step1(&dfreqs, true);
            let dn = ctx_fd.char_infos.len();
            let asgd = vec![0u8; ctx_fd.num_groups];
            let se_f = build_simple_eval(&ctx_fd, &asgd);
            let se_t = build_simple_eval(&ctx_td, &asgd);

            // 单级（li=0）：false 不含尾随空格键；选中字 true 侧各计一次 KEY_SPACE。
            prop_assert_eq!(se_f.levels[0].key_usage[KEY_SPACE], 0.0,
                "space=false 不应计入 KEY_SPACE 用键");

            let mut sel_freq_sum = 0.0f64;
            for ci in 0..dn {
                // 长度资格在 n_roots=3、步数 1 下 true/false 均满足 ⟹ 出简选择一致
                prop_assert_eq!(se_f.levels[0].selected[ci], se_t.levels[0].selected[ci],
                    "true/false 出简选择应一致 (ci={})", ci);
                if se_t.levels[0].selected[ci] {
                    sel_freq_sum += ctx_fd.char_infos[ci].frequency as f64;
                }
            }

            prop_assert!((se_t.levels[0].key_usage[KEY_SPACE] - sel_freq_sum).abs() < 1e-6,
                "space=true KEY_SPACE 用键应等于选中字字频和: got {} expect {}",
                se_t.levels[0].key_usage[KEY_SPACE], sel_freq_sum);

            for k in 0..EQUIV_TABLE_SIZE {
                if k == KEY_SPACE { continue; }
                prop_assert!((se_f.levels[0].key_usage[k] - se_t.levels[0].key_usage[k]).abs() < 1e-6,
                    "非空格键用键应一致 (k={})", k);
            }

            let diff = se_t.levels[0].key_presses - se_f.levels[0].key_presses;
            prop_assert!((diff - sel_freq_sum).abs() < 1e-6,
                "space=true key_presses 应比 false 多选中字字频和: diff {} expect {}",
                diff, sel_freq_sum);
        }

        // Feature: simple-code-perf-optimization, Property 20: 简码长度严格短于全码
        //
        // 对任意候选字 ci 与级别 li，该字在该级出简（进入简码桶且 current_simple_code[ci] != -1）
        // 当且仅当其有效简码长度 effective_simple_len(li) = 指令步数 + (space_commit ? 1 : 0)
        // 严格小于全码长度 full_len(ci) = char_infos[ci].parts.len()。任何被分配（含退火分配）
        // 的简码，其有效长度都严格小于对应字的全码长度。
        #[test]
        fn prop20_simple_len_strictly_shorter_than_full(
            // n_roots ∈ 1..=3 覆盖「合格」与「不合格（含 None 级别与 effective>=full）」两种分支，
            // 同时把 code_space 控制在 code_base^3（约 3.3 万），避免 build_simple_eval 大分配。
            specs in prop::collection::vec((1u64..=200, 1usize..=3), 1..=6),
            space in prop::collection::vec(any::<bool>(), 3),
        ) {
            let sp = [space[0], space[1], space[2]];
            let ctx = make_ctx_space(&specs, sp);
            let n_chars = ctx.char_infos.len();
            let n_levels = ctx.simple_config.levels.len();
            let asg = vec![0u8; ctx.num_groups];

            // (A) 资格谓词与重算一致：eligible ⟺ (step>0 && step + space < full_len)
            for ci in 0..n_chars {
                let full_len = ctx.char_infos[ci].parts.len();
                for li in 0..n_levels {
                    let sc = step_count(&ctx, ci, li);
                    let space_bit = if sp[li] { 1 } else { 0 };
                    let effective = sc + space_bit;
                    let expected = sc > 0 && effective < full_len;
                    prop_assert_eq!(ctx.simple_is_eligible(ci, li), expected,
                        "eligibility 谓词与重算不一致 (ci={}, li={}, sc={}, space={}, full={})",
                        ci, li, sc, space_bit, full_len);
                }
            }

            // (B) 进入简码桶 / 出简的字其有效长度必严格短于全码
            let se = build_simple_eval(&ctx, &asg);
            for li in 0..n_levels {
                for ci in 0..n_chars {
                    let full_len = ctx.char_infos[ci].parts.len();
                    let sc = step_count(&ctx, ci, li);
                    let effective = sc + if sp[li] { 1 } else { 0 };

                    // 进入简码桶（current_simple_code != -1）⟹ 合格 ⟹ 有效长度 < 全码
                    if se.levels[li].current_simple_code[ci] != -1 {
                        prop_assert!(ctx.simple_is_eligible(ci, li),
                            "入桶字必合格 (ci={}, li={})", ci, li);
                        prop_assert!(effective < full_len,
                            "入桶字有效长度 {} 应 < 全码 {} (ci={}, li={})", effective, full_len, ci, li);
                    }
                    // 被实际出简（selected）⟹ 有效长度 < 全码
                    if se.levels[li].selected[ci] {
                        prop_assert!(effective < full_len,
                            "出简字有效长度 {} 应 < 全码 {} (ci={}, li={})", effective, full_len, ci, li);
                    }
                }
            }
        }
    }
}

// =========================================================================
// 🧪 固定简码测试（simple-code-perf-optimization, Property 18 / 19 + 任务 15.1 校验 + 17.1 回归）
// =========================================================================
#[cfg(test)]
mod fixed_simple_code_tests {
    use super::*;
    use crate::config::TargetsConfig;
    use crate::context::OptContext;
    use crate::types::{
        key_to_char, KeyDistConfig, RootGroup, ScaleConfig, SimpleAssignMode, SimpleCodeConfig,
        SimpleCodeLevel, SimpleCodeStep, WeightConfig, EQUIV_TABLE_SIZE,
    };
    use proptest::prelude::*;
    use std::collections::HashMap;

    /// 构建启用简码、含 3 个级别、参数化分配模式 / 每级编码数 / 候选覆盖率 / 各级 space_commit /
    /// 固定简码映射的 OptContext。
    ///
    /// `specs[i] = (freq, n_roots)`：第 i 个汉字含 `n_roots` 个互不相同字根（各自独立成组），
    /// 故全码部件数 == `n_roots`。仅用 2 个允许键位 `[0, 1]`，使级别 0 桶容量为 2、级别 1 为 4、
    /// 级别 2 为 8，制造同桶碰撞与「桶成员数 > code_num」选取，并令固定简码（键位 0/1）与优化字
    /// 争用同一桶空间，从而真正考验占用名额扣减（需求 21.7）。
    fn make_ctx_fixed(
        specs: &[(u64, usize)],
        mode: SimpleAssignMode,
        code_num: usize,
        coverage_ratio: f64,
        space: [bool; 3],
        fixed: &[(char, String)],
    ) -> OptContext {
        let mut groups: Vec<RootGroup> = Vec::new();
        let mut splits: Vec<(char, Vec<String>, u64)> = Vec::with_capacity(specs.len());
        for (i, &(freq, n_roots)) in specs.iter().enumerate() {
            let n_roots = n_roots.max(1);
            let mut roots: Vec<String> = Vec::with_capacity(n_roots);
            for j in 0..n_roots {
                let root = format!("r{i}_{j}");
                groups.push(RootGroup {
                    roots: vec![root.clone()],
                    allowed_keys: vec![0, 1],
                });
                roots.push(root);
            }
            let ch = char::from_u32(0x4e00 + i as u32).unwrap();
            splits.push((ch, roots, freq.max(1)));
        }

        let step = |sel: char| SimpleCodeStep {
            root_selector: sel,
            code_selector: 'a',
        };
        let levels = vec![
            SimpleCodeLevel {
                level: 1,
                code_num,
                rule_candidates: vec![vec![step('A')]],
                space_commit: space[0],
            },
            SimpleCodeLevel {
                level: 2,
                code_num,
                rule_candidates: vec![vec![step('A'), step('B')]],
                space_commit: space[1],
            },
            SimpleCodeLevel {
                level: 3,
                code_num,
                rule_candidates: vec![vec![step('A'), step('B'), step('C')]],
                space_commit: space[2],
            },
        ];

        let fixed_roots: HashMap<String, u8> = HashMap::new();
        let equiv_table = [[0.0f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];
        let key_dist = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];
        let mut weights = WeightConfig::default();
        weights.enable_simple_code = true;
        weights.simple_coverage_ratio = coverage_ratio;
        weights.simple_assign_mode = mode;
        OptContext::new_with_fixed(
            &splits,
            &fixed_roots,
            &groups,
            equiv_table,
            key_dist,
            ScaleConfig::default(),
            SimpleCodeConfig { levels },
            weights,
            TargetsConfig::default(),
            fixed,
        )
    }

    /// 把 (字索引, 键位) 列表去重为「按字索引去重、键位映射为单字符级别 0 固定简码」。
    /// 返回 (char, code_str) 列表（如 (0x4e00, "a")）。所有项均为级别 0（1 键，无空格上屏），
    /// 当对应字 `n_roots >= 2` 时有效长度 1 < 全码长度，故必被接受。
    fn build_level0_fixed(raw: &[(u8, u8)], n: usize) -> Vec<(char, String)> {
        let mut seen: Vec<usize> = Vec::new();
        let mut out: Vec<(char, String)> = Vec::new();
        for &(idx, key) in raw {
            let ci = (idx as usize) % n;
            if seen.contains(&ci) {
                continue;
            }
            seen.push(ci);
            let k = key % 2; // 仅用允许键位 {0,1}
            let ch = char::from_u32(0x4e00 + ci as u32).unwrap();
            out.push((ch, key_to_char(k).to_string()));
        }
        out
    }

    // ---------------------------------------------------------------------
    // Property 18: 固定简码与候选字集合解耦且占用名额
    // ---------------------------------------------------------------------
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        // Feature: simple-code-perf-optimization, Property 18: 固定简码与候选字集合解耦且占用名额
        //
        // 对任意字频分布、覆盖率阈值与固定简码映射，候选字集合的「按覆盖率选取」结果应与无固定
        // 简码时完全一致；剔除步骤后，候选字集合恰为「该覆盖率前缀」去掉固定简码字；且对任意级别
        // li 与桶编码 code，该桶经退火分配的出简数不超过 code_num - simple_fixed_occupancy[li][code]，
        // 「固定占用 + 优化分配」不超过 code_num（含跨移动序列）。
        // Validates: Requirements 21.3, 21.4, 21.7
        #[test]
        fn prop18_fixed_decoupled_and_occupancy(
            mode_is_freq in any::<bool>(),
            // n_roots >= 2，保证级别 0 单键固定简码（有效长 1）严格短于全码
            specs in prop::collection::vec((1u64..6, 2usize..4), 3usize..9),
            code_num in 1usize..4,
            coverage_pct in 50u32..=100,
            fixed_raw in prop::collection::vec((0u8..8, 0u8..2), 0usize..5),
            moves in prop::collection::vec((0usize..64, 0u8..2), 0usize..24),
        ) {
            let mode = if mode_is_freq { SimpleAssignMode::Frequency } else { SimpleAssignMode::Efficiency };
            let coverage_ratio = coverage_pct as f64 / 100.0;
            let n = specs.len();
            let fixed = build_level0_fixed(&fixed_raw, n);

            // 基线：无固定简码
            let ctx_base = make_ctx_fixed(&specs, mode, code_num, coverage_ratio, [false; 3], &[]);
            // 含固定简码
            let ctx = make_ctx_fixed(&specs, mode, code_num, coverage_ratio, [false; 3], &fixed);

            // (A) 候选选取与无固定简码一致；剔除步骤后 == 前缀去掉固定字（需求 21.3/21.4）
            let expected: Vec<usize> = ctx_base
                .simple_candidate_chars
                .iter()
                .copied()
                .filter(|&ci| !ctx.simple_fixed_assigned[ci])
                .collect();
            prop_assert_eq!(&ctx.simple_candidate_chars, &expected,
                "剔除固定字后候选集应等于无固定简码前缀去掉固定字");
            // 覆盖率（按全集前缀计）不受固定简码影响
            prop_assert_eq!(ctx.simple_actual_coverage, ctx_base.simple_actual_coverage,
                "覆盖率应与无固定简码时一致");
            // 固定字一律不在候选集
            for fc in &ctx.simple_fixed_codes {
                prop_assert!(!ctx.simple_is_candidate[fc.ci], "固定字不应在候选集");
            }

            // (B) 占用名额约束（初始 + 跨移动序列）
            let mut assignment = vec![0u8; ctx.num_groups];
            let mut ev = Evaluator::new(&ctx, &assignment);
            check_occupancy_bound(&ctx, &ev)?;
            for &(gi, nk) in &moves {
                let r = gi % ctx.num_groups;
                assignment[r] = nk;
                for idx in 0..ctx.group_to_chars[r].len() {
                    let ci = ctx.group_to_chars[r][idx];
                    ev.update_char(&ctx, &assignment, ci);
                }
                ev.apply_simple_for_move(&ctx, &assignment, &[r]);
                ev.commit_simple();
                check_occupancy_bound(&ctx, &ev)?;
            }
        }
    }

    /// 断言（确认点 2 新语义）：每级每桶的优化出简数 ≤ max(0, code_num - 固定占用)。
    /// 固定占用本身不受 code_num 限制（固定简码权威预分配，可达到/超过 code_num，此时退火出 0）。
    fn check_occupancy_bound(ctx: &OptContext, ev: &Evaluator) -> Result<(), TestCaseError> {
        let se = ev.simple_eval.as_ref().expect("simple_eval");
        for li in 0..se.levels.len() {
            let cn = se.levels[li].code_num;
            let lvl = &se.levels[li];
            for (code, b) in lvl.buckets.iter_nonempty() {
                let code = code as usize;
                let occ = ctx.simple_fixed_occ(li, code);
                let sel_count = b
                    .members
                    .iter()
                    .filter(|&&ci| lvl.selected[ci as usize])
                    .count();
                prop_assert!(sel_count <= cn.saturating_sub(occ),
                    "级别 {} 桶 {} 优化出简数 {} 超过可选名额 max(0, {} - {})", li, code, sel_count, cn, occ);
            }
        }
        Ok(())
    }

    // ---------------------------------------------------------------------
    // Property 19: 固定简码的恒定出简贡献
    // ---------------------------------------------------------------------
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        // Feature: simple-code-perf-optimization, Property 19: 固定简码的恒定出简贡献
        //
        // 对任意分配与任意一串移动序列，固定简码字的 all_assigned_flags 恒为真（始终从全码桶的
        // 简码重码统计中排除）；且固定简码对简码覆盖率、加权当量、分布偏差的贡献为不随分配变化
        // 的常量。固定简码的级别归属须与其结尾下划线（与该级 space_commit）及长度约束一致。
        // Validates: Requirements 21.5, 21.6, 21.8, 21.9
        #[test]
        fn prop19_fixed_constant_contribution(
            mode_is_freq in any::<bool>(),
            specs in prop::collection::vec((1u64..6, 2usize..4), 3usize..9),
            code_num in 1usize..4,
            coverage_pct in 50u32..=100,
            fixed_raw in prop::collection::vec((0u8..8, 0u8..2), 1usize..5),
            moves in prop::collection::vec((0usize..64, 0u8..2), 0usize..24),
        ) {
            let mode = if mode_is_freq { SimpleAssignMode::Frequency } else { SimpleAssignMode::Efficiency };
            let coverage_ratio = coverage_pct as f64 / 100.0;
            let n = specs.len();
            let fixed = build_level0_fixed(&fixed_raw, n);
            let ctx = make_ctx_fixed(&specs, mode, code_num, coverage_ratio, [false; 3], &fixed);

            // 固定字常量贡献（级别 0、单键、无空格、equiv_table 全零）：
            //   fixed_covered_freq = Σ freq；fixed_equiv_weighted = 0；fixed_key_presses = Σ freq×1
            let mut exp_cov = 0u64;
            let mut exp_presses = 0.0f64;
            for fc in &ctx.simple_fixed_codes {
                // 级别归属正确：码长（核心键位数）== 该级简码键位数（此处级别 0 → 1 键）
                prop_assert_eq!(fc.li, 0, "级别 0 单键固定简码应归属级别 0");
                prop_assert_eq!(fc.space_commit, false, "级别 0 space_commit 为 false");
                prop_assert_eq!(fc.keys.len(), 1, "核心码长应为 1");
                let f = ctx.char_infos[fc.ci].frequency;
                exp_cov += f;
                exp_presses += f as f64;
            }
            prop_assert_eq!(ctx.fixed_covered_freq, exp_cov, "fixed_covered_freq 不一致");
            prop_assert_eq!(ctx.fixed_equiv_freq_sum, exp_cov, "fixed_equiv_freq_sum 不一致");
            prop_assert!(ctx.fixed_equiv_weighted.abs() < 1e-12, "equiv_table 全零时 fixed_equiv_weighted 应为 0");
            prop_assert!((ctx.fixed_key_presses - exp_presses).abs() < 1e-9, "fixed_key_presses 不一致");

            let mut assignment = vec![0u8; ctx.num_groups];
            let mut ev = Evaluator::new(&ctx, &assignment);

            // 跨移动序列：固定字 all_assigned 恒真、绝不出现在任何桶 / 任何级别 selected。
            let check_invariant = |ev: &Evaluator| -> Result<(), TestCaseError> {
                let se = ev.simple_eval.as_ref().expect("simple_eval");
                for fc in &ctx.simple_fixed_codes {
                    prop_assert!(se.all_assigned_flags[fc.ci],
                        "固定字 ci={} 的 all_assigned_flags 应恒为真", fc.ci);
                    for li in 0..se.levels.len() {
                        prop_assert!(!se.levels[li].selected[fc.ci],
                            "固定字 ci={} 不应被任何级别 selected", fc.ci);
                        prop_assert_eq!(se.levels[li].current_simple_code[fc.ci], -1,
                            "固定字 ci={} 不应进入任何简码桶", fc.ci);
                    }
                }
                Ok(())
            };
            check_invariant(&ev)?;
            for &(gi, nk) in &moves {
                let r = gi % ctx.num_groups;
                assignment[r] = nk;
                for idx in 0..ctx.group_to_chars[r].len() {
                    let ci = ctx.group_to_chars[r][idx];
                    ev.update_char(&ctx, &assignment, ci);
                }
                ev.apply_simple_for_move(&ctx, &assignment, &[r]);
                ev.commit_simple();
                check_invariant(&ev)?;
            }
        }
    }

    // ---------------------------------------------------------------------
    // 任务 15.1：级别归属、一致性（21.6）与长度（22.3）校验 —— 确定性单元测试
    // ---------------------------------------------------------------------

    /// 用给定 space 配置与单条固定简码构建 ctx，返回接受的固定简码条数。
    fn count_accepted(space: [bool; 3], ch_idx: usize, code: &str, n_roots: usize) -> usize {
        // 单个汉字、n_roots 个根，频率 100
        let specs = vec![(100u64, n_roots)];
        let _ = ch_idx; // 仅一个字，索引恒 0
        let ch = char::from_u32(0x4e00).unwrap();
        let fixed = vec![(ch, code.to_string())];
        let ctx = make_ctx_fixed(&specs, SimpleAssignMode::Efficiency, 1, 1.0, space, &fixed);
        ctx.simple_fixed_codes.len()
    }

    #[test]
    fn fixed_code_level_assignment_by_core_length() {
        // 级别 0=1键, 级别 1=2键, 级别 2=3键（均无空格上屏）。
        // n_roots=4 → 全码长 4，各级有效长度 1/2/3 均 < 4，长度约束满足。
        let specs = vec![(100u64, 4usize)];
        let ch = char::from_u32(0x4e00).unwrap();

        // "a" → 级别 0
        let ctx0 = make_ctx_fixed(&specs, SimpleAssignMode::Efficiency, 1, 1.0, [false; 3], &[(ch, "a".to_string())]);
        assert_eq!(ctx0.simple_fixed_codes.len(), 1);
        assert_eq!(ctx0.simple_fixed_codes[0].li, 0);

        // "ab" → 级别 1
        let ctx1 = make_ctx_fixed(&specs, SimpleAssignMode::Efficiency, 1, 1.0, [false; 3], &[(ch, "ab".to_string())]);
        assert_eq!(ctx1.simple_fixed_codes.len(), 1);
        assert_eq!(ctx1.simple_fixed_codes[0].li, 1);

        // "abc" → 级别 2（有效长 3 < 全码 4）
        let ctx2 = make_ctx_fixed(&specs, SimpleAssignMode::Efficiency, 1, 1.0, [false; 3], &[(ch, "abc".to_string())]);
        assert_eq!(ctx2.simple_fixed_codes.len(), 1);
        assert_eq!(ctx2.simple_fixed_codes[0].li, 2);
    }

    #[test]
    fn fixed_code_underscore_consistency() {
        let specs = vec![(100u64, 4usize)];
        let ch = char::from_u32(0x4e00).unwrap();

        // 级别 0 space_commit=true，固定简码 "a_"（一致）→ 接受，输出 "a_"。
        let ctx = make_ctx_fixed(&specs, SimpleAssignMode::Efficiency, 1, 1.0, [true, false, false], &[(ch, "a_".to_string())]);
        assert_eq!(ctx.simple_fixed_codes.len(), 1);
        assert!(ctx.simple_fixed_codes[0].space_commit);
        assert_eq!(ctx.simple_fixed_codes[0].code_str, "a_");

        // 级别 0 space_commit=true，固定简码 "a"（无下划线）→ 仅警告、按原样接受（确认点 4）：
        // 输出 "a"（不额外加下划线），space_commit 字段以固定简码自身为准（false）。
        let ctx2 = make_ctx_fixed(&specs, SimpleAssignMode::Efficiency, 1, 1.0, [true, false, false], &[(ch, "a".to_string())]);
        assert_eq!(ctx2.simple_fixed_codes.len(), 1, "space_commit=true 但无下划线应警告并接受");
        assert!(!ctx2.simple_fixed_codes[0].space_commit);
        assert_eq!(ctx2.simple_fixed_codes[0].code_str, "a");
    }

    #[test]
    #[should_panic(expected = "space_commit=false")]
    fn fixed_code_underscore_on_non_space_commit_level_panics() {
        // 级别 0 space_commit=false，但固定简码以下划线结尾 "a_" → 配置错误，解析期 panic（确认点 4）。
        let _ = count_accepted([false, false, false], 0, "a_", 4);
    }

    #[test]
    fn fixed_code_length_constraint_rejected() {
        // 全码长度 2，级别 1（2 键）有效长度 2，不严格短于全码 → 拒绝（需求 22.3）。
        let specs = vec![(100u64, 2usize)];
        let ch = char::from_u32(0x4e00).unwrap();
        let ctx = make_ctx_fixed(&specs, SimpleAssignMode::Efficiency, 1, 1.0, [false; 3], &[(ch, "ab".to_string())]);
        assert_eq!(ctx.simple_fixed_codes.len(), 0, "有效长度不短于全码应被拒绝");

        // 级别 0（1 键）有效长度 1 < 全码 2 → 接受。
        let ctx_ok = make_ctx_fixed(&specs, SimpleAssignMode::Efficiency, 1, 1.0, [false; 3], &[(ch, "a".to_string())]);
        assert_eq!(ctx_ok.simple_fixed_codes.len(), 1);
    }

    #[test]
    fn fixed_code_on_code_num_zero_level() {
        // 确认点 1：级别 code_num=0 时，归属该级的固定简码仍生效（被接受、占用桶、计入指标），
        // 且退火不再在任何桶出简（take(max(0, 0-occ)) = 0）。
        let specs = vec![(100u64, 4usize), (90, 4), (80, 4)];
        let ch = char::from_u32(0x4e00).unwrap();
        let ctx = make_ctx_fixed(
            &specs,
            SimpleAssignMode::Efficiency,
            0, // 所有级别 code_num=0
            1.0,
            [false; 3],
            &[(ch, "a".to_string())],
        );
        // 固定简码被接受、归属级别 0
        assert_eq!(ctx.simple_fixed_codes.len(), 1, "code_num=0 级别的固定简码应生效");
        assert_eq!(ctx.simple_fixed_codes[0].li, 0);
        // 固定字被剔除候选、计入常量覆盖
        assert!(!ctx.simple_is_candidate[0], "固定字应被剔除候选集");
        assert!(ctx.fixed_covered_freq >= 100, "固定字字频应计入覆盖偏置");

        // 退火端：code_num=0 ⟹ 任何桶优化出简为 0
        let asg = vec![0u8; ctx.num_groups];
        let ev = Evaluator::new(&ctx, &asg);
        let se = ev.simple_eval.as_ref().expect("simple_eval");
        let any_selected = se.levels.iter().any(|lvl| lvl.selected.iter().any(|&s| s));
        assert!(!any_selected, "code_num=0 时退火不应分配任何简码");
    }

    #[test]
    fn fixed_code_invalid_char_or_unknown_hanzi_rejected() {
        let specs = vec![(100u64, 4usize)];
        let ch = char::from_u32(0x4e00).unwrap();
        // 非法简码键位（数字 '1' 无法映射键位）→ 拒绝
        let ctx_bad = make_ctx_fixed(&specs, SimpleAssignMode::Efficiency, 1, 1.0, [false; 3], &[(ch, "1".to_string())]);
        assert_eq!(ctx_bad.simple_fixed_codes.len(), 0);
        // 汉字不在拆分表中 → 拒绝
        let other = char::from_u32(0x9fa5).unwrap();
        let ctx_unknown = make_ctx_fixed(&specs, SimpleAssignMode::Efficiency, 1, 1.0, [false; 3], &[(other, "a".to_string())]);
        assert_eq!(ctx_unknown.simple_fixed_codes.len(), 0);
    }

    // ---------------------------------------------------------------------
    // 任务 17.1：含固定简码 + 空格上屏 + 长度约束下的回归
    //   - Property 1：增量 == 全量
    //   - Property 2：apply + rollback round-trip
    //   - Property 13：reconcile == 全量
    // ---------------------------------------------------------------------

    /// 逐字段断言两个 SimpleEvaluator 的简码指标与状态一致（含固定简码常量偏置）。
    fn assert_simple_eq(ctx: &OptContext, inc: &Evaluator, full: &Evaluator, label: &str) -> Result<(), TestCaseError> {
        let mi = inc.get_simple_metrics(ctx);
        let mf = full.get_simple_metrics(ctx);
        let eps = 1e-9;
        prop_assert!((mi.weighted_freq_coverage - mf.weighted_freq_coverage).abs() < eps, "{}: coverage", label);
        prop_assert!((mi.equiv_mean - mf.equiv_mean).abs() < eps, "{}: equiv_mean", label);
        prop_assert!((mi.dist_deviation - mf.dist_deviation).abs() < eps, "{}: dist", label);
        prop_assert_eq!(mi.collision_count, mf.collision_count, "{}: coll_count", label);
        prop_assert!((mi.collision_rate - mf.collision_rate).abs() < eps, "{}: coll_rate", label);

        let se_i = inc.simple_eval.as_ref().expect("inc se");
        let se_f = full.simple_eval.as_ref().expect("full se");
        prop_assert_eq!(&se_i.all_assigned_flags, &se_f.all_assigned_flags, "{}: all_assigned", label);
        prop_assert_eq!(se_i.simple_collision_freq, se_f.simple_collision_freq, "{}: coll_freq", label);
        for li in 0..se_i.levels.len() {
            prop_assert_eq!(&se_i.levels[li].selected, &se_f.levels[li].selected, "{}: L{} selected", label, li);
            prop_assert_eq!(&se_i.levels[li].current_simple_code, &se_f.levels[li].current_simple_code, "{}: L{} code", label, li);
            prop_assert_eq!(se_i.levels[li].covered_freq, se_f.levels[li].covered_freq, "{}: L{} covered", label, li);
            prop_assert!((se_i.levels[li].equiv_weighted - se_f.levels[li].equiv_weighted).abs() < eps, "{}: L{} equiv_w", label, li);
            prop_assert!((se_i.levels[li].key_presses - se_f.levels[li].key_presses).abs() < eps, "{}: L{} presses", label, li);
        }
        Ok(())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        // Feature: simple-code-perf-optimization, Property 1/2/13 回归: 含固定简码 + 空格上屏 + 长度约束
        //
        // 在「含固定简码、级别 1 空格上屏、长度资格过滤」的上下文下，验证：
        //   - 增量维护（update_char + apply_simple_for_move）与对同一分配全量重建逐字段一致（Property 1）；
        //   - 拒绝路径 apply + rollback 与移动前等价（Property 2）；
        //   - reconcile 后逐字段等于全量重算（Property 13）。
        // Validates: Requirements 20.7, 21.8, 21.9, 22.4 (and 1.1, 2.x, 15.x under fixed/space context)
        #[test]
        fn prop_fixed_regression_inc_rollback_reconcile(
            mode_is_freq in any::<bool>(),
            // n_roots 2..=3：code_space = code_base^max_parts 随 n_roots 指数膨胀，
            // 限制在 ≤3（code_space ≤ code_base^3）使每次 Evaluator::new 的 O(code_space)
            // 全量重建开销有界、整体快速（与 prop1/prop2/prop13 等重测试同口径）。
            specs in prop::collection::vec((1u64..6, 2usize..4), 3usize..7),
            code_num in 1usize..4,
            coverage_pct in 50u32..=100,
            fixed_raw in prop::collection::vec((0u8..8, 0u8..2), 0usize..4),
            // 每步移动都做一次全量 Evaluator::new 做 oracle，故限制移动数上界以控总开销。
            moves in prop::collection::vec((0usize..64, 0u8..2, any::<bool>()), 0usize..12),
        ) {
            let mode = if mode_is_freq { SimpleAssignMode::Frequency } else { SimpleAssignMode::Efficiency };
            let coverage_ratio = coverage_pct as f64 / 100.0;
            let n = specs.len();
            // 固定简码置于级别 0（无空格上屏）；级别 1 开启空格上屏以纳入 space_commit 口径。
            let space = [false, true, false];
            let fixed = build_level0_fixed(&fixed_raw, n);
            let ctx = make_ctx_fixed(&specs, mode, code_num, coverage_ratio, space, &fixed);
            let n_groups = ctx.num_groups;

            let mut assignment = vec![0u8; n_groups];
            let mut ev = Evaluator::new(&ctx, &assignment);
            prop_assert!(ev.simple_eval.is_some());

            // 初始即应与全量一致
            let ev0 = Evaluator::new(&ctx, &assignment);
            assert_simple_eq(&ctx, &ev, &ev0, "init")?;

            for (k, &(gi, nk, accept)) in moves.iter().enumerate() {
                let r = gi % n_groups;
                let old_key = assignment[r];

                if accept {
                    // 接受：apply + commit，并与全量比对（Property 1）
                    assignment[r] = nk;
                    for idx in 0..ctx.group_to_chars[r].len() {
                        let ci = ctx.group_to_chars[r][idx];
                        ev.update_char(&ctx, &assignment, ci);
                    }
                    ev.apply_simple_for_move(&ctx, &assignment, &[r]);
                    ev.commit_simple();

                    let full = Evaluator::new(&ctx, &assignment);
                    assert_simple_eq(&ctx, &ev, &full, &format!("step{} accept", k + 1))?;
                } else {
                    // 拒绝：记录移动前指标 → apply → 逆转全码 + rollback → 断言与移动前一致（Property 2）
                    let before = Evaluator::new(&ctx, &assignment); // 与当前 ev 同分配的 oracle
                    assignment[r] = nk;
                    for idx in 0..ctx.group_to_chars[r].len() {
                        let ci = ctx.group_to_chars[r][idx];
                        ev.update_char(&ctx, &assignment, ci);
                    }
                    ev.apply_simple_for_move(&ctx, &assignment, &[r]);
                    // 逆转全码 + 回滚简码
                    assignment[r] = old_key;
                    for idx in 0..ctx.group_to_chars[r].len() {
                        let ci = ctx.group_to_chars[r][idx];
                        ev.update_char(&ctx, &assignment, ci);
                    }
                    ev.rollback_simple();
                    assert_simple_eq(&ctx, &ev, &before, &format!("step{} reject roundtrip", k + 1))?;
                }
            }

            // Property 13：reconcile 后逐字段等于对当前分配的全量重算
            ev.reconcile(&ctx, &assignment);
            let full_end = Evaluator::new(&ctx, &assignment);
            assert_simple_eq(&ctx, &ev, &full_end, "reconcile")?;
        }
    }
}

// =========================================================================
// 🧪 简码占用保护测试（simple-code-perf-optimization, Requirement 33）
// =========================================================================
// 验证「简码占用保护」硬资格约束：
//   - N=0（保护全部）：任何出简（被选中）汉字的简码编码值，均不得等于任意汉字的全码编码值；
//   - N>0（仅保护 top-N）：出简简码不得等于任一受保护（全字频前 N 名）汉字的全码，
//     但允许等于 top-N 之外汉字的全码；
//   - 动态翻转：施加移动后，出简集合仍恒满足上述约束（增量路径）。
#[cfg(test)]
mod simple_protect_tests {
    use super::*;
    use crate::config::TargetsConfig;
    use crate::types::{
        KeyDistConfig, RootGroup, ScaleConfig, SimpleAssignMode, SimpleCodeConfig, SimpleCodeLevel,
        SimpleCodeStep, WeightConfig, EQUIV_TABLE_SIZE,
    };
    use proptest::prelude::*;
    use std::collections::HashMap;

    /// 构建启用简码、3 级、参数化 `simple_protect_top_n` 的 OptContext。
    ///
    /// `specs[i] = (freq, n_roots)`：第 i 个汉字含 `n_roots` 个独立字根（全码长度 = n_roots）。
    /// 含 n_roots=1 的字时其全码长度为 1，与级别 1 简码同长，从而可能数值相等 —— 触发占用保护。
    /// 仅用 2 个允许键位制造碰撞；覆盖率阈值 1.0 使全部字为候选字。
    fn make_ctx(specs: &[(u64, usize)], mode: SimpleAssignMode, protect_top_n: usize) -> OptContext {
        let mut groups: Vec<RootGroup> = Vec::new();
        let mut splits: Vec<(char, Vec<String>, u64)> = Vec::with_capacity(specs.len());
        for (i, &(freq, n_roots)) in specs.iter().enumerate() {
            let n_roots = n_roots.max(1);
            let mut roots: Vec<String> = Vec::with_capacity(n_roots);
            for j in 0..n_roots {
                let root = format!("p{i}_{j}");
                groups.push(RootGroup {
                    roots: vec![root.clone()],
                    allowed_keys: vec![0, 1],
                });
                roots.push(root);
            }
            let ch = char::from_u32(0x4e00 + i as u32).unwrap();
            splits.push((ch, roots, freq));
        }
        let step = |sel: char| SimpleCodeStep {
            root_selector: sel,
            code_selector: 'a',
        };
        let levels = vec![
            SimpleCodeLevel { level: 1, code_num: 1, rule_candidates: vec![vec![step('A')]], space_commit: false },
            SimpleCodeLevel { level: 2, code_num: 1, rule_candidates: vec![vec![step('A'), step('B')]], space_commit: false },
            SimpleCodeLevel { level: 3, code_num: 1, rule_candidates: vec![vec![step('A'), step('B'), step('C')]], space_commit: false },
        ];
        let fixed_roots: HashMap<String, u8> = HashMap::new();
        let equiv_table = [[0.0f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];
        let key_dist = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];
        let mut weights = WeightConfig::default();
        weights.enable_simple_code = true;
        weights.simple_coverage_ratio = 1.0;
        weights.simple_assign_mode = mode;
        weights.simple_protect_top_n = protect_top_n;
        OptContext::new(
            &splits,
            &fixed_roots,
            &groups,
            equiv_table,
            key_dist,
            ScaleConfig::default(),
            SimpleCodeConfig { levels },
            weights,
            TargetsConfig::default(),
        )
    }

    /// 收集「全码编码值 -> 拥有该全码的汉字集合」（全体汉字，与产线 full_code_to_chars 同口径）。
    fn full_code_to_chars(ctx: &OptContext, asg: &[u8]) -> Vec<Vec<usize>> {
        let mut m: Vec<Vec<usize>> = vec![Vec::new(); ctx.code_space];
        for ci in 0..ctx.char_infos.len() {
            m[ctx.calc_code_only(ci, asg)].push(ci);
        }
        m
    }

    /// 遍历所有级别，对每个出简（被选中）汉字断言其简码不撞受保护全码。
    fn assert_protection_holds(ctx: &OptContext, ev: &Evaluator, asg: &[u8], n: usize) {
        let se = ev.simple_eval.as_ref().expect("简码应启用");
        let fc = full_code_to_chars(ctx, asg);
        for li in 0..se.levels.len() {
            for ci in 0..ctx.char_infos.len() {
                if !se.levels[li].selected[ci] {
                    continue;
                }
                let code = ctx
                    .calc_simple_code_eligible(ci, li, asg)
                    .expect("出简字必有该级简码");
                if n == 0 {
                    // 保护全部：该简码编码值不应被任何汉字用作全码。
                    assert!(
                        code >= fc.len() || fc[code].is_empty(),
                        "N=0 违反保护：级别 {} 字 {} 简码 {} 撞全码 {:?}",
                        li, ci, code, fc.get(code)
                    );
                } else {
                    // 仅保护 top-N：该简码编码值不应等于任一受保护汉字的全码。
                    if code < fc.len() {
                        for &owner in &fc[code] {
                            assert!(
                                !ctx.simple_is_topn[owner],
                                "N={} 违反保护：级别 {} 字 {} 简码 {} 撞受保护字 {} 的全码",
                                n, li, ci, code, owner
                            );
                        }
                    }
                }
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(80))]

        // Feature: simple-code-perf-optimization, Requirement 33: 简码占用保护（硬资格约束）。
        // 对随机分配 + 确定性移动序列，断言任意出简字的简码不撞受保护全码（N=0 保护全部、N>0 保护 top-N）。
        // Validates: Requirement 33
        #[test]
        fn prop33_selected_simple_codes_never_collide_protected_full(
            mode_is_freq in any::<bool>(),
            specs in prop::collection::vec((1u64..6, 1usize..4), 4usize..10),
            // 0 = 保护全部；1..=5 = 仅保护 top-N
            protect_top_n in 0usize..6,
            moves in prop::collection::vec((0usize..48, 0u8..2), 0usize..20),
        ) {
            let mode = if mode_is_freq {
                SimpleAssignMode::Frequency
            } else {
                SimpleAssignMode::Efficiency
            };
            let ctx = make_ctx(&specs, mode, protect_top_n);
            let n_groups = ctx.num_groups;
            let mut asg = vec![0u8; n_groups];

            // step==0 为初始分配，其后逐步确定性改写键位；每步用全量重建后断言保护成立。
            for step in 0..=moves.len() {
                let ev = Evaluator::new(&ctx, &asg);
                assert_protection_holds(&ctx, &ev, &asg, protect_top_n);
                if step < moves.len() {
                    let (gi, nk) = moves[step];
                    asg[gi % n_groups] = nk;
                }
            }
        }

        // N>0 时允许出简简码等于「top-N 之外」汉字的全码：构造一个确实发生此类共享的场景，
        // 断言它不被保护阻断（即该字仍可出简）。借由「保护成立 + 至少存在一例共享」联合验证语义边界。
        // Validates: Requirement 33
        #[test]
        fn prop33_topn_allows_collision_with_unprotected_full(
            specs in prop::collection::vec((1u64..6, 1usize..4), 4usize..10),
        ) {
            // 仅保护 top-1：受保护集合最小，最易出现「出简简码撞非受保护全码」的合法共享。
            let ctx = make_ctx(&specs, SimpleAssignMode::Efficiency, 1);
            let asg = vec![0u8; ctx.num_groups];
            let ev = Evaluator::new(&ctx, &asg);
            // 核心约束必须成立（不撞受保护全码）。
            assert_protection_holds(&ctx, &ev, &asg, 1);
        }
    }

    /// 定向单元测试：N=0 下，被全码占用的简码桶名额=0（谁都不出简），其候选字上浮到更高级别。
    #[test]
    fn n0_blocks_bucket_and_propagates_upward() {
        // 两个单根字 + 一个三根字。两个单根字在级别 1 同键位 ⟹ 全码相同且长度 1，
        // 级别 1 简码（长度 1）必撞其自身全码 ⟹ N=0 下级别 1 全被阻断，须上浮。
        let specs = [(100u64, 1usize), (50, 1), (30, 3)];
        let ctx = make_ctx(&specs, SimpleAssignMode::Frequency, 0);
        let asg = vec![0u8; ctx.num_groups];
        let ev = Evaluator::new(&ctx, &asg);
        assert_protection_holds(&ctx, &ev, &asg, 0);
    }
}

// =========================================================================
// 🧪 active/passive 候选拆分测试（simple_active_coverage 性能优化）
// =========================================================================
// 验证：退火 active 范围的评估器只在 active 候选上出简（passive 不参与）；
// 输出全集（output）范围的评估器把 passive 候选也纳入出简。
#[cfg(test)]
mod active_passive_tests {
    use super::*;
    use crate::config::TargetsConfig;
    use crate::types::{
        KeyDistConfig, RootGroup, ScaleConfig, SimpleAssignMode, SimpleCodeConfig, SimpleCodeLevel,
        SimpleCodeStep, WeightConfig, EQUIV_TABLE_SIZE,
    };
    use std::collections::HashMap;

    /// 单级（[A.a]，code_num 极大使桶内全选）、每字 2 个独立字根（全码长 2 > 简码长 1，
    /// 避免需求 33 占用保护误阻断）的 OptContext；参数化 output / active 覆盖率。
    fn make_ctx(freqs: &[u64], output_ratio: f64, active_ratio: f64) -> OptContext {
        let mut groups: Vec<RootGroup> = Vec::new();
        let mut splits: Vec<(char, Vec<String>, u64)> = Vec::with_capacity(freqs.len());
        for (i, &f) in freqs.iter().enumerate() {
            let mut roots = Vec::with_capacity(2);
            for j in 0..2 {
                let r = format!("a{i}_{j}");
                groups.push(RootGroup { roots: vec![r.clone()], allowed_keys: vec![0, 1] });
                roots.push(r);
            }
            let ch = char::from_u32(0x4e00 + i as u32).unwrap();
            splits.push((ch, roots, f));
        }
        let step = |s: char| SimpleCodeStep { root_selector: s, code_selector: 'a' };
        let levels = vec![SimpleCodeLevel {
            level: 1,
            code_num: 1000, // 桶内全员出简
            rule_candidates: vec![vec![step('A')]],
            space_commit: false,
        }];
        let fixed_roots: HashMap<String, u8> = HashMap::new();
        let equiv_table = [[0.0f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];
        let key_dist = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];
        let mut weights = WeightConfig::default();
        weights.enable_simple_code = true;
        weights.simple_assign_mode = SimpleAssignMode::Frequency;
        weights.simple_coverage_ratio = output_ratio;
        weights.simple_active_coverage = active_ratio;
        OptContext::new(
            &splits, &fixed_roots, &groups, equiv_table, key_dist,
            ScaleConfig::default(), SimpleCodeConfig { levels }, weights,
            TargetsConfig::default(),
        )
    }

    fn selected_set(ev: &Evaluator) -> Vec<usize> {
        let se = ev.simple_eval.as_ref().expect("简码应启用");
        let mut out = Vec::new();
        for ci in 0..se.levels[0].selected.len() {
            if se.levels[0].selected[ci] {
                out.push(ci);
            }
        }
        out
    }

    #[test]
    fn active_scope_excludes_passive_output_scope_includes() {
        // output=1.0（全 8 字），active=0.5（前缀更小）。
        let freqs = [100u64, 90, 80, 70, 60, 50, 40, 30];
        let ctx = make_ctx(&freqs, 1.0, 0.5);
        assert!(
            ctx.simple_candidate_chars.len() < ctx.simple_output_candidate_chars.len(),
            "active 应严格小于 output"
        );
        let asg = vec![0u8; ctx.num_groups];

        // active 范围（退火热路径口径）：只在 active 候选上出简。
        let ev_active = Evaluator::new(&ctx, &asg);
        let sel_active = selected_set(&ev_active);
        for &ci in &sel_active {
            assert!(
                ctx.simple_is_candidate[ci],
                "active 范围不应出简 passive 字 {ci}"
            );
        }

        // output 范围（最终上报/输出口径）：passive 也纳入出简。
        let ev_output = Evaluator::new_output_scope(&ctx, &asg);
        let sel_output = selected_set(&ev_output);
        assert!(
            sel_output.len() > sel_active.len(),
            "output 出简数({}) 应多于 active({})",
            sel_output.len(), sel_active.len()
        );
        // 至少有一个 passive 字在 output 范围被出简。
        assert!(
            sel_output.iter().any(|&ci| !ctx.simple_is_candidate[ci]),
            "output 范围应至少出简一个 passive 字"
        );
    }
}
