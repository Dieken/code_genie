// =========================================================================
// 📂 文件加载模块
// =========================================================================

use std::collections::{HashMap, HashSet};
use std::fs;

use crate::types::{char_to_key_index, KeyDistConfig, RootGroup, KEY_SPACE};

/// 加载固定字根和受限字根组
/// 
/// # 返回值
/// - (固定字根映射, 受限字根组)
pub fn load_fixed(path: &str) -> (HashMap<String, u8>, Vec<RootGroup>) {
    let content = fs::read_to_string(path).expect("无法读取固定字根文件");
    let mut truly_fixed: HashMap<String, u8> = HashMap::new();
    let mut constrained: Vec<RootGroup> = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 2 {
            let roots: Vec<String> = parts[0].split_whitespace().map(|s| s.to_string()).collect();
            if roots.is_empty() {
                continue;
            }
            let keys: Vec<u8> = parts[1]
                .split_whitespace()
                .filter_map(|s| {
                    s.chars()
                        .next()
                        .and_then(char_to_key_index)
                        .map(|i| i as u8)
                })
                .collect();

            if keys.len() == 1 {
                for root in roots {
                    truly_fixed.insert(root, keys[0]);
                }
            } else if keys.len() > 1 {
                constrained.push(RootGroup {
                    roots,
                    allowed_keys: keys,
                });
            }
        }
    }
    (truly_fixed, constrained)
}

/// 加载动态字根组
pub fn load_dynamic(path: &str, constrained: &[RootGroup], allowed_keys: &str) -> Vec<RootGroup> {
    let global_allowed: Vec<u8> = allowed_keys
        .chars()
        .filter_map(char_to_key_index)
        .map(|i| i as u8)
        .collect();

    let content = fs::read_to_string(path).expect("无法读取动态字根文件");

    let mut existing: HashSet<String> = HashSet::new();
    for g in constrained {
        for r in &g.roots {
            existing.insert(r.clone());
        }
    }

    let mut groups: Vec<RootGroup> = constrained.to_vec();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let roots: Vec<String> = line
            .split_whitespace()
            .map(|s| s.to_string())
            .filter(|s| !existing.contains(s))
            .collect();

        if roots.is_empty() {
            continue;
        }

        let mut merged = false;
        for g in &mut groups {
            if roots.iter().any(|r| g.roots.contains(r)) {
                for r in &roots {
                    if !g.roots.contains(r) && !existing.contains(r) {
                        g.roots.push(r.clone());
                        existing.insert(r.clone());
                    }
                }
                merged = true;
                break;
            }
        }

        if !merged {
            for r in &roots {
                existing.insert(r.clone());
            }
            groups.push(RootGroup {
                roots,
                allowed_keys: global_allowed.clone(),
            });
        }
    }

    groups
}

/// 加载拆分表
/// 
/// # 返回值
/// - Vec<(字符, 根名列表, 频率)>
pub fn load_splits(path: &str) -> Vec<(char, Vec<String>, u64)> {
    let content = fs::read_to_string(path).expect("无法读取拆分表");
    let mut res = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 2 {
            let ch = parts[0].chars().next().unwrap();
            let roots: Vec<String> = parts[1].split_whitespace().map(|s| s.to_string()).collect();
            let freq: u64 = if parts.len() >= 3 {
                parts[2].trim().parse().unwrap_or(1)
            } else {
                1
            };
            res.push((ch, roots, freq));
        }
    }
    res
}

/// 加载字根对当量表
/// 
/// # 返回值
/// - 31x31 当量矩阵
pub fn load_pair_equivalence(path: &str) -> [[f64; 31]; 31] {
    let mut table = [[0.0f64; 31]; 31];
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => {
            println!("警告: 无法读取当量文件 {}，使用默认值0", path);
            return table;
        }
    };
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 2 {
            let chars: Vec<char> = parts[0].chars().collect();
            if chars.len() == 2 {
                if let (Some(k1), Some(k2)) =
                    (char_to_key_index(chars[0]), char_to_key_index(chars[1]))
                {
                    if let Ok(equiv) = parts[1].trim().parse::<f64>() {
                        if k1 < 31 && k2 < 31 {
                            table[k1][k2] = equiv;
                        }
                    }
                }
            }
        }
    }
    table
}

