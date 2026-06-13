// =========================================================================
// 🚀 基础数据类型
// =========================================================================

use std::collections::HashMap;

/// 键位空间大小（a-z + _ + ; + , + . + /）
pub const KEY_SPACE: usize = 26;
/// 当量表大小
pub const EQUIV_TABLE_SIZE: usize = 31;
/// 分组标记起始值
pub const GROUP_MARKER: u16 = 1000;

/// 将字符转换为键位索引
/// - a-z: 0-25
/// - _: 26
/// - ;: 27
/// - ,: 28
/// - .: 29
/// - /: 30
pub fn char_to_key_index(c: char) -> Option<usize> {
    match c {
        'a'..='z' => Some((c as u8 - b'a') as usize),
        '_' => Some(KEY_SPACE),
        ';' => Some(27),
        ',' => Some(28),
        '.' => Some(29),
        '/' => Some(30),
        _ => None,
    }
}

/// 将键位索引转换为字符
pub fn key_to_char(key: u8) -> char {
    match key {
        0..=25 => (key + b'a') as char,
        26 => '_',
        27 => ';',
        28 => ',',
        29 => '.',
        30 => '/',
        _ => '?',
    }
}

/// 计算 base 的 exp 次方
pub fn pow_base(base: usize, exp: usize) -> usize {
    let mut result = 1;
    for _ in 0..exp {
        result *= base;
    }
    result
}

/// 简码桶内出简排序模式
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimpleAssignMode {
    /// 按字频排序（与旧实现一致）
    Frequency,
    /// 按效率排序（freq × (base_saving + sel_len)）
    Efficiency,
}

impl Default for SimpleAssignMode {
    fn default() -> Self {
        SimpleAssignMode::Efficiency
    }
}

/// 权重配置 - 用于得分计算
#[derive(Clone, Copy, Debug)]
pub struct WeightConfig {
    // 全码权重
    pub weight_collision_count: f64,
    pub weight_collision_rate: f64,
    pub weight_equivalence: f64,
    pub weight_equiv_cv: f64,
    pub weight_distribution: f64,
    // 简码开关与主权重
    pub enable_simple_code: bool,
    pub weight_full_code: f64,
    pub weight_simple_code: f64,
    // 简码子权重
    pub simple_weight_freq: f64,
    pub simple_weight_equiv: f64,
    pub simple_weight_dist: f64,
    pub simple_weight_collision_count: f64,
    pub simple_weight_collision_rate: f64,
    // 简码评估性能优化：候选字覆盖率阈值与桶内出简排序模式
    pub simple_coverage_ratio: f64,
    pub simple_assign_mode: SimpleAssignMode,
    /// 简码占用保护（需求 33）：0 = 保护全部汉字全码；N>0 = 仅保护全字频前 N 名。
    pub simple_protect_top_n: usize,
}

impl Default for WeightConfig {
    fn default() -> Self {
        Self {
            weight_collision_count: 0.07,
            weight_collision_rate: 0.62,
            weight_equivalence: 0.2,
            weight_equiv_cv: 0.01,
            weight_distribution: 0.1,
            enable_simple_code: true,
            weight_full_code: 0.7,
            weight_simple_code: 0.3,
            simple_weight_freq: 0.5,
            simple_weight_equiv: 0.15,
            simple_weight_dist: 0.05,
            simple_weight_collision_count: 0.05,
            simple_weight_collision_rate: 0.25,
            simple_coverage_ratio: 0.90,
            simple_assign_mode: SimpleAssignMode::Efficiency,
            simple_protect_top_n: 0,
        }
    }
}

