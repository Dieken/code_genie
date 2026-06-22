// =========================================================================
// 📤 输出模块
// =========================================================================

use std::collections::{HashMap, HashSet};
use std::fs;

use crate::context::OptContext;
use crate::config::Config;
use crate::evaluator::SimpleEvaluator;
use crate::types::{extract_base_name, extract_suffix_num, key_to_char, Metrics, SimpleMetrics};

/// 统计字根使用频率
pub fn count_root_usage(ctx: &OptContext) -> HashMap<String, u64> {
    let mut usage: HashMap<String, u64> = HashMap::new();
    for (_, roots, freq) in &ctx.raw_splits {
        let mut seen_bases: HashSet<String> = HashSet::new();
        for root in roots {
            let base = extract_base_name(root);
            if seen_bases.insert(base.clone()) {
                *usage.entry(base).or_default() += freq;
            }
        }
    }
    usage
}

/// 构建排序后的根编码列表
pub fn build_root_encodings_sorted(
    fixed: &HashMap<String, u8>,
    groups: &[crate::types::RootGroup],
    assignment: &[u8],
    root_usage: &HashMap<String, u64>,
) -> Vec<(String, Vec<u8>)> {
    let mut all_roots: Vec<(String, u8)> = Vec::new();

    for (root, &key) in fixed {
        all_roots.push((root.clone(), key));
    }
    for (gi, group) in groups.iter().enumerate() {
        let key = assignment[gi];
        for root in &group.roots {
            all_roots.push((root.clone(), key));
        }
    }

    let mut grouped: HashMap<String, Vec<(String, u8)>> = HashMap::new();
    for (name, key) in all_roots {
        let base = extract_base_name(&name);
        grouped.entry(base).or_default().push((name, key));
    }

    let mut result: Vec<(String, Vec<u8>)> = Vec::new();
    for (base, mut entries) in grouped {
        entries.sort_by(|a, b| {
            let sa = extract_suffix_num(&a.0);
            let sb = extract_suffix_num(&b.0);
            sa.cmp(&sb)
        });
        let keys: Vec<u8> = entries.iter().map(|(_, k)| *k).collect();
        result.push((base, keys));
    }

    // 按使用频率降序排序
    result.sort_by(|a, b| {
        let ua = root_usage.get(&a.0).copied().unwrap_or(0);
        let ub = root_usage.get(&b.0).copied().unwrap_or(0);
        ub.cmp(&ua).then_with(|| a.0.cmp(&b.0))
    });

    result
}

/// 格式化编码为可读字符串
pub fn format_encoding(keys: &[u8]) -> String {
    let mut s = String::with_capacity(keys.len());
    for (i, &k) in keys.iter().enumerate() {
        let c = key_to_char(k);
        if i == 0 {
            s.extend(c.to_uppercase());
        } else {
            s.push(c);
        }
    }
    s
}

/// 写入键位映射输出
pub fn write_keymap_output(
    root_out: &mut String,
    fixed: &HashMap<String, u8>,
    groups: &[crate::types::RootGroup],
    assignment: &[u8],
    root_usage: &HashMap<String, u64>,
) {
    let encodings = build_root_encodings_sorted(fixed, groups, assignment, root_usage);
    for (base_name, keys) in &encodings {
        let enc = format_encoding(keys);
        let usage = root_usage.get(base_name).copied().unwrap_or(0);
        root_out.push_str(&format!("{}\t{}\t{}\n", base_name, enc, usage));
    }
}

/// 复用评估器的出简选择（需求 23）：构建 `SimpleEvaluator` 并返回按分配顺序排列的
/// `(level_idx, ci)` 出简列表，使各 output 文件与被优化方案逐字一致。简码未启用或无级别时
/// 返回空列表。`is_first_candidate` 与 `Evaluator::new` / `save_simple_code_output` 同口径
/// （每个非空全码桶取「最大频率、最小 ci」为首选字，供 efficiency 排序键的 sel_len 取值）。
fn evaluator_simple_selection(ctx: &OptContext, assignment: &[u8]) -> Vec<(usize, usize, String)> {
    if !ctx.enable_simple_code || ctx.simple_config.levels.is_empty() {
        return Vec::new();
    }
    let (full_buckets, is_first) = crate::evaluator::build_full_buckets(ctx, assignment);
    let se = SimpleEvaluator::new(ctx, assignment, &full_buckets, &is_first, true);
    se.selected_ordered(ctx, &is_first)
        .into_iter()
        .map(|(li, ci)| (li, ci, se.rendered_code(li, ci)))
        .collect()
}

