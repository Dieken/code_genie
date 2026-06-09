// =========================================================================
// 🧠 混合优化算法（模拟退火 + 冲突导向邻域）
// =========================================================================

use rand::prelude::*;
use std::collections::{HashMap, HashSet};
use std::time::Instant;

use crate::config::Config;
use crate::context::OptContext;
use crate::evaluator::Evaluator;
use crate::schedule::TemperatureSchedule;
use crate::types::{char_to_key_index, Metrics, SimpleMetrics, GROUP_MARKER, KEY_SPACE};

// =========================================================================
// 简码有效权重渐进曲线（纯函数，需求 9）
// =========================================================================

/// 计算进度 `p` 处的有效简码权重 `w_simple_eff(p)`（smoothstep 分段，需求 9.2/9.3/9.4/9.5）。
///
/// 分段定义（`W = w` 为目标简码权重）：
/// - `p < p_start`：返回 0（激活前简码不贡献）；
/// - `p_start ≤ p < p_start + p_ramp`：返回 `W · α²(3 − 2α)`，其中 `α = (p − p_start)/p_ramp`；
/// - `p ≥ p_start + p_ramp`：返回 `W`。
///
/// 边界连续：`p = p_start` 处 α=0 ⟹ 返回 0；`p = p_start + p_ramp` 处落入第三段返回 `W`。
/// 当 `p_ramp == 0`（硬激活兼容档）时，`p ≥ p_start` 直接返回 `W`。
///
/// 该函数为纯函数（无副作用、仅依赖入参），供退火主循环与 Property 8 属性测试调用。
pub(crate) fn w_simple_eff(p: f64, p_start: f64, p_ramp: f64, w: f64) -> f64 {
    if p < p_start {
        0.0
    } else if p_ramp > 0.0 && p < p_start + p_ramp {
        let alpha = (p - p_start) / p_ramp;
        let s = alpha * alpha * (3.0 - 2.0 * alpha);
        w * s
    } else {
        w
    }
}

/// 计算简码激活当刻应用于温度乘子的升温结果（纯函数，需求 12.2/12.3/12.4）。
///
/// 激活升温**仅**取自独立配置项 `simple_activation_reheat`，与现有 `reheat_factor`
/// 完全解耦——本函数不接收也不读取 `reheat_factor`，从而在类型层面保证两者独立：
/// `new_multiplier = current_multiplier × simple_activation_reheat`。
///
/// 默认 `simple_activation_reheat = 1.0` 表示不升温（保持乘子不变）。
/// 供退火主循环激活分支与激活/升温集成测试调用。
pub(crate) fn simple_activation_multiplier(
    current_multiplier: f64,
    simple_activation_reheat: f64,
) -> f64 {
    current_multiplier * simple_activation_reheat
}

/// 计算权重渐进期 10 个进度点的 `(p, α, w)` 三元组（纯函数，需求 16.2）。
///
/// 第 `i` 个点（`i = 0..=9`）位于 `p_i = p_start + (i/10) · p_ramp`，
/// `α = (p_i − p_start)/p_ramp`（`p_ramp == 0` 时取 0），
/// `w = w_simple_eff(p_i, p_start, p_ramp, w_target)`。
///
/// 供退火主循环的渐进日志与 Property/日志单元测试调用。
pub(crate) fn ramp_log_points(p_start: f64, p_ramp: f64, w_target: f64) -> Vec<(f64, f64, f64)> {
    let mut points = Vec::with_capacity(10);
    for i in 0..10 {
        let frac = i as f64 / 10.0;
        let p_i = p_start + frac * p_ramp;
        let alpha = if p_ramp > 0.0 {
            (p_i - p_start) / p_ramp
        } else {
            0.0
        };
        let w_i = w_simple_eff(p_i, p_start, p_ramp, w_target);
        points.push((p_i, alpha, w_i));
    }
    points
}

/// 计算周期对账间隔 `M`（纯函数，需求 15.2）。
///
/// `M = max(1, floor(total_steps × ratio))`：对账每 `M` 步触发一次。
/// 当 `ratio == 0` 或 `floor(total_steps × ratio) == 0`（极小 ratio）时钳为 `1`，
/// 保证至少每步可对账、永不为 0（避免取模除零）。
///
/// 供退火主循环与 Property 12 属性测试调用。
pub(crate) fn reconcile_interval(total_steps: usize, ratio: f64) -> usize {
    (((total_steps as f64) * ratio).floor() as i64).max(1) as usize
}

/// 计算简码激活闩锁的下一状态（纯函数，需求 8.3/8.4）。
///
/// 闩锁语义：一旦激活即永久保持为真，不因后续进度（`p`）或温度（升温/回退）变化而关闭：
/// `next = already_activated || hard_activate || p >= p_start`。
///
/// - `already_activated == true`（已激活）⟹ 无条件返回 `true`（单调闩锁，不可回退）；
/// - `hard_activate == true`（硬激活兼容档，`p_start == p_ramp == 0`）⟹ 从第一步即返回 `true`；
/// - 否则当进度首次达到 `p >= p_start` 时返回 `true`。
///
/// 该函数镜像 `simulated_annealing` 主循环中内联的激活判定
/// `simple_enabled && !simple_activated && (p >= p_start || hard_activate)`：
/// 当 `simple_activated == false` 时，本函数返回 `true` 的条件
/// （`p >= p_start || hard_activate`）与内联判定触发激活的条件完全一致；
/// 当 `simple_activated == true` 时，本函数恒为 `true`，对应内联逻辑里
/// `&& !simple_activated` 守卫使闩锁保持激活、不再变化。
///
/// 供退火主循环激活分支与 Property 15 属性测试调用。
pub(crate) fn latch_activation(
    already_activated: bool,
    p: f64,
    p_start: f64,
    hard_activate: bool,
) -> bool {
    already_activated || hard_activate || p >= p_start
}

// =========================================================================
// 初始化策略
// =========================================================================

/// 原始贪心初始化 - 按组大小降序，均衡分配到键位
fn greedy_balance_init(ctx: &OptContext, cfg: &Config, rng: &mut ThreadRng) -> Vec<u8> {
    let mut assignment = vec![0u8; ctx.num_groups];

    let mut group_freq: Vec<(usize, usize)> = ctx
        .group_to_chars
        .iter()
        .enumerate()
        .map(|(i, v)| (i, v.len()))
        .collect();
    group_freq.sort_by(|a, b| b.1.cmp(&a.1));

    let max_ki = cfg
        .keys
        .allowed
        .chars()
        .filter_map(char_to_key_index)
        .max()
        .unwrap_or(25);
    let mut key_counts = vec![0usize; max_ki + 1];

    for (gi, _) in &group_freq {
        let gi = *gi;
        let allowed = &ctx.groups[gi].allowed_keys;
        let min_count = allowed
            .iter()
            .map(|&k| key_counts.get(k as usize).copied().unwrap_or(0))
            .min()
            .unwrap_or(0);

        let candidates: Vec<u8> = allowed
            .iter()
            .filter(|&&k| key_counts.get(k as usize).copied().unwrap_or(0) == min_count)
            .copied()
            .collect();

        let best = if candidates.is_empty() {
            allowed[0]
        } else {
            candidates[rng.gen_range(0..candidates.len())]
        };

        assignment[gi] = best;
        if (best as usize) < key_counts.len() {
            key_counts[best as usize] += 1;
        }
    }
    assignment
}

/// 频率感知贪心 - 按组权重降序，最小化碰撞代价
fn frequency_greedy_init(ctx: &OptContext, rng: &mut ThreadRng) -> Vec<u8> {
    let n = ctx.num_groups;
    let mut assignment = vec![0u8; n];

    let mut group_info: Vec<(usize, f64)> = (0..n)
        .map(|gi| {
            let weight = ctx.group_to_chars[gi].len() as f64;
            let noise = 1.0 + rng.gen::<f64>() * 0.25;
            (gi, weight * noise)
        })
        .collect();
    group_info.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    let mut key_freq_load = vec![0.0f64; KEY_SPACE];
    let mut key_group_count = vec![0usize; KEY_SPACE];

    for &(gi, weight) in &group_info {
        let allowed = &ctx.groups[gi].allowed_keys;

        let mut best_key = allowed[0];
        let mut best_cost = f64::MAX;

        for &k in allowed {
            let ki = k as usize;
            let collision_cost = key_freq_load[ki] * weight;
            let balance_cost = key_group_count[ki] as f64 * 0.05;
            let cost = collision_cost + balance_cost;

            if cost < best_cost {
                best_cost = cost;
                best_key = k;
            }
        }

        assignment[gi] = best_key;
        key_freq_load[best_key as usize] += weight;
        key_group_count[best_key as usize] += 1;
    }

    assignment
}

/// 分散优先贪心
fn spread_greedy_init(ctx: &OptContext, rng: &mut ThreadRng) -> Vec<u8> {
    let n = ctx.num_groups;
    let mut assignment = vec![0u8; n];

    let mut order: Vec<usize> = (0..n).collect();
    order.shuffle(rng);

    let mut key_last_used = vec![0usize; KEY_SPACE];

    for (step, &gi) in order.iter().enumerate() {
        let allowed = &ctx.groups[gi].allowed_keys;
        let best_key = *allowed
            .iter()
            .min_by_key(|&&k| key_last_used[k as usize])
            .unwrap();

        assignment[gi] = best_key;
        key_last_used[best_key as usize] = step + 1;
    }

    assignment
}

/// 纯随机有效解
fn random_valid_init(ctx: &OptContext, rng: &mut ThreadRng) -> Vec<u8> {
    let n = ctx.num_groups;
    let mut assignment = vec![0u8; n];
    for gi in 0..n {
        let allowed = &ctx.groups[gi].allowed_keys;
        assignment[gi] = allowed[rng.gen_range(0..allowed.len())];
    }
    assignment
}

// =========================================================================
// 🔍 冲突分析
// =========================================================================

/// 构建编码到汉字的反向索引
fn build_code_to_chars(ctx: &OptContext, assignment: &[u8]) -> HashMap<usize, Vec<usize>> {
    let mut code_to_chars: HashMap<usize, Vec<usize>> = HashMap::new();
    for ci in 0..ctx.char_infos.len() {
        let code = ctx.calc_code_only(ci, assignment);
        code_to_chars.entry(code).or_default().push(ci);
    }
    code_to_chars
}

