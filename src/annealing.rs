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
        evaluator.rebuild_simple(ctx, assignment);
    }

    evaluator.score_dirty = true;
    let new_score = evaluator.get_score(ctx);
    let delta = new_score - old_score;

    if delta <= 0.0 || rng.gen::<f64>() < (-delta / temp).exp() {
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
            evaluator.rebuild_simple(ctx, assignment);
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

    let mut best_assignment = assignment.clone();
    let mut best_score = evaluator.get_score(ctx);
    let mut best_metrics = evaluator.get_metrics(ctx);
    let mut best_simple_metrics = evaluator.get_simple_metrics(ctx);

    if thread_id == 0 {
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

    // 主循环
    for step in 0..steps {
        let _progress = step as f64 / steps as f64;
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
        if current_score < best_score {
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
                println!(
                    "   [T0] 步数 {}/{} | {:.1} 万步/分钟 | 温度 {:.6} | 重码:{}({:.4}) 重码率:{:.4}%({:.4}) 当量:{:.4}({:.4}) CV:{:.4}({:.4}) 分布:{:.4}({:.4}) | 得分: {:.4}",
                    step, steps, speed * 60.0 / 10000.0, temp,
                    m.collision_count, scores.collision_count,
                    m.collision_rate * 100.0, scores.collision_rate,
                    m.equiv_mean, scores.equivalence,
                    m.equiv_cv, scores.equiv_cv,
                    m.dist_deviation, scores.distribution,
                    best_score
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
            println!(
                "   [T0] 进度: {}% | {:.1} 万步/分钟 | 基温: {:.6} | 重码={}({:.4}) 重码率={:.4}%({:.4}) 当量={:.4}({:.4}) CV={:.4}({:.4}) 分布={:.4}({:.4}) | 当前: {:.4} 🏆最优: {:.4}",
                pct, speed * 60.0 / 10000.0, base_temp,
                m.collision_count, scores.collision_count,
                m.collision_rate * 100.0, scores.collision_rate,
                m.equiv_mean, scores.equivalence,
                m.equiv_cv, scores.equiv_cv,
                m.dist_deviation, scores.distribution,
                evaluator.get_score(ctx), best_score
            );
        }
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
