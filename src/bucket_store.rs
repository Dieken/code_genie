// =========================================================================
// 📦 桶存储抽象（BucketStore）
// =========================================================================
//
// 统一封装「按编码值 `code` 索引的桶集合」，对调用方屏蔽底层是密集 `Vec`
// 还是稀疏 `FxHashMap`（需求 1/5）。
//
// 选择依据：仅看容量上界（`code_space` 或 `code_base^L`），不看实际占用——
// 因为密集后端按容量一次性预分配，无论多稀疏都吃满 `capacity × sizeof(B)`。
// 容量超过阈值则改用稀疏后端，使内存随非空桶数（其上界为汉字数 n_chars）增长，
// 而非随容量增长（需求 5）。
//
// 关键不变量（需求 2）：调用方在桶成员降为 0 后必须调用 `remove`，使稀疏后端
// 的条目集合恒等于「当前非空桶集合」，条目数 ≤ n_chars。

use rustc_hash::FxHashMap;

/// 后端选择阈值（需求 12.6：唯一允许的编译期规模常量）。
///
/// 取 `2^21 = 2,097,152`：使 `code_space ≤ 1,048,576`（`max_parts ≤ 4`）走密集后端、
/// `code_space = 33,554,432`（`max_parts = 5`）走稀疏后端（需求 5.4）。
pub const SPARSE_THRESHOLD: usize = 1 << 21;

/// 后端类型（用于日志与测试观测）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendKind {
    Dense,
    Sparse,
}

/// 依据容量上界选择后端（纯函数，便于测试，需求 5.1/12.2）。
#[inline]
pub fn choose_backend_with_threshold(capacity: usize, threshold: usize) -> BackendKind {
    if capacity <= threshold {
        BackendKind::Dense
    } else {
        BackendKind::Sparse
    }
}

/// 依据容量上界与默认阈值选择后端。
#[inline]
pub fn choose_backend(capacity: usize) -> BackendKind {
    choose_backend_with_threshold(capacity, SPARSE_THRESHOLD)
}

// 测试用：线程局部阈值覆盖，使 `BucketStore::new`（含 Evaluator/SimpleEvaluator 内部构造）
// 可被强制走稀疏后端，从而在小 code_space 的属性测试中覆盖稀疏路径（生产不编译）。
#[cfg(test)]
thread_local! {
    static TEST_THRESHOLD: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}
#[cfg(test)]
pub(crate) fn test_threshold_override() -> Option<usize> {
    TEST_THRESHOLD.with(|c| c.get())
}
#[cfg(test)]
pub(crate) fn set_test_threshold_override(v: Option<usize>) {
    TEST_THRESHOLD.with(|c| c.set(v));
}

/// 桶元素需实现「是否为空」判定，使存储层能在两种后端下统一识别非空桶
/// （密集后端遍历/读取时据此跳过空槽；稀疏后端据此维持有界不变量）。
pub trait Bucket: Default + Clone {
    /// 该桶是否为空（无成员）。
    fn is_empty(&self) -> bool;
}

enum Backend<B: Bucket> {
    /// 按 `code` 直接索引、大小 = capacity（等价于基线实现）。
    Dense(Vec<B>),
    /// 仅存非空桶；缺失键等价于空桶（需求 2.4）。
    Sparse(FxHashMap<u32, B>),
}

/// 统一桶存储：对调用方屏蔽密集/稀疏后端（需求 5.5）。
#[allow(dead_code)] // capacity/kind 字段及若干访问器仅供日志/测试与 API 完整性
pub struct BucketStore<B: Bucket> {
    backend: Backend<B>,
    capacity: usize,
    kind: BackendKind,
}

impl<B: Bucket> BucketStore<B> {
    #![allow(dead_code)] // 部分访问器（backend_kind/capacity/nonempty_count/get_or_default）仅测试/日志用
    /// 按默认阈值构建（需求 5.1-5.4）。
    pub fn new(capacity: usize) -> Self {
        // 测试可经线程局部覆盖阈值以强制走稀疏/密集后端（生产编译时整段移除，零开销）。
        #[cfg(test)]
        {
            if let Some(t) = test_threshold_override() {
                return Self::new_with_threshold(capacity, t);
            }
        }
        Self::new_with_threshold(capacity, SPARSE_THRESHOLD)
    }

    /// 按指定阈值构建（供测试强制稀疏/密集路径）。
    pub fn new_with_threshold(capacity: usize, threshold: usize) -> Self {
        let kind = choose_backend_with_threshold(capacity, threshold);
        let backend = match kind {
            BackendKind::Dense => Backend::Dense(vec![B::default(); capacity]),
            BackendKind::Sparse => Backend::Sparse(FxHashMap::default()),
        };
        Self {
            backend,
            capacity,
            kind,
        }
    }

    /// 所选后端类型（日志/测试用）。
    #[inline]
    pub fn backend_kind(&self) -> BackendKind {
        self.kind
    }