/// 从 keymap 文件加载字根到键位的映射
///
/// keymap 文件格式: 字根名\t编码\t使用次数
/// 编码格式: 首字母大写的键位字符串，如 Wko -> [w, k, o]
///
/// 需要同时传入 division 文件路径，以确定每个基础字根的实际子字根后缀列表。
/// 例如 keymap 中 `口	Wko` 表示口的编码为 [w, k, o]，
/// 而 division 中口的子字根为 口、口.1、口.2，
/// 因此映射为: 口=w, 口.1=k, 口.2=o
///
/// # 返回值
/// - HashMap<String, u8>: 子字根名 -> 键位索引
pub fn load_keymap(keymap_path: &str, division_path: &str) -> HashMap<String, u8> {
    use crate::types::{extract_base_name, extract_suffix_num};

    // 第一步：从 division 文件中提取每个基础字根的子字根后缀列表
    let splits = load_splits(division_path);
    let mut base_to_suffixes: HashMap<String, Vec<i32>> = HashMap::new();

    for (_, roots, _) in &splits {
        for root in roots {
            let base = extract_base_name(root);
            let suffix = extract_suffix_num(root);
            let entry = base_to_suffixes.entry(base).or_default();
            if !entry.contains(&suffix) {
                entry.push(suffix);
            }
        }
    }

    // 对每个基础字根的后缀列表排序
    for suffixes in base_to_suffixes.values_mut() {
        suffixes.sort();
    }

    // 第二步：解析 keymap 文件
    let content = fs::read_to_string(keymap_path).expect("无法读取 keymap 文件");
    let mut root_to_key: HashMap<String, u8> = HashMap::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() < 2 {
            continue;
        }
        let base_name = parts[0].trim();
        let encoding = parts[1].trim().to_lowercase();
        let keys: Vec<u8> = encoding
            .chars()
            .filter_map(|c| char_to_key_index(c).map(|i| i as u8))
            .collect();

        if keys.is_empty() {
            continue;
        }

        // 获取该基础字根的后缀列表
        let suffixes = base_to_suffixes
            .get(base_name)
            .cloned()
            .unwrap_or_else(|| {
                // 如果 division 中没有该字根，使用默认后缀 [-1, 1, 2, ...]
                let mut default_suffixes = vec![-1];
                for i in 1..keys.len() as i32 {
                    default_suffixes.push(i);
                }
                default_suffixes
            });

        // 将键位按后缀顺序映射到子字根名
        for (i, &key) in keys.iter().enumerate() {
            if i < suffixes.len() {
                let suffix = suffixes[i];
                let sub_name = if suffix < 0 {
                    base_name.to_string()
                } else {
                    format!("{}.{}", base_name, suffix)
                };
                root_to_key.insert(sub_name, key);
            }
        }
    }

    root_to_key
}

/// 将 keymap（子字根名 → 键位）据当前 `ctx` 折叠为退火初始分配（assignment），并做约束校验。
///
/// 用于「从既有结果给 optimize 播种」：把一份既有 `output-keymap.txt` 解析（`load_keymap`）
/// 得到的「子字根名 → 键位」映射，按**当前** `ctx` 的字根组结构折叠成 `assignment[group] = key`。
///
/// 校验（均基于当前 `ctx`，不依赖种子目录里备份的旧输入）：
/// - 仅处理属于动态/受限组的字根名；固定字根名不在 `ctx.root_to_group` 中，跳过；
/// - 键位必须 ∈ 该组 `allowed_keys`，否则返回 `Err`（含字根名与键位）；
/// - 同组多个子字根名映射到不同键位（组内不一致）返回 `Err`（含组信息）。
///
/// 缺失组（种子未覆盖到的组）从其 `allowed_keys` 用 `rng` 随机合法填充，使最终分配为
/// 当前方案下的合法分配；返回随机填充的组数 `filled` 供调用方日志提醒。
///
/// 所有运行时量（`ctx.num_groups`、各组 `allowed_keys`）均取自 `ctx`，不写死。
///
/// # 返回值
/// - `Ok((assignment, filled))`：`assignment` 长度为 `ctx.num_groups`，每组键位均合法；
///   `filled` 为随机填充的缺失组数。
/// - `Err(msg)`：键位非法或组内不一致。
pub fn keymap_to_assignment(
    ctx: &crate::context::OptContext,
    root_to_key: &HashMap<String, u8>,
    rng: &mut impl rand::Rng,
) -> Result<(Vec<u8>, usize), String> {
    let n = ctx.num_groups;
    // None = 未赋值；用于检测缺失组与组内不一致
    let mut chosen: Vec<Option<u8>> = vec![None; n];

    for (name, &key) in root_to_key {
        // 仅处理属于动态/受限组的字根名；固定字根（fixed_roots）不在 root_to_group 中 → 跳过
        let gi = match ctx.root_to_group.get(name) {
            Some(&gi) => gi,
            None => continue,
        };
        // 约束 1：键位必须在该组 allowed_keys 内
        if !ctx.groups[gi].allowed_keys.contains(&key) {
            return Err(format!(
                "字根 '{}' 的键位 {} 不在组 {} 的 allowed_keys {:?} 内",
                name, key, gi, ctx.groups[gi].allowed_keys
            ));
        }
        // 约束 2：组内一致
        match chosen[gi] {
            Some(prev) if prev != key => {
                return Err(format!(
                    "组 {} 内字根映射到不同键位({} vs {})，种子与当前方案不一致",
                    gi, prev, key
                ));
            }
            _ => chosen[gi] = Some(key),
        }
    }

    // 缺失组：随机合法填充
    let mut filled = 0usize;
    let mut assignment = vec![0u8; n];
    for gi in 0..n {
        match chosen[gi] {
            Some(k) => assignment[gi] = k,
            None => {
                let allowed = &ctx.groups[gi].allowed_keys;
                // allowed 恒非空（由 load_fixed/load_dynamic 保证）
                assignment[gi] = allowed[rng.gen_range(0..allowed.len())];
                filled += 1;
            }
        }
    }
    Ok((assignment, filled))
}

