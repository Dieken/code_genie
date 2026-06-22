// =========================================================================
// 🚀 字根编码优化器 - 主入口
// =========================================================================

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use chrono::Local;
use clap::{Parser, Subcommand};
use rayon::prelude::*;

mod annealing;
mod bucket_store;
mod calibrate;
mod checkpoint;
mod config;
mod context;
mod evaluator;
mod fsutil;
mod loader;
mod output;
mod schedule;
mod simple;
mod types;
mod validate;

use crate::annealing::{simulated_annealing_resumable, SaResult};
use crate::checkpoint::ThreadCheckpoint;
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
    Optimize {
        /// 输出目录：不存在则自动创建；存在则复用（允许非空），逐文件防覆盖（旧文件加时间戳归档）。
        /// 缺省时使用 output-{时间戳}
        #[arg(short = 'd', long = "output-dir")]
        output_dir: Option<String>,

        /// 从既有 output 目录播种：读取 {DIR}/thread-NN/output-keymap.txt 作为各线程初始解
        /// （按 thread-NN 编号轮询映射）。与 --seed-keymap 互斥。
        #[arg(long = "seed-dir")]
        seed_dir: Option<String>,

        /// 从单个 keymap 文件播种：所有退火线程使用同一初始解。与 --seed-dir 互斥。
        #[arg(long = "seed-keymap")]
        seed_keymap: Option<String>,
    },

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

    /// 从既有 output 目录的 checkpoint 断点续算
    Resume {
        /// 之前运行的 output 目录（含 inputs/ 与 checkpoint/）
        #[arg(short = 'd', long)]
        dir: String,
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
        Some(Commands::Resume { dir }) => run_resume(&dir),
        Some(Commands::Optimize {
            output_dir,
            seed_dir,
            seed_keymap,
        }) => run_optimize(&cfg, &cli.config, output_dir, seed_dir, seed_keymap),
        None => run_optimize(&cfg, &cli.config, None, None, None),
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

/// 线程→种子的轮询映射（需求 7）：线程 `thread_id` 使用第 `thread_id % k` 个种子。
/// `k` 为种子个数（调用方保证 `k >= 1`）。
fn seed_index(thread_id: usize, k: usize) -> usize {
    thread_id % k
}

/// 解析两个互斥的播种来源选项为「种子 keymap 文件路径」序列（需求 1/2/3）。
///
/// - 两者均为 `None`：返回空 `Vec`（不播种，保持基线行为）。
/// - 同时指定：返回 `Err`（互斥）。
/// - `--seed-keymap <FILE>`：校验文件存在 → 返回 `vec![FILE]`（K = 1）。
/// - `--seed-dir <DIR>`：枚举 `{DIR}/thread-{NN}/output-keymap.txt`（NN 为两位数字），
///   按 NN 升序收集；为空则 `Err`。仅触碰 thread-NN 子目录，不读 `inputs/`、`checkpoint/`。
fn resolve_seed_paths(
    seed_dir: Option<&str>,
    seed_keymap: Option<&str>,
) -> Result<Vec<String>, String> {
    match (seed_dir, seed_keymap) {
        (Some(_), Some(_)) => {
            Err("--seed-dir 与 --seed-keymap 互斥，不能同时指定".to_string())
        }
        (None, None) => Ok(Vec::new()),
        (None, Some(file)) => {
            if !std::path::Path::new(file).is_file() {
                return Err(format!("--seed-keymap 指定的文件不存在: {}", file));
            }
            Ok(vec![file.to_string()])
        }
        (Some(dir), None) => {
            let dir_path = std::path::Path::new(dir);
            if !dir_path.is_dir() {
                return Err(format!("--seed-dir 指定的目录不存在: {}", dir));
            }
            // 收集 (NN, keymap_path)，按 NN 升序
            let mut found: Vec<(u32, String)> = Vec::new();
            let entries = std::fs::read_dir(dir_path)
                .map_err(|e| format!("无法读取目录 {}: {}", dir, e))?;
            for entry in entries.flatten() {
                if !entry.path().is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                // 仅匹配 thread-{NN}
                let nn = match name.strip_prefix("thread-") {
                    Some(s) if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) => {
                        match s.parse::<u32>() {
                            Ok(n) => n,
                            Err(_) => continue,
                        }
                    }
                    _ => continue,
                };
                let keymap = entry.path().join("output-keymap.txt");
                if keymap.is_file() {
                    found.push((nn, keymap.to_string_lossy().to_string()));
                }
            }
            if found.is_empty() {
                return Err(format!(
                    "--seed-dir {} 下未找到任何 thread-NN/output-keymap.txt",
                    dir
                ));
            }
            found.sort_by_key(|(nn, _)| *nn);
            Ok(found.into_iter().map(|(_, p)| p).collect())
        }
    }
}

