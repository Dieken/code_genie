// =========================================================================
// 🔧 配置模块
// =========================================================================

use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;

use crate::types::{SimpleAssignMode, SimpleCodeConfig, SimpleCodeLevel, SimpleCodeStep, WeightConfig};

// =========================================================================
// 📋 配置结构体定义
// =========================================================================

/// 主配置结构体
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub files: FilesConfig,
    pub keys: KeysConfig,
    pub weights: WeightsConfig,
    pub annealing: AnnealingConfig,
    pub simple_levels: Vec<SimpleLevelConfig>,
    pub scale: Option<ScaleConfigToml>,    // 可选，缺失则完全依赖 calibrate
    pub targets: Option<TargetsConfig>,    // 可选，缺失则使用默认（disabled）
    /// 固定简码映射（需求 21）：顶层内联表 `[fixed_simple_codes]`，形如 `"不" = "u"`、
    /// `"了" = "a_"`。键为汉字、值为字面简码串（可选以 `_` 结尾表示空格上屏）。
    /// 缺失时为 None。用 `BTreeMap` 保证遍历顺序确定。
    #[serde(default)]
    pub fixed_simple_codes: Option<BTreeMap<String, String>>,
}

/// 文件路径配置
#[derive(Debug, Clone, Deserialize)]
pub struct FilesConfig {
    pub fixed: String,
    pub dynamic: String,
    pub splits: String,
    pub pair_equiv: String,
    pub key_dist: String,
}

/// 键位配置
#[derive(Debug, Clone, Deserialize)]
pub struct KeysConfig {
    pub allowed: String,
    pub display_order: String,
}

/// 权重配置
#[derive(Debug, Clone, Deserialize)]
pub struct WeightsConfig {
    pub full_code: FullCodeWeights,
    pub simple_code: SimpleCodeWeights,
}

/// 全码权重
#[derive(Debug, Clone, Deserialize)]
pub struct FullCodeWeights {
    pub collision_count: f64,
    pub collision_rate: f64,
    pub equivalence: f64,
    pub equiv_cv: f64,
    pub distribution: f64,
}

/// 简码权重
#[derive(Debug, Clone, Deserialize)]
pub struct SimpleCodeWeights {
    pub enabled: bool,
    pub full_code_weight: f64,
    pub simple_code_weight: f64,
    pub freq: f64,
    pub equiv: f64,
    pub dist: f64,
    pub collision_count: f64,
    pub collision_rate: f64,

    // ---- 简码评估性能优化新增项（需求 17/18） ----
    /// 简码计算激活进度阈值（默认 0.4）
    #[serde(default = "default_simple_start_progress")]
    pub simple_start_progress: f64,
    /// 权重从 0 渐进到 W 的进度长度（默认 0.1）
    #[serde(default = "default_simple_ramp_progress")]
    pub simple_ramp_progress: f64,
    /// 激活当刻升温倍率（默认 1.2，独立于 reheat_factor）
    #[serde(default = "default_simple_activation_reheat")]
    pub simple_activation_reheat: f64,
    /// 候选字累计字频覆盖率阈值（默认 1.0）
    #[serde(default = "default_simple_coverage_ratio")]
    pub simple_coverage_ratio: f64,
    /// 退火期「active 候选」累计字频覆盖率阈值（默认 0.90，须 ≤ simple_coverage_ratio）。
    /// active 候选参与每步增量评估与周期对账；其余（passive）候选仅在最终上报与输出时纳入，
    /// 从而在保留「全集出简输出」的同时大幅降低每步简码计算量。
    #[serde(default = "default_simple_active_coverage")]
    pub simple_active_coverage: f64,
    /// 周期对账间隔比例，M = floor(total_steps × ratio)，M ≥ 1（默认 0.05）
    #[serde(default = "default_reconcile_interval_ratio")]
    pub reconcile_interval_ratio: f64,
    /// 桶内出简排序模式："frequency" 或 "efficiency"（默认 "efficiency"）
    #[serde(default = "default_simple_assign_mode")]
    pub simple_assign_mode: String,
    /// 简码占用保护（需求 33）：禁止简码等于「全字频前 N 名汉字」的全码。
    /// 默认 0 表示 N = 全部汉字（简码不得等于任何汉字的全码）；N>0 仅保护前 N 名。
    #[serde(default = "default_simple_protect_top_n")]
    pub simple_protect_top_n: usize,
}

fn default_simple_start_progress() -> f64 { 0.4 }
fn default_simple_ramp_progress() -> f64 { 0.1 }
fn default_simple_activation_reheat() -> f64 { 1.2 }
fn default_simple_coverage_ratio() -> f64 { 1.0 }
fn default_simple_active_coverage() -> f64 { 0.90 }
fn default_reconcile_interval_ratio() -> f64 { 0.05 }
fn default_simple_assign_mode() -> String { "efficiency".to_string() }
fn default_simple_protect_top_n() -> usize { 0 }

/// 模拟退火参数配置
#[derive(Debug, Clone, Deserialize)]
pub struct AnnealingConfig {
    pub threads: usize,
    pub total_steps: usize,
    pub temp_start: f64,
    pub temp_end: f64,
    pub comfort_temp: f64,
    pub comfort_width: f64,
    pub comfort_slowdown: f64,
    pub swap_probability: f64,
    pub min_improve_steps_ratio: f64,
    pub perturb_interval_ratio: f64,
    pub perturb_strength: f64,
    pub reheat_factor: f64,
    pub max_parts: usize,