    /// 容量上界。
    #[inline]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 只读取非空桶；空桶（密集空槽或稀疏缺失键）返回 `None`（需求 2.4）。
    ///
    /// 不变量「`Some` ⟺ 非空」在两种后端下一致成立，使 `is_code_blocked` 等判定
    /// 可统一写为 `get(code).is_some()`（需求 9.1）。
    #[inline]
    pub fn get(&self, code: u32) -> Option<&B> {
        match &self.backend {
            Backend::Dense(v) => {
                let b = &v[code as usize];
                if b.is_empty() {
                    None
                } else {
                    Some(b)
                }
            }
            Backend::Sparse(m) => m.get(&code),
        }
    }

    /// 可变取非空桶；空桶返回 `None`。
    #[inline]
    pub fn get_mut(&mut self, code: u32) -> Option<&mut B> {
        match &mut self.backend {
            Backend::Dense(v) => {
                let b = &mut v[code as usize];
                if b.is_empty() {
                    None
                } else {
                    Some(b)
                }
            }
            Backend::Sparse(m) => m.get_mut(&code),
        }
    }

    /// 取该 `code` 的桶可变引用，必要时以空桶（`Default`）插入（需求 1.1）。
    ///
    /// 调用方负责在操作后若桶变空则调用 `remove`，以维持有界不变量（需求 2.1）。
    #[inline]
    pub fn get_mut_or_insert(&mut self, code: u32) -> &mut B {
        match &mut self.backend {
            Backend::Dense(v) => &mut v[code as usize],
            Backend::Sparse(m) => m.entry(code).or_default(),
        }
    }

    /// 移除桶（密集：置回 `Default` 空桶且不缩容；稀疏：删除条目）（需求 2.1/2.2）。
    #[inline]
    pub fn remove(&mut self, code: u32) {
        match &mut self.backend {
            Backend::Dense(v) => v[code as usize] = B::default(),
            Backend::Sparse(m) => {
                m.remove(&code);
            }
        }
    }

    /// 遍历全部非空桶（需求 7）。密集后端跳过空槽，稀疏后端直接迭代。
    ///
    /// 仅供测试/非热路径使用（返回 `Box<dyn>`）。生产代码请用 `for_each_nonempty`
    /// （单态化、可内联、无堆分配与虚调用），以保证密集后端构建/对账路径零性能回归。
    pub fn iter_nonempty(&self) -> Box<dyn Iterator<Item = (u32, &B)> + '_> {
        match &self.backend {
            Backend::Dense(v) => Box::new(
                v.iter()
                    .enumerate()
                    .filter(|(_, b)| !b.is_empty())
                    .map(|(i, b)| (i as u32, b)),
            ),
            Backend::Sparse(m) => Box::new(m.iter().map(|(&c, b)| (c, b))),
        }
    }

    /// 对每个非空桶调用 `f(code, &bucket)`（单态化、可内联、无 `Box`/虚调用）。
    /// 生产构建/对账等非热路径用此遍历，确保密集后端与基线 `for code in 0..cs` 同等性能。
    #[inline]
    pub fn for_each_nonempty(&self, mut f: impl FnMut(u32, &B)) {
        match &self.backend {
            Backend::Dense(v) => {
                for (i, b) in v.iter().enumerate() {
                    if !b.is_empty() {
                        f(i as u32, b);
                    }
                }
            }
            Backend::Sparse(m) => {
                for (&c, b) in m.iter() {
                    f(c, b);
                }
            }
        }
    }

    /// 当前非空桶数（观测/测试用，需求 2.3）。
    pub fn nonempty_count(&self) -> usize {
        match &self.backend {
            Backend::Dense(v) => v.iter().filter(|b| !b.is_empty()).count(),
            Backend::Sparse(m) => m.len(),
        }
    }
}

/// 合并后的全码桶状态（取代基线的 `code_to_chars`/`bucket_freq_sum`/
/// `bucket_max_freq`/`bucket_first` 四个 `code_space` 大小数组，需求 1.2）。
///
/// 成员用 `u32` 表示汉字下标 `ci`（n_chars 远小于 `u32::MAX`，需求 6.1）。
#[derive(Clone)]
pub struct FullBucket {
    /// 桶成员（汉字下标 ci）。
    pub members: Vec<u32>,
    /// 桶频率和。
    pub freq_sum: u64,
    /// 桶内最大频率。
    pub max_freq: u64,
    /// 桶首选字 ci；空桶为 `u32::MAX`（需求 2.4）。
    pub first: u32,
}

impl Default for FullBucket {
    #[inline]
    fn default() -> Self {
        Self {
            members: Vec::new(),
            freq_sum: 0,
            max_freq: 0,
            first: u32::MAX,
        }
    }
}

