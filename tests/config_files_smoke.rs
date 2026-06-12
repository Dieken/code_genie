//! 配置文件烟雾测试（任务 3.3）
//!
//! 解析 `config.toml.example` 与 `moling/config.toml` 配置文件，
//! 断言「简码评估性能优化」新增的 6 个配置项：
//!   1. 存在于 `[weights.simple_code]` 段；
//!   2. 取值等于代码内置默认值（见 `src/config.rs` 的 `default_*` 函数）；
//!   3. 在文件原始文本中带有行内注释（`#`）。
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
}

const EXPECTED_ITEMS: &[(&str, Expected)] = &[
    ("simple_start_progress", Expected::Float(0.4)),
    ("simple_ramp_progress", Expected::Float(0.1)),
    ("simple_activation_reheat", Expected::Float(1.2)),
    ("simple_coverage_ratio", Expected::Float(0.90)),
    ("reconcile_interval_ratio", Expected::Float(0.05)),
    ("simple_assign_mode", Expected::Str("efficiency")),
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

/// 对单个配置文件执行完整的烟雾断言。
fn check_config_file(relative: &str) {
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
        // 1) 存在性 + 2) 取值等于代码内置默认
        let value = simple
            .get(*key)
            .unwrap_or_else(|| panic!("[{relative}] 缺少配置项 `{key}`"));

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
        }

        // 3) 带注释
        assert_line_has_comment(&raw, key);
    }
}

#[test]
fn config_toml_example_has_new_simple_code_items_with_defaults_and_comments() {
    check_config_file("config.toml.example");
}

#[test]
fn moling_config_toml_has_new_simple_code_items_with_defaults_and_comments() {
    // 需求 18.2：新增配置项同步写入 moling/config.toml。
    check_config_file("moling/config.toml");
}