    /// 冲突导向移动的执行概率（0.0~1.0），0.0 表示关闭本特性
    #[serde(default = "default_conflict_probability")]
    pub conflict_probability: f64,
    /// 每隔多少步重建一次冲突缓存；0 表示初始化后不再重建
    #[serde(default = "default_conflict_refresh_interval")]
    pub conflict_refresh_interval: usize,
    /// 在排序后冲突列表的前 N 个元素中采样
    #[serde(default = "default_conflict_sample_window")]
    pub conflict_sample_window: usize,
    /// 冲突组排序是否按频率加权（true=按字频之和，false=按汉字数量）
    #[serde(default = "default_conflict_weight_by_freq")]
    pub conflict_weight_by_freq: bool,

    /// checkpoint 写出间隔比例：单位为占 total_steps 的比例，
    /// 实际间隔步数 = max(1, floor(total_steps × ratio))（默认 0.05，约每 5% 写一次，与汇报频率一致）
    #[serde(default = "default_checkpoint_interval_ratio")]
    pub checkpoint_interval_ratio: f64,
}

fn default_conflict_probability() -> f64 { 0.0 }
fn default_conflict_refresh_interval() -> usize { 1000 }
fn default_conflict_sample_window() -> usize { 20 }
fn default_conflict_weight_by_freq() -> bool { false }
fn default_checkpoint_interval_ratio() -> f64 { 0.05 }

/// 全码目标配置（对应 [targets.full_code] 段）
#[derive(Debug, Clone, Deserialize)]
pub struct FullCodeTargets {
    pub enabled: bool,
    pub collision_count: f64,
    pub collision_rate: f64,
    pub equivalence: f64,
    pub equiv_cv: f64,
    pub distribution: f64,
    pub low_weight: f64,
    // _max 硬约束（0.0 = 不启用）
    pub collision_count_max: f64,
    pub collision_rate_max: f64,
    pub equivalence_max: f64,
    pub equiv_cv_max: f64,
    pub distribution_max: f64,
}

impl Default for FullCodeTargets {
    fn default() -> Self {
        Self {
            enabled: false,
            collision_count: 0.0,
            collision_rate: 0.0,
            equivalence: 0.0,
            equiv_cv: 0.0,
            distribution: 0.0,
            low_weight: 0.01,
            collision_count_max: 0.0,
            collision_rate_max: 0.0,
            equivalence_max: 0.0,
            equiv_cv_max: 0.0,
            distribution_max: 0.0,
        }
    }
}

/// 简码目标配置（对应 [targets.simple_code] 段）
/// freq_max 是频率覆盖率下限约束（coverage < freq_max 时拒绝），0.0 表示不启用
#[derive(Debug, Clone, Deserialize)]
pub struct SimpleCodeTargets {
    pub enabled: bool,
    pub collision_count: f64,
    pub collision_rate: f64,
    /// 频率覆盖率目标（如 0.85 = 85%）
    pub freq: f64,
    pub equiv: f64,
    pub dist: f64,
    pub low_weight: f64,
    // _max 硬约束（0.0 = 不启用）
    pub collision_count_max: f64,
    pub collision_rate_max: f64,
    /// 覆盖率下限约束（coverage < freq_max 时拒绝），0.0 表示不启用
    pub freq_max: f64,
    pub equiv_max: f64,
    pub dist_max: f64,
}

impl Default for SimpleCodeTargets {
    fn default() -> Self {
        Self {
            enabled: false,
            collision_count: 0.0,
            collision_rate: 0.0,
            freq: 0.0,
            equiv: 0.0,
            dist: 0.0,
            low_weight: 0.01,
            collision_count_max: 0.0,
            collision_rate_max: 0.0,
            freq_max: 0.0,
            equiv_max: 0.0,
            dist_max: 0.0,
        }
    }
}

/// 简码级别配置（TOML 格式）
#[derive(Debug, Clone, Deserialize)]
pub struct SimpleLevelConfig {
    pub level: usize,
    pub code_num: usize,
    pub rules: Vec<String>,
    /// 是否需要空格上屏（需求 20）；缺省为 false
    #[serde(default)]
    pub space_commit: bool,
}

/// 目标配置容器（对应 [targets] 段）
#[derive(Debug, Clone, Deserialize, Default)]
pub struct TargetsConfig {
    #[serde(default)]
    pub full_code: FullCodeTargets,
    #[serde(default)]
    pub simple_code: SimpleCodeTargets,
}

/// TOML 可选 scale 配置（所有字段 Option<f64>，对应 [scale] 段）
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ScaleConfigToml {
    pub collision_count: Option<f64>,
    pub collision_rate: Option<f64>,
    pub equivalence: Option<f64>,
    pub equiv_cv: Option<f64>,
    pub distribution: Option<f64>,
    pub simple_freq: Option<f64>,
    pub simple_equiv: Option<f64>,
    pub simple_dist: Option<f64>,
    pub simple_collision_count: Option<f64>,
    pub simple_collision_rate: Option<f64>,
}

// =========================================================================
// 📥 配置加载
// =========================================================================

impl Config {
    /// 从 config.toml 加载配置
    pub fn load() -> Self {
        Self::load_from_path("config.toml")
    }

    /// 从指定路径加载配置
    pub fn load_from_path(path: &str) -> Self {
        match fs::read_to_string(path) {
            Ok(content) => {
                match toml::from_str(&content) {
                    Ok(config) => {
                        println!("✅ 已加载配置文件: {}", path);
                        config
                    }
                    Err(e) => {
                        eprintln!("⚠️ 配置文件解析失败: {}, 使用默认配置", e);
                        Self::default()
                    }
                }
            }
            Err(e) => {
                eprintln!("⚠️ 无法读取配置文件 {}: {}, 使用默认配置", path, e);
                Self::default()
            }
        }
    }

