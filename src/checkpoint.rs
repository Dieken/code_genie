// =========================================================================
// 💾 退火进度 checkpoint — 断点续算（每线程一个文件 + 时间戳归档）
// =========================================================================
//
// 设计要点（spec: annealing-checkpoint-resume）：
// - 每个退火线程写自己的「规范文件」`thread-{NN}.json`（真实文件，恒为最新，resume 稳定入口）。
// - 写新版前，先把现有规范文件按其自身 timestamp 重命名归档为 `thread-{NN}-{TS}.json`
//   （旧版本永不删除，供手动回滚）；规范文件不使用符号链接（Windows/Unix 一致）。
// - 原子写：先写 `*.tmp` 再 `rename`，避免写入中途中断损坏文件。
// - checkpoint 只存退火控制状态 + assignment，不序列化 Evaluator（resume 时由 assignment 重建）。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::types::{Metrics, ScaleConfig, SimpleMetrics};

/// 当前 checkpoint 文件格式版本（兼容性校验）。
pub const CHECKPOINT_VERSION: u32 = 1;

/// 单个退火线程的可续算控制状态。
///
/// 不含 `Evaluator`（全码桶、简码评估器等大结构）——resume 时由 `assignment` 重建。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThreadCheckpoint {
    /// 线程 ID。
    pub thread_id: usize,
    /// 该检查点生成时间戳（毫秒级，如 "20260618-093015.123"），用于归档命名。
    pub timestamp: String,
    /// 当前解（group → key 映射）。
    pub assignment: Vec<u8>,
    /// 当前已完成步数（下次从此步继续）。
    pub current_step: usize,

    // 按分量存储的最优解。
    pub best_assignment: Vec<u8>,
    pub best_full_score: f64,
    pub best_simple_score: f64,
    pub best_score: f64,
    pub best_metrics: Metrics,
    pub best_simple_metrics: SimpleMetrics,

    // 退火控制状态。
    pub temp_multiplier: f64,
    pub steps_since_improve: usize,
    pub last_best_score: f64,

    /// 简码激活闩锁（一旦激活保持为真）。
    pub simple_activated: bool,
}

/// 本次运行的全局元信息，运行期不变；resume 时只读、视为权威。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckpointMeta {
    /// 格式版本号。
    pub version: u32,
    /// 保存时间戳。
    pub timestamp: String,
    /// 校准得到的 ScaleConfig（resume 复用，避免重新校准导致评分口径漂移）。
    pub scale_config: ScaleConfig,
    /// 总步数（来自配置）。
    pub total_steps: usize,
    /// 线程数。
    pub num_threads: usize,
    /// 实际使用的温度参数（当前直接来自配置；预留供将来自动校准温度复用）。
    pub temp_start: f64,
    pub temp_end: f64,
    pub comfort_temp: f64,
}

/// checkpoint 子目录路径：`{output_dir}/checkpoint`。
pub fn checkpoint_dir(output_dir: &str) -> PathBuf {
    Path::new(output_dir).join("checkpoint")
}

/// 规范线程检查点路径：`{ckpt_dir}/thread-{NN}.json`。
pub fn thread_path(ckpt_dir: &Path, thread_id: usize) -> PathBuf {
    ckpt_dir.join(format!("thread-{:02}.json", thread_id))
}

/// 归档线程检查点路径：`{ckpt_dir}/thread-{NN}-{TS}.json`。
pub fn thread_archive_path(ckpt_dir: &Path, thread_id: usize, ts: &str) -> PathBuf {
    ckpt_dir.join(format!("thread-{:02}-{}.json", thread_id, ts))
}

/// 元信息路径：`{ckpt_dir}/meta.json`。
pub fn meta_path(ckpt_dir: &Path) -> PathBuf {
    ckpt_dir.join("meta.json")
}

/// 生成毫秒级时间戳字符串（如 "20260618-093015.123"），供 `ThreadCheckpoint.timestamp` 使用。
pub fn now_timestamp_ms() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S%.3f").to_string()
}

/// 原子写：先写 `*.tmp` 临时文件再 `rename` 到目标路径，避免写入中途中断损坏目标文件。
fn save_atomic<T: Serialize>(value: &T, path: &Path) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let tmp_path = path.with_extension("json.tmp");
    std::fs::write(&tmp_path, &json)?;
    std::fs::rename(&tmp_path, path)?;
    Ok(())
}

