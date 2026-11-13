//! 独立实验副本：统一队列、位集、精排和图布局下的评分与位置消融。
use super::*;
use std::ops::Deref;
fn raw_sm_query(q: &[f32]) -> SmQuery {
    let raw = Bq2Signature::from_vector(q);
    SmQuery {
        pos: std::array::from_fn(|j| raw.pos[j]),
        strong: std::array::from_fn(|j| raw.strong[j]),
        weak: 1.,
        delta: 1.,
        count: raw.strong[..12].iter().map(|v| v.count_ones()).sum::<u32>() as f32,
    }
}
fn bq_combine_counts(row: &SmRow, q: &SmQuery, s: [u32; 5]) -> f32 {
    let s = s.map(|v| v as f32);
    (DIM as f32 - 2. * s[0])
        + (row.strong_count as f32 - 2. * s[1])
        + (q.count - 2. * s[2])
        + (s[3] - 2. * s[4])
}
fn bq_dot(row: &SmRow, q: &SmQuery, simd: bool) -> f32 {
    #[cfg(target_arch = "x86_64")]
    if simd && simd_available() {
        return unsafe { bq_packed_avx512(row, q) };
    }
    let mut s = [0u32; 5];
    for j in 0..12 {
        let diff = row.pos[j] ^ q.pos[j];
        let both = row.strong[j] & q.strong[j];
        for (k, v) in [
            diff,
            diff & row.strong[j],
            diff & q.strong[j],
            both,
            diff & both,
        ]
        .iter()
        .enumerate()
        {
            s[k] += v.count_ones();
        }
    }
    bq_combine_counts(row, q, s)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Score {
    Bq,
    Dot,
    Cosine,
    L2,
}
#[derive(Clone, Copy)]
pub struct Options {
    pub score: Score,
    pub navigation: bool,
    pub retention: bool,
    pub anchor: bool,
    pub sm_simd: bool,
    pub reuse: bool,
    pub batch: bool,
    pub prefetch: bool,
    pub heap_replace: bool,
    pub fp32_batch: bool,
    pub scalar_exact: bool,
    pub wide: bool,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            score: Score::Dot,
            navigation: true,
            retention: true,
            anchor: true,
            sm_simd: true,
            reuse: true,
            batch: true,
            prefetch: true,
            heap_replace: true,
            fp32_batch: true,
            scalar_exact: false,
            wide: false,
        }
    }
}
pub struct ExperimentIndex {
    pub index: CorrectedSmIndex,
    norms: Vec<f32>,
    wide_graph: FlatAdj,
}
impl Deref for ExperimentIndex {
    type Target = CorrectedSmIndex;
    fn deref(&self) -> &Self::Target {
        &self.index
    }
}
struct ScalarSource<'a>(&'a [f32]);
fn scalar_exact(q: &[f32], x: &[f32]) -> f32 {
    let mut a = [0f32; 16];
    for i in (0..DIM).step_by(16) {
        for j in 0..16 {
            a[j] = q[i + j].mul_add(x[i + j], a[j]);
        }
    }
    for j in 0..8 {
        a[j] += a[j + 8];
    }
    for j in 0..4 {
        a[j] += a[j + 4];
    }
    (a[0] + a[1]) + (a[2] + a[3])
}
impl ExactSource for ScalarSource<'_> {
    fn one(&mut self, q: &[f32], id: usize) -> io::Result<Option<f32>> {
        Ok(Some(scalar_exact(q, &self.0[id * DIM..(id + 1) * DIM])))
    }
    fn four(&mut self, q: &[f32], ids: [usize; 4]) -> io::Result<[Option<f32>; 4]> {
        Ok(ids.map(|id| Some(scalar_exact(q, &self.0[id * DIM..(id + 1) * DIM]))))
    }
}
impl ExperimentIndex {
    pub fn new(index: CorrectedSmIndex) -> Self {
        let norms = index
            .rows
            .iter()
            .map(|r| {
                let hi = r.weak + r.delta;
                (DIM - r.strong_count as usize) as f32 * r.weak * r.weak
                    + r.strong_count as f32 * hi * hi
            })
            .collect();
        let mut wide_graph = FlatAdj::new(129);
        wide_graph.reset_full(index.len());
        for u in 0..index.len() {
            wide_graph.set_neighbors(u as u32, index.graph.neighbors(u as u32));
        }
        Self {
            index,
            norms,
            wide_graph,
        }
    }
    pub fn configure(&mut self, coeff: &[f32], anchor: u32) -> io::Result<()> {
        if coeff.len() != self.len()
            || anchor as usize >= self.len()
            || coeff.iter().any(|c| !c.is_finite())
        {
            return Err(invalid("实验系数或锚点无效"));
        }
        for (r, &c) in self.index.rows.iter_mut().zip(coeff) {
            r.coefficient = c;
        }
        self.index.anchor = anchor;
        Ok(())
    }
    pub fn search_experiment(
        &self,
        q: &[f32],
        base: &[f32],
        cfg: &QuIVerSearchConfig,
        options: Options,
        traced: bool,
    ) -> io::Result<SmSearchOutput> {
        if base.len() != self.len() * DIM {
            return Err(invalid("实验向量库形状无效"));
        }
        if (options.navigation || options.retention)
            && (!options.anchor || options.score != Score::Dot)
        {
            return Err(invalid("校正只应用于有锚点的重建内积"));
        }
        let graph = if options.wide {
            &self.wide_graph
        } else {
            &self.index.graph
        };
        if options.scalar_exact {
            let mut source = ScalarSource(base);
            if traced {
                self.experiment_impl::<true, _>(q, graph, &mut source, cfg, options)
            } else {
                self.experiment_impl::<false, _>(q, graph, &mut source, cfg, options)
            }
        } else {
            let mut source = SliceSource(base);
            if traced {
                self.experiment_impl::<true, _>(q, graph, &mut source, cfg, options)
            } else {
                self.experiment_impl::<false, _>(q, graph, &mut source, cfg, options)
            }
        }
    }
    #[inline]
    fn eval_score(
        &self,
        q: &SmQuery,
        raw: &SmQuery,
        id: usize,
        error: f32,
        options: Options,
        correct: bool,
    ) -> f32 {
        if options.score == Score::Bq {
            let r = &self.rows[id];
            let dot = bq_dot(r, raw, options.sm_simd);
            return 1. - (4. * DIM as f32 - dot) / 4096.;
        }
        let value = self.score(q, id, options.sm_simd);
        match options.score {
            Score::Dot => {
                value
                    + if correct {
                        self.rows[id].coefficient * error
                    } else {
                        0.
                    }
            }
            Score::Cosine => {
                let hi = q.weak + q.delta;
                let norm = (DIM as f32 - q.count) * q.weak * q.weak + q.count * hi * hi;
                value / (norm * self.norms[id]).sqrt().max(f32::MIN_POSITIVE)
            }
            Score::L2 => {
                let hi = q.weak + q.delta;
                let norm = (DIM as f32 - q.count) * q.weak * q.weak + q.count * hi * hi;
                1. - (norm + self.norms[id] - 2. * value)
            }
            Score::Bq => unreachable!(),
        }
    }
    /// 独立核对评分数学式与原BQ签名，避免只检验实现自洽。
    pub fn verify_math(&self, base: &[f32], queries: &[f32]) -> io::Result<f64> {
        let mut max_error = 0f64;
        for q in queries.chunks_exact(DIM).take(8) {
            let sq = query_scalar(q);
            let signature_query = Bq2Signature::from_vector(q);
            let raw = raw_sm_query(q);
            for id in (0..self.len()).step_by((self.len() / 32).max(1)) {
                let r = &self.rows[id];
                let mut inner = 0f64;
                let mut nx = 0f64;
                let mut nq = 0f64;
                for j in 0..DIM {
                    let x = (r.weak
                        + if r.strong[j / 64] >> (j % 64) & 1 == 1 {
                            r.delta
                        } else {
                            0.
                        }) as f64
                        * if r.pos[j / 64] >> (j % 64) & 1 == 1 {
                            1.
                        } else {
                            -1.
                        };
                    let y = (sq.weak
                        + if sq.strong[j / 64] >> (j % 64) & 1 == 1 {
                            sq.delta
                        } else {
                            0.
                        }) as f64
                        * if sq.pos[j / 64] >> (j % 64) & 1 == 1 {
                            1.
                        } else {
                            -1.
                        };
                    inner += x * y;
                    nx += x * x;
                    nq += y * y;
                }
                for (score, expected) in [
                    (Score::Dot, inner),
                    (Score::Cosine, inner / (nx * nq).sqrt()),
                    (Score::L2, 1. - (nx + nq - 2. * inner)),
                ] {
                    let o = Options {
                        score,
                        navigation: false,
                        retention: false,
                        ..Options::default()
                    };
                    let actual = self.eval_score(&sq, &raw, id, 0., o, false) as f64;
                    let error = (actual - expected).abs();
                    max_error = max_error.max(error);
                    if error > 3e-6 {
                        return Err(corrupt("评分与显式重建数学式不一致"));
                    }
                }
                let o = Options {
                    score: Score::Bq,
                    navigation: false,
                    retention: false,
                    ..Options::default()
                };
                let actual = self.eval_score(&sq, &raw, id, 0., o, false);
                let signature = Bq2Signature::from_vector(&base[id * DIM..(id + 1) * DIM]);
                let expected = 1. - signature_query.distance(&signature, DIM) as f32 / 4096.;
                if actual.to_bits() != expected.to_bits() {
                    return Err(corrupt("统一原BQ评分与官方签名距离不一致"));
                }
            }
        }
        Ok(max_error)
    }
}