fn run_optimize(
    cfg: &Config,
    cli_config_path: &str,
    output_dir_opt: Option<String>,
    seed_dir: Option<String>,
    seed_keymap: Option<String>,
) {
    let start_time = Instant::now();
    println!("=== CodeGenie 码灵算法优化器 v10 ===");

    // 验证配置
    cfg.validate_weights();

    // 解析种子来源（互斥校验 + 路径收集，需求 1/2/3）：尽早失败，避免做完校准才报错。
    let seed_paths = match resolve_seed_paths(seed_dir.as_deref(), seed_keymap.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("❌ 种子参数无效: {}", e);
            std::process::exit(1);
        }
    };

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

    // 创建/复用输出目录
    let output_dir = match output_dir_opt {
        // 指定 -d：不存在则创建，存在则复用（允许非空）；逐文件写入时防覆盖归档。
        Some(d) => d,
        // 缺省：output-{时间戳}
        None => {
            let timestamp = Local::now().format("%Y%m%d-%H%M%S").to_string();
            format!("output-{}", timestamp)
        }
    };
    std::fs::create_dir_all(&output_dir).expect("无法创建输出目录");
    println!("输出目录: {}", output_dir);

    // 备份依赖输入文件到 {output_dir}/inputs/（需求 4），供 resume 用一致输入重建上下文。
    backup_inputs(cfg, cli_config_path, &output_dir);

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
    // 校准分支会构建 temp_ctx（含完整全码/简码预计算）。为避免退火前重复构建，
    // 校准后复用 temp_ctx 作为正式 ctx，仅就地更新评分用的 scale/targets（二者不参与
    // 构建期预计算，仅评分路径读取）。all_manual（手动 scale 跳过校准）无 temp_ctx，
    // 正式 ctx 仍需单独构建一次。
    let (scale_config, scale_source, calib_ctx): (types::ScaleConfig, &'static str, Option<OptContext>) =
        if all_manual {
            println!("\n📐 ScaleConfig 已全部手动配置，跳过自动校准...");
            let (sc, src) = resolve_scale_config(cfg, types::ScaleConfig::default());
            (sc, src, None)
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
        let (sc, src) = resolve_scale_config(cfg, calibrated);
        // 复用校准期构建的 temp_ctx 作为正式 ctx（省去退火前第二次完整构建）。
        (sc, src, Some(temp_ctx))
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
    let targets_config = cfg.get_targets_config();
    let ctx = match calib_ctx {
        // 复用校准期构建的上下文（含全码/简码预计算），仅就地更新评分用的 scale/targets。
        // 二者不参与 OptContext 构建期预计算、仅在评分路径读取，故就地替换等价于用最终
        // scale/targets 重新构建——消除退火前的重复构建（含简码预计算）。
        Some(mut c) => {
            c.scale_config = scale_config;
            c.targets_config = targets_config;
            c
        }
        // all_manual：跳过了校准、无可复用上下文，正式 ctx 单独构建一次。
        None => {
            let equiv_table_2 = loader::load_pair_equivalence(&cfg.files.pair_equiv);
            let key_dist_config_2 = loader::load_key_distribution(&cfg.files.key_dist);
            OptContext::new_with_fixed(
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
            )
        }
    };

    println!("\n  - 编码基数: {}", ctx.code_base);
    println!("  - 编码空间: {}", ctx.code_space);

    // 桶存储后端选择日志（需求 12）：单线程、配置确认阶段输出一次。
    evaluator::log_bucket_backends(&ctx);

    // 写出 checkpoint 元信息（需求 6）：校准已完成，scale_config 固定，运行期不再修改。
    let ckpt_dir = checkpoint::checkpoint_dir(&output_dir);
    std::fs::create_dir_all(&ckpt_dir).expect("无法创建 checkpoint 目录");
    {
        let meta = checkpoint::CheckpointMeta {
            version: checkpoint::CHECKPOINT_VERSION,
            timestamp: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            scale_config,
            total_steps: cfg.annealing.total_steps,
            num_threads: cfg.annealing.threads,
            temp_start: cfg.annealing.temp_start,
            temp_end: cfg.annealing.temp_end,
            comfort_temp: cfg.annealing.comfort_temp,
        };
        if let Err(e) = checkpoint::save_meta(&meta, &ckpt_dir) {
            eprintln!("⚠️ 写 checkpoint 元信息失败: {}", e);
        }
    }

    // 安装 Ctrl-C 处理器（需求 8）：置位停止标志，退火线程检测后写最新 checkpoint 并退出。
    let stop_flag = Arc::new(AtomicBool::new(false));
    {
        let sf = Arc::clone(&stop_flag);
        let _ = ctrlc::set_handler(move || sf.store(true, Ordering::Relaxed));
    }
    println!(
        "💡 提示: 按 Ctrl-C 可暂停并保存检查点，之后用 `resume -d {}` 继续",
        output_dir
    );

    // 并行执行模拟退火（带 checkpoint 与可中断）
    println!("\n🚀 开始优化...");
    let num_threads = cfg.annealing.threads;

    // 从既有结果构造种子分配（ctx 就绪后；需求 4/5/7/8）。空 = 不播种，保持基线行为。
    let seeds: Vec<Vec<u8>> = if seed_paths.is_empty() {
        Vec::new()
    } else {
        let mut rng = rand::thread_rng();
        let mut v = Vec::with_capacity(seed_paths.len());
        for p in &seed_paths {
            let map = loader::load_keymap(p, &cfg.files.splits);
            match loader::keymap_to_assignment(&ctx, &map, &mut rng) {
                Ok((asg, filled)) => {
                    // 种子未覆盖任何组（filled 等于全部组数）：视为无有效编码行或与当前方案
                    // 完全不匹配，报错退出（需求 2.3）。
                    if ctx.num_groups > 0 && filled == ctx.num_groups {
                        eprintln!(
                            "❌ 种子 {} 未覆盖任何字根组（无有效编码行或与当前方案不匹配）",
                            p
                        );
                        std::process::exit(1);
                    }
                    if filled > 0 {
                        println!("⚠️ 种子 {} 有 {} 个组未被覆盖，已随机合法填充", p, filled);
                    }
                    v.push(asg);
                }
                Err(e) => {
                    eprintln!("❌ 种子 {} 无效: {}", p, e);
                    std::process::exit(1);
                }
            }
        }
        println!(
            "🌱 播种来源: {} 个种子；{} 线程按 round-robin(i % K) 映射",
            v.len(),
            num_threads
        );
        v
    };

    let interval = annealing::checkpoint_interval(
        cfg.annealing.total_steps,
        cfg.annealing.checkpoint_interval_ratio,
    );
    let sa_results: Vec<SaResult> = (0..num_threads)
        .into_par_iter()
        .map(|i| {
            let seed_ref = if seeds.is_empty() {
                None
            } else {
                Some(seeds[seed_index(i, seeds.len())].as_slice())
            };
            simulated_annealing_resumable(
                &ctx,
                cfg,
                i,
                &stop_flag,
                None,
                Some(&ckpt_dir),
                interval,
                seed_ref,
            )
        })
        .collect();

    let interrupted = stop_flag.load(Ordering::Relaxed) || sa_results.iter().any(|r| r.interrupted);
    finalize_and_save(cfg, &ctx, sa_results, &output_dir, start_time, interrupted);
}

/// 汇总各线程结果、打印最优、保存全部产物；optimize 与 resume 共用（需求 7.8）。
fn finalize_and_save(
    cfg: &Config,
    ctx: &OptContext,
    sa_results: Vec<SaResult>,
    output_dir: &str,
    start_time: Instant,
    interrupted: bool,
) {
    let root_usage = output::count_root_usage(ctx);
    let all_results: Vec<(usize, Vec<u8>, f64, types::Metrics, SimpleMetrics)> = sa_results
        .into_iter()
        .enumerate()
        .map(|(i, r)| (i, r.assignment, r.score, r.metrics, r.simple_metrics))
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
    let best_eval = Evaluator::new(ctx, &best_assignment);
    let best_scores = best_eval.get_metric_scores(ctx);
    let simple_sub = best_eval.get_simple_metric_scores(ctx);
    println!("\n=================================");
    if interrupted {
        println!("⏸️  优化已暂停（Ctrl-C），以下为当前最优；可继续续算");
    }
    println!("🏆 最优结果 (线程 {}):", best_thread);
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
        println!("   「简码」总分: {:.4}  (范围: output 输出全集)", simple_sub.total);
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
            ctx,
            assignment,
            *score,
            metrics,
            smetrics,
            *tid,
            output_dir,
            &root_usage,
        );
    }

    save_results(
        ctx,
        &best_assignment,
        best_score,
        &best_metrics,
        &best_simple_metrics,
        output_dir,
        &root_usage,
    );
    save_summary(cfg, &all_results, best_thread, output_dir, elapsed);

    println!("\n所有结果已保存至 {}/", output_dir);
    println!("  - summary.txt              汇总排名");
    println!("  - output-*.txt             全局最优结果");
    println!("  - output-simple-codes.txt  简码分配");
    println!("  - thread-XX/               各线程结果");
    if interrupted {
        println!("\n⏸️  已暂停。继续续算：");
        println!("   cargo run --release -- resume -d {}", output_dir);
    }
}