    /// 获取简码配置（转换为内部格式）。
    ///
    /// 级别保留规则（需求 21 / 确认点 1）：`code_num > 0` 的级别一律保留；`code_num == 0` 的级别
    /// 仅在「存在按核心码长归属到该级的固定简码」时才保留——使该级的固定简码仍生效（输出并排除
    /// 退火分配），同时不给「无固定简码的 code_num=0 级别」（如默认配置）凭空增加桶分配开销。
    pub fn get_simple_code_config(&self) -> SimpleCodeConfig {
        // 固定简码的核心码长集合（去结尾 `_`），用于判断 code_num=0 级别是否需保留。
        let fixed_core_lens: std::collections::HashSet<usize> = self
            .get_fixed_simple_codes()
            .iter()
            .map(|(_, code)| code.trim_end_matches('_').chars().count())
            .filter(|&l| l > 0)
            .collect();

        let levels: Vec<SimpleCodeLevel> = self
            .simple_levels
            .iter()
            .map(|l| {
                let rule_candidates: Vec<Vec<SimpleCodeStep>> = l
                    .rules
                    .iter()
                    .filter_map(|rule| parse_rule_string(rule))
                    .collect();

                SimpleCodeLevel {
                    level: l.level,
                    code_num: l.code_num,
                    rule_candidates,
                    space_commit: l.space_commit,
                }
            })
            .filter(|l| !l.rule_candidates.is_empty())
            .filter(|l| {
                if l.code_num > 0 {
                    return true;
                }
                // code_num == 0：仅当有固定简码按码长归属到该级时保留（该级简码键位数 =
                // 各候选规则步数的最大值，与 context 的 max_len 口径一致）。
                let level_len = l
                    .rule_candidates
                    .iter()
                    .map(|r| r.len())
                    .max()
                    .unwrap_or(0);
                fixed_core_lens.contains(&level_len)
            })
            .collect();

        SimpleCodeConfig { levels }
    }

    /// 解析固定简码映射为 `Vec<(char, String)>`（需求 21.1）。
    ///
    /// 键取汉字串的首个 `char`（忽略多字符键的余下部分），值为字面简码串（保留结尾 `_`）。
    /// 缺失或空映射时返回空 `Vec`。具体的级别归属与一致性/长度校验在 `OptContext::new_with_fixed`
    /// 中完成（需依赖该字全码长度与各级 `space_commit` / 简码键位数）。
    pub fn get_fixed_simple_codes(&self) -> Vec<(char, String)> {
        let mut out: Vec<(char, String)> = Vec::new();
        if let Some(map) = &self.fixed_simple_codes {
            for (hanzi, code) in map {
                if let Some(ch) = hanzi.chars().next() {
                    out.push((ch, code.clone()));
                } else {
                    eprintln!("⚠️ 警告：固定简码映射含空汉字键，已忽略");
                }
            }
        }
        out
    }

    /// 验证权重配置是否合理
    pub fn validate_weights(&self) {
        let total_full = self.weights.full_code.collision_count
            + self.weights.full_code.collision_rate
            + self.weights.full_code.equivalence
            + self.weights.full_code.equiv_cv
            + self.weights.full_code.distribution;
        if (total_full - 1.0).abs() > 0.001 {
            eprintln!(
                "⚠️ 警告：全码权重总和不为 1.0 (当前: {:.3})",
                total_full
            );
        }

        let total_simple = self.weights.simple_code.freq
            + self.weights.simple_code.equiv
            + self.weights.simple_code.dist
            + self.weights.simple_code.collision_count
            + self.weights.simple_code.collision_rate;
        if self.weights.simple_code.enabled && (total_simple - 1.0).abs() > 0.001 {
            eprintln!(
                "⚠️ 警告：简码子权重总和不为 1.0 (当前: {:.3})",
                total_simple
            );
        }

        let total_main = self.weights.simple_code.full_code_weight
            + self.weights.simple_code.simple_code_weight;
        if self.weights.simple_code.enabled && (total_main - 1.0).abs() > 0.001 {
            eprintln!(
                "⚠️ 警告：全码/简码总权重不为 1.0 (当前: {:.3})",
                total_main
            );
        }
    }

    /// 计算最小改进步数
    pub fn min_improve_steps(&self) -> usize {
        (self.annealing.total_steps as f64 * self.annealing.min_improve_steps_ratio) as usize
    }

    /// 计算扰动间隔
    pub fn perturb_interval(&self) -> usize {
        (self.annealing.total_steps as f64 * self.annealing.perturb_interval_ratio) as usize
    }

    /// 获取 TargetsConfig，缺失时返回默认值（全部 disabled）
    pub fn get_targets_config(&self) -> TargetsConfig {
        self.targets.clone().unwrap_or_default()
    }