/// 保存简码+全码编码文件
pub fn save_combined_code_output(ctx: &OptContext, assignment: &[u8], dir: &str) {
    // 构建根名到键位的映射
    let mut root_to_key: HashMap<String, u8> = HashMap::new();
    for (root, &key) in &ctx.fixed_roots {
        root_to_key.insert(root.clone(), key);
    }
    for (gi, group) in ctx.groups.iter().enumerate() {
        let key = assignment[gi];
        for root in &group.roots {
            root_to_key.insert(root.clone(), key);
        }
    }

    let mut out = String::new();

    // 按字频排序（供下方全码部分输出）
    let n_chars = ctx.char_infos.len();
    let mut sorted_chars: Vec<usize> = (0..n_chars).collect();
    sorted_chars.sort_by(|&a, &b| {
        ctx.char_infos[b]
            .frequency
            .cmp(&ctx.char_infos[a].frequency)
    });

    // 简码部分（需求 23）：镜像评估器出简选择，按分配顺序（级别升序、桶内排序键）输出，
    // 与被优化方案逐字一致（含 efficiency 模式 / sel_len / 上屏键偏好 / 固定占用扣减 / 跨级排除）。
    let ordered = evaluator_simple_selection(ctx, assignment);
    let n_levels = ctx.simple_config.levels.len();
    let mut per_level: Vec<Vec<(usize, String)>> = vec![Vec::new(); n_levels];
    for (li, ci, code_str) in ordered {
        per_level[li].push((ci, code_str));
    }
    for li in 0..n_levels {
        for (ci, code_str) in &per_level[li] {
            let ch = ctx.raw_splits[*ci].0;
            out.push_str(&format!("{}\t{}\n", ch, code_str));
        }
        // 固定简码（需求 21.10）：输出归属本级的固定简码字，保留其字面上屏键。
        for fc in &ctx.simple_fixed_codes {
            if fc.li == Some(li) {
                let ch = ctx.raw_splits[fc.ci].0;
                out.push_str(&format!("{}\t{}\n", ch, fc.code_str));
            }
        }
    }
    // 无归属级别的固定简码（需求 21.6/21.10）：单列一段，不遗漏。
    for fc in &ctx.simple_fixed_codes {
        if fc.li.is_none() {
            let ch = ctx.raw_splits[fc.ci].0;
            out.push_str(&format!("{}\t{}\n", ch, fc.code_str));
        }
    }

    // 全码部分 - 所有字都输出全码
    for &ci in &sorted_chars {
        let ch = ctx.raw_splits[ci].0;
        let (_, roots, _) = &ctx.raw_splits[ci];
        let mut code_parts = Vec::new();
        for root in roots {
            if let Some(&key) = root_to_key.get(root) {
                code_parts.push(key_to_char(key));
            }
        }
        let code_str: String = code_parts.into_iter().collect();
        out.push_str(&format!("{}\t{}\n", ch, code_str));
    }

    crate::fsutil::write_with_backup(format!("{}/output-combined.txt", dir), out).unwrap();
}