/// 缩放配置 - 用于将不同量纲的指标归一化
#[derive(Clone, Copy)]
pub struct ScaleConfig {
    /// 重码数缩放因子
    pub collision_count: f64,
    /// 重码率缩放因子
    pub collision_rate: f64,
    /// 当量缩放因子
    pub equivalence: f64,
    /// 当量变异系数缩放因子
    pub equiv_cv: f64,
    /// 分布偏差缩放因子
    pub distribution: f64,
    /// 简码频率覆盖缩放因子
    pub simple_freq: f64,
    /// 简码当量缩放因子
    pub simple_equiv: f64,
    /// 简码分布缩放因子
    pub simple_dist: f64,
    /// 简码重码数缩放因子
    pub simple_collision_count: f64,
    /// 简码重码率缩放因子
    pub simple_collision_rate: f64,
}

impl Default for ScaleConfig {
    fn default() -> Self {
        Self {
            collision_count: 1.0,
            collision_rate: 1.0,
            equivalence: 1.0,
            equiv_cv: 1.0,
            distribution: 1.0,
            simple_freq: 1.0,
            simple_equiv: 1.0,
            simple_dist: 1.0,
            simple_collision_count: 1.0,
            simple_collision_rate: 1.0,
        }
    }
}

/// 键位分布配置
#[derive(Clone, Copy, Default)]
pub struct KeyDistConfig {
    /// 目标使用率 (%)
    pub target_rate: f64,
    /// 低于目标时的惩罚系数
    pub low_penalty: f64,
    /// 高于目标时的惩罚系数
    pub high_penalty: f64,
}

/// 汉字拆分信息
#[derive(Clone)]
pub struct CharInfo {
    /// 拆分后的字根列表（键位索引或分组标记）
    pub parts: Vec<u16>,
    /// 使用频率
    pub frequency: u64,
}

/// 字根组 - 用于需要优化的动态字根
#[derive(Clone)]
pub struct RootGroup {
    /// 字根列表
    pub roots: Vec<String>,
    /// 允许分配的键位
    pub allowed_keys: Vec<u8>,
}

/// 评估指标
#[derive(Clone, Copy, Default)]
pub struct Metrics {
    /// 重码数
    pub collision_count: usize,
    /// 重码率
    pub collision_rate: f64,
    /// 平均当量
    pub equiv_mean: f64,
    /// 当量变异系数
    pub equiv_cv: f64,
    /// 分布偏差
    pub dist_deviation: f64,
}

/// 全码各指标的分数分量（用于日志输出，避免在日志代码中重复算分公式）
#[derive(Clone, Copy, Default)]
pub struct MetricScores {
    pub collision_count: f64,
    pub collision_rate: f64,
    pub equivalence: f64,
    pub equiv_cv: f64,
    pub distribution: f64,
    pub total_full: f64,
    pub total_simple: f64,
    pub total: f64,
}

/// 简码各子指标的分数分量（用于日志输出，需求 28.5/28.6）。
/// 各子分数之和等于 `total`（简码总分，未乘综合权重 weight_simple_code）。
#[derive(Clone, Copy, Default)]
pub struct SimpleMetricScores {
    pub freq: f64,
    pub equiv: f64,
    pub dist: f64,
    pub collision_count: f64,
    pub collision_rate: f64,
    pub total: f64,
}

/// 简码评估指标
#[derive(Clone, Copy, Default)]
pub struct SimpleMetrics {
    /// 频率覆盖率
    pub weighted_freq_coverage: f64,
    /// 平均当量
    pub equiv_mean: f64,
    /// 分布偏差
    pub dist_deviation: f64,
    /// 重码数
    pub collision_count: usize,
    /// 重码率
    pub collision_rate: f64,
}

/// 简码步骤 - 选择哪个逻辑根的哪个编码
#[derive(Clone, Debug)]
pub struct SimpleCodeStep {
    /// 根选择器 (A-Y, Z)
    pub root_selector: char,
    /// 编码选择器 (a-z)
    pub code_selector: char,
}

/// 简码级别配置
#[derive(Clone, Debug)]
pub struct SimpleCodeLevel {
    /// 简码级别
    pub level: usize,
    /// 该级别可分配的汉字数
    pub code_num: usize,
    /// 候选规则列表
    pub rule_candidates: Vec<Vec<SimpleCodeStep>>,
    /// 是否需要空格上屏（需求 20）：为真时该级简码尾随一个空格键 `KEY_SPACE`，
    /// 使有效击键序列末尾计入空格（影响效率排序键长度、加权当量、分布偏差与输出表示）。
    pub space_commit: bool,
}