/// 备份依赖输入文件到 `{output_dir}/inputs/`（需求 4）。失败仅告警，不中止。
fn backup_inputs(cfg: &Config, cli_config_path: &str, output_dir: &str) {
    let inputs_dir = format!("{}/inputs", output_dir);
    if let Err(e) = std::fs::create_dir_all(&inputs_dir) {
        eprintln!("⚠️ 创建 inputs 目录失败: {}", e);
        return;
    }
    // config 文件固定备份为 config.toml，确保 resume 能用硬编码路径找到它。
    let dst = format!("{}/config.toml", inputs_dir);
    if let Err(e) = fsutil::copy_with_backup(cli_config_path, &dst) {
        eprintln!("⚠️ 备份输入文件 {} 失败: {}", cli_config_path, e);
    }

    let data_srcs = [
        cfg.files.fixed.as_str(),
        cfg.files.dynamic.as_str(),
        cfg.files.splits.as_str(),
        cfg.files.pair_equiv.as_str(),
        cfg.files.key_dist.as_str(),
    ];
    for src in data_srcs {
        let base = std::path::Path::new(src)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| src.to_string());
        let dst = format!("{}/{}", inputs_dir, base);
        if let Err(e) = fsutil::copy_with_backup(src, &dst) {
            eprintln!("⚠️ 备份输入文件 {} 失败: {}", src, e);
        }
    }
    println!("已备份输入文件至 {}/", inputs_dir);
}