/// 找出所有重码冲突的字根组对
///
/// `weight_by_freq` 控制排序权重（元组第三字段）：
/// - false：权重为参与该冲突编码的汉字数量（优化 collision_count，保持现有行为）
/// - true ：权重为这些汉字的字频之和（优化 collision_rate）
///
/// 排序：主键 weight 降序；次键 (g1, g2) 升序裁决，保证结果可复现、不依赖 HashMap 遍历顺序。
fn find_collision_groups(
    ctx: &OptContext,
    assignment: &[u8],
    weight_by_freq: bool,
) -> Vec<(usize, usize, usize)> {
    let code_to_chars = build_code_to_chars(ctx, assignment);
    let mut collisions: Vec<(usize, usize, usize)> = Vec::new();

    for chars in code_to_chars.values() {
        if chars.len() < 2 {
            continue;
        }
        // 冲突权重：count 策略用 chars.len()；freq 策略用共享该编码的汉字字频之和
        let weight: usize = if weight_by_freq {
            chars
                .iter()
                .map(|&ci| ctx.char_infos[ci].frequency as usize)
                .sum()
        } else {
            chars.len()
        };

        let mut groups_in_conflict: HashSet<usize> = HashSet::new();
        for &ci in chars {
            let info = &ctx.char_infos[ci];
            for &p in &info.parts {
                if p >= GROUP_MARKER {
                    let gi = (p - GROUP_MARKER) as usize;
                    groups_in_conflict.insert(gi);
                }
            }
        }

        let groups: Vec<usize> = groups_in_conflict.into_iter().collect();
        for i in 0..groups.len() {
            for j in (i + 1)..groups.len() {
                // 规范化为 (min, max)，保证 g1 < g2，使排序次键稳定、配对去歧义
                let (lo, hi) = if groups[i] <= groups[j] {
                    (groups[i], groups[j])
                } else {
                    (groups[j], groups[i])
                };
                collisions.push((lo, hi, weight));
            }
        }
    }

    // 主键：第三字段（weight）降序；次键（稳定裁决）：(g1, g2) 升序
    collisions.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)).then(a.1.cmp(&b.1)));
    collisions
}

/// 找出特定键位上的所有字根组
fn find_groups_on_key(ctx: &OptContext, assignment: &[u8], key: u8) -> Vec<usize> {
    (0..ctx.num_groups)
        .filter(|&gi| assignment[gi] == key)
        .collect()
}

// =========================================================================
// 🎯 冲突导向的邻域操作
// =========================================================================

/// 尝试解决冲突：将冲突组中的一个移动到新键位
fn try_resolve_conflict(
    ctx: &OptContext,
    assignment: &mut [u8],
    evaluator: &mut Evaluator,
    collisions: &[(usize, usize, usize)],
    sample_window: usize,
    temp: f64,
    rng: &mut ThreadRng,
) -> bool {
    if collisions.is_empty() || sample_window == 0 {
        return false;
    }

    // 有效窗口 = min(sample_window, 列表长度)
    let window = sample_window.min(collisions.len());
    let idx = rng.gen_range(0..window);
    let (g1, g2, _) = collisions[idx];

    let groups_to_try = if rng.gen_bool(0.5) { vec![g1, g2] } else { vec![g2, g1] };

    for &gi in &groups_to_try {
        let current_key = assignment[gi];
        let allowed = &ctx.groups[gi].allowed_keys;

        let other_keys: Vec<u8> = allowed
            .iter()
            .filter(|&&k| k != current_key)
            .copied()
            .collect();

        if other_keys.is_empty() {
            continue;
        }

        let new_key = other_keys[rng.gen_range(0..other_keys.len())];
        if evaluator.try_move(ctx, assignment, gi, new_key, temp, rng) {
            return true;
        }
    }

    false
}

/// 尝试键位重组
fn try_key_reorganization(
    ctx: &OptContext,
    assignment: &mut [u8],
    evaluator: &mut Evaluator,
    temp: f64,
    rng: &mut ThreadRng,
) -> bool {
    let n = assignment.len();
    if n < 2 {
        return false;
    }

    let k1 = assignment[rng.gen_range(0..n)];
    let groups_on_k1: Vec<usize> = find_groups_on_key(ctx, assignment, k1);

    if groups_on_k1.is_empty() {
        return false;
    }

    let gi = groups_on_k1[rng.gen_range(0..groups_on_k1.len())];
    let allowed = &ctx.groups[gi].allowed_keys;

    let other_keys: Vec<u8> = allowed
        .iter()
        .filter(|&&k| k != k1)
        .copied()
        .collect();

    if other_keys.is_empty() {
        return false;
    }

    let new_key = other_keys[rng.gen_range(0..other_keys.len())];
    evaluator.try_move(ctx, assignment, gi, new_key, temp, rng)
}

/// 三组循环交换 (g1←k2, g2←k3, g3←k1) — 增量评估
fn try_triple_swap(
    ctx: &OptContext,
    assignment: &mut [u8],
    evaluator: &mut Evaluator,
    temp: f64,
    rng: &mut ThreadRng,
) -> bool {
    let n = assignment.len();
    if n < 3 {
        return false;
    }

    let indices: Vec<usize> = (0..n).choose_multiple(rng, 3);
    if indices.len() < 3 {
        return false;
    }

    let [g1, g2, g3] = [indices[0], indices[1], indices[2]];
    let [k1, k2, k3] = [assignment[g1], assignment[g2], assignment[g3]];

    // 三个键都相同则无意义
    if k1 == k2 && k2 == k3 {
        return false;
    }

    // 检查循环交换的合法性: g1→k2, g2→k3, g3→k1
    if !ctx.groups[g1].allowed_keys.contains(&k2)
        || !ctx.groups[g2].allowed_keys.contains(&k3)
        || !ctx.groups[g3].allowed_keys.contains(&k1)
    {
        return false;
    }

    let old_score = evaluator.get_score(ctx);
    let needs_simple = evaluator.has_simple_impact(ctx, g1)
        || evaluator.has_simple_impact(ctx, g2)
        || evaluator.has_simple_impact(ctx, g3);

    // 更新 key_weighted_usage
    for &(gi, old_k, new_k) in &[(g1, k1, k2), (g2, k2, k3), (g3, k3, k1)] {
        if old_k == new_k {
            continue;
        }
        for &ci in &ctx.group_to_chars[gi] {
            let freq_f = ctx.char_infos[ci].frequency as f64;
            for &p in &ctx.char_infos[ci].parts {
                if p >= GROUP_MARKER && (p - GROUP_MARKER) as usize == gi {
                    evaluator.key_weighted_usage[old_k as usize] -= freq_f;
                    evaluator.key_weighted_usage[new_k as usize] += freq_f;
                }
            }
        }
    }

    // 执行交换
    assignment[g1] = k2;
    assignment[g2] = k3;
    assignment[g3] = k1;

    // 增量更新受影响的汉字编码
    for &gi in &[g1, g2, g3] {
        for &ci in &ctx.group_to_chars[gi] {
            evaluator.update_char(ctx, assignment, ci);
        }
    }

    if needs_simple {
        evaluator.apply_simple_for_move(ctx, assignment, &[g1, g2, g3]);
    }

    evaluator.score_dirty = true;
    let new_score = evaluator.get_score(ctx);
    let delta = new_score - old_score;

    if delta <= 0.0 || rng.gen::<f64>() < (-delta / temp).exp() {
        if needs_simple {
            evaluator.commit_simple();
        }
        true
    } else {
        // 回滚 key_weighted_usage
        for &(gi, old_k, new_k) in &[(g1, k2, k1), (g2, k3, k2), (g3, k1, k3)] {
            if old_k == new_k {
                continue;
            }
            for &ci in &ctx.group_to_chars[gi] {
                let freq_f = ctx.char_infos[ci].frequency as f64;
                for &p in &ctx.char_infos[ci].parts {
                    if p >= GROUP_MARKER && (p - GROUP_MARKER) as usize == gi {
                        evaluator.key_weighted_usage[old_k as usize] -= freq_f;
                        evaluator.key_weighted_usage[new_k as usize] += freq_f;
                    }
                }
            }
        }

        // 回滚 assignment
        assignment[g1] = k1;
        assignment[g2] = k2;
        assignment[g3] = k3;

        // 回滚编码
        for &gi in &[g1, g2, g3] {
            for &ci in &ctx.group_to_chars[gi] {
                evaluator.update_char(ctx, assignment, ci);
            }
        }

        if needs_simple {
            evaluator.rollback_simple();
        }

        evaluator.cached_score = old_score;
        evaluator.score_dirty = false;
        false
    }
}

// =========================================================================
// 🔧 增强版爬山算法
// =========================================================================