    /// 获取权重配置
    pub fn get_weight_config(&self) -> WeightConfig {
        WeightConfig {
            weight_collision_count: self.weights.full_code.collision_count,
            weight_collision_rate: self.weights.full_code.collision_rate,
            weight_equivalence: self.weights.full_code.equivalence,
            weight_equiv_cv: self.weights.full_code.equiv_cv,
            weight_distribution: self.weights.full_code.distribution,
            enable_simple_code: self.weights.simple_code.enabled,
            // 简码关闭时 weight_full_code 取 1.0：综合得分退化为纯原始全码分数（与简码引入前的
            // 基线一致），避免用 full_code_weight(<1) 整体缩放分数而改变 SA 接受概率与全码搜索行为。
            // 简码启用时仍取 full_code_weight，与 simple_code_weight 共同平衡两个分量。
            weight_full_code: if self.weights.simple_code.enabled {
                self.weights.simple_code.full_code_weight
            } else {
                1.0
            },
            weight_simple_code: self.weights.simple_code.simple_code_weight,
            simple_weight_freq: self.weights.simple_code.freq,
            simple_weight_equiv: self.weights.simple_code.equiv,
            simple_weight_dist: self.weights.simple_code.dist,
            simple_weight_collision_count: self.weights.simple_code.collision_count,
            simple_weight_collision_rate: self.weights.simple_code.collision_rate,
            simple_coverage_ratio: self.weights.simple_code.simple_coverage_ratio,
            simple_active_coverage: self.weights.simple_code.simple_active_coverage,
            simple_assign_mode: parse_simple_assign_mode(&self.weights.simple_code.simple_assign_mode),
            simple_protect_top_n: self.weights.simple_code.simple_protect_top_n,
        }
    }

    /// 校验并钳制简码激活与渐进配置（需求 13）。
    ///
    /// 规则：
    /// - `simple_start_progress`、`simple_ramp_progress` 负值钳为 0；
    /// - `simple_start_progress >= 1` 钳到 `[0,1)` 内的最大有效值；
    /// - `start + ramp > 1` 时钳定 `ramp = 1 - start`；
    /// - `(start, ramp) == (0, 0)` 时返回硬激活标记 `true`。
    ///
    /// 任何越界钳制都会输出告警。返回 `hard_activate`。
    pub fn validate_simple_activation(&mut self) -> bool {
        let sc = &mut self.weights.simple_code;

        if sc.simple_start_progress < 0.0 {
            eprintln!(
                "⚠️ 警告：simple_start_progress < 0 (当前: {:.3})，钳制为 0",
                sc.simple_start_progress
            );
            sc.simple_start_progress = 0.0;
        }
        if sc.simple_ramp_progress < 0.0 {
            eprintln!(
                "⚠️ 警告：simple_ramp_progress < 0 (当前: {:.3})，钳制为 0",
                sc.simple_ramp_progress
            );
            sc.simple_ramp_progress = 0.0;
        }
        if sc.simple_start_progress >= 1.0 {
            // 钳到 [0,1) 内的最大有效值
            let clamped = 1.0 - f64::EPSILON;
            eprintln!(
                "⚠️ 警告：simple_start_progress >= 1 (当前: {:.3})，钳制到 {:.6}",
                sc.simple_start_progress, clamped
            );
            sc.simple_start_progress = clamped;
        }
        if sc.simple_start_progress + sc.simple_ramp_progress > 1.0 {
            let clamped_ramp = 1.0 - sc.simple_start_progress;
            eprintln!(
                "⚠️ 警告：simple_start_progress + simple_ramp_progress > 1 (当前: {:.3})，钳定 ramp 为 {:.6}",
                sc.simple_start_progress + sc.simple_ramp_progress,
                clamped_ramp
            );
            sc.simple_ramp_progress = clamped_ramp;
        }

        // 激活升温倍率小于 1 等于「激活即降温」，属误配；钳制为 1.0（需求 27.3）。
        if sc.simple_activation_reheat < 1.0 {
            eprintln!(
                "⚠️ 警告：simple_activation_reheat < 1.0 (当前: {:.3})，钳制为 1.0",
                sc.simple_activation_reheat
            );
            sc.simple_activation_reheat = 1.0;
        }

        // 简码占用保护性能优化（active/passive 候选）：active 覆盖率须 ≤ 输出覆盖率，
        // 否则钳制为 simple_coverage_ratio 并告警（active 候选应为输出候选的子集）。
        if sc.simple_active_coverage < 0.0 {
            eprintln!(
                "⚠️ 警告：simple_active_coverage < 0 (当前: {:.3})，钳制为 0",
                sc.simple_active_coverage
            );
            sc.simple_active_coverage = 0.0;
        }
        if sc.simple_active_coverage > sc.simple_coverage_ratio {
            eprintln!(
                "⚠️ 警告：simple_active_coverage ({:.3}) > simple_coverage_ratio ({:.3})，钳制为 {:.3}",
                sc.simple_active_coverage, sc.simple_coverage_ratio, sc.simple_coverage_ratio
            );
            sc.simple_active_coverage = sc.simple_coverage_ratio;
        }

        // (start, ramp) == (0, 0) → 从开始即硬激活（兼容档，需求 13.5）
        sc.simple_start_progress == 0.0 && sc.simple_ramp_progress == 0.0
    }
}

/// 解析简码出简排序模式字符串；非法值回落 `Efficiency` 并告警（需求 13）
fn parse_simple_assign_mode(s: &str) -> SimpleAssignMode {
    match s.trim().to_ascii_lowercase().as_str() {
        "frequency" => SimpleAssignMode::Frequency,
        "efficiency" => SimpleAssignMode::Efficiency,
        other => {
            eprintln!(
                "⚠️ 警告：simple_assign_mode 取值非法 \"{}\"，回落到默认 \"efficiency\"",
                other
            );
            SimpleAssignMode::Efficiency
        }
    }
}

/// 解析规则字符串为 SimpleCodeStep 列表
fn parse_rule_string(rule: &str) -> Option<Vec<SimpleCodeStep>> {
    let chars: Vec<char> = rule.trim().chars().collect();
    if chars.len() % 2 != 0 || chars.is_empty() {
        return None;
    }

    let mut steps = Vec::new();
    for chunk in chars.chunks(2) {
        steps.push(SimpleCodeStep {
            root_selector: chunk[0],
            code_selector: chunk[1],
        });
    }
    Some(steps)
}