/// 加载键位分布配置
/// 
/// # 返回值
/// - 31 个键位的分布配置
pub fn load_key_distribution(path: &str) -> [KeyDistConfig; 31] {
    let mut cfg = [KeyDistConfig::default(); 31];
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => {
            println!("警告: 无法读取用指分布文件 {}，使用默认值", path);
            return cfg;
        }
    };
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 4 {
            if let Some(ki) = parts[0].chars().next().and_then(char_to_key_index) {
                if ki < 31 {
                    cfg[ki] = KeyDistConfig {
                        target_rate: parts[1].trim().parse().unwrap_or(0.0),
                        low_penalty: parts[2].trim().parse().unwrap_or(0.0),
                        high_penalty: parts[3].trim().parse().unwrap_or(0.0),
                    };
                }
            }
        }
    }
    cfg
}

// =========================================================================
// 🧪 keymap → assignment 转换/校验测试（seed-optimize-from-result, 需求 4/5）
// =========================================================================
#[cfg(test)]
mod keymap_assignment_tests {
    use super::*;
    use crate::config::TargetsConfig;
    use crate::context::OptContext;
    use crate::types::{
        ScaleConfig, SimpleCodeConfig, WeightConfig, EQUIV_TABLE_SIZE,
    };
    use proptest::prelude::*;
    use rand::{thread_rng, Rng};

    /// 由一组 `RootGroup` 构造最小 OptContext（不启用简码）。
    fn make_ctx_from_groups(groups: Vec<RootGroup>) -> OptContext {
        let mut splits: Vec<(char, Vec<String>, u64)> = Vec::with_capacity(groups.len());
        for (gi, g) in groups.iter().enumerate() {
            let ch = char::from_u32(0x4e00 + gi as u32).unwrap();
            splits.push((ch, vec![g.roots[0].clone()], 100u64));
        }
        let fixed_roots: HashMap<String, u8> = HashMap::new();
        let equiv_table = [[0.0f64; EQUIV_TABLE_SIZE]; EQUIV_TABLE_SIZE];
        let key_dist = [KeyDistConfig::default(); EQUIV_TABLE_SIZE];
        OptContext::new(
            &splits,
            &fixed_roots,
            &groups,
            equiv_table,
            key_dist,
            ScaleConfig::default(),
            SimpleCodeConfig { levels: vec![] },
            WeightConfig::default(),
            TargetsConfig::default(),
        )
    }

    /// 由「每组的 allowed_keys」构造 ctx，组名为 g0/g1/...（每组单根）。
    fn make_ctx(group_allowed: &[Vec<u8>]) -> OptContext {
        let groups: Vec<RootGroup> = group_allowed
            .iter()
            .enumerate()
            .map(|(gi, allowed)| RootGroup {
                roots: vec![format!("g{gi}")],
                allowed_keys: allowed.clone(),
            })
            .collect();
        make_ctx_from_groups(groups)
    }

    /// 非空键位子集（取自 [0,1,2,3]，去重）。
    fn subset() -> impl Strategy<Value = Vec<u8>> {
        prop::collection::vec(0u8..4, 1..5).prop_map(|mut v| {
            v.sort();
            v.dedup();
            v
        })
    }