/// resume 子命令（需求 7）：从既有 output 目录的 checkpoint 续算，复用该目录。
/// 对 inputs/ 与 meta.json 只读；从 inputs/ 重建上下文、复用 meta.scale_config（不重校准）。
fn run_resume(dir: &str) {
    let start_time = Instant::now();
    println!("=== CodeGenie 断点续算 ===");
    println!("  输出目录: {}", dir);

    let ckpt_dir = checkpoint::checkpoint_dir(dir);

    // 1) 读元信息（只读 + 版本校验，需求 6.4/7.4/11.4）
    let meta = match checkpoint::load_meta(&ckpt_dir) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("❌ 无法加载 checkpoint 元信息: {}", e);
            std::process::exit(1);
        }
    };

    // 2) 从备份的 inputs/ 读取配置与输入（只读，需求 7.2/7.3）。
    let inputs_dir = format!("{}/inputs", dir);
    let config_path = format!("{}/config.toml", inputs_dir);
    let mut cfg = Config::load_from_path(&config_path);
    // 将 files.* 重定向到 inputs/ 下的备份副本（按 basename），确保用一致输入重建。
    let redirect = |p: &str| -> String {
        let base = std::path::Path::new(p)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| p.to_string());
        format!("{}/{}", inputs_dir, base)
    };
    cfg.files.fixed = redirect(&cfg.files.fixed);
    cfg.files.dynamic = redirect(&cfg.files.dynamic);
    cfg.files.splits = redirect(&cfg.files.splits);
    cfg.files.pair_equiv = redirect(&cfg.files.pair_equiv);
    cfg.files.key_dist = redirect(&cfg.files.key_dist);
    let cfg = cfg;

    // 3) 重建优化上下文（复用 meta.scale_config，跳过 calibrate，需求 6.3）。
    let (fixed_roots, constrained) = loader::load_fixed(&cfg.files.fixed);
    let dynamic_groups = loader::load_dynamic(&cfg.files.dynamic, &constrained, &cfg.keys.allowed);
    let splits = loader::load_splits(&cfg.files.splits);
    let equiv_table = loader::load_pair_equivalence(&cfg.files.pair_equiv);
    let key_dist_config = loader::load_key_distribution(&cfg.files.key_dist);
    if !validate::check_validation(&splits, &fixed_roots, &dynamic_groups) {
        std::process::exit(1);
    }
    let simple_config = if cfg.weights.simple_code.enabled {
        cfg.get_simple_code_config()
    } else {
        SimpleCodeConfig { levels: vec![] }
    };
    let weights = cfg.get_weight_config();
    let ctx = OptContext::new_with_fixed(
        &splits,
        &fixed_roots,
        &dynamic_groups,
        equiv_table,
        key_dist_config,
        meta.scale_config,
        simple_config,
        weights,
        cfg.get_targets_config(),
        &cfg.get_fixed_simple_codes(),
    );
    evaluator::log_bucket_backends(&ctx);

    // 4) 加载各线程检查点（数量须与 meta.num_threads 一致，需求 11.4）。
    let num_threads = meta.num_threads;
    let mut tcs: Vec<ThreadCheckpoint> = Vec::with_capacity(num_threads);
    for i in 0..num_threads {
        let path = checkpoint::thread_path(&ckpt_dir, i);
        match checkpoint::load_thread_checkpoint(&path) {
            Ok(tc) => tcs.push(tc),
            Err(e) => {
                eprintln!("❌ 无法加载线程检查点 {}: {}", path.display(), e);
                std::process::exit(1);
            }
        }
    }
    println!(
        "已加载 {} 个线程检查点；总步数 {}，从各线程断点续算",
        num_threads, meta.total_steps
    );

    // 5) 安装 Ctrl-C，并行续算（resume=Some），收尾产出到同一目录（不新建、不改 inputs/meta）。
    let stop_flag = Arc::new(AtomicBool::new(false));
    {
        let sf = Arc::clone(&stop_flag);
        let _ = ctrlc::set_handler(move || sf.store(true, Ordering::Relaxed));
    }
    println!("💡 提示: 按 Ctrl-C 可暂停并保存检查点");

    let interval = annealing::checkpoint_interval(
        cfg.annealing.total_steps,
        cfg.annealing.checkpoint_interval_ratio,
    );
    let sa_results: Vec<SaResult> = (0..num_threads)
        .into_par_iter()
        .map(|i| {
            simulated_annealing_resumable(
                &ctx,
                &cfg,
                i,
                &stop_flag,
                Some(&tcs[i]),
                Some(&ckpt_dir),
                interval,
                None,
            )
        })
        .collect();

    let interrupted = stop_flag.load(Ordering::Relaxed) || sa_results.iter().any(|r| r.interrupted);
    finalize_and_save(&cfg, &ctx, sa_results, dir, start_time, interrupted);
}

