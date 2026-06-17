//! 配置文件烟雾测试（任务 3.3）
//!
//! 解析 `config.toml.example` 与 `moling/config.toml` 配置文件，
//! 断言「简码评估性能优化」新增的 8 个配置项：
//!   1. 存在于 `[weights.simple_code]` 段；
//!   2. 在文件原始文本中带有行内注释（`#`）；
//!   3. 仅对 `config.toml.example`（规范示例文件）额外断言取值等于代码内置默认值
//!      （见 `src/config.rs` 的 `default_*` 函数）。
//!
//! 注意：`moling/config.toml` 是使用者的实验配置文件，其取值可被自由修改、不必等于
//! 代码默认值，故对它只校验「存在 + 带注释」，不断言取值（取值断言仅施于示例文件）。
//!
//! 放置于独立集成测试文件，避免与 `src/config.rs` 的单元测试冲突。
//! _Requirements: 18.1, 18.2, 18.4_

use std::path::PathBuf;

/// 代码内置默认值（与 `src/config.rs` 的 `default_*` 函数保持一致）。
/// 每项为 (键名, 期望值)。
#[derive(Clone, Copy)]
enum Expected {
    Float(f64),
    Str(&'static str),
    Int(i64),
}

const EXPECTED_ITEMS: &[(&str, Expected)] = &[
    ("simple_start_progress", Expected::Float(0.4)),
    ("simple_ramp_progress", Expected::Float(0.1)),
    ("simple_activation_reheat", Expected::Float(1.2)),
    ("simple_coverage_ratio", Expected::Float(1.0)),
    ("simple_active_coverage", Expected::Float(0.90)),
    ("reconcile_interval_ratio", Expected::Float(0.05)),
    ("simple_assign_mode", Expected::Str("efficiency")),
    ("simple_protect_top_n", Expected::Int(0)),
];

/// 相对于 `CARGO_MANIFEST_DIR`（crate 根 = 仓库根）解析配置文件路径。
fn manifest_path(relative: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push(relative);
    p
}

/// 在原始 toml 文本中查找某个键的定义行，并断言该行带有行内注释（`#`）。
fn assert_line_has_comment(raw: &str, key: &str) {
    let def_line = raw.lines().find(|line| {
        let trimmed = line.trim_start();
        // 形如 `key = ...`（容忍键名后的空白与等号）
        trimmed
            .strip_prefix(key)
            .map(|rest| rest.trim_start().starts_with('='))
            .unwrap_or(false)
    });

    let def_line = def_line
        .unwrap_or_else(|| panic!("未在配置文件中找到配置项 `{key}` 的定义行"));

    assert!(
        def_line.contains('#'),
        "配置项 `{key}` 的定义行缺少注释: {def_line:?}"
    );
}

/// 对单个配置文件执行烟雾断言。
///
/// `check_value` 为真时额外断言取值等于代码内置默认（仅用于规范示例文件
/// `config.toml.example`）；对使用者实验文件 `moling/config.toml` 传入 false，
/// 只校验配置项存在与带注释，不耦合其取值与默认值。
fn check_config_file(relative: &str, check_value: bool) {
    let path = manifest_path(relative);
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("无法读取配置文件 {}: {e}", path.display()));

    let parsed: toml::Value = toml::from_str(&raw)
        .unwrap_or_else(|e| panic!("解析配置文件 {} 失败: {e}", path.display()));

    let simple = parsed
        .get("weights")
        .and_then(|w| w.get("simple_code"))
        .and_then(|s| s.as_table())
        .unwrap_or_else(|| {
            panic!("配置文件 {} 缺少 [weights.simple_code] 段", path.display())
        });

    for (key, expected) in EXPECTED_ITEMS {
        // 1) 存在性
        let value = simple
            .get(*key)
            .unwrap_or_else(|| panic!("[{relative}] 缺少配置项 `{key}`"));

        // 2) 取值等于代码内置默认（仅规范示例文件）
        if check_value {
            match expected {
                Expected::Float(exp) => {
                    let got = value
                        .as_float()
                        .unwrap_or_else(|| panic!("[{relative}] 配置项 `{key}` 不是浮点数: {value:?}"));
                    assert_eq!(
                        got, *exp,
                        "[{relative}] 配置项 `{key}` 默认值与代码内置默认不一致"
                    );
                }
                Expected::Str(exp) => {
                    let got = value
                        .as_str()
                        .unwrap_or_else(|| panic!("[{relative}] 配置项 `{key}` 不是字符串: {value:?}"));
                    assert_eq!(
                        got, *exp,
                        "[{relative}] 配置项 `{key}` 默认值与代码内置默认不一致"
                    );
                }
                Expected::Int(exp) => {
                    let got = value
                        .as_integer()
                        .unwrap_or_else(|| panic!("[{relative}] 配置项 `{key}` 不是整数: {value:?}"));
                    assert_eq!(
                        got, *exp,
                        "[{relative}] 配置项 `{key}` 默认值与代码内置默认不一致"
                    );
                }
            }
        }

        // 3) 带注释
        assert_line_has_comment(&raw, key);
    }
}

#[test]
fn config_toml_example_has_new_simple_code_items_with_defaults_and_comments() {
    // 规范示例文件：断言存在性 + 取值等于代码默认 + 带注释。
    check_config_file("config.toml.example", true);
}

#[test]
fn moling_config_toml_has_new_simple_code_items_with_comments() {
    // 需求 18.2：新增配置项同步写入 moling/config.toml（便于使用）。
    // moling 为使用者实验文件，取值可被自由修改，故只校验存在性与带注释，不断言取值。
    check_config_file("moling/config.toml", false);
}

/// 校验 `[annealing]` 段的 `checkpoint_interval_ratio`（断点续算特性，需求 9.4）。
/// `check_value` 为真时额外断言取值等于代码默认 0.05（仅示例文件）。
fn check_checkpoint_interval_ratio(relative: &str, check_value: bool) {
    let path = manifest_path(relative);
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("无法读取配置文件 {}: {e}", path.display()));
    let parsed: toml::Value = toml::from_str(&raw)
        .unwrap_or_else(|e| panic!("解析配置文件 {} 失败: {e}", path.display()));
    let ann = parsed
        .get("annealing")
        .and_then(|s| s.as_table())
        .unwrap_or_else(|| panic!("配置文件 {} 缺少 [annealing] 段", path.display()));
    let value = ann
        .get("checkpoint_interval_ratio")
        .unwrap_or_else(|| panic!("[{relative}] [annealing] 缺少 `checkpoint_interval_ratio`"));
    if check_value {
        let got = value
            .as_float()
            .unwrap_or_else(|| panic!("[{relative}] `checkpoint_interval_ratio` 不是浮点数: {value:?}"));
        assert_eq!(got, 0.05, "[{relative}] `checkpoint_interval_ratio` 默认值应为 0.05");
    }
    assert_line_has_comment(&raw, "checkpoint_interval_ratio");
}

#[test]
fn config_toml_example_has_checkpoint_interval_ratio() {
    check_checkpoint_interval_ratio("config.toml.example", true);
}

#[test]
fn moling_config_toml_has_checkpoint_interval_ratio() {
    check_checkpoint_interval_ratio("moling/config.toml", false);
}
