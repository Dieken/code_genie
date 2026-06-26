// =========================================================================
// 🚀 AMHB 算子定义
// 使用 Evaluator 增量 probe（探测+回滚）计算 delta_score
// =========================================================================

use rand::prelude::*;
use rand::distributions::Uniform;
use crate::context::OptContext;
use crate::evaluator::Evaluator;

/// 算子结果类型
#[derive(Clone, Debug)]
pub enum OperatorResult {
    /// 单点修改：改变某个字根的键位
    Pointwise(PointwiseResult),
    /// 交换修改：交换两个字根的键位
    Exchange(ExchangeResult),
}

impl OperatorResult {
    /// 获取 delta_score
    #[inline]
    pub fn delta_score(&self) -> f64 {
        match self {
            OperatorResult::Pointwise(r) => r.delta_score,
            OperatorResult::Exchange(r) => r.delta_score,
        }
    }

    /// 获取 task_buffer 索引（用于 EXP3 update_stats）
    #[inline]
    pub fn task_index(&self) -> usize {
        match self {
            OperatorResult::Pointwise(r) => r.task_index,
            OperatorResult::Exchange(r) => r.task_index,
        }
    }

    /// 将该移动增量应用到 evaluator + assignment 上
    #[inline]
    pub fn apply(&self, ctx: &OptContext, evaluator: &mut Evaluator, assignment: &mut [u8]) {
        match self {
            OperatorResult::Pointwise(r) => {
                evaluator.apply_move(ctx, assignment, r.radical_idx, r.new_key);
            }
            OperatorResult::Exchange(r) => {
                evaluator.apply_swap(ctx, assignment, r.radical_idx1, r.radical_idx2);
            }
        }
    }
}

/// 单点修改结果
#[derive(Clone, Debug)]
pub struct PointwiseResult {
    pub delta_score: f64,
    pub task_index: usize,
    pub radical_idx: usize,
    pub new_key: u8,
}

/// 交换修改结果
#[derive(Clone, Debug)]
pub struct ExchangeResult {
    pub delta_score: f64,
    pub task_index: usize,
    pub radical_idx1: usize,
    pub radical_idx2: usize,
}

/// AMHB 算子类型枚举
#[derive(Clone)]
pub enum AmhbOperator {
    Pointwise(PointwiseOperator),
    Exchange(ExchangeOperator),
}

impl AmhbOperator {
    /// 算子名称
    #[allow(dead_code)]
    pub fn name(&self) -> &str {
        match self {
            AmhbOperator::Pointwise(_) => "Pointwise",
            AmhbOperator::Exchange(_) => "Exchange",
        }
    }

    /// 探测邻域：增量计算 delta_score 并自动回滚。
    /// 返回 `(候选, wasted)`：
    /// - 候选为 `Some((delta_score, result))` 表示成功采到有效邻域；`None` 表示连续
    ///   `MAX_RETRY` 次都没采到有效邻域而放弃。
    /// - `wasted` 为本次 explore 内部失败的重采样次数（0 = 一次命中），用于统计采样健康度。
    #[inline]
    pub fn explore<R: Rng + ?Sized>(
        &self,
        ctx: &OptContext,
        evaluator: &mut Evaluator,
        assignment: &mut [u8],
        task_index: usize,
        rng: &mut R,
    ) -> (Option<(f64, OperatorResult)>, u32) {
        match self {
            AmhbOperator::Pointwise(op) => op.explore(ctx, evaluator, assignment, task_index, rng),
            AmhbOperator::Exchange(op) => op.explore(ctx, evaluator, assignment, task_index, rng),
        }
    }
}

/// 单次 explore 内部最大重采样次数（对齐 C++ 的 while/do-while 重采样，但加上限防病态死循环）
const MAX_RETRY: u32 = 16;

/// 单点修改算子
#[derive(Clone)]
pub struct PointwiseOperator {
    rad_dist: Option<Uniform<usize>>,
}