/// 保存简码输出
pub fn save_simple_code_output(ctx: &OptContext, assignment: &[u8], dir: &str) {
    if !ctx.enable_simple_code || ctx.simple_config.levels.is_empty() {
        return;
    }

    // 构建全码桶存储与首选标记（与 Evaluator::new 同口径），供 Efficiency 模式 sel_len 取值。
    let (full_buckets, is_first_candidate) = crate::evaluator::build_full_buckets(ctx, assignment);

    let se = SimpleEvaluator::new(ctx, assignment, &full_buckets, &is_first_candidate, true);
    let sm = se.get_simple_metrics(ctx);

    let mut out = String::new();
    out.push_str("# 简码分配结果\n");
    out.push_str(&format!(
        "# 简码覆盖频率: {:.4}%\n",
        sm.weighted_freq_coverage * 100.0
    ));
    out.push_str(&format!("# 简码加权当量: {:.4}\n", sm.equiv_mean));
    out.push_str(&format!("# 简码分布偏差: {:.4}\n", sm.dist_deviation));
    out.push_str(&format!("# 简码重码数: {}\n", sm.collision_count));
    out.push_str(&format!(
        "# 简码重码率: {:.6}%\n",
        sm.collision_rate * 100.0
    ));
    out.push_str("#\n");

    let n_chars = ctx.char_infos.len();

    // 镜像评估器出简选择（需求 23）：直接复用上面已构建的 `se` 的 selected 状态，
    // 按分配顺序（级别升序、桶内排序键）输出，使输出方案与被优化方案逐字一致。
    let _ = n_chars;
    let ordered = se.selected_ordered(ctx, &is_first_candidate);
    let n_levels = ctx.simple_config.levels.len();
    let mut per_level: Vec<Vec<usize>> = vec![Vec::new(); n_levels];
    for (li, ci) in ordered {
        per_level[li].push(ci);
    }

    for (li, level_cfg) in ctx.simple_config.levels.iter().enumerate() {
        let rules_str: String = level_cfg
            .rule_candidates
            .iter()
            .map(|rule| {
                rule.iter()
                    .map(|s| format!("{}{}", s.root_selector, s.code_selector))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "\n# === {}级简码 (每位{}字, 规则:{}) ===\n",
            level_cfg.level, level_cfg.code_num, rules_str
        ));
        out.push_str("# 汉字\t简码\t字频\n");

        // 优化出简：镜像评估器 selected（含 efficiency 排序 / sel_len / 上屏键偏好 / 占用扣减 / 跨级排除）。
        for &ci in &per_level[li] {
            let ch = ctx.raw_splits[ci].0;
            let freq = ctx.char_infos[ci].frequency;
            let code_str = se.rendered_code(li, ci);
            out.push_str(&format!("{}\t{}\t{}\n", ch, code_str, freq));
        }

        // 固定简码（需求 21.10）：输出归属本级的固定简码字，保留其字面上屏键。
        let mut fixed_count = 0usize;
        for fc in &ctx.simple_fixed_codes {
            if fc.li == Some(li) {
                let ch = ctx.raw_splits[fc.ci].0;
                let freq = ctx.char_infos[fc.ci].frequency;
                out.push_str(&format!("{}\t{}\t{}\n", ch, fc.code_str, freq));
                fixed_count += 1;
            }
        }

        out.push_str(&format!(
            "# 该级简码覆盖 {} 字（含固定简码 {} 字）\n",
            per_level[li].len() + fixed_count,
            fixed_count
        ));
    }

    // 无归属级别的固定简码（需求 21.6/21.10）：单列一段输出，不遗漏。
    let no_level: Vec<&crate::types::FixedSimpleCode> = ctx
        .simple_fixed_codes
        .iter()
        .filter(|fc| fc.li.is_none())
        .collect();
    if !no_level.is_empty() {
        out.push_str("\n# === 无归属级别的固定简码（仍输出、不参与退火） ===\n");
        out.push_str("# 汉字\t简码\t字频\n");
        for fc in no_level {
            let ch = ctx.raw_splits[fc.ci].0;
            let freq = ctx.char_infos[fc.ci].frequency;
            out.push_str(&format!("{}\t{}\t{}\n", ch, fc.code_str, freq));
        }
    }

    crate::fsutil::write_with_backup(format!("{}/output-simple-codes.txt", dir), out).unwrap();
}

