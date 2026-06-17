// =========================================================================
// 🚀 字根编码优化器 - 主入口
// =========================================================================

use std::collections::HashMap;
use std::time::Instant;

use chrono::Local;
use clap::{Parser, Subcommand};
use rayon::prelude::*;

mod annealing;
mod bucket_store;
mod calibrate;
mod config;
mod context;
mod evaluator;
mod loader;
mod output;
mod schedule;
mod simple;
mod types;
mod validate;

use crate::annealing::simulated_annealing;
use crate::calibrate::calibrate_scales;
use crate::config::{Config, TargetsConfig};
use crate::context::OptContext;
use crate::evaluator::Evaluator;
use crate::output::{save_results, save_summary, save_thread_results};
use crate::types::{
    key_to_char, ScaleConfig, SimpleCodeConfig, SimpleMetrics, EQUIV_TABLE_SIZE, KeyDistConfig,
};

// =========================================================================
// CLI 定义
// =========================================================================

#[derive(Parser)]
#[command(name = "CodeGenie", about = "字根编码优化器", version)]
struct Cli {
    /// 配置文件路径
    #[arg(short = 'c', long, default_value = "config.toml")]
    config: String,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// 运行模拟退火优化（默认行为）
    Optimize,

    /// 根据 keymap 为汉字编码
    Encode {
        /// 汉字拆分元素表
        #[arg(short = 'd', long)]
        division: Option<String>,

        /// 逻辑字根映射文件（必需）
        #[arg(short = 'k', long)]
        keymap: String,

        /// 编码输出文件
        #[arg(short = 'o', long, default_value = "output-encode.txt")]
        output: String,
    },

    /// 全方位评估编码方案
    Evaluate {
        /// 汉字拆分元素表
        #[arg(short = 'd', long)]
        division: Option<String>,

        /// 逻辑字根映射文件（必需）
        #[arg(short = 'k', long)]
        keymap: String,

        /// 目标键位分布文件
        #[arg(long)]
        keydist: Option<String>,

        /// 当量数据文件
        #[arg(long)]
        equiv: Option<String>,

        /// 简码规则文件（可选）
        #[arg(long)]
        simple: Option<String>,

        /// 评估输出文件
        #[arg(short = 'o', long, default_value = "output-evaluate.txt")]
        output: String,
    },
}

// =========================================================================
// 主入口
// =========================================================================

fn main() {
    let cli = Cli::parse();

    // 加载配置
    let cfg = Config::load_from_path(&cli.config);

    match cli.command {
        Some(Commands::Encode {
            division,
            keymap,
            output,
        }) => {
            let division_path = division.as_deref().unwrap_or(&cfg.files.splits);
            run_encode(division_path, &keymap, &output);
        }
        Some(Commands::Evaluate {
            division,
            keymap,
            keydist,
            equiv,
            simple,
            output,
        }) => {
            let division_path = division.as_deref().unwrap_or(&cfg.files.splits);
            let keydist_path = keydist.as_deref().unwrap_or(&cfg.files.key_dist);
            let equiv_path = equiv.as_deref().unwrap_or(&cfg.files.pair_equiv);
            run_evaluate(
                &cfg,
                division_path,
                &keymap,
                keydist_path,
                equiv_path,
                simple.as_deref(),
                &output,
            );
        }
        Some(Commands::Optimize) | None => run_optimize(&cfg),
    }
}

// =========================================================================
// encode 子命令
// =========================================================================

fn run_encode(division_path: &str, keymap_path: &str, output_path: &str) {
    println!("=== CodeGenie 编码模式 ===");
    println!("  拆分表: {}", division_path);
    println!("  键位映射: {}", keymap_path);
    println!("  输出文件: {}", output_path);

    // 加载数据
    let root_to_key = loader::load_keymap(keymap_path, division_path);
    let splits = loader::load_splits(division_path);

    println!("  已加载 {} 个字根映射", root_to_key.len());
    println!("  已加载 {} 个汉字拆分", splits.len());

    // 为每个汉字编码
    let mut code_out = String::new();
    let mut missing_roots: HashMap<String, usize> = HashMap::new();
    let mut encoded_count = 0usize;
    let mut failed_count = 0usize;

    for (ch, roots, freq) in &splits {
        let mut code_parts = Vec::new();
        let mut all_found = true;

        for root in roots {
            if let Some(&key) = root_to_key.get(root) {
                code_parts.push(key_to_char(key));
            } else {
                all_found = false;
                *missing_roots.entry(root.clone()).or_default() += 1;
            }
        }

        if all_found && !code_parts.is_empty() {
            let code_str: String = code_parts.into_iter().collect();
            code_out.push_str(&format!("{}\t{}\t{}\n", ch, code_str, freq));
            encoded_count += 1;
        } else {
            // 即使有缺失字根，也输出已有部分（用 ? 标记缺失）
            let mut partial_code = Vec::new();
            for root in roots {
                if let Some(&key) = root_to_key.get(root) {
                    partial_code.push(key_to_char(key));
                } else {
                    partial_code.push('?');
                }
            }
            let code_str: String = partial_code.into_iter().collect();
            code_out.push_str(&format!("{}\t{}\t{}\n", ch, code_str, freq));
            failed_count += 1;
        }
    }

    // 写入文件
    std::fs::write(output_path, &code_out).expect("无法写入编码输出文件");

    println!("\n✅ 编码完成:");
    println!("  成功编码: {} 字", encoded_count);
    if failed_count > 0 {
        println!("  ⚠️ 部分编码: {} 字（存在未映射字根）", failed_count);
    }
    if !missing_roots.is_empty() {
        println!("  ⚠️ 未找到映射的字根 ({} 种):", missing_roots.len());
        let mut sorted: Vec<_> = missing_roots.into_iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1));
        for (root, count) in sorted.iter().take(20) {
            println!("    {} (出现 {} 次)", root, count);
        }
        if sorted.len() > 20 {
            println!("    ... 还有 {} 种", sorted.len() - 20);
        }
    }
    println!("  结果已保存至 {}", output_path);
}