    /// 1..6 个组，每组一个非空 allowed_keys 子集。
    fn groups_strategy() -> impl Strategy<Value = Vec<Vec<u8>>> {
        prop::collection::vec(subset(), 1..6)
    }

    #[test]
    fn group_internal_inconsistency_errors() {
        // 同组两个子根映射到不同键 → Err（需求 4.4）
        let groups = vec![RootGroup {
            roots: vec!["a".to_string(), "b".to_string()],
            allowed_keys: vec![0, 1, 2],
        }];
        let ctx = make_ctx_from_groups(groups);
        let mut rng = thread_rng();
        let mut m: HashMap<String, u8> = HashMap::new();
        m.insert("a".to_string(), 0);
        m.insert("b".to_string(), 1);
        assert!(keymap_to_assignment(&ctx, &m, &mut rng).is_err());
    }

    #[test]
    fn fixed_root_name_skipped() {
        // 不属于任何组的字根名被跳过，不触发 allowed_keys 校验（需求 4.2）
        let groups = vec![RootGroup {
            roots: vec!["a".to_string()],
            allowed_keys: vec![0, 1, 2],
        }];
        let ctx = make_ctx_from_groups(groups);
        let mut rng = thread_rng();
        let mut m: HashMap<String, u8> = HashMap::new();
        m.insert("不在任何组的固定根".to_string(), 99); // 非法键但被跳过
        let (asg, filled) = keymap_to_assignment(&ctx, &m, &mut rng).unwrap();
        assert_eq!(filled, 1, "唯一组未覆盖应随机填充");
        assert!(ctx.groups[0].allowed_keys.contains(&asg[0]));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        // Feature: seed-optimize-from-result, Property 1: 合法种子转换得到合法且一致的分配
        #[test]
        fn prop_full_coverage_legal(group_allowed in groups_strategy()) {
            let ctx = make_ctx(&group_allowed);
            let mut rng = thread_rng();
            let mut root_to_key: HashMap<String, u8> = HashMap::new();
            let mut chosen = vec![0u8; group_allowed.len()];
            for gi in 0..group_allowed.len() {
                let allowed = &group_allowed[gi];
                let k = allowed[rng.gen_range(0..allowed.len())];
                chosen[gi] = k;
                root_to_key.insert(format!("g{gi}"), k);
            }
            let (asg, filled) = keymap_to_assignment(&ctx, &root_to_key, &mut rng).unwrap();
            prop_assert_eq!(filled, 0);
            for gi in 0..group_allowed.len() {
                prop_assert_eq!(asg[gi], chosen[gi]);
                prop_assert!(group_allowed[gi].contains(&asg[gi]));
            }
        }

        // Feature: seed-optimize-from-result, Property 2: 非法键位被拒绝
        #[test]
        fn prop_illegal_key_rejected(group_allowed in groups_strategy(), pick in 0usize..100) {
            let ctx = make_ctx(&group_allowed);
            let mut rng = thread_rng();
            let gi = pick % group_allowed.len();
            let mut root_to_key: HashMap<String, u8> = HashMap::new();
            // 99 永不在 allowed_keys（⊆ [0,3]）内 → 必为非法
            root_to_key.insert(format!("g{gi}"), 99u8);
            prop_assert!(keymap_to_assignment(&ctx, &root_to_key, &mut rng).is_err());
        }

        // Feature: seed-optimize-from-result, Property 3: 缺失组随机填充后分配合法且计数正确
        #[test]
        fn prop_partial_fill(group_allowed in groups_strategy(), mask in any::<u64>()) {
            let n = group_allowed.len();
            let ctx = make_ctx(&group_allowed);
            let mut rng = thread_rng();
            let mut root_to_key: HashMap<String, u8> = HashMap::new();
            let mut covered = 0usize;
            for gi in 0..n {
                if (mask >> gi) & 1 == 1 {
                    let allowed = &group_allowed[gi];
                    let k = allowed[rng.gen_range(0..allowed.len())];
                    root_to_key.insert(format!("g{gi}"), k);
                    covered += 1;
                }
            }
            let (asg, filled) = keymap_to_assignment(&ctx, &root_to_key, &mut rng).unwrap();
            prop_assert_eq!(filled, n - covered);
            for gi in 0..n {
                prop_assert!(group_allowed[gi].contains(&asg[gi]));
            }
        }
    }
}