/// 保存线程结果
pub fn save_thread_results(
    ctx: &OptContext,
    assignment: &[u8],
    score: f64,
    metrics: &Metrics,
    simple_metrics: &SimpleMetrics,
    thread_id: usize,
    output_dir: &str,
    root_usage: &HashMap<String, u64>,
) {
    let thread_dir = format!("{}/thread-{:02}", output_dir, thread_id);
    fs::create_dir_all(&thread_dir).expect("无法创建线程输出目录");

    let mut root_out = String::new();
    root_out.push_str(&format!("# 线程: {}\n", thread_id));
    root_out.push_str(&format!("# 综合得分: {:.4}\n", score));
    root_out.push_str(&format!("# 重码数: {}\n", metrics.collision_count));
    root_out.push_str(&format!(
        "# 重码率: {:.6}%\n",
        metrics.collision_rate * 100.0
    ));
    root_out.push_str(&format!("# 加权键均当量: {:.4}\n", metrics.equiv_mean));
    root_out.push_str(&format!("# 当量变异系数(CV): {:.4}\n", metrics.equiv_cv));
    root_out.push_str(&format!(
        "# 用指分布偏差(L2): {:.4}\n",
        metrics.dist_deviation
    ));
    if ctx.enable_simple_code {
        root_out.push_str(&format!(
            "# 简码覆盖频率: {:.4}%\n",
            simple_metrics.weighted_freq_coverage * 100.0
        ));
        root_out.push_str(&format!(
            "# 简码加权当量: {:.4}\n",
            simple_metrics.equiv_mean
        ));
        root_out.push_str(&format!(
            "# 简码分布偏差: {:.4}\n",
            simple_metrics.dist_deviation
        ));
        root_out.push_str(&format!(
            "# 简码重码数: {}\n",
            simple_metrics.collision_count
        ));
        root_out.push_str(&format!(
            "# 简码重码率: {:.6}%\n",
            simple_metrics.collision_rate * 100.0
        ));
    }
    root_out.push_str("#\n");
    root_out.push_str("# 格式: 字根名 [tab] 编码 [tab] 使用次数(频率加权)\n");
    root_out.push_str("#\n");

    write_keymap_output(
        &mut root_out,
        &ctx.fixed_roots,
        &ctx.groups,
        assignment,
        root_usage,
    );
    crate::fsutil::write_with_backup(format!("{}/output-keymap.txt", thread_dir), &root_out).unwrap();

    // 保存编码结果
    let mut root_to_key: HashMap<String, u8> = HashMap::new();
    for (root, &key) in &ctx.fixed_roots {
        root_to_key.insert(root.clone(), key);
    }
    for (gi, group) in ctx.groups.iter().enumerate() {
        let key = assignment[gi];
        for root in &group.roots {
            root_to_key.insert(root.clone(), key);
        }
    }

    let mut code_out = String::new();
    for (ch, roots, freq) in &ctx.raw_splits {
        let mut code_parts = Vec::new();
        for root in roots {
            if let Some(&key) = root_to_key.get(root) {
                code_parts.push(key_to_char(key));
            }
        }
        let code_str: String = code_parts.into_iter().collect();
        code_out.push_str(&format!("{}\t{}\t{}\n", ch, code_str, freq));
    }
    crate::fsutil::write_with_backup(format!("{}/output-encode.txt", thread_dir), code_out).unwrap();

    save_key_distribution_to_dir(ctx, assignment, &thread_dir);
    save_equiv_distribution_to_dir(ctx, assignment, &thread_dir);
    save_simple_code_output(ctx, assignment, &thread_dir);
    save_combined_code_output(ctx, assignment, &thread_dir);
}

/// 保存键位分布到目录
pub fn save_key_distribution_to_dir(ctx: &OptContext, assignment: &[u8], dir: &str) {
    let display_order = &ctx.weights; // 从 ctx 获取显示顺序需要额外存储，暂时使用默认
    let evaluator = crate::evaluator::Evaluator::new(ctx, assignment);
    let mut out = String::new();
    out.push_str("# 用指分布统计\n");
    out.push_str("# 键位\t实际%\t目标%\t偏差\t偏差²\n");

    // 使用固定的显示顺序
    let display_order = "qwertyuiopasdfghjklzxcvbnm";
    for kc in display_order.chars() {
        if let Some(ki) = crate::types::char_to_key_index(kc) {
            if ki >= 31 {
                continue;
            }
            let actual = evaluator.key_weighted_usage[ki] * 100.0 * evaluator.inv_total_key_presses;
            let target = ctx.key_dist_config[ki].target_rate;
            let diff = actual - target;
            out.push_str(&format!(
                "{}\t{:.4}\t{:.4}\t{:+.4}\t{:.4}\n",
                kc,
                actual,
                target,
                diff,
                diff * diff
            ));
        }
    }

    let order_set: HashSet<char> = display_order.chars().collect();
    let special_keys = [('_', 26usize), (';', 27), (',', 28), ('.', 29), ('/', 30)];
    for (kc, ki) in &special_keys {
        if !order_set.contains(kc) && *ki < 31 {
            let usage = evaluator.key_weighted_usage[*ki];
            if usage > 0.0 {
                let actual = usage * 100.0 * evaluator.inv_total_key_presses;
                let target = ctx.key_dist_config[*ki].target_rate;
                let diff = actual - target;
                out.push_str(&format!(
                    "{}\t{:.4}\t{:.4}\t{:+.4}\t{:.4}\n",
                    kc,
                    actual,
                    target,
                    diff,
                    diff * diff
                ));
            }
        }
    }

    crate::fsutil::write_with_backup(format!("{}/output-distribution.txt", dir), out).unwrap();
}