// =========================================================================
// evaluate 子命令
// =========================================================================

fn run_evaluate(
    cfg: &Config,
    division_path: &str,
    keymap_path: &str,
    keydist_path: &str,
    equiv_path: &str,
    simple_path: Option<&str>,
    output_path: &str,
) {
    println!("=== CodeGenie 评估模式 ===");
    println!("  拆分表: {}", division_path);
    println!("  键位映射: {}", keymap_path);
    println!("  键位分布: {}", keydist_path);
    println!("  当量数据: {}", equiv_path);
    if let Some(sp) = simple_path {
        println!("  简码规则: {}", sp);
    }
    println!("  输出文件: {}", output_path);

    // 加载数据
    let root_to_key = loader::load_keymap(keymap_path, division_path);
    let splits = loader::load_splits(division_path);
    let equiv_table = loader::load_pair_equivalence(equiv_path);
    let key_dist_config = loader::load_key_distribution(keydist_path);

    println!("  已加载 {} 个字根映射", root_to_key.len());
    println!("  已加载 {} 个汉字拆分", splits.len());

    // 加载简码配置
    let simple_config = if let Some(sp) = simple_path {
        let scfg = simple::parse_simple_code_config(sp);
        println!("  已加载简码配置: {} 级", scfg.levels.len());
        scfg
    } else {
        cfg.get_simple_code_config()
    };

    // 将所有字根作为 fixed_roots，groups 为空
    let fixed_roots: HashMap<String, u8> = root_to_key;
    let groups: Vec<types::RootGroup> = vec![];
    let assignment: Vec<u8> = vec![];

    // 构建 OptContext
    let scale_config = types::ScaleConfig::default();
    let weights = cfg.get_weight_config();
    let ctx = OptContext::new_with_fixed(
        &splits,
        &fixed_roots,
        &groups,
        equiv_table,
        key_dist_config,
        scale_config,
        simple_config,
        weights,
        TargetsConfig::default(),
        &cfg.get_fixed_simple_codes(),
    );

    println!("  编码基数: {}", ctx.code_base);
    println!("  编码空间: {}", ctx.code_space);
    println!("  最大码长: {}", ctx.max_parts);

    // 运行评估
    let evaluator = Evaluator::new(&ctx, &assignment);
    let metrics = evaluator.get_metrics(&ctx);
    let simple_metrics = evaluator.get_simple_metrics(&ctx);

    // 打印评估结果
    println!("\n📊 评估结果:");
    println!("  ═══════════════════════════════════════");
    println!("  「全码」重码数:          {}", metrics.collision_count);
    println!(
        "  「全码」重码率:          {:.6}%",
        metrics.collision_rate * 100.0
    );
    println!(
        "  「全码」加权键均当量:    {:.4}",
        metrics.equiv_mean
    );
    println!(
        "  「全码」当量变异系数(CV): {:.4}",
        metrics.equiv_cv
    );
    println!(
        "  「全码」用指分布偏差(L2): {:.4}",
        metrics.dist_deviation
    );

    if !ctx.simple_config.levels.is_empty() {
        println!("  ─────────────────────────────────────");
        println!(
            "  「简码」重码数:          {}",
            simple_metrics.collision_count
        );
        println!(
            "  「简码」重码率:          {:.6}%",
            simple_metrics.collision_rate * 100.0
        );
        println!(
            "  「简码」覆盖率:          {:.4}%",
            simple_metrics.weighted_freq_coverage * 100.0
        );
        println!(
            "  「简码」加权当量:        {:.4}",
            simple_metrics.equiv_mean
        );
        println!(
            "  「简码」分布偏差:        {:.4}",
            simple_metrics.dist_deviation
        );
    }
    println!("  ═══════════════════════════════════════");

    // 生成详细评估报告
    let report = build_evaluate_report(
        cfg,
        &ctx,
        &assignment,
        &evaluator,
        &metrics,
        &simple_metrics,
        &key_dist_config,
    );

    std::fs::write(output_path, &report).expect("无法写入评估输出文件");
    println!("\n✅ 详细评估报告已保存至 {}", output_path);
}