// =========================================================================
// 🔧 默认配置（后备）
// =========================================================================

impl Default for Config {
    fn default() -> Self {
        Config {
            files: FilesConfig {
                fixed: "input-fixed.txt".to_string(),
                dynamic: "input-roots.txt".to_string(),
                splits: "input-division.txt".to_string(),
                pair_equiv: "pair_equivalence.txt".to_string(),
                key_dist: "key_distribution.txt".to_string(),
            },
            keys: KeysConfig {
                allowed: "qwertyuiopasdfghjklzxcvbnm".to_string(),
                display_order: "qwertyuiopasdfghjklzxcvbnm".to_string(),
            },
            weights: WeightsConfig {
                full_code: FullCodeWeights {
                    collision_count: 0.07,
                    collision_rate: 0.62,
                    equivalence: 0.2,
                    equiv_cv: 0.01,
                    distribution: 0.1,
                },
                simple_code: SimpleCodeWeights {
                    enabled: true,
                    full_code_weight: 0.7,
                    simple_code_weight: 0.3,
                    freq: 0.5,
                    equiv: 0.15,
                    dist: 0.05,
                    collision_count: 0.05,
                    collision_rate: 0.25,
                    simple_start_progress: default_simple_start_progress(),
                    simple_ramp_progress: default_simple_ramp_progress(),
                    simple_activation_reheat: default_simple_activation_reheat(),
                    simple_coverage_ratio: default_simple_coverage_ratio(),
                    simple_active_coverage: default_simple_active_coverage(),
                    reconcile_interval_ratio: default_reconcile_interval_ratio(),
                    simple_assign_mode: default_simple_assign_mode(),
                    simple_protect_top_n: default_simple_protect_top_n(),
                },
            },
            annealing: AnnealingConfig {
                threads: 16,
                total_steps: 10_000,
                temp_start: 1.0,
                temp_end: 0.000001,
                comfort_temp: 0.2,
                comfort_width: 0.15,
                comfort_slowdown: 0.8,
                swap_probability: 0.3,
                min_improve_steps_ratio: 0.1,
                perturb_interval_ratio: 0.05,
                perturb_strength: 0.15,
                reheat_factor: 1.25,
                max_parts: 3,
                conflict_probability: 0.0,
                conflict_refresh_interval: 1000,
                conflict_sample_window: 20,
                conflict_weight_by_freq: false,
                checkpoint_interval_ratio: default_checkpoint_interval_ratio(),
            },
            simple_levels: vec![
                SimpleLevelConfig {
                    level: 1,
                    code_num: 0,
                    rules: vec!["Aa".to_string()],
                    space_commit: false,
                },
                SimpleLevelConfig {
                    level: 2,
                    code_num: 1,
                    rules: vec!["AaBa".to_string()],
                    space_commit: false,
                },
                SimpleLevelConfig {
                    level: 3,
                    code_num: 1,
                    rules: vec!["AaBaCa".to_string()],
                    space_commit: false,
                },
            ],
            scale: None,
            targets: None,
            fixed_simple_codes: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // ScaleConfigToml 解析测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_scale_config_toml_parse() {
        let toml_str = r#"
simple_levels = []

[files]
fixed = "input-fixed.txt"
dynamic = "input-roots.txt"
splits = "input-division.txt"
pair_equiv = "pair_equivalence.txt"
key_dist = "key_distribution.txt"

[keys]
allowed = "qwertyuiopasdfghjklzxcvbnm"
display_order = "qwertyuiopasdfghjklzxcvbnm"

[weights.full_code]
collision_count = 0.07
collision_rate = 0.62
equivalence = 0.2
equiv_cv = 0.01
distribution = 0.1

[weights.simple_code]
enabled = false
full_code_weight = 0.7
simple_code_weight = 0.3
freq = 0.5
equiv = 0.15
dist = 0.05
collision_count = 0.05
collision_rate = 0.25

[annealing]
threads = 4
total_steps = 1000
temp_start = 1.0
temp_end = 0.000001
comfort_temp = 0.2
comfort_width = 0.15
comfort_slowdown = 0.8
swap_probability = 0.3
min_improve_steps_ratio = 0.1
perturb_interval_ratio = 0.0
perturb_strength = 0.0
reheat_factor = 1.0
max_parts = 3

[scale]
collision_count = 0.001
collision_rate = 10.0
equivalence = 1.5
equiv_cv = 5.0
distribution = 0.2
simple_freq = 4.0
simple_equiv = 1.2
simple_dist = 0.15
simple_collision_count = 0.02
simple_collision_rate = 8.0
"#;
        let cfg: Config = toml::from_str(toml_str).expect("解析失败");
        let scale = cfg.scale.expect("[scale] 段应存在");
        assert_eq!(scale.collision_count, Some(0.001));
        assert_eq!(scale.collision_rate, Some(10.0));
        assert_eq!(scale.equivalence, Some(1.5));
        assert_eq!(scale.equiv_cv, Some(5.0));
        assert_eq!(scale.distribution, Some(0.2));
        assert_eq!(scale.simple_freq, Some(4.0));
        assert_eq!(scale.simple_equiv, Some(1.2));
        assert_eq!(scale.simple_dist, Some(0.15));
        assert_eq!(scale.simple_collision_count, Some(0.02));
        assert_eq!(scale.simple_collision_rate, Some(8.0));
    }

    #[test]
    fn test_scale_config_missing() {
        // 不含 [scale] 段的最小配置
        let toml_str = r#"
simple_levels = []

[files]
fixed = "f"
dynamic = "d"
splits = "s"
pair_equiv = "p"
key_dist = "k"

[keys]
allowed = "abc"
display_order = "abc"

[weights.full_code]
collision_count = 0.5
collision_rate = 0.5
equivalence = 0.0
equiv_cv = 0.0
distribution = 0.0

[weights.simple_code]
enabled = false
full_code_weight = 1.0
simple_code_weight = 0.0
freq = 1.0
equiv = 0.0
dist = 0.0
collision_count = 0.0
collision_rate = 0.0

[annealing]
threads = 1
total_steps = 100
temp_start = 1.0
temp_end = 0.001
comfort_temp = 0.2
comfort_width = 0.1
comfort_slowdown = 0.8
swap_probability = 0.3
min_improve_steps_ratio = 0.1
perturb_interval_ratio = 0.0
perturb_strength = 0.0
reheat_factor = 1.0
max_parts = 3
"#;
        let cfg: Config = toml::from_str(toml_str).expect("解析失败");
        assert!(cfg.scale.is_none(), "缺失 [scale] 段时应为 None");
    }

    // -----------------------------------------------------------------------
    // TargetsConfig 解析测试
    // -----------------------------------------------------------------------

    fn minimal_config_prefix() -> &'static str {
        r#"
simple_levels = []

[files]
fixed = "f"
dynamic = "d"
splits = "s"
pair_equiv = "p"
key_dist = "k"

[keys]
allowed = "abc"
display_order = "abc"

[weights.full_code]
collision_count = 0.5
collision_rate = 0.5
equivalence = 0.0
equiv_cv = 0.0
distribution = 0.0

[weights.simple_code]
enabled = false
full_code_weight = 1.0
simple_code_weight = 0.0
freq = 1.0
equiv = 0.0
dist = 0.0
collision_count = 0.0
collision_rate = 0.0

[annealing]
threads = 1
total_steps = 100
temp_start = 1.0
temp_end = 0.001
comfort_temp = 0.2
comfort_width = 0.1
comfort_slowdown = 0.8
swap_probability = 0.3
min_improve_steps_ratio = 0.1
perturb_interval_ratio = 0.0
perturb_strength = 0.0
reheat_factor = 1.0
max_parts = 3
"#
    }