// =========================================================================
// 🧪 输入备份测试（annealing-checkpoint-resume, 需求 4）
// =========================================================================
#[cfg(test)]
mod backup_tests {
    use super::*;

    #[test]
    fn backup_inputs_copies_all_dependency_files() {
        // 准备临时源目录与 6 个依赖文件。
        let base = std::env::temp_dir().join(format!("cg_backup_{}", std::process::id()));
        let src = base.join("src");
        let out = base.join("output-test");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&out).unwrap();

        let files = [
            ("config.toml", "config-content"),
            ("input-fixed.txt", "fixed-content"),
            ("input-roots.txt", "roots-content"),
            ("input-division.txt", "division-content"),
            ("pair_equivalence.txt", "equiv-content"),
            ("key_distribution.txt", "keydist-content"),
        ];
        for (name, content) in files {
            std::fs::write(src.join(name), content).unwrap();
        }

        let mut cfg = Config::default();
        cfg.files.fixed = src.join("input-fixed.txt").to_string_lossy().to_string();
        cfg.files.dynamic = src.join("input-roots.txt").to_string_lossy().to_string();
        cfg.files.splits = src.join("input-division.txt").to_string_lossy().to_string();
        cfg.files.pair_equiv = src.join("pair_equivalence.txt").to_string_lossy().to_string();
        cfg.files.key_dist = src.join("key_distribution.txt").to_string_lossy().to_string();
        let config_path = src.join("config.toml").to_string_lossy().to_string();

