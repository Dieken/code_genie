// =========================================================================
// 🔧 配置模块
// =========================================================================

use serde::Deserialize;
use std::fs;

use crate::types::{SimpleCodeConfig, SimpleCodeLevel, SimpleCodeStep, WeightConfig};

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
}

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
}

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

    /// 获取简码配置（转换为内部格式）
    pub fn get_simple_code_config(&self) -> SimpleCodeConfig {
        let levels: Vec<SimpleCodeLevel> = self
            .simple_levels
            .iter()
            .filter(|l| l.code_num > 0)
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
                }
            })
            .filter(|l| !l.rule_candidates.is_empty())
            .collect();

        SimpleCodeConfig { levels }
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
            weight_full_code: self.weights.simple_code.full_code_weight,
            weight_simple_code: self.weights.simple_code.simple_code_weight,
            simple_weight_freq: self.weights.simple_code.freq,
            simple_weight_equiv: self.weights.simple_code.equiv,
            simple_weight_dist: self.weights.simple_code.dist,
            simple_weight_collision_count: self.weights.simple_code.collision_count,
            simple_weight_collision_rate: self.weights.simple_code.collision_rate,
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
            },
            simple_levels: vec![
                SimpleLevelConfig {
                    level: 1,
                    code_num: 0,
                    rules: vec!["Aa".to_string()],
                },
                SimpleLevelConfig {
                    level: 2,
                    code_num: 1,
                    rules: vec!["AaBa".to_string()],
                },
                SimpleLevelConfig {
                    level: 3,
                    code_num: 1,
                    rules: vec!["AaBaCa".to_string()],
                },
            ],
            scale: None,
            targets: None,
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
}