/// 保存当量分布到目录
pub fn save_equiv_distribution_to_dir(ctx: &OptContext, assignment: &[u8], dir: &str) {
    let evaluator = crate::evaluator::Evaluator::new(ctx, assignment);
    let m = evaluator.get_metrics(ctx);

    let mut char_equivs: Vec<(char, f64, u64)> = Vec::new();
    for (i, (ch, _, _)) in ctx.raw_splits.iter().enumerate() {
        let equiv = ctx.calc_equiv_from_parts(i, assignment);
        char_equivs.push((*ch, equiv, ctx.char_infos[i].frequency));
    }
    char_equivs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    let mut out = String::new();
    out.push_str("# 当量分布统计\n");
    out.push_str(&format!("# 平均当量: {:.4}\n", m.equiv_mean));
    out.push_str(&format!("# 变异系数(CV): {:.4}\n", m.equiv_cv));
    out.push_str(&format!("# 标准差: {:.4}\n", m.equiv_cv * m.equiv_mean));
    out.push_str("#\n# 当量最高的20个高频字 (字频>1000000):\n# 汉字\t当量\t字频\n");

    let mut count = 0;
    for (ch, eq, freq) in &char_equivs {
        if *freq > 1_000_000 && count < 20 {
            out.push_str(&format!("{}\t{:.4}\t{}\n", ch, eq, freq));
            count += 1;
        }
    }

    out.push_str("#\n# 当量最低的20个高频字 (字频>1000000):\n");
    let high: Vec<_> = char_equivs
        .iter()
        .filter(|(_, _, f)| *f > 1_000_000)
        .collect();
    let start = high.len().saturating_sub(20);
    for (ch, eq, freq) in high.iter().skip(start) {
        out.push_str(&format!("{}\t{:.4}\t{}\n", ch, eq, freq));
    }

    crate::fsutil::write_with_backup(format!("{}/output-equiv-dist.txt", dir), out).unwrap();
}

/// 保存结果到输出目录
pub fn save_results(
    ctx: &OptContext,
    assignment: &[u8],
    score: f64,
    metrics: &Metrics,
    simple_metrics: &SimpleMetrics,
    output_dir: &str,
    root_usage: &HashMap<String, u64>,
) {
    let mut root_out = String::new();
    root_out.push_str(&format!("# 综合得分: {:.4}\n", score));
    root_out.push_str(&format!("# 重码数: {}\n", metrics.collision_count));
    root_out.push_str(&format!(
        "# 重码率: {:.6}%\n",
        metrics.collision_rate * 100.0
    ));
    root_out.push_str(&format!("# 加权键均当量: {:.4}\n", metrics.equiv_mean));
    root_out.push_str(&format!("# 当量变异系数(CV): {:.4}\n", metrics.equiv_cv));
    root_out.push_str(&format!(
        "# 用指分布偏差(L2): {:.4}\n",
        metrics.dist_deviation
    ));
    if ctx.enable_simple_code {
        root_out.push_str(&format!(
            "# 简码覆盖频率: {:.4}%\n",
            simple_metrics.weighted_freq_coverage * 100.0
        ));
        root_out.push_str(&format!(
            "# 简码加权当量: {:.4}\n",
            simple_metrics.equiv_mean
        ));
        root_out.push_str(&format!(
            "# 简码分布偏差: {:.4}\n",
            simple_metrics.dist_deviation
        ));
        root_out.push_str(&format!(
            "# 简码重码数: {}\n",
            simple_metrics.collision_count
        ));
        root_out.push_str(&format!(
            "# 简码重码率: {:.6}%\n",
            simple_metrics.collision_rate * 100.0
        ));
    }
    root_out.push_str("#\n");
    root_out.push_str("# 格式: 字根名 [tab] 编码 [tab] 使用次数(频率加权)\n");
    root_out.push_str("#\n");

    write_keymap_output(
        &mut root_out,
        &ctx.fixed_roots,
        &ctx.groups,
        assignment,
        root_usage,
    );
    crate::fsutil::write_with_backup(format!("{}/output-keymap.txt", output_dir), &root_out).unwrap();

    // 保存编码结果
    let mut root_to_key: HashMap<String, u8> = HashMap::new();
    for (root, &key) in &ctx.fixed_roots {
        root_to_key.insert(root.clone(), key);
    }
    for (gi, group) in ctx.groups.iter().enumerate() {
        let key = assignment[gi];
        for root in &group.roots {
            root_to_key.insert(root.clone(), key);
        }
    }

    let mut code_out = String::new();
    for (ch, roots, freq) in &ctx.raw_splits {
        let mut code_parts = Vec::new();
        for root in roots {
            if let Some(&key) = root_to_key.get(root) {
                code_parts.push(key_to_char(key));
            }
        }
        let code_str: String = code_parts.into_iter().collect();
        code_out.push_str(&format!("{}\t{}\t{}\n", ch, code_str, freq));
    }
    crate::fsutil::write_with_backup(format!("{}/output-encode.txt", output_dir), code_out).unwrap();

    save_key_distribution_to_dir(ctx, assignment, output_dir);
    save_equiv_distribution_to_dir(ctx, assignment, output_dir);
    save_simple_code_output(ctx, assignment, output_dir);
    save_combined_code_output(ctx, assignment, output_dir);

    println!(
        "结果已保存至 {}/output-keymap.txt, output-encode.txt, output-simple-codes.txt, output-combined.txt 等",
        output_dir
    );
}