/// 构建详细评估报告
fn build_evaluate_report(
    cfg: &Config,
    ctx: &OptContext,
    assignment: &[u8],
    evaluator: &Evaluator,
    metrics: &types::Metrics,
    simple_metrics: &SimpleMetrics,
    key_dist_config: &[KeyDistConfig; EQUIV_TABLE_SIZE],
) -> String {
    let mut out = String::new();

    // ===== 总览 =====
    out.push_str("# CodeGenie 编码方案评估报告\n");
    out.push_str("#\n");
    out.push_str(&format!("# 汉字数量: {}\n", ctx.char_infos.len()));
    out.push_str(&format!("# 总字频: {}\n", ctx.total_frequency));
    out.push_str(&format!("# 编码基数: {}\n", ctx.code_base));
    out.push_str(&format!("# 最大码长: {}\n", ctx.max_parts));
    out.push_str("#\n");

    // ===== 全码指标 =====
    out.push_str("# ═══════════════════════════════════════\n");
    out.push_str("# 全码指标\n");
    out.push_str("# ═══════════════════════════════════════\n");
    out.push_str(&format!("# 重码数: {}\n", metrics.collision_count));
    out.push_str(&format!(
        "# 重码率: {:.6}%\n",
        metrics.collision_rate * 100.0
    ));
    out.push_str(&format!(
        "# 加权键均当量: {:.4}\n",
        metrics.equiv_mean
    ));
    out.push_str(&format!(
        "# 当量变异系数(CV): {:.4}\n",
        metrics.equiv_cv
    ));
    out.push_str(&format!(
        "# 用指分布偏差(L2): {:.4}\n",
        metrics.dist_deviation
    ));
    out.push_str("#\n");

    // ===== 简码指标 =====
    if !ctx.simple_config.levels.is_empty() {
        out.push_str("# ═══════════════════════════════════════\n");
        out.push_str("# 简码指标\n");
        out.push_str("# ═══════════════════════════════════════\n");
        out.push_str(&format!(
            "# 简码重码数: {}\n",
            simple_metrics.collision_count
        ));
        out.push_str(&format!(
            "# 简码重码率: {:.6}%\n",
            simple_metrics.collision_rate * 100.0
        ));
        out.push_str(&format!(
            "# 简码覆盖率: {:.4}%\n",
            simple_metrics.weighted_freq_coverage * 100.0
        ));
        out.push_str(&format!(
            "# 简码加权当量: {:.4}\n",
            simple_metrics.equiv_mean
        ));
        out.push_str(&format!(
            "# 简码分布偏差: {:.4}\n",
            simple_metrics.dist_deviation
        ));
        out.push_str("#\n");
    }

    // ===== 用指分布 =====
    out.push_str("\n# ═══════════════════════════════════════\n");
    out.push_str("# 用指分布\n");
    out.push_str("# ═══════════════════════════════════════\n");
    out.push_str("# 键位\t实际%\t目标%\t偏差\t偏差²\n");

    let inv_tkp = evaluator.inv_total_key_presses;
    for kc in cfg.keys.display_order.chars() {
        if let Some(ki) = types::char_to_key_index(kc) {
            if ki >= 31 {
                continue;
            }
            let actual = evaluator.key_weighted_usage[ki] * 100.0 * inv_tkp;
            let target = key_dist_config[ki].target_rate;
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

    // 特殊键位
    let order_set: std::collections::HashSet<char> = cfg.keys.display_order.chars().collect();
    let special_keys = [('_', 26usize), (';', 27), (',', 28), ('.', 29), ('/', 30)];
    for (kc, ki) in &special_keys {
        if !order_set.contains(kc) && *ki < 31 {
            let usage = evaluator.key_weighted_usage[*ki];
            if usage > 0.0 {
                let actual = usage * 100.0 * inv_tkp;
                let target = key_dist_config[*ki].target_rate;
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

    // ===== 当量分布 =====
    out.push_str("\n# ═══════════════════════════════════════\n");
    out.push_str("# 当量分布\n");
    out.push_str("# ═══════════════════════════════════════\n");
    out.push_str(&format!("# 平均当量: {:.4}\n", metrics.equiv_mean));
    out.push_str(&format!(
        "# 变异系数(CV): {:.4}\n",
        metrics.equiv_cv
    ));
    out.push_str(&format!(
        "# 标准差: {:.4}\n",
        metrics.equiv_cv * metrics.equiv_mean
    ));

    // 计算每个汉字的当量
    let mut char_equivs: Vec<(char, f64, u64)> = Vec::new();
    for (i, (ch, _, _)) in ctx.raw_splits.iter().enumerate() {
        let equiv = ctx.calc_equiv_from_parts(i, assignment);
        char_equivs.push((*ch, equiv, ctx.char_infos[i].frequency));
    }
    char_equivs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

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

    // ===== 重码详情 =====
    out.push_str("\n# ═══════════════════════════════════════\n");
    out.push_str("# 重码详情\n");
    out.push_str("# ═══════════════════════════════════════\n");

    // 构建编码到汉字的映射
    let n = ctx.char_infos.len();
    let mut code_to_chars: HashMap<usize, Vec<usize>> = HashMap::new();
    for ci in 0..n {
        let code = ctx.calc_code_only(ci, assignment);
        code_to_chars.entry(code).or_default().push(ci);
    }

    // 收集重码组
    let mut collision_groups: Vec<(usize, Vec<(char, u64)>)> = Vec::new();
    for (code, chars) in &code_to_chars {
        if chars.len() >= 2 {
            let mut group: Vec<(char, u64)> = chars
                .iter()
                .map(|&ci| (ctx.raw_splits[ci].0, ctx.char_infos[ci].frequency))
                .collect();
            group.sort_by(|a, b| b.1.cmp(&a.1));
            collision_groups.push((*code, group));
        }
    }

    // 按组内最高频率降序排序
    collision_groups.sort_by(|a, b| {
        let max_a = a.1.first().map(|x| x.1).unwrap_or(0);
        let max_b = b.1.first().map(|x| x.1).unwrap_or(0);
        max_b.cmp(&max_a)
    });

    out.push_str(&format!(
        "# 共 {} 组重码 (按最高频率降序)\n",
        collision_groups.len()
    ));
    out.push_str("# 编码\t重码字\t字频列表\n");

    for (_, group) in collision_groups.iter().take(200) {
        let chars_str: String = group.iter().map(|(ch, _)| *ch).collect();
        let freqs_str: String = group
            .iter()
            .map(|(_, f)| f.to_string())
            .collect::<Vec<_>>()
            .join(",");

        // 获取编码字符串
        let first_ci = code_to_chars
            .values()
            .find(|v| v.len() >= 2 && ctx.raw_splits[v[0]].0 == group[0].0)
            .and_then(|v| v.first())
            .copied();

        let code_str = if let Some(ci) = first_ci {
            let info = &ctx.char_infos[ci];
            let keys: String = info
                .parts
                .iter()
                .map(|&p| key_to_char(ctx.resolve_key(p, assignment)))
                .collect();
            keys
        } else {
            "?".to_string()
        };

        out.push_str(&format!("{}\t{}\t{}\n", code_str, chars_str, freqs_str));
    }

    if collision_groups.len() > 200 {
        out.push_str(&format!(
            "# ... 还有 {} 组重码未列出\n",
            collision_groups.len() - 200
        ));
    }

    out
}

// =========================================================================
// optimize 子命令（原有优化流程）
// =========================================================================

/// 根据配置决定最终使用的 ScaleConfig 及其来源说明
/// - cfg.scale 为 None：完全依赖 calibrate，返回 calibrated 值
/// - cfg.scale 所有必要字段均为 Some：跳过 calibrate，直接使用手动配置
///   （简码未启用时，简码相关字段不计入"必要"）
/// - cfg.scale 部分字段为 Some：先用 calibrated 值，再用 Some 字段覆盖
///
/// 注意：[scale] 中配置的值与 targets 中的指标同量纲（即"典型值"），
/// 读入时取倒数转换为 ScaleConfig 内部使用的缩放因子（scale = 1/典型值）
fn resolve_scale_config(cfg: &Config, calibrated: ScaleConfig) -> (ScaleConfig, &'static str) {
    let simple_enabled = cfg.weights.simple_code.enabled;

    match &cfg.scale {
        None => (calibrated, "自动校准"),
        Some(toml) => {
            // 全码 5 个字段必须全部设置
            let full_code_set = toml.collision_count.is_some()
                && toml.collision_rate.is_some()
                && toml.equivalence.is_some()
                && toml.equiv_cv.is_some()
                && toml.distribution.is_some();

            // 简码字段：仅在 simple_code.enabled = true 时才要求设置
            let simple_code_set = !simple_enabled || (
                toml.simple_freq.is_some()
                    && toml.simple_equiv.is_some()
                    && toml.simple_dist.is_some()
                    && toml.simple_collision_count.is_some()
                    && toml.simple_collision_rate.is_some()
            );

            let all_set = full_code_set && simple_code_set;

            // 辅助闭包：将"典型值"转换为缩放因子（取倒数），0 值保护
            let to_scale = |v: f64| if v > 0.0 { 1.0 / v } else { 1.0 };

            if all_set {
                let sc = ScaleConfig {
                    collision_count:        to_scale(toml.collision_count.unwrap()),
                    collision_rate:         to_scale(toml.collision_rate.unwrap()),
                    equivalence:            to_scale(toml.equivalence.unwrap()),
                    equiv_cv:               to_scale(toml.equiv_cv.unwrap()),
                    distribution:           to_scale(toml.distribution.unwrap()),
                    simple_freq:            toml.simple_freq.map(to_scale).unwrap_or(calibrated.simple_freq),
                    simple_equiv:           toml.simple_equiv.map(to_scale).unwrap_or(calibrated.simple_equiv),
                    simple_dist:            toml.simple_dist.map(to_scale).unwrap_or(calibrated.simple_dist),
                    simple_collision_count: toml.simple_collision_count.map(to_scale).unwrap_or(calibrated.simple_collision_count),
                    simple_collision_rate:  toml.simple_collision_rate.map(to_scale).unwrap_or(calibrated.simple_collision_rate),
                };
                (sc, "手动配置")
            } else {
                // 部分覆盖：以 calibrated 为基础，用手动值（取倒数后）覆盖
                let mut sc = calibrated;
                if let Some(v) = toml.collision_count        { sc.collision_count = to_scale(v); }
                if let Some(v) = toml.collision_rate         { sc.collision_rate = to_scale(v); }
                if let Some(v) = toml.equivalence            { sc.equivalence = to_scale(v); }
                if let Some(v) = toml.equiv_cv               { sc.equiv_cv = to_scale(v); }
                if let Some(v) = toml.distribution           { sc.distribution = to_scale(v); }
                if let Some(v) = toml.simple_freq            { sc.simple_freq = to_scale(v); }
                if let Some(v) = toml.simple_equiv           { sc.simple_equiv = to_scale(v); }
                if let Some(v) = toml.simple_dist            { sc.simple_dist = to_scale(v); }
                if let Some(v) = toml.simple_collision_count { sc.simple_collision_count = to_scale(v); }
                if let Some(v) = toml.simple_collision_rate  { sc.simple_collision_rate = to_scale(v); }
                (sc, "部分手动覆盖")
            }
        }
    }
}

fn run_optimize(cfg: &Config) {
    let start_time = Instant::now();
    println!("=== CodeGenie 码灵算法优化器 v10 ===");

    // 验证配置
    cfg.validate_weights();

    // 打印配置信息
    println!(
        "线程数: {}, 总步数: {}",
        cfg.annealing.threads, cfg.annealing.total_steps
    );
    println!(
        "初始温度: {}, 结束温度: {}",
        cfg.annealing.temp_start, cfg.annealing.temp_end
    );
    println!("全局允许键位: {}", cfg.keys.allowed);
    println!(
        "全码权重: 重码数={:.2}, 重码率={:.2}, 当量={:.2}, CV={:.2}, 分布={:.2}",
        cfg.weights.full_code.collision_count,
        cfg.weights.full_code.collision_rate,
        cfg.weights.full_code.equivalence,
        cfg.weights.full_code.equiv_cv,
        cfg.weights.full_code.distribution,
    );
    println!(
        "简码优化: {} (全码占比={:.0}%, 简码占比={:.0}%)",
        if cfg.weights.simple_code.enabled {
            "开启"
        } else {
            "关闭"
        },
        cfg.weights.simple_code.full_code_weight * 100.0,
        cfg.weights.simple_code.simple_code_weight * 100.0
    );
    if cfg.weights.simple_code.enabled {
        println!(
            "简码子权重: 频率覆盖={:.2}, 当量={:.2}, 分布={:.2}, 重码数={:.2}, 重码率={:.2}",
            cfg.weights.simple_code.freq,
            cfg.weights.simple_code.equiv,
            cfg.weights.simple_code.dist,
            cfg.weights.simple_code.collision_count,
            cfg.weights.simple_code.collision_rate,
        );
    }
    println!("用指分布输出顺序: {}", cfg.keys.display_order);

    // 创建输出目录
    let timestamp = Local::now().format("%Y%m%d-%H%M%S").to_string();
    let output_dir = format!("output-{}", timestamp);
    std::fs::create_dir_all(&output_dir).expect("无法创建输出目录");
    println!("输出目录: {}", output_dir);

    // ==================== 加载数据 ====================
    let (fixed_roots, constrained) = loader::load_fixed(&cfg.files.fixed);
    let dynamic_groups = loader::load_dynamic(&cfg.files.dynamic, &constrained, &cfg.keys.allowed);
    let splits = loader::load_splits(&cfg.files.splits);
    let equiv_table = loader::load_pair_equivalence(&cfg.files.pair_equiv);
    let key_dist_config = loader::load_key_distribution(&cfg.files.key_dist);

    // 加载简码配置
    let simple_config = if cfg.weights.simple_code.enabled {
        let scfg = cfg.get_simple_code_config();
        println!("\n📋 简码配置:");
        for level in &scfg.levels {
            let rules_str: String = level
                .rule_candidates
                .iter()
                .map(|rule| {
                    rule.iter()
                        .map(|s| format!("{}{}", s.root_selector, s.code_selector))
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join(" | ");
            println!(
                "  - {}级简码: 每位{}字, 规则: {}",
                level.level, level.code_num, rules_str
            );
        }
        scfg
    } else {
        SimpleCodeConfig { levels: vec![] }
    };

    // 打印数据统计
    let max_parts_in_data = splits.iter().map(|(_, r, _)| r.len()).max().unwrap_or(0);
    let total_roots: usize = dynamic_groups.iter().map(|g| g.roots.len()).sum();
    let total_freq: u64 = splits.iter().map(|(_, _, f)| f).sum();

    println!("\n数据加载完毕:");
    println!("  - 固定字根(单键): {}", fixed_roots.len());
    println!("  - 受限字根组(多键): {} 组", constrained.len());
    println!(
        "  - 动态字根组: {} 组 (共 {} 字根)",
        dynamic_groups.len(),
        total_roots
    );
    println!("  - 汉字数量: {}", splits.len());
    println!("  - 总字频: {}", total_freq);
    println!("  - 最大码长: {}", max_parts_in_data);

    // 警告：最大码长超过配置
    if max_parts_in_data > cfg.annealing.max_parts {
        println!(
            "⚠️ 拆分表中最大码长({})超过 max_parts({}), 请调大配置",
            max_parts_in_data, cfg.annealing.max_parts
        );
    }

    // ==================== 校验 ====================
    if !validate::check_validation(&splits, &fixed_roots, &dynamic_groups) {
        std::process::exit(1);
    }

    // ==================== 初始校准 ====================
    // 判断是否需要 calibrate
    // 全码 5 个字段必须全部设置；简码字段仅在 simple_code.enabled = true 时才要求
    let full_code_scale_set = cfg.scale.as_ref().map_or(false, |s| {
        s.collision_count.is_some()
            && s.collision_rate.is_some()
            && s.equivalence.is_some()
            && s.equiv_cv.is_some()
            && s.distribution.is_some()
    });
    let simple_code_scale_set = !cfg.weights.simple_code.enabled
        || cfg.scale.as_ref().map_or(false, |s| {
            s.simple_freq.is_some()
                && s.simple_equiv.is_some()
                && s.simple_dist.is_some()
                && s.simple_collision_count.is_some()
                && s.simple_collision_rate.is_some()
        });
    let all_manual = full_code_scale_set && simple_code_scale_set;

    let weights = cfg.get_weight_config();
    let (scale_config, scale_source) = if all_manual {
        println!("\n📐 ScaleConfig 已全部手动配置，跳过自动校准...");
        resolve_scale_config(cfg, types::ScaleConfig::default())
    } else {
        println!("\n📐 正在进行初始尺度校准...");
        let temp_scale = types::ScaleConfig::default();
        let temp_ctx = OptContext::new_with_fixed(
            &splits,
            &fixed_roots,
            &dynamic_groups,
            equiv_table,
            key_dist_config,
            temp_scale,
            simple_config.clone(),
            weights,
            TargetsConfig::default(),
            &cfg.get_fixed_simple_codes(),
        );

        let initial_assignment = annealing::smart_init(&temp_ctx, cfg);
        let initial_eval = Evaluator::new(&temp_ctx, &initial_assignment);
        let initial_metrics = initial_eval.get_metrics(&temp_ctx);
        let initial_simple_metrics = initial_eval.get_simple_metrics(&temp_ctx);

        println!("  初始状态观测:");
        println!(
            "    重码数: {},  重码率: {:.6}",
            initial_metrics.collision_count, initial_metrics.collision_rate
        );
        println!(
            "    当量: {:.4},  CV: {:.4},  分布偏差: {:.4}",
            initial_metrics.equiv_mean, initial_metrics.equiv_cv, initial_metrics.dist_deviation
        );
        if cfg.weights.simple_code.enabled {
            println!(
                "    简码覆盖: {:.4}%,  简码当量: {:.4},  简码分布: {:.4}",
                initial_simple_metrics.weighted_freq_coverage * 100.0,
                initial_simple_metrics.equiv_mean,
                initial_simple_metrics.dist_deviation
            );
            println!(
                "    简码重码数: {},  简码重码率: {:.6}%",
                initial_simple_metrics.collision_count,
                initial_simple_metrics.collision_rate * 100.0
            );
        }

        // 逻辑根验证（仅在启用简码时）
        if cfg.weights.simple_code.enabled {
            println!("\n  📝 逻辑根解析验证 (前3字):");
            for ci in 0..3.min(temp_ctx.raw_splits.len()) {
                let (ch, roots, _) = &temp_ctx.raw_splits[ci];
                let si = &temp_ctx.char_simple_infos[ci];
                println!("    '{}' 拆分: {:?}", ch, roots);
                for (ri, lr) in si.logical_roots.iter().enumerate() {
                    let full_keys: Vec<char> = lr
                        .full_code_parts
                        .iter()
                        .map(|&p| key_to_char(temp_ctx.resolve_key(p, &initial_assignment)))
                        .collect();
                    println!(
                        "      逻辑根[{}] '{}': 拆分中占位={:?}, 完整编码={:?}",
                        ri, lr.base_name, lr.split_part_indices, full_keys
                    );
                }
                for (li, instr) in si.level_instructions.iter().enumerate() {
                    if let Some(ref steps) = instr {
                        let keys: Vec<char> = steps
                            .iter()
                            .map(|&(root_idx, code_idx)| {
                                let lr = &si.logical_roots[root_idx];
                                let part = lr.full_code_parts[code_idx];
                                key_to_char(temp_ctx.resolve_key(part, &initial_assignment))
                            })
                            .collect();
                        let level_cfg = &temp_ctx.simple_config.levels[li];
                        let mut matched_rule_idx = 0;
                        for (ri, rule) in level_cfg.rule_candidates.iter().enumerate() {
                            if types::try_resolve_rule(
                                rule,
                                &si.logical_roots,
                                si.logical_roots.len(),
                            )
                            .is_some()
                            {
                                matched_rule_idx = ri;
                                break;
                            }
                        }
                        let rule_str: String = level_cfg.rule_candidates[matched_rule_idx]
                            .iter()
                            .map(|s| format!("{}{}", s.root_selector, s.code_selector))
                            .collect();
                        let all_rules_str: String = level_cfg
                            .rule_candidates
                            .iter()
                            .enumerate()
                            .map(|(i, rule)| {
                                let s: String = rule
                                    .iter()
                                    .map(|s| format!("{}{}", s.root_selector, s.code_selector))
                                    .collect();
                                if i == matched_rule_idx {
                                    format!("[{}]", s)
                                } else {
                                    s
                                }
                            })
                            .collect::<Vec<_>>()
                            .join(", ");
                        println!(
                            "      {}级简码(规则: {} 命中: {}): {:?}",
                            level_cfg.level, all_rules_str, rule_str, keys
                        );
                    } else {
                        let level_cfg = &temp_ctx.simple_config.levels[li];
                        let all_rules_str: String = level_cfg
                            .rule_candidates
                            .iter()
                            .map(|rule| {
                                rule.iter()
                                    .map(|s| format!("{}{}", s.root_selector, s.code_selector))
                                    .collect::<String>()
                            })
                            .collect::<Vec<_>>()
                            .join(", ");
                        println!(
                            "      {}级简码(规则: {}): 无合规候选",
                            level_cfg.level, all_rules_str
                        );
                    }
                }
            }
        }

        let calibrated = calibrate_scales(&initial_metrics, &initial_simple_metrics, &weights);
        resolve_scale_config(cfg, calibrated)
    };

    println!("  ScaleConfig 来源: {}", scale_source);
    println!("  校准尺度 (Scale):");
    println!("    CollisionCount: {:.6}", scale_config.collision_count);
    println!("    CollisionRate:  {:.6}", scale_config.collision_rate);
    println!("    Equivalence:    {:.6}", scale_config.equivalence);
    println!("    EquivCV:        {:.6}", scale_config.equiv_cv);
    println!("    Distribution:   {:.6}", scale_config.distribution);
    if cfg.weights.simple_code.enabled {
        println!("    SimpleFreq:     {:.6}", scale_config.simple_freq);
        println!("    SimpleEquiv:    {:.6}", scale_config.simple_equiv);
        println!("    SimpleDist:     {:.6}", scale_config.simple_dist);
        println!("    SimpleCollCnt:  {:.6}", scale_config.simple_collision_count);
        println!("    SimpleCollRate: {:.6}", scale_config.simple_collision_rate);
    }

    // ==================== 正式优化 ====================
    let equiv_table_2 = loader::load_pair_equivalence(&cfg.files.pair_equiv);
    let key_dist_config_2 = loader::load_key_distribution(&cfg.files.key_dist);

    let targets_config = cfg.get_targets_config();
    let ctx = OptContext::new_with_fixed(
        &splits,
        &fixed_roots,
        &dynamic_groups,
        equiv_table_2,
        key_dist_config_2,
        scale_config,
        simple_config,
        weights,
        targets_config,
        &cfg.get_fixed_simple_codes(),
    );

    println!("\n  - 编码基数: {}", ctx.code_base);
    println!("  - 编码空间: {}", ctx.code_space);

    // 桶存储后端选择日志（需求 12）：单线程、配置确认阶段输出一次。
    evaluator::log_bucket_backends(&ctx);

    let root_usage = output::count_root_usage(&ctx);

    // 并行执行模拟退火
    println!("\n🚀 开始优化...");
    let num_threads = cfg.annealing.threads;
    let results: Vec<(Vec<u8>, f64, types::Metrics, SimpleMetrics)> = (0..num_threads)
        .into_par_iter()
        .map(|i| simulated_annealing(&ctx, cfg, i))
        .collect();

    let all_results: Vec<(usize, Vec<u8>, f64, types::Metrics, SimpleMetrics)> = results
        .into_iter()
        .enumerate()
        .map(|(i, (a, s, m, sm))| (i, a, s, m, sm))
        .collect();

    // 找出最优结果
    let (best_thread, best_assignment, best_score, best_metrics, best_simple_metrics) = all_results
        .iter()
        .min_by(|a, b| a.2.partial_cmp(&b.2).unwrap())
        .map(|(tid, a, s, m, sm)| (*tid, a.clone(), *s, *m, *sm))
        .unwrap();

    let elapsed = start_time.elapsed();

    // 打印最优结果
    let m = best_metrics;
    let sm = best_simple_metrics;
    let best_eval = Evaluator::new(&ctx, &best_assignment);
    let best_scores = best_eval.get_metric_scores(&ctx);
    let simple_sub = best_eval.get_simple_metric_scores(&ctx);
    println!("\n=================================");
    println!("🏆 最优结果 (线程 {}):", best_thread);
    // 综合得分三分量（需求 28.4）：全码分量 = weight_full_code·total_full，
    // 简码分量 = weight_simple_code·total_simple。
    if cfg.weights.simple_code.enabled {
        let full_comp = ctx.weights.weight_full_code * best_scores.total_full;
        let simple_comp = ctx.weights.weight_simple_code * best_scores.total_simple;
        println!("   综合得分: {:.4} (全码:{:.4} 简码:{:.4})", best_score, full_comp, simple_comp);
    } else {
        println!("   综合得分: {:.4}", best_score);
    }
    println!("   「全码」重码数: {}  (分: {:.4})", m.collision_count, best_scores.collision_count);
    println!("   「全码」重码率: {:.6}%  (分: {:.4})", m.collision_rate * 100.0, best_scores.collision_rate);
    println!("   「全码」加权键均当量: {:.4}  (分: {:.4})", m.equiv_mean, best_scores.equivalence);
    println!("   「全码」当量变异系数(CV): {:.4}  (分: {:.4})", m.equiv_cv, best_scores.equiv_cv);
    println!("   「全码」用指分布偏差(L2): {:.4}  (分: {:.4})", m.dist_deviation, best_scores.distribution);
    if cfg.weights.simple_code.enabled {
        println!("---------------------------------");
        println!("   「简码」总分: {:.4}", simple_sub.total);
        println!("   「简码」重码数: {}  (分: {:.4})", sm.collision_count, simple_sub.collision_count);
        println!("   「简码」重码率: {:.6}%  (分: {:.4})", sm.collision_rate * 100.0, simple_sub.collision_rate);
        println!("   「简码」覆盖率: {:.4}%  (分: {:.4})", sm.weighted_freq_coverage * 100.0, simple_sub.freq);
        println!("   「简码」加权当量: {:.4}  (分: {:.4})", sm.equiv_mean, simple_sub.equiv);
        println!("   「简码」分布偏差: {:.4}  (分: {:.4})", sm.dist_deviation, simple_sub.dist);
    }
    println!("⏱️ 总耗时: {:?}", elapsed);
    println!("=================================");

    // ==================== 保存结果 ====================
    println!("\n📁 保存所有线程结果...");
    for (tid, assignment, score, metrics, smetrics) in &all_results {
        save_thread_results(
            &ctx,
            assignment,
            *score,
            metrics,
            smetrics,
            *tid,
            &output_dir,
            &root_usage,
        );
    }

    save_results(
        &ctx,
        &best_assignment,
        best_score,
        &best_metrics,
        &best_simple_metrics,
        &output_dir,
        &root_usage,
    );
    save_summary(cfg, &all_results, best_thread, &output_dir, elapsed);

    println!("\n所有结果已保存至 {}/", output_dir);
    println!("  - summary.txt              汇总排名");
    println!("  - output-*.txt             全局最优结果");
    println!("  - output-simple-codes.txt  简码分配");
    println!("  - thread-XX/               各线程结果");
}