impl Bucket for FullBucket {
    #[inline]
    fn is_empty(&self) -> bool {
        self.members.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 测试用最小桶：成员计数即可判空。
    #[derive(Clone, Default)]
    struct TestBucket {
        members: Vec<u32>,
        freq_sum: u64,
    }
    impl Bucket for TestBucket {
        fn is_empty(&self) -> bool {
            self.members.is_empty()
        }
    }

    #[test]
    fn choose_backend_threshold_boundary() {
        // 容量 ≤ 阈值 → Dense；＞ 阈值 → Sparse（需求 5.1-5.4, 12.2）。
        assert_eq!(choose_backend(SPARSE_THRESHOLD), BackendKind::Dense);
        assert_eq!(choose_backend(SPARSE_THRESHOLD + 1), BackendKind::Sparse);
        // 运行时容量（不写死 max_parts）：32^4 走 Dense、32^5 走 Sparse。
        let code_base = 32usize;
        assert_eq!(choose_backend(code_base.pow(4)), BackendKind::Dense);
        assert_eq!(choose_backend(code_base.pow(5)), BackendKind::Sparse);
    }

    // Feature: sparse-bucket-memory-optimization, Property 1: 空桶语义一致
    // 未写入或已移除的桶经 get 返回 None（两后端一致）。
    #[test]
    fn prop1_empty_bucket_semantics_both_backends() {
        for &threshold in &[0usize, usize::MAX] {
            let mut s: BucketStore<TestBucket> = BucketStore::new_with_threshold(16, threshold);
            // 从未写入 → None
            assert!(s.get(3).is_none());
            // 写入后非空 → Some
            s.get_mut_or_insert(3).members.push(7);
            assert!(s.get(3).is_some());
            // 移除后 → None
            s.remove(3);
            assert!(s.get(3).is_none());
        }
    }

    // Feature: sparse-bucket-memory-optimization, Property 2: 稀疏有界性
    // 任意操作序列后，非空桶数 == 实际非空桶集合大小（稀疏后端无残留空条目）。
    #[test]
    fn prop2_sparse_bounded_no_empty_residue() {
        let cap = 64usize;
        let mut sparse: BucketStore<TestBucket> = BucketStore::new_with_threshold(cap, 0);
        assert_eq!(sparse.backend_kind(), BackendKind::Sparse);
        // 插入若干，再清空其中一部分
        for code in 0..20u32 {
            sparse.get_mut_or_insert(code).members.push(code);
        }
        for code in 0..8u32 {
            let b = sparse.get_mut(code).unwrap();
            b.members.clear();
            if b.is_empty() {
                sparse.remove(code);
            }
        }
        assert_eq!(sparse.nonempty_count(), 12);
        // 非空桶数不超过曾插入的不同 code 数（有界，需求 2.3）
        assert!(sparse.nonempty_count() <= 20);
    }

    // Feature: sparse-bucket-memory-optimization, Property 3: 双后端可观察等价
    // 对同一操作序列，Dense 与 Sparse 的 iter_nonempty 结果集合一致。
    #[test]
    fn prop3_dense_sparse_observable_equivalence() {
        let cap = 128usize;
        let mut dense: BucketStore<TestBucket> = BucketStore::new_with_threshold(cap, usize::MAX);
        let mut sparse: BucketStore<TestBucket> = BucketStore::new_with_threshold(cap, 0);
        assert_eq!(dense.backend_kind(), BackendKind::Dense);
        assert_eq!(sparse.backend_kind(), BackendKind::Sparse);

        // 伪随机操作序列（确定性）
        let ops: &[(u32, u64)] = &[
            (5, 10), (5, 20), (9, 3), (40, 100), (5, 0), (9, 7), (127, 1), (40, 2),
        ];
        for &(code, v) in ops {
            for st in [&mut dense, &mut sparse] {
                let b = st.get_mut_or_insert(code);
                b.members.push(v as u32);
                b.freq_sum += v;
            }
        }
        // 清空 code 9 并移除
        for st in [&mut dense, &mut sparse] {
            let b = st.get_mut(9).unwrap();
            b.members.clear();
            b.freq_sum = 0;
            st.remove(9);
        }

        let mut d: Vec<(u32, Vec<u32>, u64)> = dense
            .iter_nonempty()
            .map(|(c, b)| (c, b.members.clone(), b.freq_sum))
            .collect();
        let mut s: Vec<(u32, Vec<u32>, u64)> = sparse
            .iter_nonempty()
            .map(|(c, b)| (c, b.members.clone(), b.freq_sum))
            .collect();
        d.sort_by_key(|t| t.0);
        s.sort_by_key(|t| t.0);
        assert_eq!(d, s);
        assert_eq!(dense.nonempty_count(), sparse.nonempty_count());
    }

    #[test]
    fn full_bucket_default_is_empty_sentinel() {
        let b = FullBucket::default();
        assert!(b.is_empty());
        assert_eq!(b.first, u32::MAX);
        assert_eq!(b.freq_sum, 0);
        assert_eq!(b.max_freq, 0);
    }
}