    #[test]
    fn test_targets_full_code_parse() {
        let toml_str = format!(
            r#"{}
[targets.full_code]
enabled = true
collision_count = 50.0
collision_rate = 0.005
equivalence = 1.5
equiv_cv = 0.3
distribution = 5.0
low_weight = 0.02
collision_count_max = 200.0
collision_rate_max = 0.02
equivalence_max = 2.0
equiv_cv_max = 0.5
distribution_max = 10.0
"#,
            minimal_config_prefix()
        );
        let cfg: Config = toml::from_str(&toml_str).expect("解析失败");
        let targets = cfg.targets.expect("[targets] 段应存在");
        let fc = &targets.full_code;
        assert!(fc.enabled);
        assert_eq!(fc.collision_count, 50.0);
        assert_eq!(fc.collision_rate, 0.005);
        assert_eq!(fc.equivalence, 1.5);
        assert_eq!(fc.equiv_cv, 0.3);
        assert_eq!(fc.distribution, 5.0);
        assert_eq!(fc.low_weight, 0.02);
        assert_eq!(fc.collision_count_max, 200.0);
        assert_eq!(fc.collision_rate_max, 0.02);
        assert_eq!(fc.equivalence_max, 2.0);
        assert_eq!(fc.equiv_cv_max, 0.5);
        assert_eq!(fc.distribution_max, 10.0);
    }

    #[test]
    fn test_targets_simple_code_parse() {
        let toml_str = format!(
            r#"{}
[targets.simple_code]
enabled = true
collision_count = 10.0
collision_rate = 0.002
freq = 0.85
equiv = 1.3
dist = 3.0
low_weight = 0.05
collision_count_max = 50.0
collision_rate_max = 0.01
freq_max = 0.7
equiv_max = 2.0
dist_max = 8.0
"#,
            minimal_config_prefix()
        );
        let cfg: Config = toml::from_str(&toml_str).expect("解析失败");
        let targets = cfg.targets.expect("[targets] 段应存在");
        let sc = &targets.simple_code;
        assert!(sc.enabled);
        assert_eq!(sc.collision_count, 10.0);
        assert_eq!(sc.collision_rate, 0.002);
        assert_eq!(sc.freq, 0.85);
        assert_eq!(sc.equiv, 1.3);
        assert_eq!(sc.dist, 3.0);
        assert_eq!(sc.low_weight, 0.05);
        assert_eq!(sc.collision_count_max, 50.0);
        assert_eq!(sc.collision_rate_max, 0.01);
        assert_eq!(sc.freq_max, 0.7);
        assert_eq!(sc.equiv_max, 2.0);
        assert_eq!(sc.dist_max, 8.0);
    }

    #[test]
    fn test_targets_defaults() {
        // 不含 [targets] 段时，get_targets_config() 应返回默认值
        let toml_str = minimal_config_prefix();
        let cfg: Config = toml::from_str(toml_str).expect("解析失败");
        assert!(cfg.targets.is_none(), "缺失 [targets] 段时应为 None");

        let targets = cfg.get_targets_config();
        // 全码默认值
        assert!(!targets.full_code.enabled);
        assert_eq!(targets.full_code.collision_count, 0.0);
        assert_eq!(targets.full_code.low_weight, 0.01);
        assert_eq!(targets.full_code.collision_count_max, 0.0);
        // 简码默认值
        assert!(!targets.simple_code.enabled);
        assert_eq!(targets.simple_code.freq, 0.0);
        assert_eq!(targets.simple_code.low_weight, 0.01);
        assert_eq!(targets.simple_code.freq_max, 0.0);
    }