        backup_inputs(&cfg, &config_path, &out.to_string_lossy());

        // 断言 inputs/ 下 6 个文件均存在且内容一致（按 basename）。
        let inputs = out.join("inputs");
        for (name, content) in files {
            let dst = inputs.join(name);
            assert!(dst.exists(), "缺少备份文件: {}", dst.display());
            assert_eq!(std::fs::read_to_string(&dst).unwrap(), content, "{name} 内容不一致");
        }

        let _ = std::fs::remove_dir_all(&base);
    }

    /// config 文件名非 config.toml 时，备份后应固定叫 config.toml，确保 resume 能找到它。
    #[test]
    fn backup_inputs_renames_config_to_config_toml() {
        let base = std::env::temp_dir().join(format!("cg_backup_rename_{}", std::process::id()));
        let src = base.join("src");
        let out = base.join("output-test");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&out).unwrap();

        // config 文件使用非默认名称
        let custom_config = "my-custom-config.toml";
        std::fs::write(src.join(custom_config), "custom-config-content").unwrap();
        std::fs::write(src.join("input-fixed.txt"), "fixed").unwrap();
        std::fs::write(src.join("input-roots.txt"), "roots").unwrap();
        std::fs::write(src.join("input-division.txt"), "division").unwrap();
        std::fs::write(src.join("pair_equivalence.txt"), "equiv").unwrap();
        std::fs::write(src.join("key_distribution.txt"), "keydist").unwrap();

        let mut cfg = Config::default();
        cfg.files.fixed = src.join("input-fixed.txt").to_string_lossy().to_string();
        cfg.files.dynamic = src.join("input-roots.txt").to_string_lossy().to_string();
        cfg.files.splits = src.join("input-division.txt").to_string_lossy().to_string();
        cfg.files.pair_equiv = src.join("pair_equivalence.txt").to_string_lossy().to_string();
        cfg.files.key_dist = src.join("key_distribution.txt").to_string_lossy().to_string();
        let config_path = src.join(custom_config).to_string_lossy().to_string();

        backup_inputs(&cfg, &config_path, &out.to_string_lossy());

        let inputs = out.join("inputs");
        // 固定备份为 config.toml，内容与源文件一致
        let dst = inputs.join("config.toml");
        assert!(dst.exists(), "备份的 config 文件应命名为 config.toml");
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "custom-config-content");
        // 原始文件名不应出现在 inputs/ 下
        assert!(!inputs.join(custom_config).exists(), "不应以原始文件名备份 config");

        let _ = std::fs::remove_dir_all(&base);
    }
}