/// 写出线程检查点：先归档现有规范文件（按 `prev_ts`），再原子写新规范文件。
///
/// - `prev_ts`：上一次写出（或 resume 恢复）的检查点时间戳；为 `Some(ts)` 且规范文件存在时，
///   先把规范文件重命名为 `thread-{NN}-{ts}.json`（归档，不删除）。fresh 运行首写传 `None`。
/// - 仅使用文件 `rename` 与原子写，不创建符号链接（跨平台）。
pub fn save_thread_checkpoint(
    tc: &ThreadCheckpoint,
    ckpt_dir: &Path,
    prev_ts: Option<&str>,
) -> std::io::Result<()> {
    let canonical = thread_path(ckpt_dir, tc.thread_id);
    if let Some(ts) = prev_ts {
        if canonical.exists() {
            let archive = thread_archive_path(ckpt_dir, tc.thread_id, ts);
            // 归档旧版本（不删除）。若归档目标已存在（极端同时间戳碰撞），rename 覆盖之，无害。
            std::fs::rename(&canonical, &archive)?;
        }
    } else {
        // prev_ts 为 None（fresh/续算首次写）：若复用目录残留了上一轮的规范文件，
        // 先按「重命名时刻时间戳」归档，避免误覆盖（与 optimize -d 复用目录策略一致）。
        crate::fsutil::archive_if_exists(&canonical);
    }
    save_atomic(tc, &canonical)
}

/// 写出全局元信息到 `{ckpt_dir}/meta.json`（原子写，运行期写一次）。
/// 若复用目录已存在 meta.json，先按时间戳归档旧文件再写新（避免误覆盖既有运行的元信息）。
pub fn save_meta(meta: &CheckpointMeta, ckpt_dir: &Path) -> std::io::Result<()> {
    let path = meta_path(ckpt_dir);
    crate::fsutil::archive_if_exists(&path);
    save_atomic(meta, &path)
}

/// 从文件加载线程检查点。
pub fn load_thread_checkpoint(path: &Path) -> Result<ThreadCheckpoint, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("无法读取线程检查点 {}: {}", path.display(), e))?;
    serde_json::from_str(&content)
        .map_err(|e| format!("线程检查点格式错误 {}: {}", path.display(), e))
}

/// 从 `{ckpt_dir}/meta.json` 加载元信息并校验版本兼容性。
pub fn load_meta(ckpt_dir: &Path) -> Result<CheckpointMeta, String> {
    let path = meta_path(ckpt_dir);
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("无法读取检查点元信息 {}: {}", path.display(), e))?;
    let meta: CheckpointMeta = serde_json::from_str(&content)
        .map_err(|e| format!("检查点元信息格式错误 {}: {}", path.display(), e))?;
    if meta.version != CHECKPOINT_VERSION {
        return Err(format!(
            "检查点版本不兼容: 文件版本 {}, 当前版本 {}",
            meta.version, CHECKPOINT_VERSION
        ));
    }
    Ok(meta)
}

