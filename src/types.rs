// =========================================================================
// 🚀 基础数据类型
// =========================================================================

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

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
    /// 退火期 active 候选覆盖率（须 ≤ simple_coverage_ratio）；passive 候选仅在最终上报/输出纳入。
    pub simple_active_coverage: f64,
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
            simple_coverage_ratio: 1.0,
            // 测试/程序化默认取 1.0（active = 全集，不启用 active/passive 近似），
            // 与面向用户的配置默认（config.rs 的 0.90）有意不同：使既有以 WeightConfig::default()
            // 构建、并设 simple_coverage_ratio=1.0 的测试保持「全部字均为 active 候选」的行为。
            // active/passive 拆分的针对性测试会显式设置 simple_active_coverage < 1.0。
            simple_active_coverage: 1.0,
            simple_assign_mode: SimpleAssignMode::Efficiency,
            simple_protect_top_n: 0,
        }
    }
}

/// 缩放配置 - 用于将不同量纲的指标归一化
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
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
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
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
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
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
    /// 上屏键序列（需求 20，取代旧 `space_commit` 布尔）：解析后的上屏键键位索引，
    /// 按优先级排序（字母→键索引、`_`→`KEY_SPACE`）。为空表示自动上屏（无额外上屏键）。
    pub commit_keys: Vec<u8>,
    /// 偏好表（需求 36）：核心末键属**左手**时，桶内名次 i 的出简字所用上屏键序列。
    /// 退火前预计算，长度 = `commit_keys` 去重后的不同键数 K。`commit_keys` 为空则为空表。
    pub commit_pref_last_left: Vec<u8>,
    /// 偏好表（需求 36）：核心末键属**右手**时的上屏键序列。
    pub commit_pref_last_right: Vec<u8>,
    /// 上屏键异手过滤（需求 37）：为真时退火出简仅用「与核心末键异手的字母上屏键」与 `_`，
    /// 排除同手字母上屏键（手感优化）。缺省 false（不过滤）。
    pub commit_alt_hand_only: bool,
}

impl SimpleCodeLevel {
    /// 该级是否配置了上屏键（非空即追加一个上屏键、有效码长 +1）。
    #[inline]
    pub fn has_commit(&self) -> bool {
        !self.commit_keys.is_empty()
    }

    /// K = 不同上屏键数（即偏好表长度）。
    #[inline]
    pub fn commit_k(&self) -> usize {
        self.commit_keys.len()
    }

    /// 测试/兼容辅助：由上屏键序列构造级别并预计算偏好表。
    pub fn with_commit_keys(
        level: usize,
        code_num: usize,
        rule_candidates: Vec<Vec<SimpleCodeStep>>,
        commit_keys: Vec<u8>,
    ) -> Self {
        let (commit_pref_last_left, commit_pref_last_right) = build_commit_pref_tables(&commit_keys);
        SimpleCodeLevel {
            level,
            code_num,
            rule_candidates,
            commit_keys,
            commit_pref_last_left,
            commit_pref_last_right,
            commit_alt_hand_only: false,
        }
    }

    /// 测试/兼容辅助：由旧 `space_commit` 布尔构造级别（true→`["_"]`、false→空）。
    pub fn with_space_commit(
        level: usize,
        code_num: usize,
        rule_candidates: Vec<Vec<SimpleCodeStep>>,
        space_commit: bool,
    ) -> Self {
        let commit_keys = if space_commit {
            vec![KEY_SPACE as u8]
        } else {
            Vec::new()
        };
        Self::with_commit_keys(level, code_num, rule_candidates, commit_keys)
    }
}

/// 简码配置
#[derive(Clone, Debug)]
pub struct SimpleCodeConfig {
    /// 简码级别列表
    pub levels: Vec<SimpleCodeLevel>,
}

/// 固定简码（需求 21）：一条经校验的「汉字 → 字面简码」映射。
///
/// 固定简码的汉字不参与退火的简码分配，其对简码各项指标的贡献为不随分配变化的常量
/// （因简码为字面键位串）。`code_str` 为完整输出串（含末位上屏键字符），供输出直接使用。
#[derive(Clone, Debug)]
pub struct FixedSimpleCode {
    /// 汉字索引（char_infos / raw_splits 下标）
    pub ci: usize,
    /// 所属简码级别索引；`None` 表示无法归属任何级别（仍输出、仍排除候选，但不占 commit slot）。
    pub li: Option<usize>,
    /// 核心简码键位（不含末位上屏键）
    pub keys: Vec<u8>,
    /// 末位上屏键（`Some(k)`：字母键或 `KEY_SPACE`；`None`：无上屏键 / 自动上屏）。
    pub commit_key: Option<u8>,
    /// 输出用的简码字符串（含末位上屏键字符，空格为 `_`）
    pub code_str: String,
}

/// 键盘主区按键的左右手归属（需求 36）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hand {
    Left,
    Right,
    /// 中性键（空格 `_`），不属任何手。
    Neutral,
}

/// 按标准 QWERTY 物理布局返回键位的左右手归属（需求 36）。
///
/// 键位索引为字母表序（a=0..z=25），故需按字母的 QWERTY 物理位置判定：
/// - 左手：`q w e r t / a s d f g / z x c v b`
/// - 右手：`y u i o p / h j k l ; / n m , . /`
/// - 中性：空格 `_`（`KEY_SPACE`）。
#[inline]
pub fn key_hand(key: u8) -> Hand {
    match key {
        26 => Hand::Neutral, // KEY_SPACE
        27 | 28 | 29 | 30 => Hand::Right, // ; , . /
        0..=25 => match (key + b'a') as char {
            'q' | 'w' | 'e' | 'r' | 't' | 'a' | 's' | 'd' | 'f' | 'g' | 'z' | 'x' | 'c' | 'v'
            | 'b' => Hand::Left,
            _ => Hand::Right, // y u i o p h j k l n m
        },
        _ => Hand::Neutral,
    }
}