/// 保存汇总信息
pub fn save_summary(
    cfg: &Config,
    results: &[(usize, Vec<u8>, f64, Metrics, SimpleMetrics)],
    best_thread: usize,
    output_dir: &str,
    elapsed: std::time::Duration,
) {
    let mut summary = String::new();
    summary.push_str("# 优化结果汇总\n");
    summary.push_str(&format!("# 输出目录: {}\n", output_dir));
    summary.push_str(&format!("# 线程数: {}\n", cfg.annealing.threads));
    summary.push_str(&format!("# 总步数: {}\n", cfg.annealing.total_steps));
    summary.push_str(&format!("# 总耗时: {:?}\n", elapsed));
    summary.push_str(&format!("# 最优线程: {}\n", best_thread));
    summary.push_str(&format!("# 简码优化: {}\n", cfg.weights.simple_code.enabled));
    summary.push_str("#\n");

    if cfg.weights.simple_code.enabled {
        summary.push_str(&format!(
            "{:<8} {:<12} {:<10} {:<12} {:<10} {:<10} {:<12} {:<12} {:<10} {:<10} {:<10} {:<12}\n",
            "线程",
            "得分",
            "重码数",
            "重码率%",
            "当量",
            "CV",
            "分布偏差",
            "简码覆盖%",
            "简码当量",
            "简码分布",
            "简码重码",
            "简码重码率%"
        ));
        summary.push_str(&format!("{}\n", "-".repeat(140)));
    } else {
        summary.push_str(&format!(
            "{:<8} {:<12} {:<10} {:<12} {:<10} {:<10} {:<12}\n",
            "线程", "得分", "重码数", "重码率%", "当量", "CV", "分布偏差"
        ));
        summary.push_str(&format!("{}\n", "-".repeat(80)));
    }

    // 按得分排序
    let mut sorted: Vec<&(usize, Vec<u8>, f64, Metrics, SimpleMetrics)> = results.iter().collect();
    sorted.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap());

    for (tid, _, score, m, sm) in &sorted {
        let marker = if *tid == best_thread { " 🏆" } else { "" };
        if cfg.weights.simple_code.enabled {
            summary.push_str(&format!(
                "T{:<7} {:<12.4} {:<10} {:<12.6} {:<10.4} {:<10.4} {:<12.4} {:<12.4} {:<10.4} {:<10.4} {:<10} {:<12.6}{}\n",
                tid, score, m.collision_count, m.collision_rate * 100.0,
                m.equiv_mean, m.equiv_cv, m.dist_deviation,
                sm.weighted_freq_coverage * 100.0, sm.equiv_mean, sm.dist_deviation,
                sm.collision_count, sm.collision_rate * 100.0,
                marker
            ));
        } else {
            summary.push_str(&format!(
                "T{:<7} {:<12.4} {:<10} {:<12.6} {:<10.4} {:<10.4} {:<12.4}{}\n",
                tid,
                score,
                m.collision_count,
                m.collision_rate * 100.0,
                m.equiv_mean,
                m.equiv_cv,
                m.dist_deviation,
                marker
            ));
        }
    }

    crate::fsutil::write_with_backup(format!("{}/summary.txt", output_dir), summary).unwrap();
}

// =========================================================================
// 🧪 空格上屏输出表示属性测试（simple-code-perf-optimization, Property 17）
// =========================================================================
#[cfg(test)]
mod space_commit_output_tests {
    use super::*;
    use crate::config::TargetsConfig;
    use crate::context::OptContext;
    use crate::types::{
        KeyDistConfig, RootGroup, ScaleConfig, SimpleCodeConfig, SimpleCodeLevel, SimpleCodeStep,
        WeightConfig, EQUIV_TABLE_SIZE,
    };
    use proptest::prelude::*;
    use std::collections::HashMap;