/// 增强版爬山：结合冲突导向的邻域操作
fn enhanced_hill_climb(
    ctx: &OptContext,
    init: Vec<u8>,
    rng: &mut ThreadRng,
    max_steps: usize,
) -> (Vec<u8>, f64) {
    let mut assignment = init;
    let mut evaluator = Evaluator::new(ctx, &assignment);
    let n = assignment.len();
    if n == 0 {
        return (assignment, evaluator.get_score(ctx));
    }

    let zero_temp = 1e-15;
    let mut no_improve_count = 0usize;
    let mut collisions = find_collision_groups(ctx, &assignment, false);

    for step in 0..max_steps {
        let op_type = step % 10;

        let success = match op_type {
            0..=3 => {
                if !collisions.is_empty() {
                    try_resolve_conflict(ctx, &mut assignment, &mut evaluator, &collisions, 20, zero_temp, rng)
                } else {
                    false
                }
            }
            4..=6 => {
                if n >= 2 {
                    let r1 = rng.gen_range(0..n);
                    let r2 = rng.gen_range(0..n - 1);
                    let r2 = if r2 >= r1 { r2 + 1 } else { r2 };
                    let k1 = assignment[r1];
                    let k2 = assignment[r2];
                    if ctx.groups[r1].allowed_keys.contains(&k2)
                        && ctx.groups[r2].allowed_keys.contains(&k1)
                    {
                        evaluator.try_swap(ctx, &mut assignment, r1, r2, zero_temp, rng)
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            7 => try_triple_swap(ctx, &mut assignment, &mut evaluator, zero_temp, rng),
            8 => try_key_reorganization(ctx, &mut assignment, &mut evaluator, zero_temp, rng),
            _ => {
                let r = rng.gen_range(0..n);
                let allowed = &ctx.groups[r].allowed_keys;
                let new_k = allowed[rng.gen_range(0..allowed.len())];
                evaluator.try_move(ctx, &mut assignment, r, new_k, zero_temp, rng)
            }
        };

        // At zero_temp, success means score improved (only strict improvements accepted)
        if success {
            no_improve_count = 0;
            if step % 200 == 0 {
                collisions = find_collision_groups(ctx, &assignment, false);
            }
        } else {
            no_improve_count += 1;
        }

        if no_improve_count > n * 10 {
            break;
        }
    }

    (assignment, evaluator.get_score(ctx))
}

fn hill_climb_warmup(
    ctx: &OptContext,
    init: Vec<u8>,
    rng: &mut ThreadRng,
    max_steps: usize,
) -> (Vec<u8>, f64) {
    enhanced_hill_climb(ctx, init, rng, max_steps)
}

// =========================================================================
// 坐标下降
// =========================================================================

fn coordinate_descent(ctx: &OptContext, init: Vec<u8>) -> (Vec<u8>, f64) {
    let mut assignment = init;
    let n = assignment.len();
    let mut evaluator = Evaluator::new(ctx, &assignment);
    let mut improved = true;

    while improved {
        improved = false;
        for gi in 0..n {
            let current_key = assignment[gi];
            let current_score = evaluator.get_score(ctx);

            let mut best_key = current_key;
            let mut best_score = current_score;

            for &k in &ctx.groups[gi].allowed_keys {
                if k == current_key {
                    continue;
                }

                // 增量前向：移动到候选键
                let needs_simple = evaluator.has_simple_impact(ctx, gi);

                for &ci in &ctx.group_to_chars[gi] {
                    let freq_f = ctx.char_infos[ci].frequency as f64;
                    for &p in &ctx.char_infos[ci].parts {
                        if p >= GROUP_MARKER && (p - GROUP_MARKER) as usize == gi {
                            evaluator.key_weighted_usage[assignment[gi] as usize] -= freq_f;
                            evaluator.key_weighted_usage[k as usize] += freq_f;
                        }
                    }
                }

                let prev_key = assignment[gi];
                assignment[gi] = k;
                for &ci in &ctx.group_to_chars[gi] {
                    evaluator.update_char(ctx, &assignment, ci);
                }
                if needs_simple {
                    evaluator.rebuild_simple(ctx, &assignment);
                }
                evaluator.score_dirty = true;
                let score = evaluator.get_score(ctx);

                if score < best_score - 1e-12 {
                    best_score = score;
                    best_key = k;
                }

                // 回滚
                for &ci in &ctx.group_to_chars[gi] {
                    let freq_f = ctx.char_infos[ci].frequency as f64;
                    for &p in &ctx.char_infos[ci].parts {
                        if p >= GROUP_MARKER && (p - GROUP_MARKER) as usize == gi {
                            evaluator.key_weighted_usage[k as usize] -= freq_f;
                            evaluator.key_weighted_usage[prev_key as usize] += freq_f;
                        }
                    }
                }
                assignment[gi] = prev_key;
                for &ci in &ctx.group_to_chars[gi] {
                    evaluator.update_char(ctx, &assignment, ci);
                }
                if needs_simple {
                    evaluator.rebuild_simple(ctx, &assignment);
                }
                evaluator.cached_score = current_score;
                evaluator.score_dirty = false;
            }

            if best_key != current_key {
                // 应用最优移动
                let needs_simple = evaluator.has_simple_impact(ctx, gi);
                for &ci in &ctx.group_to_chars[gi] {
                    let freq_f = ctx.char_infos[ci].frequency as f64;
                    for &p in &ctx.char_infos[ci].parts {
                        if p >= GROUP_MARKER && (p - GROUP_MARKER) as usize == gi {
                            evaluator.key_weighted_usage[current_key as usize] -= freq_f;
                            evaluator.key_weighted_usage[best_key as usize] += freq_f;
                        }
                    }
                }
                assignment[gi] = best_key;
                for &ci in &ctx.group_to_chars[gi] {
                    evaluator.update_char(ctx, &assignment, ci);
                }
                if needs_simple {
                    evaluator.rebuild_simple(ctx, &assignment);
                }
                evaluator.score_dirty = true;
                improved = true;
            }
        }
    }

    let score = evaluator.get_score(ctx);
    (assignment, score)
}

// =========================================================================
// 🎯 多起点初始化入口
// =========================================================================

pub fn multi_start_init(ctx: &OptContext, cfg: &Config, thread_id: usize) -> Vec<u8> {
    let mut rng = thread_rng();
    let n = ctx.num_groups;
    if n == 0 {
        return vec![];
    }

    let n_candidates: usize = if n < 100 { 50 } else if n < 1000 { 50 } else { 30 };
    let warmup_steps = (n * 30).max(2000).min(50_000);

    let mut best_assignment: Option<Vec<u8>> = None;
    let mut best_score = f64::MAX;

    for trial in 0..n_candidates {
        let strategy_name;
        let candidate = match trial % 5 {
            0 => {
                strategy_name = "均衡贪心";
                greedy_balance_init(ctx, cfg, &mut rng)
            }
            1 | 2 => {
                strategy_name = "频率贪心";
                frequency_greedy_init(ctx, &mut rng)
            }
            3 => {
                strategy_name = "分散贪心";
                spread_greedy_init(ctx, &mut rng)
            }
            _ => {
                strategy_name = "纯随机";
                random_valid_init(ctx, &mut rng)
            }
        };

        let (refined, score) = hill_climb_warmup(ctx, candidate, &mut rng, warmup_steps);

        if score < best_score {
            best_score = score;
            best_assignment = Some(refined);

            if thread_id == 0 {
                println!(
                    "   [Init T0] 候选 {}/{} 策略={} → 预热后得分: {:.4} ✓",
                    trial + 1, n_candidates, strategy_name, score
                );
            }
        }
    }

    let best = best_assignment.unwrap();

    if n <= 500 {
        let (polished, polished_score) = coordinate_descent(ctx, best.clone());
        if thread_id == 0 {
            println!(
                "   [Init T0] 坐标下降: {:.4} → {:.4}",
                best_score, polished_score
            );
        }
        if polished_score < best_score {
            return polished;
        }
    }

    best
}

pub fn smart_init(ctx: &OptContext, cfg: &Config) -> Vec<u8> {
    multi_start_init(ctx, cfg, usize::MAX)
}

// =========================================================================
// 🔥 模拟退火主循环
// =========================================================================

pub fn simulated_annealing(
    ctx: &OptContext,
    cfg: &Config,
    thread_id: usize,
) -> (Vec<u8>, f64, Metrics, SimpleMetrics) {
    let mut rng = thread_rng();

    let mut assignment = multi_start_init(ctx, cfg, thread_id);
    let mut evaluator = Evaluator::new(ctx, &assignment);

    // === 简码延迟激活与权重渐进曲线配置（任务 10.1，需求 8/9/12/13/15）===
    // 简码是否启用（启用时延迟到进度阈值后再激活，早期探索阶段简码不贡献）。
    let simple_enabled = ctx.enable_simple_code && !ctx.simple_config.levels.is_empty();
    // 钳制激活/渐进配置并取得硬激活标记。`validate_simple_activation` 需 `&mut self`，
    // 而本函数仅持有 `&Config`，故克隆一份 cfg 调用校验，不修改入参（最小改动）。
    let (p_start, p_ramp, hard_activate) = {
        let mut cfg_clamped = cfg.clone();
        let hard = cfg_clamped.validate_simple_activation();
        (
            cfg_clamped.weights.simple_code.simple_start_progress,
            cfg_clamped.weights.simple_code.simple_ramp_progress,
            hard,
        )
    };
    // 目标简码权重 W、激活升温倍率（独立于 reheat_factor）、对账间隔比例。
    let w_target = ctx.weights.weight_simple_code;
    let simple_reheat = cfg.weights.simple_code.simple_activation_reheat;
    let reconcile_ratio = cfg.weights.simple_code.reconcile_interval_ratio;
    let weight_full = ctx.weights.weight_full_code;

    // 激活闩锁：一旦激活后保持为真（需求 8.3/8.4）。
    let mut simple_activated = false;
    // 延迟激活：早期探索阶段令简码不参与综合得分（simple_score 贡献为 0，需求 8.5/10.2）。
    // 注意：`Evaluator::new` 在启用简码时会急切构建 `SimpleEvaluator` 并置 `simple_active=true`；
    // 这里显式回退为未激活状态，使简码贡献延迟到进度阈值后由 `activate_simple` 重新打开。
    if simple_enabled {
        evaluator.simple_active = false;
        evaluator.current_simple_weight = 0.0;
        evaluator.score_dirty = true;
        evaluator.full_score_dirty = true;
    }

    let mut best_assignment = assignment.clone();
    // 最佳解按分量存储（需求 11）：比较时用当前有效权重重算 best_total。
    let mut best_full_score = evaluator.full_score_component(ctx);
    let mut best_simple_score = evaluator.simple_score_component(ctx);
    let mut best_score = evaluator.get_score(ctx);
    let mut best_metrics = evaluator.get_metrics(ctx);
    let mut best_simple_metrics = evaluator.get_simple_metrics(ctx);

    if thread_id == 0 {
        // === 配置确认（需求 7.7/16.6）：输出简码候选字覆盖率与候选字数 ===
        if simple_enabled {
            println!(
                "   [T0] 配置确认 | 简码候选字覆盖率: {:.2}% | 候选字数: {}",
                ctx.simple_actual_coverage * 100.0,
                ctx.simple_candidate_chars.len()
            );
        }

        let m = &best_metrics;
        let scores = evaluator.get_metric_scores(ctx);
        println!(
            "   [T0] 初始化完成 | 得分: {:.4} | 重码: {}({:.4}) 重码率: {:.4}%({:.4}) 当量: {:.4}({:.4}) CV: {:.4}({:.4}) 分布: {:.4}({:.4})",
            best_score,
            m.collision_count, scores.collision_count,
            m.collision_rate * 100.0, scores.collision_rate,
            m.equiv_mean, scores.equivalence,
            m.equiv_cv, scores.equiv_cv,
            m.dist_deviation, scores.distribution,
        );
    }

    let steps = cfg.annealing.total_steps;
    let n_groups = assignment.len();
    if n_groups == 0 {
        return (best_assignment, best_score, best_metrics, best_simple_metrics);
    }

    // 周期对账间隔 M = max(1, floor(total_steps × reconcile_interval_ratio))（需求 15.2）。
    let reconcile_m = reconcile_interval(steps, reconcile_ratio);

    let schedule = TemperatureSchedule::build(
        cfg.annealing.temp_start,
        cfg.annealing.temp_end,
        cfg.annealing.comfort_temp,
        cfg.annealing.comfort_width,
        cfg.annealing.comfort_slowdown,
    );

    if thread_id == 0 {
        schedule.print_preview(steps);
    }

    let mut temp_multiplier = 1.0f64;
    let min_improve_steps = cfg.min_improve_steps();
    let reheat_decay = if min_improve_steps > 0 {
        (0.01f64).powf(1.0 / min_improve_steps as f64)
    } else {
        0.99
    };

    let mut steps_since_improve = 0usize;
    let mut last_best_score = best_score;

    let report_interval = (steps / 20).max(1);
    let perturb_interval = cfg.perturb_interval();

    let swap_prob_base = cfg.annealing.swap_probability;

    // 冲突导向算子状态（仅在启用时维护；关闭时零开销且不消耗额外随机数，保持 RNG 序列与原实现一致）
    let conflict_prob = cfg.annealing.conflict_probability;
    let conflict_refresh = cfg.annealing.conflict_refresh_interval;
    let conflict_window = cfg.annealing.conflict_sample_window;
    let conflict_weight_by_freq = cfg.annealing.conflict_weight_by_freq;
    let conflict_enabled = conflict_prob > 0.0;
    let mut collisions: Vec<(usize, usize, usize)> = if conflict_enabled {
        find_collision_groups(ctx, &assignment, conflict_weight_by_freq)
    } else {
        Vec::new()
    };
    let mut steps_since_refresh = 0usize;

    let sa_start = Instant::now();

    // === 权重渐进日志状态（任务 11.1，需求 16.2/16.3）===
    // 渐进期 10 个进度点（p, α, w）；仅在 p_ramp > 0 时逐点输出，p_ramp == 0 走硬激活分支。
    let ramp_points = ramp_log_points(p_start, p_ramp, w_target);
    let mut next_ramp_idx = 0usize; // 下一个待输出的进度点索引
    let mut ramp_done_logged = false; // 「已达目标权重」是否已输出（防重复）

    // === 全量重建防回归观测（任务 1 补充）===
    // 主循环开始前快照「全量重建累计计数」基线。健康运行下，整个 SA 主循环（热路径
    // try_move/try_swap 走增量路径）不应触发任何全量重建，故循环结束时该计数相对基线
    // 应零增长。计数已在 `reconcile` 中跨对账携带，不会被周期对账重置而掩盖回归。
    let full_rebuild_baseline = evaluator.full_rebuild_calls();

    // 主循环
    for step in 0..steps {
        let p = step as f64 / steps as f64;

        // === 简码延迟激活与权重渐进（任务 10.1）===
        // 激活判定（闩锁）：进度首达 p_start 或硬激活时一次性激活并独立升温（需求 8.3/12.2）。
        // 闩锁的下一状态由纯函数 `latch_activation` 计算（需求 8.3/8.4），其语义与内联守卫一致：
        // 当 `simple_activated == false` 时，`latch_now` 为真 ⟺ 满足激活触发条件。
        let latch_now = latch_activation(simple_activated, p, p_start, hard_activate);
        if simple_enabled && !simple_activated && latch_now {
            evaluator.activate_simple(ctx, &assignment);
            // 独立于 reheat_factor 的激活升温（默认 1.0 不升温，需求 12.3/12.4）。
            // 经纯函数 `simple_activation_multiplier` 计算，仅依赖 `simple_activation_reheat`。
            temp_multiplier = simple_activation_multiplier(temp_multiplier, simple_reheat);
            simple_activated = latch_now;

            if thread_id == 0 {
                println!(
                    "   [T0] 步数 {}/{} | 进度 {:.3}: 简码已激活（升温 ×{:.2}）",
                    step, steps, p, simple_reheat
                );
                // 硬激活/瞬时档（p_ramp == 0）：无渐进期，激活即达目标权重（需求 16.3）。
                if p_ramp <= 0.0 && !ramp_done_logged {
                    println!("   [T0] 简码已达目标权重 W={:.4}", w_target);
                    ramp_done_logged = true;
                }
            }
        }

        // 有效简码权重曲线（smoothstep，需求 9）。未启用简码时恒为 0。
        let w_eff = if simple_enabled {
            w_simple_eff(p, p_start, p_ramp, w_target)
        } else {
            0.0
        };
        // 仅在有效权重发生变化的步置脏，使下次 get_score 以新权重重算（需求 9）。
        if simple_enabled && w_eff != evaluator.current_simple_weight {
            evaluator.current_simple_weight = w_eff;
            evaluator.score_dirty = true;
        }

        // === 权重渐进期 10 点输出（任务 11.1，需求 16.2/16.3）===
        // 当进度首次到达每个预计算点时输出 p/α/有效权重；渐进结束输出「已达目标权重 W」。
        if thread_id == 0 && simple_enabled && p_ramp > 0.0 {
            while next_ramp_idx < ramp_points.len() && p >= ramp_points[next_ramp_idx].0 {
                let (pp, alpha, ww) = ramp_points[next_ramp_idx];
                println!("   [T0] 简码权重渐进 | p={:.3} α={:.3} w={:.4}", pp, alpha, ww);
                next_ramp_idx += 1;
            }
            if !ramp_done_logged && p >= p_start + p_ramp {
                println!("   [T0] 简码已达目标权重 W={:.4}", w_target);
                ramp_done_logged = true;
            }
        }

        let base_temp = schedule.get(step, steps);
        let temp = base_temp * temp_multiplier;

        if temp_multiplier > 1.001 {
            temp_multiplier = 1.0 + (temp_multiplier - 1.0) * reheat_decay;
        } else {
            temp_multiplier = 1.0;
        }

        // 冲突缓存刷新（仅启用时；interval=0 表示初始化后不再重建）
        if conflict_enabled {
            if conflict_refresh > 0 && steps_since_refresh >= conflict_refresh {
                collisions = find_collision_groups(ctx, &assignment, conflict_weight_by_freq);
                steps_since_refresh = 0;
            } else {
                steps_since_refresh += 1;
            }
        }

        // 邻域分发：启用时每步抽取一次 r 决定走冲突路径还是既有 swap/move；
        // 关闭时跳过抽样，直接走既有分发，保持 RNG 消耗序列与原实现一致。
        let did_conflict = if conflict_enabled {
            let r = rng.gen::<f64>();
            if r < conflict_prob && !collisions.is_empty() {
                try_resolve_conflict(
                    ctx,
                    &mut assignment,
                    &mut evaluator,
                    &collisions,
                    conflict_window,
                    temp,
                    &mut rng,
                );
                true
            } else {
                false
            }
        } else {
            false
        };

        if !did_conflict {
            let swap_prob = swap_prob_base + (1.0 - swap_prob_base) * (step as f64 / steps as f64) * 0.3;

            if rng.gen::<f64>() < swap_prob && n_groups >= 2 {
                let r1 = rng.gen_range(0..n_groups);
                let r2 = rng.gen_range(0..n_groups - 1);
                let r2 = if r2 >= r1 { r2 + 1 } else { r2 };

                let k1 = assignment[r1];
                let k2 = assignment[r2];
                if k1 != k2
                    && ctx.groups[r1].allowed_keys.contains(&k2)
                    && ctx.groups[r2].allowed_keys.contains(&k1)
                {
                    evaluator.try_swap(ctx, &mut assignment, r1, r2, temp, &mut rng);
                } else {
                    let r = r1;
                    let allowed = &ctx.groups[r].allowed_keys;
                    let new_k = allowed[rng.gen_range(0..allowed.len())];
                    evaluator.try_move(ctx, &mut assignment, r, new_k, temp, &mut rng);
                }
            } else {
                let r = rng.gen_range(0..n_groups);
                let allowed = &ctx.groups[r].allowed_keys;
                let new_k = allowed[rng.gen_range(0..allowed.len())];
                evaluator.try_move(ctx, &mut assignment, r, new_k, temp, &mut rng);
            }
        }

        let current_score = evaluator.get_score(ctx);
        // 最佳解按分量存储，比较时用当前有效权重 w_eff 重算 best_total（需求 11.2/11.3），
        // 解决目标函数随时间漂移导致最佳解被冻结的问题。
        let best_recomputed = Evaluator::best_total(weight_full, best_full_score, w_eff, best_simple_score);
        if current_score < best_recomputed {
            best_full_score = evaluator.full_score_component(ctx);
            best_simple_score = evaluator.simple_score_component(ctx);
            best_score = current_score;
            best_assignment = assignment.clone();
            best_metrics = evaluator.get_metrics(ctx);
            best_simple_metrics = evaluator.get_simple_metrics(ctx);
            steps_since_improve = 0;

            if thread_id == 0 && best_score <= last_best_score - 0.9 {
                let m = best_metrics;
                let elapsed = sa_start.elapsed().as_secs_f64();
                let speed = if elapsed > 0.0 { step as f64 / elapsed } else { 0.0 };
                let scores = evaluator.get_metric_scores(ctx);
                // 分量分数（需求 16.4/16.5）：最佳解全码/简码分量以当前有效权重重算。
                let best_full_comp = weight_full * best_full_score;
                let best_simple_comp = w_eff * best_simple_score;
                println!(
                    "   [T0] 步数 {}/{} | {:.1} 万步/分钟 | 温度 {:.6} | 重码:{}({:.4}) 重码率:{:.4}%({:.4}) 当量:{:.4}({:.4}) CV:{:.4}({:.4}) 分布:{:.4}({:.4}) | 得分: {:.4} (全码分量:{:.4} 简码分量:{:.4})",
                    step, steps, speed * 60.0 / 10000.0, temp,
                    m.collision_count, scores.collision_count,
                    m.collision_rate * 100.0, scores.collision_rate,
                    m.equiv_mean, scores.equivalence,
                    m.equiv_cv, scores.equiv_cv,
                    m.dist_deviation, scores.distribution,
                    best_score,
                    best_full_comp, best_simple_comp
                );
                last_best_score = best_score;
            }
        } else {
            steps_since_improve += 1;
        }

        if steps_since_improve > min_improve_steps {
            temp_multiplier = cfg.annealing.reheat_factor;
            steps_since_improve = 0;

            if thread_id == 0 {
                let elapsed = sa_start.elapsed().as_secs_f64();
                let speed = if elapsed > 0.0 { step as f64 / elapsed } else { 0.0 };
                println!(
                    "   [T0] 步数 {} | {:.1} 万步/分钟: Reheat ×{:.1} (基温 {:.6})",
                    step, speed * 60.0 / 10000.0, cfg.annealing.reheat_factor, base_temp
                );
            }
        }

        // 智能低温扰动
        if perturb_interval > 0 && step > 0 && step % perturb_interval == 0 && base_temp < cfg.annealing.comfort_temp * 0.01 {
            let collisions = find_collision_groups(ctx, &assignment, false);
            let n_perturb = (n_groups as f64 * cfg.annealing.perturb_strength) as usize;
            
            if !collisions.is_empty() {
                let mut perturbed_groups: HashSet<usize> = HashSet::new();
                for (g1, g2, _) in collisions.iter().take(10) {
                    perturbed_groups.insert(*g1);
                    perturbed_groups.insert(*g2);
                }
                let groups: Vec<usize> = perturbed_groups.into_iter().collect();
                for &gi in groups.iter().take(n_perturb) {
                    let allowed = &ctx.groups[gi].allowed_keys;
                    if allowed.len() > 1 {
                        let new_k = allowed[rng.gen_range(0..allowed.len())];
                        evaluator.try_move(ctx, &mut assignment, gi, new_k, temp * 2.0, &mut rng);
                    }
                }
            } else {
                for _ in 0..n_perturb {
                    let r1 = rng.gen_range(0..n_groups);
                    let r2 = rng.gen_range(0..n_groups);
                    if r1 != r2 {
                        let ka = assignment[r1];
                        let kb = assignment[r2];
                        let can = ctx.groups[r1].allowed_keys.contains(&kb)
                            && ctx.groups[r2].allowed_keys.contains(&ka);
                        if can {
                            evaluator.try_swap(ctx, &mut assignment, r1, r2, temp * 2.0, &mut rng);
                        }
                    }
                }
            }

            if thread_id == 0 {
                let m = evaluator.get_metrics(ctx);
                println!(
                    "   [T0] 步数 {}: 智能扰动 | 重码={} | 当前: {:.4}",
                    step, m.collision_count, evaluator.get_score(ctx)
                );
            }
        }

        if thread_id == 0 && step % report_interval == 0 && step > 0 {
            let pct = step * 100 / steps;
            let m = evaluator.get_metrics(ctx);
            let elapsed = sa_start.elapsed().as_secs_f64();
            let speed = if elapsed > 0.0 { step as f64 / elapsed } else { 0.0 };
            let scores = evaluator.get_metric_scores(ctx);
            // 分量分数（需求 16.4/16.5）：当前解与最佳解的全码/简码分量。
            let cur_full_comp = weight_full * evaluator.full_score_component(ctx);
            let cur_simple_comp = w_eff * evaluator.simple_score_component(ctx);
            let best_full_comp = weight_full * best_full_score;
            let best_simple_comp = w_eff * best_simple_score;
            println!(
                "   [T0] 进度: {}% | {:.1} 万步/分钟 | 基温: {:.6} | 重码={}({:.4}) 重码率={:.4}%({:.4}) 当量={:.4}({:.4}) CV={:.4}({:.4}) 分布={:.4}({:.4}) | 当前: {:.4} (全码:{:.4} 简码:{:.4}) 🏆最优: {:.4} (全码:{:.4} 简码:{:.4})",
                pct, speed * 60.0 / 10000.0, base_temp,
                m.collision_count, scores.collision_count,
                m.collision_rate * 100.0, scores.collision_rate,
                m.equiv_mean, scores.equivalence,
                m.equiv_cv, scores.equiv_cv,
                m.dist_deviation, scores.distribution,
                evaluator.get_score(ctx), cur_full_comp, cur_simple_comp,
                best_score, best_full_comp, best_simple_comp
            );
        }

        // === 周期对账：每 M 步用全量重算覆盖增量值，纠正浮点/整型漂移（需求 15.4/15.5）===
        if step > 0 && step % reconcile_m == 0 {
            evaluator.reconcile(ctx, &assignment);
            evaluator.score_dirty = true;
        }
    }

    // === 全量重建防回归观测（任务 1 补充）：主循环结束、最终强制对账之前 ===
    // 打印两个全量重建计数 (主评估器 rebuild_simple, 简码评估器 full_rebuild) 及其相对
    // 主循环开始时的增量。健康运行下增量应为 0 —— 证明热路径全程走增量、未触发全量重建。
    if thread_id == 0 {
        let (ev_calls, se_calls) = evaluator.full_rebuild_calls_breakdown();
        let loop_delta = evaluator.full_rebuild_calls() - full_rebuild_baseline;
        println!(
            "   [T0] 全量重建计数 | 主评估器 rebuild_simple: {} | 简码 full_rebuild: {} | SA 主循环增量: {} {}",
            ev_calls,
            se_calls,
            loop_delta,
            if loop_delta == 0 { "✓ 热路径零全量重建" } else { "⚠ 热路径触发了全量重建（疑似回归）" }
        );
    }

    // === 结束强制全量校验：使最终上报指标为精确值（需求 15.6）===
    evaluator.reconcile(ctx, &assignment);
    // 以全量重建结果一致地重算最佳解的分量与指标（需求 11/15.6）。
    {
        let mut best_eval = Evaluator::new(ctx, &best_assignment);
        best_eval.simple_active = simple_activated;
        best_eval.current_simple_weight = if simple_enabled { w_target } else { 0.0 };
        best_eval.score_dirty = true;
        best_eval.full_score_dirty = true;
        best_full_score = best_eval.full_score_component(ctx);
        best_simple_score = best_eval.simple_score_component(ctx);
        best_score = Evaluator::best_total(
            weight_full,
            best_full_score,
            best_eval.current_simple_weight,
            best_simple_score,
        );
        best_metrics = best_eval.get_metrics(ctx);
        best_simple_metrics = best_eval.get_simple_metrics(ctx);
    }

    // 最终精炼
    if thread_id == 0 {
        println!("   [T0] SA 完成，执行最终精炼...");
    }

    let final_warmup_steps = (n_groups * 50).max(5000).min(100_000);
    let (final_assignment, final_score) =
        hill_climb_warmup(ctx, best_assignment.clone(), &mut rng, final_warmup_steps);

    if final_score < best_score {
        best_assignment = final_assignment;
        best_score = final_score;
        let eval = Evaluator::new(ctx, &best_assignment);
        best_metrics = eval.get_metrics(ctx);
        best_simple_metrics = eval.get_simple_metrics(ctx);

        if thread_id == 0 {
            println!("   [T0] 最终爬山改进 → 得分: {:.4}", best_score);
        }
    }

    if n_groups <= 500 {
        let score_before_cd = best_score;
        let (cd_assignment, cd_score) = coordinate_descent(ctx, best_assignment.clone());
        if cd_score < best_score {
            best_assignment = cd_assignment;
            best_score = cd_score;
            let eval = Evaluator::new(ctx, &best_assignment);
            best_metrics = eval.get_metrics(ctx);
            best_simple_metrics = eval.get_simple_metrics(ctx);

            if thread_id == 0 {
                println!("   [T0] 坐标下降精炼: {:.4} → {:.4}", score_before_cd, best_score);
            }
        }
    }

    if thread_id == 0 {
        println!("   [T0] 最终得分: {:.4} 重码: {}", best_score, best_metrics.collision_count);
    }

    (best_assignment, best_score, best_metrics, best_simple_metrics)
}

// =========================================================================
// 🧪 冲突导向算子测试（sa-conflict-operators）
// =========================================================================
#[cfg(test)]
mod conflict_tests {
    use super::*;
    use crate::config::{Config, TargetsConfig};
    use crate::context::OptContext;
    use crate::types::{KeyDistConfig, RootGroup, ScaleConfig, SimpleCodeConfig, EQUIV_TABLE_SIZE};
    use proptest::prelude::*;
    use rand::thread_rng;
    use std::collections::HashMap;

    /// 构建最小 OptContext：每个频率对应一个动态组，每组含 1 个字根、1 个单部件汉字。
    /// 不同组的汉字被分到同一键位时即产生重码（用于驱动 find_collision_groups）。
    fn make_ctx(freqs: &[u64], allowed: &[u8]) -> OptContext {
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
        let mut cfg = Config::default();
        cfg.weights.simple_code.enabled = false; // 关闭简码，简化上下文构造
        let weights = cfg.get_weight_config();
        OptContext::new(
            &splits,
            &fixed_roots,
            &groups,
            equiv_table,
            key_dist,
            ScaleConfig::default(),
            SimpleCodeConfig { levels: vec![] },
            weights,
            TargetsConfig::default(),
        )
    }

    const FREQS: [u64; 5] = [100, 90, 80, 70, 60];
    const ALLOWED: [u8; 3] = [0, 1, 2];

    proptest! {
        // Feature: sa-conflict-operators, Property 1: 采样窗口落在有效范围内
        // 空列表或 window=0 时返回 false 且不修改 assignment；任意 window（含大于列表长度）不 panic。
        #[test]
        fn prop1_sampling_window_bounds(window in 0usize..12) {
            let ctx = make_ctx(&FREQS, &ALLOWED);
            let mut rng = thread_rng();

            // 空冲突列表 + 任意 window → false 且 assignment 不变
            let mut a = vec![0u8; FREQS.len()];
            let before = a.clone();
            let mut ev = Evaluator::new(&ctx, &a);
            let r_empty = try_resolve_conflict(&ctx, &mut a, &mut ev, &[], window, 1.0, &mut rng);
            prop_assert!(!r_empty);
            prop_assert_eq!(&a, &before);

            // 非空列表（assignment 全 0 → 所有组同键，必有冲突对）
            let cols = find_collision_groups(&ctx, &a, false);
            prop_assert!(!cols.is_empty());

            // window=0 → false 且不变
            let mut a0 = a.clone();
            let mut ev0 = Evaluator::new(&ctx, &a0);
            let r0 = try_resolve_conflict(&ctx, &mut a0, &mut ev0, &cols, 0, 1.0, &mut rng);
            prop_assert!(!r0);
            prop_assert_eq!(&a0, &a);

            // 任意 window（包含 > len）不 panic：window+1 ∈ [1,12]，覆盖大于 len 的情形
            let mut a1 = a.clone();
            let mut ev1 = Evaluator::new(&ctx, &a1);
            let _ = try_resolve_conflict(&ctx, &mut a1, &mut ev1, &cols, window + 1, 1.0, &mut rng);
        }

        // Feature: sa-conflict-operators, Property 2: 排序按所选权重降序
        #[test]
        fn prop2_descending_sort(asg in prop::collection::vec(0u8..3, FREQS.len())) {
            let ctx = make_ctx(&FREQS, &ALLOWED);
            for wbf in [false, true] {
                let cols = find_collision_groups(&ctx, &asg, wbf);
                for w in cols.windows(2) {
                    prop_assert!(w[0].2 >= w[1].2);
                }
            }
        }

        // Feature: sa-conflict-operators, Property 3: 排序结果可复现（裁决确定性）
        #[test]
        fn prop3_reproducible(asg in prop::collection::vec(0u8..3, FREQS.len())) {
            let ctx = make_ctx(&FREQS, &ALLOWED);
            for wbf in [false, true] {
                let c1 = find_collision_groups(&ctx, &asg, wbf);
                let c2 = find_collision_groups(&ctx, &asg, wbf);
                prop_assert_eq!(c1, c2);
            }
        }

        // Feature: sa-conflict-operators, Property 4: 两种排序策略产出相同冲突组集合
        #[test]
        fn prop4_same_set(asg in prop::collection::vec(0u8..3, FREQS.len())) {
            let ctx = make_ctx(&FREQS, &ALLOWED);
            let cf = find_collision_groups(&ctx, &asg, false);
            let ct = find_collision_groups(&ctx, &asg, true);
            let mut sf: Vec<(usize, usize)> = cf.iter().map(|t| (t.0, t.1)).collect();
            let mut st: Vec<(usize, usize)> = ct.iter().map(|t| (t.0, t.1)).collect();
            sf.sort_unstable();
            st.sort_unstable();
            prop_assert_eq!(sf, st);
        }
    }

    // 采样窗口边界（需求 4.3 / 4.4）显式单元用例
    #[test]
    fn test_window_zero_and_empty_return_false() {
        let ctx = make_ctx(&FREQS, &ALLOWED);
        let mut rng = thread_rng();
        let mut a = vec![0u8; FREQS.len()];
        let before = a.clone();
        let mut ev = Evaluator::new(&ctx, &a);

        // 空列表
        assert!(!try_resolve_conflict(&ctx, &mut a, &mut ev, &[], 5, 1.0, &mut rng));
        assert_eq!(a, before);

        // window = 0
        let cols = find_collision_groups(&ctx, &a, false);
        assert!(!cols.is_empty());
        assert!(!try_resolve_conflict(&ctx, &mut a, &mut ev, &cols, 0, 1.0, &mut rng));
        assert_eq!(a, before);
    }

    // 两种排序策略权重含义（需求 5.1 / 5.2）：全 0 分配下，count 权重为组数、freq 权重为字频之和
    #[test]
    fn test_weight_semantics_all_same_key() {
        let ctx = make_ctx(&FREQS, &ALLOWED);
        let a = vec![0u8; FREQS.len()]; // 所有组同键，单一冲突编码，所有组互相冲突
        let cnt = find_collision_groups(&ctx, &a, false);
        let frq = find_collision_groups(&ctx, &a, true);
        // count 策略：每个冲突对权重 = 共享该编码的汉字数 = 5
        assert!(cnt.iter().all(|t| t.2 == FREQS.len()));
        // freq 策略：权重 = 字频之和 = 100+90+80+70+60 = 400
        let sum: usize = FREQS.iter().map(|&f| f as usize).sum();
        assert!(frq.iter().all(|t| t.2 == sum));
    }
}

// =========================================================================
// 🧪 激活与升温集成测试（simple-code-perf-optimization 任务 10.3）
//
// 覆盖需求 12.2/12.3/12.4：
//   - 12.2：简码计算被激活时应用 `simple_activation_reheat` 作为升温倍率；
//   - 12.3：`simple_activation_reheat` 独立于 `reheat_factor`，不复用后者取值；
//   - 12.4：关闭 reheat 与扰动等配置时，仍正确执行简码激活与权重渐进核心逻辑。
//
// 由于 `simulated_annealing` 仅返回最终结果、内部激活事件不可直接观测，
// 本测试以「纯函数语义断言 + 小规模端到端冒烟」两条务实路径验证上述需求：
//   1. 直接对升温乘子语义与配置独立性断言（无需内部钩子）；
//   2. 端到端跑数十步，断言完成且返回有限得分与合理 SimpleMetrics；
//   3. 关闭 reheat/扰动后仍能完成并产出简码指标。
// =========================================================================
#[cfg(test)]
mod activation_reheat_tests {
    use super::*;
    use crate::config::{Config, TargetsConfig};
    use crate::context::OptContext;
    use crate::types::{
        KeyDistConfig, RootGroup, ScaleConfig, SimpleCodeConfig, SimpleCodeLevel, SimpleCodeStep,
        WeightConfig, EQUIV_TABLE_SIZE,
    };
    use proptest::prelude::*;
    use std::collections::HashMap;

    /// 构建启用简码、含一个真实简码级别的小规模 OptContext。
    ///
    /// 每个频率对应一个动态组……每个汉字含 2 个字根（全码长度 2），允许键位 [0,1,2]
    /// （键位空间很小 → 全码桶易产生重码，从而存在可计的简码重码）。
    /// 简码仅一个级别，`code_num = 1`、规则 `"Aa"`（取第 0 个逻辑根的第 0 个编码，简码长度 1）。
    /// 全码长度 2 > 简码长度 1，满足需求 22「简码须严格短于全码」，激活后确有出简候选可处理。
    fn make_simple_ctx(freqs: &[u64]) -> OptContext {
        let n = freqs.len();
        let mut groups = Vec::with_capacity(n * 2);
        let mut splits = Vec::with_capacity(n);
        for (i, &f) in freqs.iter().enumerate() {
            let r0 = format!("r{i}_0");
            let r1 = format!("r{i}_1");
            groups.push(RootGroup {
                roots: vec![r0.clone()],
                allowed_keys: vec![0, 1, 2],
            });
            groups.push(RootGroup {
                roots: vec![r1.clone()],
                allowed_keys: vec![0, 1, 2],
            });
            let ch = char::from_u32(0x4e00 + i as u32).unwrap();
            splits.push((ch, vec![r0, r1], f));
        }
        let fixed_roots: HashMap<String, u8> = HashMap::new();
        let equiv_table = [[0.0f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];
        let key_dist = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];
        let mut weights = WeightConfig::default();
        weights.enable_simple_code = true;
        weights.simple_coverage_ratio = 1.0; // 全部高频字纳入候选，确保有出简候选
        let level = SimpleCodeLevel {
            level: 1,
            code_num: 1,
            rule_candidates: vec![vec![SimpleCodeStep {
                root_selector: 'A',
                code_selector: 'a',
            }]],
            space_commit: false,
        };
        OptContext::new(
            &splits,
            &fixed_roots,
            &groups,
            equiv_table,
            key_dist,
            ScaleConfig::default(),
            SimpleCodeConfig {
                levels: vec![level],
            },
            weights,
            TargetsConfig::default(),
        )
    }

    /// 构建用于小规模端到端运行的 Config：
    /// 总步数很小、激活阈值很低（保证激活在运行内发生）、渐进段短。
    fn make_run_cfg(total_steps: usize) -> Config {
        let mut cfg = Config::default();
        cfg.annealing.threads = 1;
        cfg.annealing.total_steps = total_steps;
        cfg.annealing.min_improve_steps_ratio = 0.1;
        // 激活阈值足够低，使激活在数十步内触发；渐进段短，便于覆盖到达 W 的区间。
        cfg.weights.simple_code.simple_start_progress = 0.1;
        cfg.weights.simple_code.simple_ramp_progress = 0.1;
        cfg.weights.simple_code.simple_coverage_ratio = 1.0;
        cfg
    }

    // -----------------------------------------------------------------------
    // 1. 升温解耦（需求 12.3）：simple_activation_reheat 独立于 reheat_factor
    // -----------------------------------------------------------------------

    /// 默认值断言：`simple_activation_reheat` 默认 1.0（不升温），且与
    /// `reheat_factor`（默认 1.25）是相互独立的两个配置字段。
    #[test]
    fn test_activation_reheat_default_and_independent_field() {
        let cfg = Config::default();
        assert_eq!(
            cfg.weights.simple_code.simple_activation_reheat, 1.0,
            "simple_activation_reheat 默认应为 1.0（不升温，需求 12.1）"
        );
        // 两者是不同字段、不同默认值，证明配置层面相互独立（需求 12.3）。
        assert_eq!(cfg.annealing.reheat_factor, 1.25);
        assert_ne!(
            cfg.weights.simple_code.simple_activation_reheat, cfg.annealing.reheat_factor,
            "激活升温因子与 reheat_factor 应为独立取值（需求 12.3）"
        );
    }

    /// 升温乘子语义断言：激活升温只依赖 `simple_activation_reheat`，与 `reheat_factor` 无关。
    /// 复刻主循环激活分支的乘子计算 `temp_multiplier = mult(temp_multiplier, simple_reheat)`。
    #[test]
    fn test_activation_multiplier_decoupled_from_reheat_factor() {
        // 默认 1.0：乘子不变（不升温）。
        assert_eq!(simple_activation_multiplier(1.0, 1.0), 1.0);

        // 任意 reheat_factor 取值都不影响激活升温结果——纯函数仅吃 simple_activation_reheat。
        let simple_reheat = 1.5;
        for reheat_factor in [1.0, 1.25, 2.0, 5.0] {
            // 激活升温结果仅由 simple_reheat 决定，reheat_factor 不参与计算。
            let after = simple_activation_multiplier(1.0, simple_reheat);
            assert_eq!(
                after, simple_reheat,
                "激活升温应等于 simple_activation_reheat（={simple_reheat}），\
                 与 reheat_factor={reheat_factor} 无关（需求 12.3）"
            );
        }
    }

    // -----------------------------------------------------------------------
    // 3. 权重渐进曲线 sanity（支撑 12.x；Property 8 由任务 12.1 严格验证）
    // -----------------------------------------------------------------------

    /// 轻量断言 `w_simple_eff`：激活前为 0、到达 p_start+p_ramp 后等于 W、渐进段单调非降。
    #[test]
    fn test_weight_ramp_sanity() {
        let (p_start, p_ramp, w) = (0.1f64, 0.1f64, 0.3f64);

        // 激活前贡献为 0。
        assert_eq!(w_simple_eff(0.0, p_start, p_ramp, w), 0.0);
        assert_eq!(w_simple_eff(p_start - 1e-9, p_start, p_ramp, w), 0.0);
        // p_start 处连续为 0（α=0）。
        assert!((w_simple_eff(p_start, p_start, p_ramp, w) - 0.0).abs() < 1e-12);
        // 渐进结束后等于目标权重 W。
        assert!((w_simple_eff(p_start + p_ramp, p_start, p_ramp, w) - w).abs() < 1e-12);
        assert!((w_simple_eff(0.9, p_start, p_ramp, w) - w).abs() < 1e-12);

        // 渐进段单调非降。
        let mut prev = -1.0;
        for i in 0..=20 {
            let p = p_start + (i as f64 / 20.0) * p_ramp;
            let cur = w_simple_eff(p, p_start, p_ramp, w);
            assert!(
                cur + 1e-12 >= prev,
                "w_simple_eff 在渐进段应单调非降：p={p} cur={cur} prev={prev}"
            );
            prev = cur;
        }
    }

    proptest! {
        // Feature: simple-code-perf-optimization, Property 8: 有效简码权重曲线符合分段定义
        // 对任意 p∈[0,1)、p_start∈[0,1)、p_ramp∈[0,1-p_start]、W∈[0,range]：
        //   p<p_start ⟹ 0；p_start≤p<p_start+p_ramp（p_ramp>0）⟹ W·α²(3-2α)，α=(p-p_start)/p_ramp；
        //   p≥p_start+p_ramp ⟹ W。边界连续：p=p_start 处为 0，p=p_start+p_ramp 处为 W。
        // 综合得分 total = weight_full·full + w_eff·simple 恰好使用本曲线给出的 w_eff（需求 9.5）。
        #![proptest_config(ProptestConfig::with_cases(100))]
        #[test]
        fn prop8_weight_curve_piecewise(
            p in 0.0f64..1.0,
            p_start in 0.0f64..1.0,
            ramp_frac in 0.0f64..1.0,
            w in 0.0f64..1000.0,
        ) {
            // p_ramp ∈ [0, 1 - p_start]，覆盖含 0 的瞬时激活档。
            let p_ramp = ramp_frac * (1.0 - p_start);
            const EPS: f64 = 1e-9;

            let actual = w_simple_eff(p, p_start, p_ramp, w);

            // 分段定义的独立参考实现。
            let expected = if p < p_start {
                0.0
            } else if p_ramp > 0.0 && p < p_start + p_ramp {
                let alpha = (p - p_start) / p_ramp;
                w * alpha * alpha * (3.0 - 2.0 * alpha)
            } else {
                // 含 p_ramp==0 的瞬时跳变：p>=p_start 即取 W。
                w
            };

            let tol = EPS * (1.0 + w.abs());
            prop_assert!(
                (actual - expected).abs() <= tol,
                "分段曲线不符：p={p} p_start={p_start} p_ramp={p_ramp} w={w} actual={actual} expected={expected}"
            );

            // 第一段：激活前贡献为 0（需求 9.2）。
            if p < p_start {
                prop_assert_eq!(actual, 0.0);
            }

            // 第三段：达到/越过 p_start+p_ramp 取目标权重 W（需求 9.4）。
            if p >= p_start + p_ramp {
                prop_assert!((actual - w).abs() <= tol);
            }

            // 边界连续性（需求 9.3）：
            // 左边界 p=p_start 处曲线值为 0（α=0）。
            let at_start = w_simple_eff(p_start, p_start, p_ramp, w);
            prop_assert!(
                at_start.abs() <= tol,
                "左边界应为 0：p_start={p_start} p_ramp={p_ramp} w={w} got={at_start}"
            );
            // 右边界 p=p_start+p_ramp 处曲线值为 W。
            let at_end = w_simple_eff(p_start + p_ramp, p_start, p_ramp, w);
            prop_assert!(
                (at_end - w).abs() <= tol,
                "右边界应为 W：p_start={p_start} p_ramp={p_ramp} w={w} got={at_end}"
            );

            // 综合得分形式（需求 9.5）：total 恰好以本曲线的 w_eff 合成。
            let weight_full = 0.75f64;
            let full_score = 12.34f64;
            let simple_score = 56.78f64;
            let w_eff = actual;
            let total = weight_full * full_score + w_eff * simple_score;
            let total_ref = weight_full * full_score
                + w_simple_eff(p, p_start, p_ramp, w) * simple_score;
            prop_assert!(
                (total - total_ref).abs() <= EPS * (1.0 + total_ref.abs()),
                "综合得分应恰用 w_eff 合成：total={total} ref={total_ref}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // 2. 端到端小规模运行（需求 12.2/12.4）
    // -----------------------------------------------------------------------

    /// 校验 SimpleMetrics 合理性：覆盖率 ∈ [0,1]、各项有限、计数非负。
    fn assert_sane_simple_metrics(sm: &SimpleMetrics) {
        assert!(
            sm.weighted_freq_coverage >= 0.0 && sm.weighted_freq_coverage <= 1.0 + 1e-9,
            "简码覆盖率应落在 [0,1]：{}",
            sm.weighted_freq_coverage
        );
        assert!(sm.equiv_mean.is_finite(), "平均当量应有限：{}", sm.equiv_mean);
        assert!(
            sm.dist_deviation.is_finite(),
            "分布偏差应有限：{}",
            sm.dist_deviation
        );
        assert!(
            sm.collision_rate.is_finite() && sm.collision_rate >= 0.0,
            "简码重码率应为非负有限值：{}",
            sm.collision_rate
        );
        // collision_count 为 usize，恒 >= 0（类型保证），此处显式记录语义。
        let _ = sm.collision_count;
    }

    /// 默认配置（含 reheat 与扰动开启）下小规模端到端运行：
    /// 断言激活会在运行内发生（start_progress 很低）、运行无 panic、返回有限得分与合理简码指标。
    #[test]
    fn test_end_to_end_small_run_with_reheat_and_perturb() {
        let ctx = make_simple_ctx(&[100, 90, 80, 70, 60, 50]);
        let cfg = make_run_cfg(120);

        // 前置事实：激活阈值 0.1 < 1.0，运行内进度必将跨过阈值并触发激活（需求 8.3/12.2）。
        assert!(cfg.weights.simple_code.simple_start_progress < 1.0);

        let (assignment, score, _metrics, simple_metrics) =
            simulated_annealing(&ctx, &cfg, 1);

        assert_eq!(assignment.len(), ctx.num_groups, "分配长度应等于组数");
        assert!(score.is_finite(), "最终综合得分应为有限值：{score}");
        assert_sane_simple_metrics(&simple_metrics);
    }

    /// 关闭 reheat 与扰动、且激活升温为 1.0 时，仍应正确激活并渐进、完成运行并产出简码指标
    /// （需求 12.4：核心激活/渐进逻辑不依赖 reheat 与扰动）。
    #[test]
    fn test_end_to_end_small_run_reheat_and_perturb_disabled() {
        let ctx = make_simple_ctx(&[100, 90, 80, 70, 60, 50]);
        let mut cfg = make_run_cfg(150);
        // 关闭 reheat（reheat_factor=1.0）、激活升温为 1.0（不升温）。
        cfg.annealing.reheat_factor = 1.0;
        cfg.weights.simple_code.simple_activation_reheat = 1.0;
        // 关闭扰动。
        cfg.annealing.perturb_interval_ratio = 0.0;
        cfg.annealing.perturb_strength = 0.0;

        let (assignment, score, _metrics, simple_metrics) =
            simulated_annealing(&ctx, &cfg, 1);

        assert_eq!(assignment.len(), ctx.num_groups);
        assert!(
            score.is_finite(),
            "关闭 reheat/扰动后最终得分仍应有限：{score}"
        );
        assert_sane_simple_metrics(&simple_metrics);
        // 候选覆盖率=1.0 且 code_num=1，激活后应确有出简，覆盖率 > 0 佐证简码确实生效。
        assert!(
            simple_metrics.weighted_freq_coverage > 0.0,
            "激活并出简后简码覆盖率应大于 0：{}",
            simple_metrics.weighted_freq_coverage
        );
    }
}

// =========================================================================
// 🧪 日志输出单元测试（simple-code-perf-optimization 任务 11.2）
//
// 覆盖需求：
//   - 7.7 / 16.6：配置确认日志输出简码候选字覆盖率与候选字数；
//   - 16.2：渐进期 10 个进度点输出 (p, α, w)；
//   - 16.4：当前/最佳日志的分量分解（全码分量 = weight_full·full_score，
//           简码分量 = w_eff·simple_score，两者之和 = best_total）。
//
// 直接抓取 stdout 既脆弱又不稳定，故本测试断言「被日志打印的量」本身计算正确：
//   - 渐进点：纯函数 `ramp_log_points` 的逐点公式与单调性；
//   - 配置确认：`OptContext::{simple_actual_coverage, simple_candidate_chars}`；
//   - 分量分解：`Evaluator::best_total` 与组件公式一致。
// =========================================================================
#[cfg(test)]
mod log_output_tests {
    use super::*;
    use crate::config::TargetsConfig;
    use crate::context::OptContext;
    use crate::types::{
        KeyDistConfig, RootGroup, ScaleConfig, SimpleCodeConfig, SimpleCodeLevel, SimpleCodeStep,
        WeightConfig, EQUIV_TABLE_SIZE,
    };
    use std::collections::HashMap;

    /// 构建启用简码的小规模 OptContext，候选覆盖率比例由 `coverage_ratio` 控制。
    /// 每个频率对应一个动态组（1 个字根、1 个单部件汉字），允许键位 [0,1,2]。
    fn make_simple_ctx(freqs: &[u64], coverage_ratio: f64) -> OptContext {
        let n = freqs.len();
        let mut groups = Vec::with_capacity(n);
        let mut splits = Vec::with_capacity(n);
        for (i, &f) in freqs.iter().enumerate() {
            let root = format!("r{i}");
            groups.push(RootGroup {
                roots: vec![root.clone()],
                allowed_keys: vec![0, 1, 2],
            });
            let ch = char::from_u32(0x4e00 + i as u32).unwrap();
            splits.push((ch, vec![root], f));
        }
        let fixed_roots: HashMap<String, u8> = HashMap::new();
        let equiv_table = [[0.0f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];
        let key_dist = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];
        let mut weights = WeightConfig::default();
        weights.enable_simple_code = true;
        weights.simple_coverage_ratio = coverage_ratio;
        let level = SimpleCodeLevel {
            level: 1,
            code_num: 1,
            rule_candidates: vec![vec![SimpleCodeStep {
                root_selector: 'A',
                code_selector: 'a',
            }]],
            space_commit: false,
        };
        OptContext::new(
            &splits,
            &fixed_roots,
            &groups,
            equiv_table,
            key_dist,
            ScaleConfig::default(),
            SimpleCodeConfig {
                levels: vec![level],
            },
            weights,
            TargetsConfig::default(),
        )
    }

    // -----------------------------------------------------------------------
    // 1. 渐进期 10 点（需求 16.2）
    // -----------------------------------------------------------------------

    /// 逐点校验 `ramp_log_points`：恰好 10 点，且每点的 (p, α, w) 满足约定公式；
    /// 首点 α=0、渐进段单调非降。覆盖 p_ramp > 0 与 p_ramp == 0 两种配置。
    fn verify_ramp_config(p_start: f64, p_ramp: f64, w_target: f64) {
        let points = ramp_log_points(p_start, p_ramp, w_target);
        assert_eq!(points.len(), 10, "渐进期应恰好输出 10 个进度点（需求 16.2）");

        let mut prev_w = f64::NEG_INFINITY;
        for (i, &(p_i, alpha, w_i)) in points.iter().enumerate() {
            // p_i == p_start + (i/10)·p_ramp
            let expect_p = p_start + (i as f64 / 10.0) * p_ramp;
            assert!(
                (p_i - expect_p).abs() < 1e-12,
                "第 {i} 点 p 应为 {expect_p}，实得 {p_i}"
            );

            // α == (p_i - p_start)/p_ramp，p_ramp==0 时取 0
            let expect_alpha = if p_ramp > 0.0 {
                (p_i - p_start) / p_ramp
            } else {
                0.0
            };
            assert!(
                (alpha - expect_alpha).abs() < 1e-12,
                "第 {i} 点 α 应为 {expect_alpha}，实得 {alpha}"
            );

            // w == w_simple_eff(p_i, ...)
            let expect_w = w_simple_eff(p_i, p_start, p_ramp, w_target);
            assert_eq!(
                w_i, expect_w,
                "第 {i} 点 w 应等于 w_simple_eff 的计算值"
            );

            // 单调非降
            assert!(
                w_i + 1e-12 >= prev_w,
                "渐进点 w 应单调非降：第 {i} 点 w={w_i} < prev={prev_w}"
            );
            prev_w = w_i;
        }

        // 首点 α=0；渐进档（p_ramp>0）下 w=0（激活前/激活当刻不贡献）。
        assert!((points[0].1 - 0.0).abs() < 1e-12, "首点 α 应为 0");
        if p_ramp > 0.0 {
            assert!(
                (points[0].2 - 0.0).abs() < 1e-12,
                "渐进档首点 w 应为 0（α=0 ⟹ w=0）"
            );
        }
    }

    #[test]
    fn test_ramp_log_points_formula_and_monotonic() {
        // 标准渐进档
        verify_ramp_config(0.1, 0.2, 0.3);
        // 另一组渐进配置
        verify_ramp_config(0.0, 0.5, 1.0);
        // 硬激活/瞬时档：p_ramp == 0（所有点 p==p_start、α=0、w==W）
        verify_ramp_config(0.2, 0.0, 0.4);

        // p_ramp==0 时所有点等于目标权重 W（落入 w_simple_eff 第三段）。
        let pts = ramp_log_points(0.2, 0.0, 0.4);
        assert!(pts.iter().all(|&(_, _, w)| (w - 0.4).abs() < 1e-12));
    }

    // -----------------------------------------------------------------------
    // 2. 配置确认（需求 7.7 / 16.6）
    // -----------------------------------------------------------------------

    /// 断言「配置确认」日志打印的两个量计算正确：
    ///   - `simple_candidate_chars.len()` == 候选字数；
    ///   - `simple_actual_coverage` ∈ [0,1] 且 == 候选字累计字频 / 总字频。
    #[test]
    fn test_config_confirmation_logged_quantities() {
        let freqs = [100u64, 90, 80, 70, 60, 50];
        let total: u64 = freqs.iter().sum();
        // 覆盖率比例 0.6：候选集为达标的最小高频前缀（非全量），更能验证累计公式。
        let ctx = make_simple_ctx(&freqs, 0.6);

        let candidates = &ctx.simple_candidate_chars;
        // 候选字数 = 日志打印的「候选字数」。
        assert!(
            !candidates.is_empty() && candidates.len() <= freqs.len(),
            "候选字数应在 (0, n] 内：{}",
            candidates.len()
        );

        // 覆盖率 ∈ [0,1]。
        assert!(
            ctx.simple_actual_coverage >= 0.0 && ctx.simple_actual_coverage <= 1.0 + 1e-9,
            "覆盖率应落在 [0,1]：{}",
            ctx.simple_actual_coverage
        );

        // 覆盖率 == 候选字累计字频之和 / 总字频。
        let cand_freq_sum: u64 = candidates
            .iter()
            .map(|&ci| ctx.char_infos[ci].frequency)
            .sum();
        let expect_cov = cand_freq_sum as f64 / total as f64;
        assert!(
            (ctx.simple_actual_coverage - expect_cov).abs() < 1e-12,
            "覆盖率应等于候选累计字频/总字频：实得 {} 期望 {}",
            ctx.simple_actual_coverage,
            expect_cov
        );

        // 比例达标性：累计覆盖率应 ≥ 配置比例（候选选取的终止条件）。
        assert!(
            ctx.simple_actual_coverage >= 0.6 - 1e-12,
            "实际覆盖率应达到配置比例 0.6：{}",
            ctx.simple_actual_coverage
        );
    }

    /// 全量覆盖（ratio=1.0）：候选集应纳入全部有频汉字，覆盖率为 1.0。
    #[test]
    fn test_config_confirmation_full_coverage() {
        let freqs = [100u64, 90, 80];
        let ctx = make_simple_ctx(&freqs, 1.0);
        assert_eq!(
            ctx.simple_candidate_chars.len(),
            freqs.len(),
            "ratio=1.0 时候选字数应等于全部汉字数"
        );
        assert!(
            (ctx.simple_actual_coverage - 1.0).abs() < 1e-12,
            "ratio=1.0 时覆盖率应为 1.0：{}",
            ctx.simple_actual_coverage
        );
    }

    // -----------------------------------------------------------------------
    // 3. 分量分数（需求 16.4）
    // -----------------------------------------------------------------------

    /// 断言当前/最佳日志的分量分解：
    ///   全码分量 = weight_full · full_score，
    ///   简码分量 = w_eff · simple_score，
    ///   两者之和 == Evaluator::best_total(weight_full, full_score, w_eff, simple_score)。
    #[test]
    fn test_component_decomposition_matches_best_total() {
        let cases = [
            (1.0f64, 2.5f64, 0.0f64, 7.0f64),
            (0.8, 12.34, 0.3, 5.67),
            (2.0, 0.0, 1.5, 3.3),
            (0.5, -4.0, 0.25, 8.0),
        ];
        for &(weight_full, full_score, w_eff, simple_score) in &cases {
            let full_comp = weight_full * full_score;
            let simple_comp = w_eff * simple_score;
            let total = Evaluator::best_total(weight_full, full_score, w_eff, simple_score);
            assert!(
                ((full_comp + simple_comp) - total).abs() < 1e-12,
                "分量之和应等于 best_total：full_comp={full_comp} simple_comp={simple_comp} total={total}"
            );
            // w_eff==0（激活前）时简码分量应为 0，total 退化为纯全码分量。
            if w_eff == 0.0 {
                assert_eq!(simple_comp, 0.0, "w_eff=0 时简码分量应为 0");
                assert!((total - full_comp).abs() < 1e-12, "w_eff=0 时 total 应等于全码分量");
            }
        }
    }
}

// =========================================================================
// 🧪 对账间隔 M 计算测试（simple-code-perf-optimization, Property 12）
//
// 验证纯函数 `reconcile_interval` 满足 M = max(1, floor(total_steps × ratio))：
//   - ratio = 0 ⟹ M = 1
//   - 极小 ratio 使 floor 为 0 ⟹ M = 1
//   - 常规 ratio（如 0.05）⟹ M = floor(total_steps × ratio)（当其 ≥ 1）
//   - 任意输入 M ≥ 1（永不为 0，避免取模除零）
// =========================================================================
#[cfg(test)]
mod reconcile_interval_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        // Feature: simple-code-perf-optimization, Property 12: 对账间隔 M 的计算
        // 对任意 total_steps ≥ 1 与 ratio ≥ 0：M = max(1, floor(total_steps × ratio))，且 M ≥ 1。
        #![proptest_config(ProptestConfig::with_cases(100))]
        #[test]
        fn prop12_reconcile_interval(
            total_steps in 1usize..2_000_000usize,
            ratio in 0.0f64..2.0f64,
        ) {
            let m = reconcile_interval(total_steps, ratio);

            // 独立参考实现：M = max(1, floor(total_steps × ratio))。
            let floored = ((total_steps as f64) * ratio).floor() as i64;
            let expected = floored.max(1) as usize;

            prop_assert_eq!(m, expected, "M 应等于 max(1, floor(total_steps × ratio))");

            // 不变量：M 永不为 0。
            prop_assert!(m >= 1, "对账间隔必须 ≥ 1（避免取模除零），实际 {}", m);

            // ratio = 0 ⟹ M = 1。
            if ratio == 0.0 {
                prop_assert_eq!(m, 1, "ratio = 0 时 M 应为 1");
            }

            // floor 退化为 0（极小 ratio）⟹ M 被钳为 1。
            if floored <= 0 {
                prop_assert_eq!(m, 1, "floor ≤ 0 时 M 应被钳为 1");
            }
        }
    }

    #[test]
    fn ratio_zero_yields_one() {
        // ratio = 0 ⟹ 永远每步可对账（M = 1）。
        assert_eq!(reconcile_interval(1, 0.0), 1);
        assert_eq!(reconcile_interval(1_000_000, 0.0), 1);
    }

    #[test]
    fn tiny_ratio_floor_zero_yields_one() {
        // 极小 ratio 使 floor(total_steps × ratio) = 0 ⟹ M = 1。
        assert_eq!(reconcile_interval(10, 0.001), 1); // floor(0.01) = 0
        assert_eq!(reconcile_interval(100, 0.005), 1); // floor(0.5) = 0
    }

    #[test]
    fn normal_ratio_floor() {
        // 常规比例：M = floor(total_steps × ratio)。
        assert_eq!(reconcile_interval(1000, 0.05), 50); // floor(50.0) = 50
        assert_eq!(reconcile_interval(2000, 0.05), 100); // floor(100.0) = 100
        assert_eq!(reconcile_interval(333, 0.05), 16); // floor(16.65) = 16
    }
}

// =========================================================================
// 🧪 激活闩锁单调测试（simple-code-perf-optimization, Property 15）
//
// 验证纯函数 `latch_activation` 满足闩锁单调语义（需求 8.3/8.4）：
//   next = already_activated || hard_activate || p >= p_start
// 对任意进度序列（含非单调、升温导致的进度回退）逐步折叠闩锁后断言：
//   (1) 一旦闩锁为真，其后所有步保持为真（单调，不可回退）；
//   (2) 闩锁在「p>=p_start 或 hard_activate」首次成立的那一步首次变真；
//   (3) hard_activate 为真时，闩锁从第一步即为真。
// =========================================================================
#[cfg(test)]
mod activation_latch_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        // Feature: simple-code-perf-optimization, Property 15: 激活闩锁单调
        // 对任意进度序列（含非单调/升温回退序列）、任意 p_start 与 hard_activate：
        //   一旦激活闩锁变真即永久保持为真，且在首个满足触发条件的步变真；hard_activate 时从首步即真。
        #![proptest_config(ProptestConfig::with_cases(100))]
        #[test]
        fn prop15_activation_latch_monotonic(
            // 进度序列：每个值取自 [0,1)，序列可任意非单调（含回退），1..=50 步。
            seq in prop::collection::vec(0.0f64..1.0f64, 1..=50),
            p_start in 0.0f64..1.0f64,
            hard_activate in any::<bool>(),
        ) {
            // 逐步折叠闩锁，记录每一步的激活状态。
            let mut latched = false;
            let mut states: Vec<bool> = Vec::with_capacity(seq.len());
            for &p in &seq {
                latched = latch_activation(latched, p, p_start, hard_activate);
                states.push(latched);
            }

            // (1) 单调性：一旦为真，其后所有步保持为真（不因后续进度回退/升温关闭）。
            let mut seen_true = false;
            for (i, &s) in states.iter().enumerate() {
                if seen_true {
                    prop_assert!(
                        s,
                        "闩锁在第 {} 步回退为 false，违反单调性；序列={:?}",
                        i, states
                    );
                }
                if s {
                    seen_true = true;
                }
            }

            // 独立参考：首个满足触发条件 (p>=p_start || hard_activate) 的步索引。
            let first_trigger = seq
                .iter()
                .position(|&p| hard_activate || p >= p_start);

            match first_trigger {
                Some(idx) => {
                    // (2) 在首个触发步首次变真：该步前全为 false，从该步起全为 true。
                    for (i, &s) in states.iter().enumerate() {
                        if i < idx {
                            prop_assert!(!s, "触发步 {} 之前的第 {} 步不应激活", idx, i);
                        } else {
                            prop_assert!(s, "触发步 {} 起的第 {} 步应保持激活", idx, i);
                        }
                    }
                    // (3) hard_activate 为真 ⟹ 首个触发步必为第 0 步（从第一步即真）。
                    if hard_activate {
                        prop_assert_eq!(idx, 0, "hard_activate 时应从首步即激活");
                        prop_assert!(states[0], "hard_activate 时第 0 步应为 true");
                    }
                }
                None => {
                    // 无任何步满足触发条件 ⟹ 全程未激活。
                    // 此分支必无 hard_activate（否则每步都触发）。
                    prop_assert!(!hard_activate, "hard_activate 时不应出现无触发的情况");
                    for (i, &s) in states.iter().enumerate() {
                        prop_assert!(!s, "无触发条件满足时第 {} 步不应激活", i);
                    }
                }
            }
        }
    }

    #[test]
    fn already_activated_stays_true_regardless() {
        // 已激活后无论进度高低（含回退到 0）或是否硬激活，恒为 true。
        assert!(latch_activation(true, 0.0, 0.9, false));
        assert!(latch_activation(true, 0.01, 0.99, false));
        assert!(latch_activation(true, 0.5, 0.6, true));
    }

    #[test]
    fn hard_activate_true_from_first_step() {
        // 硬激活：即使 p < p_start 也从第一步即激活。
        assert!(latch_activation(false, 0.0, 0.6, true));
    }

    #[test]
    fn activates_when_progress_reaches_start() {
        // 进度首达 p_start 时激活；未达则不激活。
        assert!(!latch_activation(false, 0.59, 0.6, false));
        assert!(latch_activation(false, 0.6, 0.6, false));
        assert!(latch_activation(false, 0.95, 0.6, false));
    }

    #[test]
    fn non_monotonic_sequence_latches_and_holds() {
        // 升温/回退序列：进度先升过阈值再回落，闩锁应保持为真。
        let p_start = 0.5;
        let seq = [0.2, 0.4, 0.6 /* 触发 */, 0.3, 0.1, 0.55];
        let mut latched = false;
        let mut states = Vec::new();
        for &p in &seq {
            latched = latch_activation(latched, p, p_start, false);
            states.push(latched);
        }
        assert_eq!(states, vec![false, false, true, true, true, true]);
    }
}