/// 简码配置
#[derive(Clone, Debug)]
pub struct SimpleCodeConfig {
    /// 简码级别列表
    pub levels: Vec<SimpleCodeLevel>,
}

/// 固定简码（需求 21）：一条经校验、级别归属确定的「汉字 → 字面简码」映射。
///
/// 固定简码的汉字不参与退火的简码分配，其对简码各项指标的贡献为不随分配变化的常量
/// （因简码为字面键位串）。`code_str` 保留结尾下划线（如需空格上屏），供输出直接使用。
#[derive(Clone, Debug)]
pub struct FixedSimpleCode {
    /// 汉字索引（char_infos / raw_splits 下标）
    pub ci: usize,
    /// 所属简码级别索引
    pub li: usize,
    /// 核心简码键位（不含空格上屏的尾随空格）
    pub keys: Vec<u8>,
    /// 是否空格上屏（与所属级别 `space_commit` 一致）
    pub space_commit: bool,
    /// 输出用的简码字符串（含空格上屏时的尾随下划线 `_`）
    pub code_str: String,
}

/// 逻辑根 - 同一基础字的不同拆分变体
#[derive(Clone, Debug)]
pub struct LogicalRoot {
    /// 基础名称
    pub base_name: String,
    /// 在拆分中的位置索引
    pub split_part_indices: Vec<usize>,
    /// 完整编码的各部分（键位索引或分组标记）
    pub full_code_parts: Vec<u16>,
}

/// 汉字的简码信息
#[derive(Clone, Debug)]
pub struct CharSimpleInfo {
    /// 逻辑根列表
    pub logical_roots: Vec<LogicalRoot>,
    /// 每级简码的指令（root_idx, code_idx）
    pub level_instructions: Vec<Option<Vec<(usize, usize)>>>,
}