    /// 构建单级简码（步数 1，规则 [A.a]）的 OptContext，可指定该级 space_commit。
    /// 每个汉字含 3 个互不相同字根（各自成组），故全码长度 3 > 有效简码长度（1 或 2），
    /// 在 space true/false 下均满足长度资格（需求 22），保证有字出简。
    fn make_ctx_single_level(freqs: &[u64], space_commit: bool) -> OptContext {
        let mut groups: Vec<RootGroup> = Vec::new();
        let mut splits: Vec<(char, Vec<String>, u64)> = Vec::with_capacity(freqs.len());
        for (i, &freq) in freqs.iter().enumerate() {
            let mut roots: Vec<String> = Vec::with_capacity(3);
            for j in 0..3usize {
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
        let levels = vec![crate::types::SimpleCodeLevel::with_space_commit(
            1,
            1,
            vec![vec![step('A')]],
            space_commit,
        )];
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

    /// 收集 output-simple-codes.txt 中的数据行简码列（跳过注释/空行）。
    fn read_simple_codes(path: &str) -> Vec<String> {
        let content = fs::read_to_string(path).expect("读取简码输出文件失败");
        let mut codes = Vec::new();
        for line in content.lines() {
            let t = line.trim();
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            let cols: Vec<&str> = line.split('\t').collect();
            // 数据行格式：汉字\t简码\t字频
            if cols.len() >= 3 {
                codes.push(cols[1].to_string());
            }
        }
        codes
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        // Feature: simple-code-perf-optimization, Property 17: 空格上屏的输出表示
        //
        // 对任意简码级别 li 与该级被出简的字，其输出到 output 文件的简码字符串：当 space_commit
        // 为真时恰为「键位串 + 单个尾随下划线 `_`」，当 space_commit 为假时恰为「键位串、无尾随下划线」。
        #[test]
        fn prop17_space_commit_output_underscore(
            freqs in prop::collection::vec(1u64..=200, 1..=5),
            space_commit in any::<bool>(),
            dir_seed in any::<u64>(),
        ) {
            let ctx = make_ctx_single_level(&freqs, space_commit);
            let asg = vec![0u8; ctx.num_groups];

            let dir = std::env::temp_dir()
                .join(format!("cg_prop17_{}_{}", std::process::id(), dir_seed));
            let dir_str = dir.to_str().unwrap().to_string();
            fs::create_dir_all(&dir).unwrap();

            save_simple_code_output(&ctx, &asg, &dir_str);

            let codes = read_simple_codes(&format!("{}/output-simple-codes.txt", dir_str));
            prop_assert!(!codes.is_empty(), "应至少有一个字出简");

            for code in &codes {
                // 末尾恰一个下划线 ⟺ space_commit 为真；其余字符不应含下划线
                let trailing = code.ends_with('_');
                prop_assert_eq!(trailing, space_commit,
                    "简码 {:?} 的尾随下划线应与 space_commit={} 一致", code, space_commit);
                let core = code.trim_end_matches('_');
                prop_assert!(!core.contains('_'),
                    "简码核心串不应含下划线: {:?}", code);
                if space_commit {
                    // 恰一个尾随下划线（核心串 + 单 '_'）
                    prop_assert_eq!(code.len(), core.len() + 1,
                        "space_commit=true 应恰有单个尾随下划线: {:?}", code);
                }
            }

            // 清理临时目录
            let _ = fs::remove_dir_all(&dir);
        }
    }
}

// =========================================================================
// 🧪 output 镜像评估器选择 + 固定占用回归测试（simple-code-perf-optimization, 需求 23 / Finding A）
// =========================================================================
#[cfg(test)]
mod output_selection_mirror_tests {
    use super::*;
    use crate::config::TargetsConfig;
    use crate::context::OptContext;
    use crate::types::{
        KeyDistConfig, RootGroup, ScaleConfig, SimpleAssignMode, SimpleCodeConfig, SimpleCodeLevel,
        SimpleCodeStep, WeightConfig, EQUIV_TABLE_SIZE,
    };
    use std::collections::HashMap;

    /// 单级简码（步数 1，规则 [A.a]）、每字 2 根（全码长 2 > 简码长 1）、允许键位 [0,1]，
    /// 可指定 code_num 与固定简码映射。assignment 全 0 时各字首根均解析为键 0，故所有候选字
    /// 的级别 0 简码落入同一桶（编码 = encode([0])），便于构造「固定占用 + 优化」竞争同桶。
    fn make_ctx(freqs: &[u64], code_num: usize, fixed: &[(char, String)]) -> OptContext {
        let mut groups: Vec<RootGroup> = Vec::new();
        let mut splits: Vec<(char, Vec<String>, u64)> = Vec::with_capacity(freqs.len());
        for (i, &f) in freqs.iter().enumerate() {
            let r0 = format!("r{i}_0");
            let r1 = format!("r{i}_1");
            groups.push(RootGroup { roots: vec![r0.clone()], allowed_keys: vec![0, 1] });
            groups.push(RootGroup { roots: vec![r1.clone()], allowed_keys: vec![0, 1] });
            let ch = char::from_u32(0x4e00 + i as u32).unwrap();
            splits.push((ch, vec![r0, r1], f));
        }
        let levels = vec![SimpleCodeLevel {
            level: 1,
            code_num,
            rule_candidates: vec![vec![SimpleCodeStep { root_selector: 'A', code_selector: 'a' }]],
            commit_keys: Vec::new(),
            commit_pref_last_left: Vec::new(),
            commit_pref_last_right: Vec::new(),
        }];
        let fixed_roots: HashMap<String, u8> = HashMap::new();
        let equiv_table = [[0.0f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];
        let key_dist = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];
        let mut weights = WeightConfig::default();
        weights.enable_simple_code = true;
        weights.simple_coverage_ratio = 1.0;
        weights.simple_assign_mode = SimpleAssignMode::Frequency;
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

    /// 对 evaluator_simple_selection 的结果，按 (级别, 简码桶) 统计优化出简数，断言
    /// 「优化出简 + 固定占用 ≤ code_num」恒成立（Finding A：output 不得超额出简）。
    fn assert_occupancy(ctx: &OptContext, asg: &[u8]) {
        let sel = evaluator_simple_selection(ctx, asg);
        let mut opt: HashMap<(usize, usize), usize> = HashMap::new();
        for (li, ci, _code_str) in &sel {
            // 固定字不应出现在优化选择中
            assert!(!ctx.simple_fixed_assigned[*ci], "固定字不应在优化出简中: ci={}", ci);
            let code = ctx.calc_simple_code(*ci, *li, asg).expect("出简字必有简码");
            *opt.entry((*li, code)).or_default() += 1;
        }
        for ((li, code), &cnt) in &opt {
            let occ = ctx.simple_fixed_occ(*li, *code);
            let cn = ctx.simple_config.levels[*li].code_num;
            assert!(cnt + occ <= cn,
                "级别 {} 桶 {}: 优化 {} + 固定 {} 超过 code_num {}", li, code, cnt, occ, cn);
        }
    }

    #[test]
    fn fixed_occupancy_caps_output_selection() {
        // 4 字（freq 100/90/80/70），assignment 全 0 ⟹ 级别 0 简码均落入同一桶。
        let freqs = [100u64, 90, 80, 70];
        // 固定简码：把「一」(0x4e00) 固定为 "a"(键 0)，占用该桶。
        let fixed = vec![('\u{4e00}', "a".to_string())];

        // code_num = 1：固定占满该桶 ⟹ 优化出简数为 0（output 不得再选字进同桶）。
        let ctx1 = make_ctx(&freqs, 1, &fixed);
        let asg = vec![0u8; ctx1.num_groups];
        assert_occupancy(&ctx1, &asg);
        let sel1 = evaluator_simple_selection(&ctx1, &asg);
        assert!(sel1.is_empty(), "code_num=1 且固定占满时不应有优化出简，实际 {:?}", sel1.len());

        // code_num = 2：固定占 1 ⟹ 优化出简恰 1。
        let ctx2 = make_ctx(&freqs, 2, &fixed);
        let asg2 = vec![0u8; ctx2.num_groups];
        assert_occupancy(&ctx2, &asg2);
        let sel2 = evaluator_simple_selection(&ctx2, &asg2);
        assert_eq!(sel2.len(), 1, "code_num=2 且固定占 1 时优化出简应恰为 1");

        // 无固定简码、code_num = 1：优化出简恰 1（基线对照）。
        let ctx0 = make_ctx(&freqs, 1, &[]);
        let asg0 = vec![0u8; ctx0.num_groups];
        assert_occupancy(&ctx0, &asg0);
        let sel0 = evaluator_simple_selection(&ctx0, &asg0);
        assert_eq!(sel0.len(), 1, "无固定简码、code_num=1 时优化出简应恰为 1");
    }
}