    // -----------------------------------------------------------------------
    // 冲突导向配置项：默认值与向后兼容解析测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_conflict_fields_default_when_absent() {
        // 不含任何冲突导向字段的 [annealing] 应解析成功且四字段取默认值
        let cfg: Config = toml::from_str(minimal_config_prefix()).expect("解析失败");
        let a = &cfg.annealing;
        assert_eq!(a.conflict_probability, 0.0);
        assert_eq!(a.conflict_refresh_interval, 1000);
        assert_eq!(a.conflict_sample_window, 20);
        assert!(!a.conflict_weight_by_freq);
    }

    #[test]
    fn test_conflict_fields_partial_present() {
        // 仅含部分冲突导向字段：已存在字段取显式值，缺失字段取默认值
        let toml_str = format!(
            "{}\nconflict_probability = 0.25\nconflict_weight_by_freq = true\n",
            minimal_config_prefix()
        );
        let cfg: Config = toml::from_str(&toml_str).expect("解析失败");
        let a = &cfg.annealing;
        assert_eq!(a.conflict_probability, 0.25); // 显式
        assert!(a.conflict_weight_by_freq); // 显式
        assert_eq!(a.conflict_refresh_interval, 1000); // 默认
        assert_eq!(a.conflict_sample_window, 20); // 默认
    }

    #[test]
    fn test_conflict_fields_default_impl() {
        let a = &Config::default().annealing;
        assert_eq!(a.conflict_probability, 0.0);
        assert_eq!(a.conflict_refresh_interval, 1000);
        assert_eq!(a.conflict_sample_window, 20);
        assert!(!a.conflict_weight_by_freq);
    }

    // -----------------------------------------------------------------------
    // 简码评估性能优化新增配置项：默认值与 simple_assign_mode 解析测试
    // -----------------------------------------------------------------------

    #[test]
    fn test_simple_code_new_fields_default_when_absent() {
        // minimal_config_prefix 的 [weights.simple_code] 不含 6 个新增项，
        // 应解析成功且全部取既定默认值（需求 4.1/7.2/8.2/9.1/12.1/17.1）
        let cfg: Config = toml::from_str(minimal_config_prefix()).expect("解析失败");
        let sc = &cfg.weights.simple_code;
        assert_eq!(sc.simple_start_progress, 0.4);
        assert_eq!(sc.simple_ramp_progress, 0.1);
        assert_eq!(sc.simple_activation_reheat, 1.2);
        assert_eq!(sc.simple_coverage_ratio, 1.0);
        assert_eq!(sc.simple_active_coverage, 0.90);
        assert_eq!(sc.reconcile_interval_ratio, 0.05);
        assert_eq!(sc.simple_assign_mode, "efficiency");
        assert_eq!(sc.simple_protect_top_n, 0);
    }

    #[test]
    fn test_simple_code_new_fields_explicit_values() {
        // 显式提供新增项时应原样解析（不被默认值覆盖）
        let toml_with_new = minimal_config_prefix().replace(
            "collision_count = 0.0\ncollision_rate = 0.0\n",
            "collision_count = 0.0\ncollision_rate = 0.0\nsimple_start_progress = 0.4\nsimple_ramp_progress = 0.2\nsimple_activation_reheat = 1.5\nsimple_coverage_ratio = 0.95\nsimple_active_coverage = 0.8\nreconcile_interval_ratio = 0.1\nsimple_assign_mode = \"frequency\"\nsimple_protect_top_n = 500\n",
        );
        let cfg: Config = toml::from_str(&toml_with_new).expect("解析失败");
        let sc = &cfg.weights.simple_code;
        assert_eq!(sc.simple_start_progress, 0.4);
        assert_eq!(sc.simple_ramp_progress, 0.2);
        assert_eq!(sc.simple_activation_reheat, 1.5);
        assert_eq!(sc.simple_coverage_ratio, 0.95);
        assert_eq!(sc.simple_active_coverage, 0.8);
        assert_eq!(sc.reconcile_interval_ratio, 0.1);
        assert_eq!(sc.simple_protect_top_n, 500);
        assert_eq!(sc.simple_assign_mode, "frequency");
    }

    #[test]
    fn test_parse_simple_assign_mode_valid() {
        // "frequency" -> Frequency，"efficiency" -> Efficiency（含大小写/空白容错）
        assert_eq!(parse_simple_assign_mode("frequency"), SimpleAssignMode::Frequency);
        assert_eq!(parse_simple_assign_mode("efficiency"), SimpleAssignMode::Efficiency);
        assert_eq!(parse_simple_assign_mode("  Frequency  "), SimpleAssignMode::Frequency);
        assert_eq!(parse_simple_assign_mode("EFFICIENCY"), SimpleAssignMode::Efficiency);
    }

    #[test]
    fn test_parse_simple_assign_mode_illegal_falls_back_to_efficiency() {
        // 非法字符串回落为 Efficiency（需求 17.2）
        assert_eq!(parse_simple_assign_mode("foo"), SimpleAssignMode::Efficiency);
        assert_eq!(parse_simple_assign_mode(""), SimpleAssignMode::Efficiency);
    }

    #[test]
    fn test_get_weight_config_simple_assign_mode_illegal_falls_back() {
        // 通过 get_weight_config()：非法 simple_assign_mode 经 parse 后回落 Efficiency
        let toml_with_illegal = minimal_config_prefix().replace(
            "collision_count = 0.0\ncollision_rate = 0.0\n",
            "collision_count = 0.0\ncollision_rate = 0.0\nsimple_assign_mode = \"bogus\"\n",
        );
        let cfg: Config = toml::from_str(&toml_with_illegal).expect("解析失败");
        assert_eq!(cfg.weights.simple_code.simple_assign_mode, "bogus");

        let wc = cfg.get_weight_config();
        assert_eq!(wc.simple_assign_mode, SimpleAssignMode::Efficiency);
        // 同时确认默认覆盖率被正确传递
        assert_eq!(wc.simple_coverage_ratio, 1.0);
    }

    #[test]
    fn test_code_num_zero_level_kept_only_with_matching_fixed() {
        // 确认点 1：get_simple_code_config 中 code_num=0 的级别仅在「有按码长归属到该级的
        // 固定简码」时保留，否则丢弃（默认配置 level 1 code_num=0 无固定简码 → 丢弃）。
        let mut cfg = Config::default();
        // 默认 simple_levels：level 1 code_num=0(rules ["Aa"], 1 键)、level 2/3 code_num>0。
        cfg.fixed_simple_codes = None;
        let levels_no_fixed = cfg.get_simple_code_config().levels;
        assert!(
            levels_no_fixed.iter().all(|l| l.code_num > 0),
            "无固定简码时 code_num=0 级别应被丢弃"
        );

        // 加入归属 level 1（1 键）的固定简码 "a" → 该 code_num=0 级别应保留。
        let mut map = std::collections::BTreeMap::new();
        map.insert("不".to_string(), "a".to_string());
        cfg.fixed_simple_codes = Some(map);
        let levels_fixed = cfg.get_simple_code_config().levels;
        assert!(
            levels_fixed.iter().any(|l| l.code_num == 0),
            "有归属 code_num=0 级别的固定简码时应保留该级别"
        );
        assert_eq!(
            levels_fixed.len(),
            levels_no_fixed.len() + 1,
            "应恰好多保留一个 code_num=0 级别"
        );
    }

    // -----------------------------------------------------------------------
    // Property 11：激活与渐进配置钳制不变量
    // -----------------------------------------------------------------------

    use proptest::prelude::*;

    proptest! {
        // Feature: simple-code-perf-optimization, Property 11: 激活与渐进配置钳制不变量
        // 对任意输入的 simple_start_progress 与 simple_ramp_progress（含负值、≥1、之和 >1 等
        // 非法值），钳制后应满足：0 ≤ start < 1、start + ramp ≤ 1、负输入被钳为 0；
        // 且当二者输入均为 0（含负值经钳制后为 0）时，hard_activate 为真。
        // Validates: Requirements 13.1, 13.2, 13.3, 13.4, 13.5
        #![proptest_config(ProptestConfig::with_cases(100))]
        #[test]
        fn prop11_activation_clamp_invariants(
            start in -5.0f64..5.0,
            ramp in -5.0f64..5.0,
        ) {
            let mut cfg = Config::default();
            cfg.weights.simple_code.simple_start_progress = start;
            cfg.weights.simple_code.simple_ramp_progress = ramp;

            let hard = cfg.validate_simple_activation();
            let sc = &cfg.weights.simple_code;

            // 13.2 / 13.3：0 ≤ start < 1
            prop_assert!(sc.simple_start_progress >= 0.0);
            prop_assert!(sc.simple_start_progress < 1.0);

            // 13.1：负输入被钳为 0（start/ramp 钳制后均非负）
            prop_assert!(sc.simple_ramp_progress >= 0.0);
            if start < 0.0 {
                prop_assert_eq!(sc.simple_start_progress, 0.0);
            }
            if ramp < 0.0 && start + ramp <= 1.0 {
                // ramp 负值先被钳为 0；该值不会再被 start+ramp>1 分支改动
                prop_assert_eq!(sc.simple_ramp_progress, 0.0);
            }

            // 13.4：start + ramp ≤ 1（容许浮点误差）
            prop_assert!(sc.simple_start_progress + sc.simple_ramp_progress <= 1.0 + 1e-9);

            // 13.5：二者（经负值钳制后）均为 0 时硬激活为真，否则为假
            let start_after_neg = start.max(0.0);
            let ramp_after_neg = ramp.max(0.0);
            let expected_hard = start_after_neg == 0.0 && ramp_after_neg == 0.0;
            prop_assert_eq!(hard, expected_hard);
        }
    }

    // 需求 27.3：simple_activation_reheat < 1.0 应被钳制为 1.0 并告警。
    #[test]
    fn test_reheat_below_one_clamped_to_one() {
        let mut cfg = Config::default();
        cfg.weights.simple_code.simple_activation_reheat = 0.5;
        cfg.validate_simple_activation();
        assert_eq!(
            cfg.weights.simple_code.simple_activation_reheat, 1.0,
            "reheat < 1.0 应被钳制为 1.0（需求 27.3）"
        );

        // >= 1.0 的值不被钳制（上界仅告警、不钳制，需求 27.4）。
        let mut cfg2 = Config::default();
        cfg2.weights.simple_code.simple_activation_reheat = 3.0;
        cfg2.validate_simple_activation();
        assert_eq!(cfg2.weights.simple_code.simple_activation_reheat, 3.0);
    }
}
