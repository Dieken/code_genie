// =========================================================================
// 📦 主上下文
// =========================================================================

use std::collections::{HashMap, HashSet};

use crate::types::{
    build_root_full_codes, char_to_key_index, key_to_char, CharInfo, CharSimpleInfo,
    compute_level_instructions, extract_logical_roots_full, pow_base, try_resolve_rule,
    FixedSimpleCode, KeyDistConfig, LogicalRoot, RootGroup, ScaleConfig, SimpleAssignMode,
    SimpleCodeConfig, WeightConfig, KEY_SPACE, EQUIV_TABLE_SIZE, GROUP_MARKER,
};
use crate::config::TargetsConfig;

/// 等价表类型别名
pub type EquivTable = [[f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];

/// 优化上下文 - 存储所有算法需要的数据
pub struct OptContext {
    /// 是否启用简码优化
    pub enable_simple_code: bool,
    /// 权重配置
    pub weights: WeightConfig,
    /// 字根组数量
    pub num_groups: usize,
    /// 字根名到组索引的映射
    pub root_to_group: HashMap<String, usize>,
    /// 组索引到使用该组的汉字索引列表的映射
    pub group_to_chars: Vec<Vec<usize>>,
    /// 汉字信息列表
    pub char_infos: Vec<CharInfo>,
    /// 原始拆分数据 (字符, 根名列表, 频率)
    pub raw_splits: Vec<(char, Vec<String>, u64)>,
    /// 字根组列表
    pub groups: Vec<RootGroup>,
    /// 固定字根映射
    pub fixed_roots: HashMap<String, u8>,
    /// 当量表
    pub equiv_table: EquivTable,
    /// 键位分布配置
    pub key_dist_config: [KeyDistConfig; EQUIV_TABLE_SIZE],
    /// 总频率
    pub total_frequency: u64,
    /// 编码基数
    pub code_base: usize,
    /// 最大码长
    pub max_parts: usize,
    /// 编码空间大小
    pub code_space: usize,
    /// 缩放配置
    pub scale_config: ScaleConfig,
    /// 简码配置
    pub simple_config: SimpleCodeConfig,
    /// 汉字简码信息
    pub char_simple_infos: Vec<CharSimpleInfo>,
    /// 每个组影响的简码汉字集合
    pub group_to_simple_affected: Vec<HashSet<usize>>,
    /// 根名的完整编码映射
    pub root_full_codes: HashMap<String, Vec<String>>,
    /// 每个组的加权频率总和（用于 key_weighted_usage 的 O(1) 更新）
    pub group_freq_sum: Vec<f64>,
    /// code_base 的幂次表（code_base_powers[i] = code_base^i），用于增量编码计算
    pub code_base_powers: Vec<usize>,
    /// 目标配置（用于目标偏差评分和 _max 硬约束检查）
    pub targets_config: TargetsConfig,

    // ====================================================================
    // 简码评估性能优化：静态预计算字段（仅在启用简码时填充，频率不变 → 全程不变）
    // ====================================================================
    /// 候选字集合：按字频降序累加直到累计覆盖率达到 `simple_coverage_ratio` 的最小前缀
    pub simple_candidate_chars: Vec<usize>,
    /// 候选字位图，按 ci 直接索引，供 O(1) 判定
    pub simple_is_candidate: Vec<bool>,
    /// 简码占用保护阈值 N（需求 33）：0 = 保护全部汉字全码；N>0 = 仅保护全字频前 N 名。
    pub simple_protect_top_n: usize,
    /// 全字频前 N 名汉字位图（按 ci 索引，需求 33）：仅 `simple_protect_top_n > 0` 时填充，
    /// 供简码评估器增量维护「受保护全码占用计数」。N=0（保护全部）时为空，改用全码桶占用判定。
    pub simple_is_topn: Vec<bool>,
    /// 候选字集合的实际累计字频覆盖率（供「配置确认」日志输出）
    pub simple_actual_coverage: f64,
    /// 每个组影响的简码汉字集合 ∩ 候选字集合（用 Vec 保证顺序确定、遍历高效）
    pub group_to_simple_affected_candidate: Vec<Vec<usize>>,
    /// base_saving 预计算：simple_base_saving[ci][li] = full_len - effective_simple_len
    /// 其中 effective_simple_len = 指令步数 + (该级 space_commit ? 1 : 0)（需求 4.5/20.3/20.4）
    pub simple_base_saving: Vec<Vec<i64>>,
    /// 出简长度资格（需求 22）：simple_eligible[ci][li] 为真当且仅当
    /// effective_simple_len(li) < full_len(ci)，即简码有效长度严格短于全码长度。
    /// 退火前静态确定，全量重建与增量更新两条路径共用同一判定。
    pub simple_eligible: Vec<Vec<bool>>,
    /// 每级简码桶向量容量 = code_base^L（L = 该级各候选指令长度的最大值）
    pub simple_level_capacity: Vec<usize>,
    /// 桶内出简排序模式
    pub simple_assign_mode: SimpleAssignMode,

    // ====================================================================
    // 固定简码（需求 21）：退火前一次性预计算的静态字段（仅在启用简码时填充，全程不变）
    // ====================================================================
    /// 固定简码字位图，按 ci 索引：为真表示该字使用固定简码、不参与退火的简码分配。
    pub simple_fixed_assigned: Vec<bool>,
    /// 固定简码桶占用：simple_fixed_occupancy[li][code] = 该级该桶被固定简码占用的名额数。
    /// 用于把每桶优化分配名额降为 `code_num - 占用数`（需求 21.7）。可能为空 `Vec`
    /// （无固定简码时），访问统一经 `simple_fixed_occ` 做边界回退。
    pub simple_fixed_occupancy: Vec<Vec<usize>>,
    /// 固定简码对简码覆盖频率的常量贡献（需求 21.8/21.9）。
    pub fixed_covered_freq: u64,
    /// 固定简码对加权当量分子的常量贡献。
    pub fixed_equiv_weighted: f64,
    /// 固定简码对加权当量分母（频率和）的常量贡献。
    pub fixed_equiv_freq_sum: u64,
    /// 固定简码对各键位使用频率的常量贡献（含空格上屏时的尾随 `KEY_SPACE`）。
    pub fixed_key_usage: [f64; EQUIV_TABLE_SIZE],
    /// 固定简码对总键击次数的常量贡献。
    pub fixed_key_presses: f64,
    /// 经校验通过的固定简码列表（供输出与测试使用）。
    pub simple_fixed_codes: Vec<FixedSimpleCode>,
}

impl OptContext {
    /// 创建新的优化上下文
    pub fn new(
        splits: &[(char, Vec<String>, u64)],
        fixed_roots: &HashMap<String, u8>,
        groups: &[RootGroup],
        equiv_table: EquivTable,
        key_dist_config: [KeyDistConfig; EQUIV_TABLE_SIZE],
        scale_config: ScaleConfig,
        simple_config: SimpleCodeConfig,
        weights: WeightConfig,
        targets_config: TargetsConfig,
    ) -> Self {
        // 无固定简码的常规构造（向后兼容既有调用点）
        Self::new_with_fixed(
            splits,
            fixed_roots,
            groups,
            equiv_table,
            key_dist_config,
            scale_config,
            simple_config,
            weights,
            targets_config,
            &[],
        )
    }

    /// 创建优化上下文（含固定简码映射，需求 21）。
    ///
    /// `fixed_simple_codes` 为 `(汉字, 字面简码串)` 列表（简码串可选以 `_` 结尾表示空格上屏）。
    /// 仅在启用简码时生效：本方法在退火前一次性完成固定简码的级别归属、一致性/长度校验、
    /// 候选字解耦与常量贡献预计算（不一致/越长/无效项告警并拒绝）。
    pub fn new_with_fixed(
        splits: &[(char, Vec<String>, u64)],
        fixed_roots: &HashMap<String, u8>,
        groups: &[RootGroup],
        equiv_table: EquivTable,
        key_dist_config: [KeyDistConfig; EQUIV_TABLE_SIZE],
        scale_config: ScaleConfig,
        simple_config: SimpleCodeConfig,
        weights: WeightConfig,
        targets_config: TargetsConfig,
        fixed_simple_codes: &[(char, String)],
    ) -> Self {
        let enable_simple_code = weights.enable_simple_code;
        let mut root_to_group: HashMap<String, usize> = HashMap::new();
        for (gi, g) in groups.iter().enumerate() {
            for r in &g.roots {
                root_to_group.insert(r.clone(), gi);
            }
        }

        let root_full_codes = build_root_full_codes(fixed_roots, groups);

        let num_groups = groups.len();
        let mut group_to_chars = vec![Vec::new(); num_groups];
        let mut char_infos = Vec::with_capacity(splits.len());
        let mut total_frequency = 0u64;
        let mut max_parts = 0usize;

        let mut char_simple_infos = Vec::with_capacity(splits.len());
        let mut group_to_simple_affected: Vec<HashSet<usize>> = vec![HashSet::new(); num_groups];

        for (ci, (_, roots, freq)) in splits.iter().enumerate() {
            let mut info = CharInfo {
                parts: Vec::with_capacity(roots.len()),
                frequency: *freq,
            };

            let mut seen_groups = HashSet::new();

            for root in roots {
                if let Some(&key) = fixed_roots.get(root) {
                    info.parts.push(key as u16);
                } else if let Some(&gi) = root_to_group.get(root) {
                    info.parts.push(gi as u16 + GROUP_MARKER);
                    seen_groups.insert(gi);
                }
            }

            if info.parts.len() > max_parts {
                max_parts = info.parts.len();
            }

            for &gi in &seen_groups {
                group_to_chars[gi].push(ci);
            }

            total_frequency += freq;

            let logical_roots = extract_logical_roots_full(
                roots,
                &info.parts,
                &root_full_codes,
                fixed_roots,
                &root_to_group,
            );
            let level_instructions =
                compute_level_instructions(&logical_roots, &simple_config.levels);

            if enable_simple_code {
                for level_cfg in &simple_config.levels {
                    for rule in &level_cfg.rule_candidates {
                        if let Some(instructions) =
                            try_resolve_rule(rule, &logical_roots, logical_roots.len())
                        {
                            for &(root_idx, code_idx) in &instructions {
                                let lr = &logical_roots[root_idx];
                                if code_idx < lr.full_code_parts.len() {
                                    let part = lr.full_code_parts[code_idx];
                                    if part >= GROUP_MARKER {
                                        let gi = (part - GROUP_MARKER) as usize;
                                        group_to_simple_affected[gi].insert(ci);
                                    }
                                }
                            }
                        }
                    }
                }
            }

            char_simple_infos.push(CharSimpleInfo {
                logical_roots,
                level_instructions,
            });

            char_infos.push(info);
        }

        let code_base = EQUIV_TABLE_SIZE + 1;
        let code_space = crate::types::pow_base(code_base, max_parts);

        // 预计算每个组的加权频率总和
        // group_freq_sum[r] = sum of freq_f for each (ci, part) where part references group r
        let mut group_freq_sum = vec![0.0f64; num_groups];
        for ci in 0..char_infos.len() {
            let freq_f = char_infos[ci].frequency as f64;
            for &p in &char_infos[ci].parts {
                if p >= GROUP_MARKER {
                    let gi = (p - GROUP_MARKER) as usize;
                    group_freq_sum[gi] += freq_f;
                }
            }
        }

        // 预计算 code_base 的幂次表
        let mut code_base_powers = vec![1usize; max_parts + 1];
        for i in 1..=max_parts {
            code_base_powers[i] = code_base_powers[i - 1] * code_base;
        }

        // ====================================================================
        // 简码评估性能优化：静态预计算字段（仅在启用简码时填充）
        // ====================================================================
        let n_chars = char_infos.len();
        let n_levels = simple_config.levels.len();
        let simple_assign_mode = weights.simple_assign_mode;

        let mut simple_candidate_chars: Vec<usize> = Vec::new();
        let mut simple_is_candidate: Vec<bool> = vec![false; n_chars];
        // 简码占用保护（需求 33）：N 从配置读取；is_topn 仅在 N>0 时填充。
        let simple_protect_top_n = weights.simple_protect_top_n;
        let mut simple_is_topn: Vec<bool> = Vec::new();
        let mut simple_actual_coverage = 0.0f64;
        let mut group_to_simple_affected_candidate: Vec<Vec<usize>> = vec![Vec::new(); num_groups];
        let mut simple_base_saving: Vec<Vec<i64>> = Vec::new();
        let mut simple_eligible: Vec<Vec<bool>> = Vec::new();
        let mut simple_level_capacity: Vec<usize> = Vec::new();

        // 固定简码（需求 21）静态字段累加器
        let mut simple_fixed_assigned: Vec<bool> = vec![false; n_chars];
        let mut simple_fixed_occupancy: Vec<Vec<usize>> = Vec::new();
        let mut fixed_covered_freq: u64 = 0;
        let mut fixed_equiv_weighted: f64 = 0.0;
        let mut fixed_equiv_freq_sum: u64 = 0;
        let mut fixed_key_usage: [f64; EQUIV_TABLE_SIZE] = [0.0; EQUIV_TABLE_SIZE];
        let mut fixed_key_presses: f64 = 0.0;
        let mut simple_fixed_codes_vec: Vec<FixedSimpleCode> = Vec::new();

        if enable_simple_code {
            // 候选字集合：按字频降序累加（并列按 ci 升序）直到覆盖率首次达标的最小前缀。
            // 注意（需求 21.3）：候选字按全集汉字选取，不受固定简码影响；固定简码字的剔除
            // 在选取完成之后进行（需求 21.4）。
            let ratio = weights.simple_coverage_ratio;
            let mut sorted_by_freq: Vec<usize> = (0..n_chars).collect();
            sorted_by_freq.sort_by(|&a, &b| {
                char_infos[b]
                    .frequency
                    .cmp(&char_infos[a].frequency)
                    .then(a.cmp(&b))
            });
            let mut cumulative = 0u64;
            if total_frequency > 0 {
                for &ci in &sorted_by_freq {
                    // ratio >= 1.0：纳入全部汉字（含频率为 0 的字，使「所有字均为候选」）；
                    // ratio < 1.0：取累计覆盖率首次达标的最小前缀（频率为 0 的尾部字被排除）。
                    if ratio < 1.0 && (cumulative as f64) / (total_frequency as f64) >= ratio {
                        break;
                    }
                    simple_candidate_chars.push(ci);
                    simple_is_candidate[ci] = true;
                    cumulative += char_infos[ci].frequency;
                }
                simple_actual_coverage = cumulative as f64 / total_frequency as f64;
            }

            // 简码占用保护（需求 33）：N>0 时标记全字频前 N 名汉字（sorted_by_freq 已按字频降序、
            // 并列 ci 升序）。N=0（保护全部）不填充 is_topn，简码评估器改用全码桶占用判定。
            if simple_protect_top_n > 0 {
                simple_is_topn = vec![false; n_chars];
                for &ci in sorted_by_freq.iter().take(simple_protect_top_n) {
                    simple_is_topn[ci] = true;
                }
            }

            // base_saving 预计算：simple_base_saving[ci][li] = full_len - effective_simple_len
            // 其中 effective_simple_len = 指令步数 + (该级 space_commit ? 1 : 0)（需求 20.3/20.4）。
            // 同时预计算出简长度资格 simple_eligible[ci][li] = effective_simple_len < full_len（需求 22）。
            simple_base_saving = vec![vec![0i64; n_levels]; n_chars];
            simple_eligible = vec![vec![false; n_levels]; n_chars];
            for ci in 0..n_chars {
                let full_len = char_infos[ci].parts.len() as i64;
                let instrs = &char_simple_infos[ci].level_instructions;
                for li in 0..n_levels {
                    let step_count = instrs
                        .get(li)
                        .and_then(|o| o.as_ref())
                        .map_or(0, |v| v.len()) as i64;
                    let space = if simple_config.levels[li].space_commit { 1 } else { 0 };
                    let effective_simple_len = step_count + space;
                    simple_base_saving[ci][li] = full_len - effective_simple_len;
                    // 仅当该级有有效简码指令（step_count > 0）且有效长度严格短于全码时才合格。
                    simple_eligible[ci][li] = step_count > 0 && effective_simple_len < full_len;
                }
            }

            // 每级简码桶容量 = code_base^L，L = 该级各候选指令长度的最大值。
            // `level_key_count[li] = max_len[li]` 同时作为「该级简码键位数」用于固定简码级别归属
            // （需求 21.5）：固定简码核心码长（去尾随 `_`）等于该值的级别即其所属级别；
            // 该口径与桶容量一致，保证固定简码桶编码 < capacity（直接索引安全）。
            simple_level_capacity = vec![1usize; n_levels];
            let mut max_len = vec![0usize; n_levels];
            for ci in 0..n_chars {
                let instrs = &char_simple_infos[ci].level_instructions;
                for li in 0..n_levels {
                    if let Some(Some(v)) = instrs.get(li) {
                        if v.len() > max_len[li] {
                            max_len[li] = v.len();
                        }
                    }
                }
            }
            for li in 0..n_levels {
                simple_level_capacity[li] = pow_base(code_base, max_len[li]);
            }

            // ============================================================
            // 固定简码处理（需求 21 / 22.3）
            // ============================================================
            if !fixed_simple_codes.is_empty() {
                // 仅在确有固定简码时才按各级 capacity 分配占用表（无固定简码时保持空 Vec，
                // simple_fixed_occ 访问器对空/越界统一回退 0）——避免高级别 code_base^L 容量的
                // 无谓分配。
                simple_fixed_occupancy = (0..n_levels)
                    .map(|li| vec![0usize; simple_level_capacity[li]])
                    .collect();

                // 汉字 → ci 映射（取首个匹配；同字多条仅首条有效）
                let mut char_to_ci: HashMap<char, usize> = HashMap::new();
                for (ci, (ch, _, _)) in splits.iter().enumerate() {
                    char_to_ci.entry(*ch).or_insert(ci);
                }

                for (ch, raw_code) in fixed_simple_codes {
                    let ci = match char_to_ci.get(ch) {
                        Some(&ci) => ci,
                        None => {
                            eprintln!(
                                "⚠️ 警告：固定简码 \"{}\" = \"{}\" 的汉字不在拆分表中，已拒绝",
                                ch, raw_code
                            );
                            continue;
                        }
                    };
                    if simple_fixed_assigned[ci] {
                        eprintln!(
                            "⚠️ 警告：汉字 \"{}\" 已有固定简码，重复项 \"{}\" 已拒绝",
                            ch, raw_code
                        );
                        continue;
                    }

                    // 结尾下划线判定与核心码串提取（仅剥离单个尾随 `_`）
                    let has_underscore = raw_code.ends_with('_');
                    let core: &str = if has_underscore {
                        &raw_code[..raw_code.len() - '_'.len_utf8()]
                    } else {
                        &raw_code[..]
                    };

                    // 核心码串 → 键位序列（与 output 的 key_to_char 反向、与 calc_simple_code 一致）
                    let mut keys: Vec<u8> = Vec::with_capacity(core.chars().count());
                    let mut bad_key = false;
                    for c in core.chars() {
                        match char_to_key_index(c) {
                            Some(k) if k < EQUIV_TABLE_SIZE => keys.push(k as u8),
                            _ => {
                                bad_key = true;
                                break;
                            }
                        }
                    }
                    if bad_key || keys.is_empty() {
                        eprintln!(
                            "⚠️ 警告：固定简码 \"{}\" = \"{}\" 含非法/空简码键位，已拒绝",
                            ch, raw_code
                        );
                        continue;
                    }
                    let core_len = keys.len();

                    // 级别归属（需求 21.5）：核心码长等于该级简码键位数（max_len）的级别。
                    let mut li_opt: Option<usize> = None;
                    for li in 0..n_levels {
                        if max_len[li] == core_len {
                            // 优先选 space_commit 与结尾下划线一致的级别（应对同长多级别）
                            if simple_config.levels[li].space_commit == has_underscore {
                                li_opt = Some(li);
                                break;
                            } else if li_opt.is_none() {
                                li_opt = Some(li);
                            }
                        }
                    }
                    let li = match li_opt {
                        Some(li) => li,
                        None => {
                            eprintln!(
                                "⚠️ 警告：固定简码 \"{}\" = \"{}\" 的码长 {} 无匹配简码级别，已拒绝",
                                ch, raw_code, core_len
                            );
                            continue;
                        }
                    };

                    // 一致性校验（需求 21.6 / 确认点 4）：以固定简码自身的结尾下划线为准
                    // （固定简码即最终输出状态，不依据级别 space_commit 重建下划线）。
                    // - space_commit=false 但固定简码以 `_` 结尾 → 配置错误，解析期报错（panic）。
                    // - space_commit=true 但固定简码无 `_` 结尾 → 仅警告，按固定简码原样接受。
                    let level_space_commit = simple_config.levels[li].space_commit;
                    if has_underscore && !level_space_commit {
                        panic!(
                            "固定简码 \"{}\" = \"{}\" 以下划线结尾（需空格上屏），但级别 {} 的 space_commit=false：配置错误",
                            ch, raw_code, simple_config.levels[li].level
                        );
                    }
                    if !has_underscore && level_space_commit {
                        eprintln!(
                            "⚠️ 警告：级别 {} 的 space_commit=true，但固定简码 \"{}\" = \"{}\" 无结尾下划线；按固定简码原样处理（不额外添加下划线）",
                            simple_config.levels[li].level, ch, raw_code
                        );
                    }
                    // 此后一律以固定简码自身的下划线 `space` 为准（确认点 4）。
                    let space = has_underscore;

                    // 长度约束（需求 22.3）：有效长度（含空格上屏）须严格小于该字全码长度。
                    let full_len = char_infos[ci].parts.len();
                    let effective_len = core_len + if space { 1 } else { 0 };
                    if effective_len >= full_len {
                        eprintln!(
                            "⚠️ 警告：固定简码 \"{}\" = \"{}\" 的有效长度 {} 不小于全码长度 {}，已拒绝",
                            ch, raw_code, effective_len, full_len
                        );
                        continue;
                    }

                    // 桶编码（与 calc_simple_code 一致：code = code*code_base + (k+1)）
                    let mut bucket_code = 0usize;
                    for &k in &keys {
                        bucket_code = bucket_code * code_base + (k as usize + 1);
                    }
                    debug_assert!(bucket_code < simple_level_capacity[li]);

                    // 占用登记（需求 21.7 / 确认点 2）：固定简码一律登记，不因占用达到/超过 code_num
                    // 而拒绝。退火端按 max(0, code_num - 占用) 出简（saturating_sub）：占满（含 code_num=0
                    // 级别）则该桶退火出 0。固定简码即权威预分配，多条碰撞同桶时全部生效（由用户配置负责）。
                    simple_fixed_assigned[ci] = true;
                    simple_fixed_occupancy[li][bucket_code] += 1;

                    let freq = char_infos[ci].frequency;
                    let freq_f = freq as f64;
                    fixed_covered_freq += freq;
                    fixed_equiv_freq_sum += freq;

                    // 当量：字面键位转移当量；divisor 取核心码长（与 calc_simple_equiv 口径一致）。
                    let mut total_equiv = 0.0f64;
                    let mut prev = keys[0] as usize;
                    for i in 1..core_len {
                        let cur = keys[i] as usize;
                        total_equiv += equiv_table[prev][cur];
                        prev = cur;
                    }
                    if space {
                        total_equiv += equiv_table[prev][KEY_SPACE];
                    }
                    let eq = total_equiv / core_len as f64;
                    fixed_equiv_weighted += eq * freq_f;

                    // 分布偏差：每个核心键位计 freq_f；空格上屏时尾随 KEY_SPACE 亦计一次。
                    for &k in &keys {
                        fixed_key_usage[k as usize] += freq_f;
                    }
                    if space {
                        fixed_key_usage[KEY_SPACE] += freq_f;
                    }
                    fixed_key_presses += freq_f * effective_len as f64;

                    // 输出字符串（以固定简码自身的下划线为准；不依据级别 space_commit 额外添加）。
                    let mut code_str: String = keys.iter().map(|&k| key_to_char(k)).collect();
                    if space {
                        code_str.push('_');
                    }
                    simple_fixed_codes_vec.push(FixedSimpleCode {
                        ci,
                        li,
                        keys,
                        space_commit: space,
                        code_str,
                    });
                }

                // 候选字解耦（需求 21.4）：从候选集合与位图剔除固定简码字。
                simple_candidate_chars.retain(|&ci| !simple_fixed_assigned[ci]);
                for ci in 0..n_chars {
                    if simple_fixed_assigned[ci] {
                        simple_is_candidate[ci] = false;
                    }
                }
            }

            // 受影响交集：group_to_simple_affected[g] ∩ 候选集（已剔除固定字），
            // 用 Vec 并排序保证顺序确定。须在固定字剔除之后计算，使固定字不进入增量受影响集。
            for (gi, affected) in group_to_simple_affected.iter().enumerate() {
                let mut inter: Vec<usize> = affected
                    .iter()
                    .copied()
                    .filter(|&ci| simple_is_candidate[ci])
                    .collect();
                inter.sort_unstable();
                group_to_simple_affected_candidate[gi] = inter;
            }
        }

        Self {
            enable_simple_code,
            weights,
            num_groups,
            root_to_group,
            group_to_chars,
            char_infos,
            raw_splits: splits.to_vec(),
            groups: groups.to_vec(),
            fixed_roots: fixed_roots.clone(),
            equiv_table,
            key_dist_config,
            total_frequency,
            code_base,
            max_parts,
            code_space,
            scale_config,
            simple_config,
            char_simple_infos,
            group_to_simple_affected,
            root_full_codes,
            group_freq_sum,
            code_base_powers,
            targets_config,
            simple_candidate_chars,
            simple_is_candidate,
            simple_protect_top_n,
            simple_is_topn,
            simple_actual_coverage,
            group_to_simple_affected_candidate,
            simple_base_saving,
            simple_eligible,
            simple_level_capacity,
            simple_assign_mode,
            simple_fixed_assigned,
            simple_fixed_occupancy,
            fixed_covered_freq,
            fixed_equiv_weighted,
            fixed_equiv_freq_sum,
            fixed_key_usage,
            fixed_key_presses,
            simple_fixed_codes: simple_fixed_codes_vec,
        }
    }

    /// 解析键位 - 将部分索引解析为实际键位
    #[inline(always)]
    pub fn resolve_key(&self, part: u16, assignment: &[u8]) -> u8 {
        if part >= GROUP_MARKER {
            assignment[(part - GROUP_MARKER) as usize]
        } else {
            part as u8
        }
    }

    /// 计算仅全码
    #[inline(always)]
    pub fn calc_code_only(&self, ci: usize, assignment: &[u8]) -> usize {
        let info = &self.char_infos[ci];
        let mut code = 0usize;
        for &p in &info.parts {
            let k = self.resolve_key(p, assignment);
            code = code * self.code_base + (k as usize + 1);
        }
        code
    }

    /// 从拆分计算等价值
    #[inline(always)]
    pub fn calc_equiv_from_parts(&self, ci: usize, assignment: &[u8]) -> f64 {
        let info = &self.char_infos[ci];
        let n = info.parts.len();
        if n == 0 {
            return 0.0;
        }

        let mut prev_key = self.resolve_key(info.parts[0], assignment) as usize;
        let mut total = 0.0;

        for i in 1..n {
            let cur_key = self.resolve_key(info.parts[i], assignment) as usize;
            total += self.equiv_table[prev_key][cur_key];
            prev_key = cur_key;
        }
        total += self.equiv_table[prev_key][KEY_SPACE];
        total / n as f64
    }

    /// 计算简码
    #[inline]
    pub fn calc_simple_code(&self, ci: usize, level_idx: usize, assignment: &[u8]) -> Option<usize> {
        let si = &self.char_simple_infos[ci];
        let instr = si.level_instructions.get(level_idx)?.as_ref()?;

        let mut code = 0usize;
        for &(root_idx, code_idx) in instr {
            let lr = &si.logical_roots[root_idx];
            if code_idx >= lr.full_code_parts.len() {
                return None;
            }
            let part = lr.full_code_parts[code_idx];
            let k = self.resolve_key(part, assignment);
            code = code * self.code_base + (k as usize + 1);
        }
        Some(code)
    }

    /// 获取简码键位列表
    pub fn get_simple_keys(&self, ci: usize, level_idx: usize, assignment: &[u8]) -> Option<Vec<u8>> {
        let si = &self.char_simple_infos[ci];
        let instr = si.level_instructions.get(level_idx)?.as_ref()?;

        let mut keys = Vec::with_capacity(instr.len());
        for &(root_idx, code_idx) in instr {
            let lr = &si.logical_roots[root_idx];
            if code_idx >= lr.full_code_parts.len() {
                return None;
            }
            let part = lr.full_code_parts[code_idx];
            keys.push(self.resolve_key(part, assignment));
        }
        Some(keys)
    }

    /// 获取简码键位列表（写入复用缓冲区的内部变体，避免热路径每步堆分配）
    ///
    /// 将键位写入 `buf`（先清空）。成功返回 `true`；当该字在该级别无有效简码
    /// 指令或编码越界时返回 `false`，此时 `buf` 内容应被视为无效。
    pub fn get_simple_keys_into(
        &self,
        ci: usize,
        level_idx: usize,
        assignment: &[u8],
        buf: &mut Vec<u8>,
    ) -> bool {
        buf.clear();
        let si = &self.char_simple_infos[ci];
        let instr = match si.level_instructions.get(level_idx).and_then(|o| o.as_ref()) {
            Some(v) => v,
            None => return false,
        };
        for &(root_idx, code_idx) in instr {
            let lr = &si.logical_roots[root_idx];
            if code_idx >= lr.full_code_parts.len() {
                buf.clear();
                return false;
            }
            let part = lr.full_code_parts[code_idx];
            buf.push(self.resolve_key(part, assignment));
        }
        true
    }

    /// 计算简码等价值
    #[inline]
    pub fn calc_simple_equiv(&self, ci: usize, level_idx: usize, assignment: &[u8]) -> f64 {
        let si = &self.char_simple_infos[ci];
        let instr = match si.level_instructions.get(level_idx) {
            Some(Some(ref v)) => v,
            _ => return 0.0,
        };
        let n = instr.len();
        if n == 0 {
            return 0.0;
        }

        let (root_idx0, code_idx0) = instr[0];
        let lr0 = &si.logical_roots[root_idx0];
        if code_idx0 >= lr0.full_code_parts.len() {
            return 0.0;
        }
        let mut prev_key = self.resolve_key(lr0.full_code_parts[code_idx0], assignment) as usize;
        let mut total = 0.0;

        for i in 1..n {
            let (ri, ci_code) = instr[i];
            let lr = &si.logical_roots[ri];
            if ci_code >= lr.full_code_parts.len() {
                return 0.0;
            }
            let cur_key = self.resolve_key(lr.full_code_parts[ci_code], assignment) as usize;
            total += self.equiv_table[prev_key][cur_key];
            prev_key = cur_key;
        }
        // 空格上屏（需求 20.7/20.8）：仅当该级 space_commit 为真时，才计入末位键到空格的
        // 转移当量。divisor 仍取指令步数 n（此处 n >= 1，无除零风险），保持「每键平均」语义
        // 一致；space_commit 为假时不含尾随空格项。该值仅依赖 (ci, li, assignment, space_commit)
        // 为确定性纯函数，故全量与增量路径天然一致。
        if self
            .simple_config
            .levels
            .get(level_idx)
            .map_or(false, |lvl| lvl.space_commit)
        {
            total += self.equiv_table[prev_key][KEY_SPACE];
        }
        total / n as f64
    }

    /// 出简长度资格（需求 22）：判断候选字 `ci` 在级别 `li` 的有效简码长度是否严格短于全码长度。
    /// 不合格时视同 `calc_simple_code` 返回 `None`（不进桶、不出简）。
    #[inline]
    pub fn simple_is_eligible(&self, ci: usize, level_idx: usize) -> bool {
        self.simple_eligible
            .get(ci)
            .and_then(|row| row.get(level_idx))
            .copied()
            .unwrap_or(false)
    }

    /// 固定简码桶占用数（需求 21.7）：返回级别 `li` 桶编码 `code` 被固定简码占用的名额。
    /// 占用表可能为空（无固定简码时），此时回退为 0。供出简选择把每桶可选名额降为
    /// `code_num - 占用数`。
    #[inline]
    pub fn simple_fixed_occ(&self, level_idx: usize, code: usize) -> usize {
        self.simple_fixed_occupancy
            .get(level_idx)
            .and_then(|row| row.get(code))
            .copied()
            .unwrap_or(0)
    }

    /// 计算简码编码，并施加出简长度资格过滤（需求 22）：不合格时返回 `None`。
    /// 供出简「候选字入桶」路径使用，使全量重建与增量更新共用同一长度资格判定。
    #[inline]
    pub fn calc_simple_code_eligible(
        &self,
        ci: usize,
        level_idx: usize,
        assignment: &[u8],
    ) -> Option<usize> {
        if !self.simple_is_eligible(ci, level_idx) {
            return None;
        }
        self.calc_simple_code(ci, level_idx, assignment)
    }
}

// =========================================================================
// 🧪 候选字集合属性测试（simple-code-perf-optimization, Property 6）
// =========================================================================
#[cfg(test)]
mod candidate_set_tests {
    use super::*;
    use crate::types::{RootGroup, ScaleConfig, SimpleCodeConfig, WeightConfig, EQUIV_TABLE_SIZE};
    use crate::evaluator::Evaluator;
    use proptest::prelude::*;
    use rand::{thread_rng, Rng};

    /// 构建启用简码的最小 OptContext：每个频率对应一个动态组（1 字根、1 单部件汉字）。
    /// 简码级别留空（候选字集合的选取仅依赖字频与覆盖率阈值，与简码指令无关），
    /// 仅设置 `enable_simple_code = true` 与给定的 `simple_coverage_ratio`，
    /// 从而触发 `OptContext::new` 对候选字集合的静态预计算。
    fn make_simple_ctx(freqs: &[u64], ratio: f64) -> OptContext {
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
        weights.simple_coverage_ratio = ratio;
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

    /// ratio=1.0 时频率为 0 的字也应纳入候选集（「所有字均为候选」）。
    #[test]
    fn coverage_ratio_one_includes_zero_freq_chars() {
        // 含两个零频字（索引 1、3）；ratio=1.0 应纳入全部 5 字。
        let freqs = [100u64, 0, 50, 0, 10];
        let ctx = make_simple_ctx(&freqs, 1.0);
        assert_eq!(
            ctx.simple_candidate_chars.len(),
            freqs.len(),
            "ratio=1.0 时候选集应纳入全部汉字（含零频字）"
        );
        for ci in 0..freqs.len() {
            assert!(
                ctx.simple_is_candidate[ci],
                "ratio=1.0 时字 {ci}（freq={}）应为候选",
                freqs[ci]
            );
        }
        assert!((ctx.simple_actual_coverage - 1.0).abs() < 1e-12);
    }

    /// ratio<1.0 时频率为 0 的尾部字仍被排除（最小前缀语义不变）。
    #[test]
    fn coverage_ratio_below_one_excludes_zero_freq_chars() {
        let freqs = [100u64, 0, 50, 0, 10];
        // 0.95 < 1.0：取覆盖率达标的最小前缀，零频字不纳入。
        let ctx = make_simple_ctx(&freqs, 0.95);
        for &ci in &[1usize, 3] {
            assert!(
                !ctx.simple_is_candidate[ci],
                "ratio<1.0 时零频字 {ci} 不应为候选"
            );
        }
    }

    /// 测试预言：按字频降序（并列 ci 升序）排序得到的下标序列。
    fn freq_desc_order(freqs: &[u64]) -> Vec<usize> {
        let mut order: Vec<usize> = (0..freqs.len()).collect();
        order.sort_by(|&a, &b| freqs[b].cmp(&freqs[a]).then(a.cmp(&b)));
        order
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        // Feature: simple-code-perf-optimization, Property 6: 候选字集合为覆盖率达标的最小频率前缀且静态
        //
        // 对任意字频分布与覆盖率阈值 ratio ∈ [0,1]：
        //   - ratio < 1.0：候选字集合应等于「按字频降序累加、使累计覆盖率首次达到或超过 ratio
        //     的最小前缀」；即累计覆盖率 ≥ ratio，且去掉其中频率最低的一个字后覆盖率 < ratio（最小性）。
        //   - ratio == 1.0：候选字集合应纳入全部汉字（含频率为 0 的字）。
        // 该集合在任意移动序列后保持不变。
        #[test]
        fn prop6_candidate_set_minimal_frequency_prefix_and_static(
            freqs in prop::collection::vec(0u64..=200, 1..=8),
            ratio in 0.0f64..=1.0,
        ) {
            let ctx = make_simple_ctx(&freqs, ratio);
            let total: u64 = freqs.iter().sum();
            let candidates = &ctx.simple_candidate_chars;
            let k = candidates.len();

            // (A) 候选集为字频降序（并列 ci 升序）的前缀
            let order = freq_desc_order(&freqs);
            prop_assert_eq!(candidates.as_slice(), &order[..k]);

            // 候选位图与候选列表一致
            for ci in 0..freqs.len() {
                prop_assert_eq!(ctx.simple_is_candidate[ci], candidates.contains(&ci));
            }

            if total == 0 {
                // 总频率为 0 时无候选字，覆盖率为 0
                prop_assert_eq!(k, 0);
                prop_assert_eq!(ctx.simple_actual_coverage, 0.0);
            } else if ratio >= 1.0 {
                // ratio == 1.0：纳入全部汉字（含零频字），覆盖率为 1.0
                prop_assert_eq!(
                    k, freqs.len(),
                    "ratio=1.0 时候选字应纳入全部汉字（含零频字）"
                );
                prop_assert_eq!(ctx.simple_actual_coverage, 1.0);
            } else {
                // (B) 覆盖率达标：候选集累计覆盖率 ≥ ratio
                let sum_c: u64 = candidates.iter().map(|&ci| freqs[ci]).sum();
                let coverage = sum_c as f64 / total as f64;
                prop_assert!(
                    coverage >= ratio,
                    "coverage {} 应 >= ratio {}",
                    coverage, ratio
                );
                // 预计算的覆盖率字段与重算一致
                prop_assert_eq!(ctx.simple_actual_coverage, coverage);

                // (C) 最小性：去掉频率最低的一个候选字（前缀末位）后覆盖率 < ratio
                if k > 0 {
                    let lowest = candidates[k - 1];
                    let sum_without = sum_c - freqs[lowest];
                    let coverage_without = sum_without as f64 / total as f64;
                    prop_assert!(
                        coverage_without < ratio,
                        "去掉最低频字后 coverage {} 应 < ratio {}",
                        coverage_without, ratio
                    );
                }
            }

            // (D) 静态性：经任意一串合法移动序列后，候选字集合保持不变
            let snapshot = candidates.clone();
            let snapshot_cov = ctx.simple_actual_coverage;
            let mut assignment = vec![0u8; freqs.len()];
            let mut ev = Evaluator::new(&ctx, &assignment);
            let mut rng = thread_rng();
            for _ in 0..16 {
                let r = rng.gen_range(0..freqs.len());
                let new_key = ctx.groups[r].allowed_keys[rng.gen_range(0..3)];
                ev.try_move(&ctx, &mut assignment, r, new_key, 1.0, &mut rng);
            }
            prop_assert_eq!(&ctx.simple_candidate_chars, &snapshot);
            prop_assert_eq!(ctx.simple_actual_coverage, snapshot_cov);
        }
    }
}

// =========================================================================
// 🧪 受影响交集属性测试（simple-code-perf-optimization, Property 7）
// =========================================================================
#[cfg(test)]
mod affected_intersection_tests {
    use super::*;
    use crate::types::{
        RootGroup, ScaleConfig, SimpleCodeConfig, SimpleCodeLevel, SimpleCodeStep, WeightConfig,
        EQUIV_TABLE_SIZE,
    };
    use proptest::prelude::*;

    /// 构建启用简码且「组确实影响简码字」的最小 OptContext。
    ///
    /// 每个频率对应一个动态组（1 个字根、1 个单部件汉字）。简码仅一个级别，
    /// 其规则为 `"Aa"`：取第 0 个逻辑根的第 0 个编码——该编码即对应该字根所属动态组的
    /// 组标记，因此每个单根汉字都会「影响」其所属字根组，使
    /// `group_to_simple_affected[g] = {g}` 恒非空，从而真正考验交集裁剪逻辑。
    fn make_affecting_ctx(freqs: &[u64], ratio: f64) -> OptContext {
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
        weights.simple_coverage_ratio = ratio;
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

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        // Feature: simple-code-perf-optimization, Property 7: 受影响交集预计算正确
        //
        // 对任意字根组 g，预计算的 group_to_simple_affected_candidate[g] 应等于
        // group_to_simple_affected[g] 与候选字集合的交集；因此仅影响非候选字的移动
        // 对应空交集，进而不触发任何简码重算且 simple_score 不变。
        #[test]
        fn prop7_affected_candidate_intersection_correct(
            freqs in prop::collection::vec(0u64..=200, 1..=8),
            ratio in 0.0f64..=1.0,
        ) {
            let ctx = make_affecting_ctx(&freqs, ratio);

            // 前置事实：规则 "Aa" 使每个单根汉字 g 影响其所属组 g（受影响集合恒非空）。
            for g in 0..ctx.num_groups {
                prop_assert!(
                    ctx.group_to_simple_affected[g].contains(&g),
                    "组 {} 的受影响集合应包含其单根汉字 {}",
                    g, g
                );
            }

            for g in 0..ctx.num_groups {
                // 由两个既有字段重算交集：group_to_simple_affected[g] ∩ candidate set
                let mut expected: Vec<usize> = ctx.group_to_simple_affected[g]
                    .iter()
                    .copied()
                    .filter(|&ci| ctx.simple_is_candidate[ci])
                    .collect();
                expected.sort_unstable();

                let actual = &ctx.group_to_simple_affected_candidate[g];

                // (A) 交集相等（核心不变量）
                prop_assert_eq!(actual, &expected);

                // (B) 仅含候选字，且确属该组受影响集合
                for &ci in actual {
                    prop_assert!(
                        ctx.simple_is_candidate[ci],
                        "组 {} 的交集含非候选字 {}",
                        g, ci
                    );
                    prop_assert!(
                        ctx.group_to_simple_affected[g].contains(&ci),
                        "组 {} 的交集含不属于受影响集合的字 {}",
                        g, ci
                    );
                }

                // (C) 受影响集合仅含非候选字 → 空交集
                let only_non_candidates = !ctx.group_to_simple_affected[g].is_empty()
                    && ctx
                        .group_to_simple_affected[g]
                        .iter()
                        .all(|&ci| !ctx.simple_is_candidate[ci]);
                if only_non_candidates {
                    prop_assert!(
                        actual.is_empty(),
                        "组 {} 仅影响非候选字, 交集应为空",
                        g
                    );
                }
            }
        }
    }
}

// =========================================================================
// 🧪 base_saving 预计算属性测试（simple-code-perf-optimization, Property 5）
// =========================================================================
#[cfg(test)]
mod base_saving_tests {
    use super::*;
    use crate::types::{
        RootGroup, ScaleConfig, SimpleCodeConfig, SimpleCodeLevel, SimpleCodeStep, WeightConfig,
        EQUIV_TABLE_SIZE,
    };
    use proptest::prelude::*;

    /// 构建启用简码且「指令真实存在」的最小 OptContext。
    ///
    /// `specs[i] = (freq, n_roots)`：第 i 个汉字含 `n_roots` 个互不相同的字根
    /// （`r{i}_0 .. r{i}_{n_roots-1}`），每个字根独立成组，因此该字全码部件数
    /// `char_infos[ci].parts.len() == n_roots`。
    ///
    /// 简码配置三个级别，规则均以 `code_selector = 'a'`（取首个编码，单组根恒有效）
    /// 选取不同数量的逻辑根，从而产生不同步数的指令：
    /// - 级别 0：规则 `[A.a]`（1 步）→ `n_roots >= 1` 时解析成功
    /// - 级别 1：规则 `[A.a, B.a]`（2 步）→ `n_roots >= 2` 时解析成功
    /// - 级别 2：规则 `[A.a, B.a, C.a]`（3 步）→ `n_roots >= 3` 时解析成功
    ///
    /// 否则该级指令为 `None`（步数计 0）。这样可同时覆盖 `Some(指令)` 与 `None` 两种分支。
    fn make_base_saving_ctx(specs: &[(u64, usize)], ratio: f64) -> OptContext {
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

        // 步数 1/2/3 的规则，分别选取逻辑根 A / A,B / A,B,C 的首个编码。
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
        weights.simple_coverage_ratio = ratio;
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

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        // Feature: simple-code-perf-optimization, Property 5: base_saving 预计算正确
        //
        // 对任意候选字 ci 与简码级别 li，预计算的 simple_base_saving[ci][li] 应等于
        // char_infos[ci].parts.len() 减去该级指令 level_instructions[li] 的步数
        // （指令为 None 时步数计 0）。
        #[test]
        fn prop5_base_saving_correct(
            specs in prop::collection::vec((0u64..=200, 1usize..=4), 1..=8),
            ratio in 0.0f64..=1.0,
        ) {
            let ctx = make_base_saving_ctx(&specs, ratio);
            let n_levels = ctx.simple_config.levels.len();

            // base_saving 表维度应与汉字数 / 级别数一致
            prop_assert_eq!(ctx.simple_base_saving.len(), ctx.char_infos.len());

            // 对每个候选字 ci 与每个级别 li 校验预计算值
            for &ci in &ctx.simple_candidate_chars {
                prop_assert_eq!(ctx.simple_base_saving[ci].len(), n_levels);

                let full_len = ctx.char_infos[ci].parts.len() as i64;
                let instrs = &ctx.char_simple_infos[ci].level_instructions;

                for li in 0..n_levels {
                    // 从同一份 level_instructions 独立重算步数（None → 0）
                    let simple_len = instrs
                        .get(li)
                        .and_then(|o| o.as_ref())
                        .map_or(0, |v| v.len()) as i64;
                    let expected = full_len - simple_len;

                    prop_assert_eq!(
                        ctx.simple_base_saving[ci][li],
                        expected,
                        "候选字 {} 级别 {}: base_saving {} 应等于 full_len {} - simple_len {}",
                        ci, li, ctx.simple_base_saving[ci][li], full_len, simple_len
                    );
                }
            }
        }
    }
}