/// 构建某级的两张上屏键偏好表（需求 36.2）。
///
/// 入参 `commit_keys` 为解析后的上屏键序列（含 `KEY_SPACE` 表示空格，且 `_` 已校验只在首/尾）。
/// 返回 `(pref_last_left, pref_last_right)`：
/// - `pref_last_left`：核心末键属左手时使用——对侧（右手）字母在前、同侧（左手）字母在后；
/// - `pref_last_right`：核心末键属右手时使用——对侧（左手）字母在前、同侧（右手）字母在后；
/// 字母组内保持原相对顺序；`_`（若有）按其在 `commit_keys` 的首/尾位置加到表首或表尾。
/// 两表长度均为 K = `commit_keys.len()`。退火前一次性预计算，热路径零分配。
pub fn build_commit_pref_tables(commit_keys: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let sp = KEY_SPACE as u8;
    let has_us = commit_keys.iter().any(|&k| k == sp);
    let us_at_start = has_us && commit_keys.first() == Some(&sp);
    let mut left: Vec<u8> = Vec::new();
    let mut right: Vec<u8> = Vec::new();
    for &k in commit_keys {
        if k == sp {
            continue;
        }
        match key_hand(k) {
            Hand::Left => left.push(k),
            // 右手字母与 `; , . /` 等右区键；中性（理论上不会是字母上屏键）归右。
            _ => right.push(k),
        }
    }
    let assemble = |opp: &[u8], same: &[u8]| -> Vec<u8> {
        let mut v = Vec::with_capacity(opp.len() + same.len() + 1);
        if us_at_start {
            v.push(sp);
        }
        v.extend_from_slice(opp);
        v.extend_from_slice(same);
        if has_us && !us_at_start {
            v.push(sp);
        }
        v
    };
    // 核心末键左手 → 对侧=右手在前；核心末键右手 → 对侧=左手在前。
    let pref_last_left = assemble(&right, &left);
    let pref_last_right = assemble(&left, &right);
    (pref_last_left, pref_last_right)
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

// =========================================================================
// 🧪 上屏键左右手偏好与手别划分测试（需求 36）
// =========================================================================
#[cfg(test)]
mod commit_pref_tests {
    use super::*;

    fn k(c: char) -> u8 {
        char_to_key_index(c).unwrap() as u8
    }

    #[test]
    fn key_hand_qwerty_layout() {
        for c in "qwertasdfgzxcvb".chars() {
            assert_eq!(key_hand(k(c)), Hand::Left, "'{}' 应为左手", c);
        }
        for c in "yuiophjklnm".chars() {
            assert_eq!(key_hand(k(c)), Hand::Right, "'{}' 应为右手", c);
        }
        // ; , . / 为右区
        for c in ";,./".chars() {
            assert_eq!(key_hand(k(c)), Hand::Right, "'{}' 应为右手", c);
        }
        // 空格为中性
        assert_eq!(key_hand(KEY_SPACE as u8), Hand::Neutral);
    }

    #[test]
    fn pref_tables_dk_underscore_tail() {
        // "dk_"：d 左、k 右、_ 末。
        let ck = vec![k('d'), k('k'), KEY_SPACE as u8];
        let (last_left, last_right) = build_commit_pref_tables(&ck);
        // 核心末键左手 → 对侧(右)在前：[k, d, _]
        assert_eq!(last_left, vec![k('k'), k('d'), KEY_SPACE as u8]);
        // 核心末键右手 → 对侧(左)在前：[d, k, _]
        assert_eq!(last_right, vec![k('d'), k('k'), KEY_SPACE as u8]);
    }

    #[test]
    fn pref_tables_underscore_head() {
        // "_dk"：_ 首、d 左、k 右。
        let ck = vec![KEY_SPACE as u8, k('d'), k('k')];
        let (last_left, last_right) = build_commit_pref_tables(&ck);
        assert_eq!(last_left, vec![KEY_SPACE as u8, k('k'), k('d')]);
        assert_eq!(last_right, vec![KEY_SPACE as u8, k('d'), k('k')]);
    }

    #[test]
    fn pref_tables_space_only_and_empty() {
        // "_" → 两手均 [空格]，等价旧 space_commit=true。
        let (l, r) = build_commit_pref_tables(&[KEY_SPACE as u8]);
        assert_eq!(l, vec![KEY_SPACE as u8]);
        assert_eq!(r, vec![KEY_SPACE as u8]);
        // "" → 空表，等价旧 space_commit=false。
        let (l0, r0) = build_commit_pref_tables(&[]);
        assert!(l0.is_empty() && r0.is_empty());
    }

    #[test]
    fn pref_tables_preserve_intra_hand_order() {
        // "fjdk"：f 左、j 右、d 左、k 右 → 左=[f,d]、右=[j,k]（保序）。
        let ck = vec![k('f'), k('j'), k('d'), k('k')];
        let (last_left, last_right) = build_commit_pref_tables(&ck);
        // 末键左手 → 右在前：[j,k,f,d]
        assert_eq!(last_left, vec![k('j'), k('k'), k('f'), k('d')]);
        // 末键右手 → 左在前：[f,d,j,k]
        assert_eq!(last_right, vec![k('f'), k('d'), k('j'), k('k')]);
    }
}