impl PointwiseOperator {
    pub fn new(_pre_alloc_radical: usize) -> Self {
        Self {
            rad_dist: None,
        }
    }

    /// 预计算采样分布
    pub fn init_distributions(&mut self, num_radical: usize) {
        if num_radical > 0 {
            self.rad_dist = Some(Uniform::new(0, num_radical));
        }
    }

    fn explore<R: Rng + ?Sized>(
        &self,
        ctx: &OptContext,
        evaluator: &mut Evaluator,
        assignment: &mut [u8],
        task_index: usize,
        rng: &mut R,
    ) -> (Option<(f64, OperatorResult)>, u32) {
        let rad_dist = match self.rad_dist.as_ref() {
            Some(d) => d,
            None => return (None, 0),
        };

        let mut wasted = 0u32;
        while wasted < MAX_RETRY {
            let rad_idx = rng.sample(rad_dist);
            let allowed = &ctx.groups[rad_idx].allowed_keys;
            // 该组只有一个可选键位，无法移动 → 换一个组重试，而非直接放弃
            if allowed.len() <= 1 {
                wasted += 1;
                continue;
            }

            let old_key = assignment[rad_idx];

            // 从该组的 allowed_keys 中采样不同于当前的新键位
            let new_key = loop {
                let k = allowed[rng.gen_range(0..allowed.len())];
                if k != old_key {
                    break k;
                }
            };

            // 增量探测 + 自动回滚
            let delta_score = evaluator.probe_move(ctx, assignment, rad_idx, new_key);

            return (
                Some((
                    delta_score,
                    OperatorResult::Pointwise(PointwiseResult {
                        delta_score,
                        task_index,
                        radical_idx: rad_idx,
                        new_key,
                    }),
                )),
                wasted,
            );
        }
        (None, wasted)
    }
}

/// 交换修改算子
#[derive(Clone)]
pub struct ExchangeOperator {
    rad_dist: Option<Uniform<usize>>,
}

impl ExchangeOperator {
    pub fn new(_pre_alloc_radical: usize) -> Self {
        Self {
            rad_dist: None,
        }
    }

    /// 预计算采样分布
    pub fn init_distributions(&mut self, num_radical: usize) {
        if num_radical > 1 {
            self.rad_dist = Some(Uniform::new(0, num_radical));
        }
    }

    fn explore<R: Rng + ?Sized>(
        &self,
        ctx: &OptContext,
        evaluator: &mut Evaluator,
        assignment: &mut [u8],
        task_index: usize,
        rng: &mut R,
    ) -> (Option<(f64, OperatorResult)>, u32) {
        let rad_dist = match self.rad_dist.as_ref() {
            Some(d) => d,
            None => return (None, 0),
        };

        let mut wasted = 0u32;
        while wasted < MAX_RETRY {
            // 采样两个不同的字根
            let rad_idx1 = rng.sample(rad_dist);
            let rad_idx2 = loop {
                let idx = rng.sample(rad_dist);
                if idx != rad_idx1 {
                    break idx;
                }
            };

            let k1 = assignment[rad_idx1];
            let k2 = assignment[rad_idx2];

            // 同键位交换无意义，或交换后越出对方的 allowed_keys 约束 → 重新采样一对
            if k1 == k2
                || !ctx.groups[rad_idx1].allowed_keys.contains(&k2)
                || !ctx.groups[rad_idx2].allowed_keys.contains(&k1)
            {
                wasted += 1;
                continue;
            }

            // 增量探测 + 自动回滚
            let delta_score = evaluator.probe_swap(ctx, assignment, rad_idx1, rad_idx2);

            return (
                Some((
                    delta_score,
                    OperatorResult::Exchange(ExchangeResult {
                        delta_score,
                        task_index,
                        radical_idx1: rad_idx1,
                        radical_idx2: rad_idx2,
                    }),
                )),
                wasted,
            );
        }
        (None, wasted)
    }
}