/// 解析编码选择器
/// - a: 第0个编码
/// - b-y: 中间编码
/// - z: 最后一个编码
pub fn resolve_code_index(code_selector: char, total_codes: usize) -> Option<usize> {
    if total_codes == 0 {
        return None;
    }
    match code_selector {
        'a' => Some(0),
        'z' => {
            if total_codes >= 1 {
                Some(total_codes - 1)
            } else {
                None
            }
        }
        'b'..='y' => {
            let mid_offset = (code_selector as u8 - b'b') as usize;
            let actual_index = 1 + mid_offset;
            if total_codes >= 3 && actual_index <= total_codes - 2 {
                Some(actual_index)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// 尝试解析简码规则
/// 返回 (root_idx, code_idx) 列表
pub fn try_resolve_rule(
    rule: &[SimpleCodeStep],
    logical_roots: &[LogicalRoot],
    n_roots: usize,
) -> Option<Vec<(usize, usize)>> {
    let mut instructions: Vec<(usize, usize)> = Vec::with_capacity(rule.len());

    for step in rule {
        let root_idx = match step.root_selector {
            'A'..='Y' => {
                let idx = (step.root_selector as u8 - b'A') as usize;
                if idx >= n_roots {
                    return None;
                }
                idx
            }
            'Z' => {
                if n_roots == 0 {
                    return None;
                }
                n_roots - 1
            }
            _ => return None,
        };

        let lr = &logical_roots[root_idx];
        let n_codes = lr.full_code_parts.len();

        let code_idx = resolve_code_index(step.code_selector, n_codes)?;

        instructions.push((root_idx, code_idx));
    }

    Some(instructions)
}

/// 从名称中提取基础名（去掉数字后缀）
pub fn extract_base_name(name: &str) -> String {
    if let Some(dot_pos) = name.rfind('.') {
        let suffix = &name[dot_pos + 1..];
        if suffix.chars().all(|c| c.is_ascii_digit()) {
            return name[..dot_pos].to_string();
        }
    }
    name.to_string()
}

/// 从名称中提取数字后缀
pub fn extract_suffix_num(name: &str) -> i32 {
    if let Some(dot_pos) = name.rfind('.') {
        let suffix = &name[dot_pos + 1..];
        if let Ok(n) = suffix.parse::<i32>() {
            return n;
        }
    }
    -1
}

/// 构建根名的完整编码映射
pub fn build_root_full_codes(
    fixed_roots: &HashMap<String, u8>,
    groups: &[RootGroup],
) -> HashMap<String, Vec<String>> {
    let mut all_names: Vec<String> = Vec::new();
    for name in fixed_roots.keys() {
        all_names.push(name.clone());
    }
    for g in groups {
        for name in &g.roots {
            all_names.push(name.clone());
        }
    }

    let mut grouped: HashMap<String, Vec<(i32, String)>> = HashMap::new();
    for name in &all_names {
        let base = extract_base_name(name);
        let suffix = extract_suffix_num(name);
        grouped
            .entry(base)
            .or_default()
            .push((suffix, name.clone()));
    }

    let mut result: HashMap<String, Vec<String>> = HashMap::new();
    for (base, mut entries) in grouped {
        entries.sort_by_key(|(s, _)| *s);
        let names: Vec<String> = entries.into_iter().map(|(_, n)| n).collect();
        result.insert(base, names);
    }

    result
}

/// 从根名列表提取逻辑根
pub fn extract_logical_roots_full(
    root_names: &[String],
    _parts: &[u16],
    root_full_codes: &HashMap<String, Vec<String>>,
    fixed_roots: &HashMap<String, u8>,
    root_to_group: &HashMap<String, usize>,
) -> Vec<LogicalRoot> {
    let mut logical_roots: Vec<LogicalRoot> = Vec::new();

    for (idx, name) in root_names.iter().enumerate() {
        let base = extract_base_name(name);
        let suffix = extract_suffix_num(name);

        if suffix <= 0 {
            let full_names = root_full_codes
                .get(&base)
                .cloned()
                .unwrap_or_else(|| vec![name.clone()]);

            let full_code_parts: Vec<u16> = full_names
                .iter()
                .map(|n| {
                    if let Some(&key) = fixed_roots.get(n) {
                        key as u16
                    } else if let Some(&gi) = root_to_group.get(n) {
                        gi as u16 + GROUP_MARKER
                    } else {
                        0u16
                    }
                })
                .collect();

            logical_roots.push(LogicalRoot {
                base_name: base,
                split_part_indices: vec![idx],
                full_code_parts,
            });
        } else {
            let mut attached = false;
            for lr in logical_roots.iter_mut().rev() {
                if lr.base_name == base {
                    lr.split_part_indices.push(idx);
                    attached = true;
                    break;
                }
            }
            if !attached {
                let full_names = root_full_codes
                    .get(&base)
                    .cloned()
                    .unwrap_or_else(|| vec![name.clone()]);
                let full_code_parts: Vec<u16> = full_names
                    .iter()
                    .map(|n| {
                        if let Some(&key) = fixed_roots.get(n) {
                            key as u16
                        } else if let Some(&gi) = root_to_group.get(n) {
                            gi as u16 + GROUP_MARKER
                        } else {
                            0u16
                        }
                    })
                    .collect();

                logical_roots.push(LogicalRoot {
                    base_name: base,
                    split_part_indices: vec![idx],
                    full_code_parts,
                });
            }
        }
    }

    logical_roots
}

/// 计算每级简码的指令
pub fn compute_level_instructions(
    logical_roots: &[LogicalRoot],
    levels: &[SimpleCodeLevel],
) -> Vec<Option<Vec<(usize, usize)>>> {
    let n_roots = logical_roots.len();

    levels
        .iter()
        .map(|level| {
            for rule in &level.rule_candidates {
                if let Some(instructions) = try_resolve_rule(rule, logical_roots, n_roots) {
                    return Some(instructions);
                }
            }
            None
        })
        .collect()
}