// =========================================================================
// 🧪 播种来源解析与线程映射测试（seed-optimize-from-result, 需求 1/2/3/7）
// =========================================================================
#[cfg(test)]
mod seed_path_tests {
    use super::*;
    use proptest::prelude::*;

    fn tmp_base(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "cg_seed_{}_{}_{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn mutually_exclusive_errors() {
        // 同时指定 --seed-dir 与 --seed-keymap → Err（需求 3.1）
        assert!(resolve_seed_paths(Some("d"), Some("k")).is_err());
    }

    #[test]
    fn none_returns_empty() {
        // 均未指定 → 空 Vec（不播种，需求 3.2）
        assert_eq!(resolve_seed_paths(None, None).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn seed_keymap_single_file() {
        let base = tmp_base("kf");
        std::fs::create_dir_all(&base).unwrap();
        let f = base.join("km.txt");
        std::fs::write(&f, "口\tWko\t1\n").unwrap();
        let paths = resolve_seed_paths(None, Some(&f.to_string_lossy())).unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0], f.to_string_lossy());
        // 不存在的文件 → Err（需求 2.3）
        assert!(resolve_seed_paths(None, Some("/no/such/file.txt")).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn seed_dir_collects_thread_dirs_sorted() {
        let base = tmp_base("sd");
        // 故意乱序创建 thread-02, thread-00, thread-10，且各含 output-keymap.txt
        for nn in ["02", "00", "10"] {
            let d = base.join(format!("thread-{}", nn));
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("output-keymap.txt"), "口\tWko\t1\n").unwrap();
        }
        // 干扰项：inputs/ 与 checkpoint/ 不应被收集（需求 1.5）
        std::fs::create_dir_all(base.join("inputs")).unwrap();
        std::fs::create_dir_all(base.join("checkpoint")).unwrap();
        // 干扰项：thread-03 无 output-keymap.txt → 不收集
        std::fs::create_dir_all(base.join("thread-03")).unwrap();

        let paths = resolve_seed_paths(Some(&base.to_string_lossy()), None).unwrap();
        assert_eq!(paths.len(), 3, "应只收集 3 个含 keymap 的 thread 目录");
        // 按 NN 升序：00, 02, 10
        assert!(paths[0].ends_with("thread-00/output-keymap.txt"));
        assert!(paths[1].ends_with("thread-02/output-keymap.txt"));
        assert!(paths[2].ends_with("thread-10/output-keymap.txt"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn seed_dir_empty_errors() {
        let base = tmp_base("empty");
        std::fs::create_dir_all(&base).unwrap();
        // 无任何 thread-NN/output-keymap.txt → Err（需求 1.4）
        assert!(resolve_seed_paths(Some(&base.to_string_lossy()), None).is_err());
        // 不存在的目录 → Err
        assert!(resolve_seed_paths(Some("/no/such/dir"), None).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        // Feature: seed-optimize-from-result, Property 4: 线程→种子轮询映射
        #[test]
        fn prop_round_robin_mapping(k in 1usize..16, t in 1usize..64) {
            // 线程 i 映射到 i % k
            for i in 0..t {
                prop_assert_eq!(seed_index(i, k), i % k);
            }
            // 被使用的种子下标集合
            let used: std::collections::HashSet<usize> = (0..t).map(|i| seed_index(i, k)).collect();
            if t < k {
                // T<K：恰为 {0..T-1}
                let expected: std::collections::HashSet<usize> = (0..t).collect();
                prop_assert_eq!(used, expected);
            }
            // K==1：所有线程映射到 0
            if k == 1 {
                for i in 0..t {
                    prop_assert_eq!(seed_index(i, k), 0);
                }
            }
        }
    }
}