// =========================================================================
// 🧪 checkpoint 序列化/原子写/归档保留测试
// =========================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn unique_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "cg_ckpt_{}_{}_{}",
            tag,
            std::process::id(),
            now_timestamp_ms().replace('.', "")
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn metrics(seed: u64) -> Metrics {
        Metrics {
            collision_count: (seed % 1000) as usize,
            collision_rate: (seed % 97) as f64 / 100.0,
            equiv_mean: (seed % 53) as f64 / 10.0,
            equiv_cv: (seed % 31) as f64 / 10.0,
            dist_deviation: (seed % 41) as f64 / 10.0,
        }
    }
    fn smetrics(seed: u64) -> SimpleMetrics {
        SimpleMetrics {
            weighted_freq_coverage: (seed % 100) as f64 / 100.0,
            equiv_mean: (seed % 47) as f64 / 10.0,
            dist_deviation: (seed % 29) as f64 / 10.0,
            collision_count: (seed % 500) as usize,
            collision_rate: (seed % 89) as f64 / 100.0,
        }
    }

    prop_compose! {
        fn arb_tc()(
            thread_id in 0usize..16,
            n in 1usize..40,
            step in 0usize..1_000_000,
            seed in any::<u64>(),
            bf in -1e6f64..1e6,
            bs in -1e6f64..1e6,
            tm in 0.0f64..10.0,
            ssi in 0usize..1_000_000,
            lbs in -1e6f64..1e6,
            activated in any::<bool>(),
        ) -> ThreadCheckpoint {
            let assignment: Vec<u8> = (0..n).map(|i| ((seed.wrapping_add(i as u64)) % 26) as u8).collect();
            let best_assignment: Vec<u8> = (0..n).map(|i| ((seed.wrapping_mul(3).wrapping_add(i as u64)) % 26) as u8).collect();
            ThreadCheckpoint {
                thread_id,
                timestamp: now_timestamp_ms(),
                assignment,
                current_step: step,
                best_assignment,
                best_full_score: bf,
                best_simple_score: bs,
                best_score: bf + bs,
                best_metrics: metrics(seed),
                best_simple_metrics: smetrics(seed.wrapping_mul(7)),
                temp_multiplier: tm,
                steps_since_improve: ssi,
                last_best_score: lbs,
                simple_activated: activated,
            }
        }
    }

    fn tc_eq(a: &ThreadCheckpoint, b: &ThreadCheckpoint) -> bool {
        a.thread_id == b.thread_id
            && a.timestamp == b.timestamp
            && a.assignment == b.assignment
            && a.current_step == b.current_step
            && a.best_assignment == b.best_assignment
            && a.best_full_score == b.best_full_score
            && a.best_simple_score == b.best_simple_score
            && a.best_score == b.best_score
            && a.best_metrics.collision_count == b.best_metrics.collision_count
            && a.best_metrics.collision_rate == b.best_metrics.collision_rate
            && a.best_metrics.equiv_mean == b.best_metrics.equiv_mean
            && a.best_metrics.equiv_cv == b.best_metrics.equiv_cv
            && a.best_metrics.dist_deviation == b.best_metrics.dist_deviation
            && a.best_simple_metrics.weighted_freq_coverage == b.best_simple_metrics.weighted_freq_coverage
            && a.best_simple_metrics.collision_count == b.best_simple_metrics.collision_count
            && a.temp_multiplier == b.temp_multiplier
            && a.steps_since_improve == b.steps_since_improve
            && a.last_best_score == b.last_best_score
            && a.simple_activated == b.simple_activated
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        // Feature: annealing-checkpoint-resume, Property 2: ThreadCheckpoint 序列化往返恒等
        #[test]
        fn prop2_thread_checkpoint_roundtrip(tc in arb_tc()) {
            let json = serde_json::to_string_pretty(&tc).unwrap();
            let back: ThreadCheckpoint = serde_json::from_str(&json).unwrap();
            prop_assert!(tc_eq(&tc, &back));
        }

        // Feature: annealing-checkpoint-resume, Property 4: 原子保存后可加载且与源相等（无 .tmp 残留）
        #[test]
        fn prop4_atomic_save_then_load(tc in arb_tc()) {
            let dir = unique_dir("p4");
            save_thread_checkpoint(&tc, &dir, None).unwrap();
            let loaded = load_thread_checkpoint(&thread_path(&dir, tc.thread_id)).unwrap();
            prop_assert!(tc_eq(&tc, &loaded));
            // 无 .tmp 残留
            let canonical = thread_path(&dir, tc.thread_id);
            let tmp = canonical.with_extension("json.tmp");
            prop_assert!(!tmp.exists());
            let _ = std::fs::remove_dir_all(&dir);
        }

        // Feature: annealing-checkpoint-resume, Property 6: 归档保留旧版本且规范文件恒为最新（无 symlink）
        #[test]
        fn prop6_archive_retains_history(tcs in prop::collection::vec(arb_tc(), 2..6)) {
            let dir = unique_dir("p6");
            // 固定 thread_id 与时间戳唯一，依次写出，归档应保留每个被覆盖的历史版本。
            let mut prev_ts: Option<String> = None;
            let mut written: Vec<ThreadCheckpoint> = Vec::new();
            for (i, base) in tcs.iter().enumerate() {
                let mut tc = base.clone();
                tc.thread_id = 0;
                tc.timestamp = format!("ts{:06}", i); // 唯一、可排序
                save_thread_checkpoint(&tc, &dir, prev_ts.as_deref()).unwrap();
                prev_ts = Some(tc.timestamp.clone());
                written.push(tc);
            }
            // 规范文件 = 最后一次写入
            let canonical = load_thread_checkpoint(&thread_path(&dir, 0)).unwrap();
            prop_assert!(tc_eq(&canonical, written.last().unwrap()));
            // 每个被覆盖的历史版本都以归档存在且内容一致
            for old in &written[..written.len() - 1] {
                let ap = thread_archive_path(&dir, 0, &old.timestamp);
                prop_assert!(ap.exists(), "归档缺失: {}", ap.display());
                // 规范文件不是符号链接
                let meta = std::fs::symlink_metadata(thread_path(&dir, 0)).unwrap();
                prop_assert!(!meta.file_type().is_symlink());
                let loaded = load_thread_checkpoint(&ap).unwrap();
                prop_assert!(tc_eq(&loaded, old));
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    // Feature: annealing-checkpoint-resume, Property 3: CheckpointMeta 往返恒等且版本校验
    #[test]
    fn prop3_meta_roundtrip_and_version_check() {
        let dir = unique_dir("p3");
        let meta = CheckpointMeta {
            version: CHECKPOINT_VERSION,
            timestamp: "2026-06-18 09:30:15".to_string(),
            scale_config: ScaleConfig::default(),
            total_steps: 2_000_000,
            num_threads: 8,
            temp_start: 100.0,
            temp_end: 1e-6,
            comfort_temp: 0.4,
        };
        save_meta(&meta, &dir).unwrap();
        let back = load_meta(&dir).unwrap();
        assert_eq!(back.version, meta.version);
        assert_eq!(back.total_steps, meta.total_steps);
        assert_eq!(back.num_threads, meta.num_threads);
        assert_eq!(back.temp_start, meta.temp_start);

        // 版本不符 → 报错
        let bad = CheckpointMeta { version: CHECKPOINT_VERSION + 1, ..meta.clone() };
        save_atomic(&bad, &meta_path(&dir)).unwrap();
        assert!(load_meta(&dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
