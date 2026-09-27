//! Shiu et al. LIF dynamics, in mV/ms, on the supplied FlyWire graph.
//! Minecraft encoding and the trained readout are our adaptation, not biological claims.
use anyhow::{Context, Result, bail, ensure};
use parquet::{
    file::{reader::FileReader, serialized_reader::SerializedFileReader},
    record::Field,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufReader, BufWriter, Read, Write},
    path::Path,
    time::Instant,
};

pub const READOUT: usize = 32;
pub type Features = [f64; READOUT];
const DT: f32 = 0.1;
const DELAY: usize = 18;
const REFRACTORY: u32 = 22;
// Far below f32 voltage resolution, but still a normal float. Otherwise decayed
// inactive conductances settle into subnormals and slow long-running CPU loops.
const SILENT_G: f32 = 1e-30;
const INPUTS: usize = 16;
const POOLS: usize = 16;
const INPUT_CELLS: usize = 32;

pub struct Graph {
    offsets: Vec<u32>,
    targets: Vec<u32>,
    weights: Vec<f32>,
}
impl Graph {
    pub fn load(root: &Path, cache: &Path) -> Result<Self> {
        let csv = root.join("Completeness_783.csv");
        let parquet = root.join("Connectivity_783.parquet");
        let neurons = fs::read_to_string(&csv)?
            .lines()
            .skip(1)
            .filter(|s| !s.trim().is_empty())
            .count();
        ensure!(
            (1..=200_000).contains(&neurons),
            "Unexpected FlyWire neuron count"
        );
        let source = fs::metadata(&parquet)?;
        let stamp = source
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        if let Ok(graph) = Self::cached(cache, neurons, source.len(), stamp) {
            return Ok(graph);
        }
        let started = Instant::now();
        let reader = SerializedFileReader::new(File::open(&parquet)?)?;
        let rows = usize::try_from(reader.metadata().file_metadata().num_rows())?;
        ensure!(rows <= 30_000_000, "FlyWire graph too large");
        let mut edges = Vec::with_capacity(rows);
        let mut offsets = vec![0_u32; neurons + 1];
        let mut cols = None;
        for row in reader.get_row_iter(None)? {
            let row = row?;
            let values: Vec<_> = row.get_column_iter().collect();
            let (pre, post, weight) = *cols.get_or_insert_with(|| {
                let names: HashMap<_, _> = values
                    .iter()
                    .enumerate()
                    .map(|(i, (n, _))| (n.as_str(), i))
                    .collect();
                (
                    names.get("Presynaptic_Index").copied(),
                    names.get("Postsynaptic_Index").copied(),
                    names.get("Excitatory x Connectivity").copied(),
                )
            });
            let number = |i: Option<usize>| -> Result<f64> {
                let f = values
                    .get(i.context("Missing FlyWire column")?)
                    .context("Missing value")?
                    .1;
                match f {
                    Field::Int(v) => Ok(f64::from(*v)),
                    Field::Long(v) => Ok(*v as f64),
                    Field::UInt(v) => Ok(f64::from(*v)),
                    Field::ULong(v) => Ok(*v as f64),
                    Field::Float(v) => Ok(f64::from(*v)),
                    Field::Double(v) => Ok(*v),
                    _ => bail!("Non-numeric FlyWire value"),
                }
            };
            let p = number(pre)?;
            let q = number(post)?;
            let w = number(weight)?;
            ensure!(
                p.is_finite()
                    && q.is_finite()
                    && p.fract() == 0.
                    && q.fract() == 0.
                    && p >= 0.
                    && q >= 0.
                    && p < (neurons as f64)
                    && q < (neurons as f64),
                "Invalid FlyWire neuron index"
            );
            ensure!(
                w.is_finite() && w.abs() <= 1_000_000.,
                "Invalid FlyWire weight"
            );
            offsets[p as usize + 1] += 1;
            edges.push((p as u32, q as u32, w as f32 * 0.275));
        }
        for i in 1..offsets.len() {
            offsets[i] += offsets[i - 1];
        }
        let mut cursor = offsets.clone();
        let mut targets = vec![0; edges.len()];
        let mut weights = vec![0.; edges.len()];
        for (p, q, w) in edges {
            let j = cursor[p as usize] as usize;
            cursor[p as usize] += 1;
            targets[j] = q;
            weights[j] = w;
        }
        let graph = Self {
            offsets,
            targets,
            weights,
        };
        graph.save(cache, source.len(), stamp)?;
        println!(
            "FlyWire imported: {neurons} neurons / {} connections in {:.2}s",
            graph.targets.len(),
            started.elapsed().as_secs_f64()
        );
        Ok(graph)
    }
    fn cached(path: &Path, n: usize, bytes: u64, stamp: u64) -> Result<Self> {
        let mut r = BufReader::new(File::open(path)?);
        fn u64r(r: &mut impl Read) -> Result<u64> {
            let mut b = [0; 8];
            r.read_exact(&mut b)?;
            Ok(u64::from_le_bytes(b))
        }
        ensure!(
            u64r(&mut r)? == 0x464c595749524501
                && u64r(&mut r)? == bytes
                && u64r(&mut r)? == stamp
                && u64r(&mut r)? == n as u64,
            "Stale brain cache"
        );
        let e = usize::try_from(u64r(&mut r)?)?;
        ensure!(e <= 30_000_000, "Invalid brain cache size");
        let mut read_vec = |len: usize| -> Result<Vec<u32>> {
            let mut out = vec![0; len];
            for v in &mut out {
                let mut b = [0; 4];
                r.read_exact(&mut b)?;
                *v = u32::from_le_bytes(b);
            }
            Ok(out)
        };
        let offsets = read_vec(n + 1)?;
        let targets = read_vec(e)?;
        let weights = read_vec(e)?
            .into_iter()
            .map(f32::from_bits)
            .collect::<Vec<_>>();
        ensure!(
            offsets[0] == 0
                && offsets[n] as usize == e
                && offsets.windows(2).all(|v| v[0] <= v[1])
                && targets.iter().all(|v| (*v as usize) < n)
                && weights.iter().all(|w| w.is_finite()),
            "Invalid graph cache"
        );
        Ok(Self {
            offsets,
            targets,
            weights,
        })
    }
    fn save(&self, path: &Path, bytes: u64, stamp: u64) -> Result<()> {
        let tmp = path.with_extension("tmp");
        let mut w = BufWriter::new(File::create(&tmp)?);
        for v in [
            0x464c595749524501,
            bytes,
            stamp,
            self.offsets.len() as u64 - 1,
            self.targets.len() as u64,
        ] {
            w.write_all(&v.to_le_bytes())?;
        }
        for v in self.offsets.iter().chain(&self.targets) {
            w.write_all(&v.to_le_bytes())?;
        }
        for v in &self.weights {
            w.write_all(&v.to_le_bytes())?;
        }
        w.flush()?;
        drop(w);
        fs::rename(tmp, path)?;
        Ok(())
    }
}

pub struct Brain {
    graph: Graph,
    v: Vec<f32>,
    g: Vec<f32>,
    until: Vec<u32>,
    tick: u32,
    pending: Vec<Vec<u32>>,
    rng: u64,
    input: Vec<usize>,
    pools: Vec<usize>,
    pool_sizes: [usize; POOLS],
    trace: [f64; POOLS],
    fired: Vec<u32>,
    pub simd: bool,
    pub last_ms: f64,
    pub last_spikes: u64,
}
impl Brain {
    pub fn new(graph: Graph) -> Self {
        let n = graph.offsets.len() - 1;
        let mut eligible: Vec<_> = (0..n)
            .filter(|i| graph.offsets[*i + 1] > graph.offsets[*i])
            .collect();
        // Stable random projection, explicitly NOT a claim about sensory-cell anatomy.
        eligible.sort_by_key(|i| mix(*i as u64));
        let input: Vec<usize> = eligible.into_iter().take(INPUTS * INPUT_CELLS).collect();
        // Read out downstream neighborhoods of each input projection, not a saturated global sum.
        let mut pools = vec![usize::MAX; n];
        let mut strength = vec![0_f32; n];
        let mut driven = vec![false; n];
        for &i in &input {
            driven[i] = true;
        }
        for (k, &i) in input.iter().enumerate() {
            for e in graph.offsets[i]..graph.offsets[i + 1] {
                let j = graph.targets[e as usize] as usize;
                let w = graph.weights[e as usize].abs();
                if !driven[j] && w > strength[j] {
                    strength[j] = w;
                    pools[j] = k / INPUT_CELLS;
                }
            }
        }
        let mut pool_sizes = [0; POOLS];
        for &p in &pools {
            if p < POOLS {
                pool_sizes[p] += 1;
            }
        }
        Self {
            graph,
            v: vec![-52.; n],
            g: vec![0.; n],
            until: vec![0; n],
            tick: 0,
            pending: (0..=DELAY).map(|_| Vec::new()).collect(),
            rng: 0x0041_8112_34ab_cdef,
            input,
            pools,
            pool_sizes,
            trace: [0.; POOLS],
            fired: Vec::with_capacity(2048),
            simd: has_avx2(),
            last_ms: 0.,
            last_spikes: 0,
        }
    }
    fn uniform(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 40) as f32 / 16_777_216.
    }
    fn step(
        &mut self,
        signals: &[f32; INPUTS],
        counts: &mut [u32; POOLS],
        pulses: &[(usize, f32)],
    ) {
        let tick = self.tick;
        self.fired.clear();
        integrate(
            &mut self.v,
            &mut self.g,
            &self.until,
            tick,
            self.simd,
            &mut self.fired,
        );
        // Brian scheduling: state update -> threshold -> synaptic delivery -> reset.
        let slot = tick as usize % self.pending.len();
        let arrivals = std::mem::take(&mut self.pending[slot]);
        for pre in &arrivals {
            for edge in self.graph.offsets[*pre as usize]..self.graph.offsets[*pre as usize + 1] {
                let j = self.graph.targets[edge as usize] as usize;
                if self.until[j] <= tick {
                    self.g[j] += self.graph.weights[edge as usize];
                }
            }
        }
        self.pending[slot] = arrivals;
        self.pending[slot].clear();
        for k in 0..self.input.len() {
            let i = self.input[k];
            let probability = signals[k / INPUT_CELLS].clamp(0., 1.) * 150. * DT / 1000.;
            if self.uniform() < probability && self.until[i] <= tick {
                // The author's PoissonInput targets membrane voltage directly;
                // only connectome synapses deliver to conductance g.
                self.v[i] += 250. * 0.275;
            }
        }
        let due = (tick as usize + DELAY) % self.pending.len();
        for &(i, w) in pulses {
            if self.until[i] <= tick {
                self.v[i] += w;
            }
        }
        for &i in &self.fired {
            if self.pools[i as usize] < POOLS {
                counts[self.pools[i as usize]] += 1;
            }
            self.pending[due].push(i);
            self.v[i as usize] = -52.;
            self.g[i as usize] = 0.;
            // Driven neurons in the reference have rfc=0; all others use 2.2ms.
            self.until[i as usize] = tick + REFRACTORY;
        }
        self.last_spikes += self.fired.len() as u64;
        for &i in &self.input {
            if self.until[i] > tick {
                self.until[i] = tick + 1;
            }
        }
        self.tick += 1;
    }
    pub fn observe(&mut self, signals: &[f32; INPUTS], steps: usize) -> [f64; POOLS] {
        let began = Instant::now();
        let mut counts = [0; POOLS];
        self.last_spikes = 0;
        for _ in 0..steps {
            self.step(signals, &mut counts, &[]);
        }
        self.last_ms = began.elapsed().as_secs_f64() * 1000.;
        for (i, count) in counts.iter().enumerate() {
            let seconds = (steps as f64 * f64::from(DT) / 1000.).max(0.0001);
            let hz = f64::from(*count) / (self.pool_sizes[i].max(1) as f64 * seconds);
            self.trace[i] = 0.8 * self.trace[i] + 0.2 * (hz / 20.).tanh();
        }
        self.trace
    }
    pub fn context(&mut self, state: &crate::types::State) -> [f64; POOLS] {
        let mut x = [0.; INPUTS];
        x[0] = (state.health / 20.) as f32;
        x[1] = state.food as f32 / 20.;
        x[2] = if state.has("pickaxe") { 1. } else { 0. };
        x[3] = (state.hostiles.len() as f32 / 4.).min(1.);
        x[4] = (state.light_level as f32 / 15.).min(1.);
        x[5] = (state.navigation.ores.len() as f32 / 8.).min(1.);
        x[6] = state.free_slots.min(36) as f32 / 36.;
        x[7] = ((state.position.y + 64.) / 384.).clamp(0., 1.) as f32;
        x[8] = state
            .navigation
            .routes
            .iter()
            .filter(|r| r.clear && r.safe_floor)
            .count() as f32
            / 12.;
        x[9] = if state.navigation.centered { 1. } else { 0. };
        x[10] = if state.has("food") { 1. } else { 0. };
        x[11] = state
            .home
            .home
            .map_or(0., |h| (state.feet().distance(h) / 128.).min(1.) as f32);
        x[12] = 1. - x[0];
        x[13] = 1. - x[1];
        x[14] = 1. - x[8];
        x[15] = 0.5;
        self.observe(&x, 200) // 20 biological ms per planning decision; state persists.
    }
    pub fn benchmark(root: &Path, runtime: &Path) -> Result<()> {
        fs::create_dir_all(runtime)?;
        let graph = Graph::load(root, &runtime.join("flywire-v783.bin"))?;
        let n = graph.offsets.len() - 1;
        let edges = graph.targets.len();
        let mut brain = Self::new(graph);
        let mut times = Vec::new();
        let mut simd_spikes = Vec::new();
        for _ in 0..72 {
            brain.observe(&[0.5; INPUTS], 200);
            times.push(brain.last_ms);
            simd_spikes.push(brain.last_spikes);
        }
        let early_ms = times[..12].iter().sum::<f64>() / 12.;
        let late_ms = times[60..].iter().sum::<f64>() / 12.;
        times.sort_by(f64::total_cmp);
        let avx = brain.simd;
        let mut scalar = Self::new(brain.graph);
        scalar.simd = false;
        let mut scalar_times = Vec::new();
        let mut scalar_spikes = Vec::new();
        for _ in 0..72 {
            scalar.observe(&[0.5; INPUTS], 200);
            scalar_times.push(scalar.last_ms);
            scalar_spikes.push(scalar.last_spikes);
        }
        ensure!(
            simd_spikes == scalar_spikes,
            "SIMD and scalar spike counts differ"
        );
        scalar_times.sort_by(f64::total_cmp);
        let report = serde_json::json!({"neurons":n,"connections":edges,"avx2":avx,"dt_ms":0.1,"window_ms":20,"windows":72,"early_mean_ms":early_ms,"late_mean_ms":late_ms,"median_ms":times[36],"max_ms":times[71],"scalar_median_ms":scalar_times[36],"speedup":scalar_times[36]/times[36],"spike_counts_match":true,"last_spikes":scalar.last_spikes,"readout_channels":scalar.trace,"readout_neuron_counts":scalar.pool_sizes,"note":"Real graph / synthetic Minecraft input; not a mining-quality benchmark"});
        println!("{}", serde_json::to_string_pretty(&report)?);
        crate::bridge::atomic_json(&runtime.join("brain-benchmark.json"), &report)
    }
    pub fn reference_trace(runtime: &Path) -> Result<()> {
        let graph = Graph {
            offsets: std::iter::once(0)
                .chain([2, 3, 4])
                .chain(std::iter::repeat_n(4, 16))
                .collect(),
            targets: vec![1, 2, 2, 1],
            weights: vec![15.125, -4.95, 40., -20.],
        };
        let mut b = Self::new(graph);
        b.input = vec![0];
        let mut trace = Vec::new();
        let mut counts = [0; POOLS];
        for tick in 0..1000 {
            let pulses = if tick < 400 && tick % 5 == 0 {
                vec![(0, 68.75)]
            } else if tick == 500 {
                vec![(1, 80.)]
            } else {
                vec![]
            };
            b.step(&[0.; INPUTS], &mut counts, &pulses);
            trace.push(serde_json::json!({"tick":tick,"v":b.v,"g":b.g,"spikes":b.fired}));
        }
        fs::create_dir_all(runtime)?;
        crate::bridge::atomic_json(&runtime.join("brain-reference-rust.json"), &trace)
    }
}
fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e3779b97f4a7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}
fn has_avx2() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        std::is_x86_feature_detected!("avx2")
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}
fn integrate(
    v: &mut [f32],
    g: &mut [f32],
    until: &[u32],
    tick: u32,
    simd: bool,
    fired: &mut Vec<u32>,
) {
    let a = (-DT / 20.).exp();
    let b = (-DT / 5.).exp();
    let c = (a - b) / 3.;
    let mut start = 0;
    #[cfg(target_arch = "x86_64")]
    if simd {
        // Runtime feature detection is the sole gate to AVX2, with scalar tail.
        start = unsafe { integrate_avx2(v, g, until, tick, [a, b, c], fired) };
    }
    #[cfg(not(target_arch = "x86_64"))]
    let _ = simd;
    for i in start..v.len() {
        if until[i] <= tick {
            v[i] = -52. + (v[i] + 52.) * a + g[i] * c;
            g[i] *= b;
            if g[i].abs() < SILENT_G {
                g[i] = 0.;
            }
            if v[i] > -45. {
                fired.push(i as u32);
            }
        }
    }
}
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn integrate_avx2(
    v: &mut [f32],
    g: &mut [f32],
    until: &[u32],
    tick: u32,
    coefficients: [f32; 3],
    fired: &mut Vec<u32>,
) -> usize {
    use std::arch::x86_64::*;
    let end = v.len() / 8 * 8;
    // All arrays have identical neuron counts; unaligned loads stay within full 8-lane chunks.
    unsafe {
        let aa = _mm256_set1_ps(coefficients[0]);
        let bb = _mm256_set1_ps(coefficients[1]);
        let cc = _mm256_set1_ps(coefficients[2]);
        let rest = _mm256_set1_ps(-52.);
        let sign = _mm256_set1_ps(-0.0);
        let silence = _mm256_set1_ps(SILENT_G);
        for i in (0..end).step_by(8) {
            let oldv = _mm256_loadu_ps(v.as_ptr().add(i));
            let oldg = _mm256_loadu_ps(g.as_ptr().add(i));
            let u = _mm256_loadu_si256(until.as_ptr().add(i).cast());
            let inactive =
                _mm256_castsi256_ps(_mm256_cmpgt_epi32(u, _mm256_set1_epi32(tick as i32)));
            let nv = _mm256_add_ps(
                _mm256_add_ps(rest, _mm256_mul_ps(_mm256_sub_ps(oldv, rest), aa)),
                _mm256_mul_ps(oldg, cc),
            );
            _mm256_storeu_ps(v.as_mut_ptr().add(i), _mm256_blendv_ps(nv, oldv, inactive));
            let ng = _mm256_mul_ps(oldg, bb);
            let ng = _mm256_andnot_ps(
                _mm256_cmp_ps::<_CMP_LT_OQ>(_mm256_andnot_ps(sign, ng), silence),
                ng,
            );
            _mm256_storeu_ps(g.as_mut_ptr().add(i), _mm256_blendv_ps(ng, oldg, inactive));
            let spiking = _mm256_andnot_ps(
                inactive,
                _mm256_cmp_ps::<_CMP_GT_OQ>(nv, _mm256_set1_ps(-45.)),
            );
            let mut mask = _mm256_movemask_ps(spiking) as u32;
            while mask != 0 {
                fired.push((i + mask.trailing_zeros() as usize) as u32);
                mask &= mask - 1;
            }
        }
    }
    end
}

#[derive(Serialize, Deserialize)]
pub struct Readout {
    pub updates: u64,
    inverse: Vec<Vec<f64>>,
    b: Vec<f64>,
}
impl Default for Readout {
    fn default() -> Self {
        Self {
            updates: 0,
            inverse: (0..READOUT)
                .map(|i| (0..READOUT).map(|j| if i == j { 1. } else { 0. }).collect())
                .collect(),
            b: vec![0.; READOUT],
        }
    }
}
impl Readout {
    pub fn valid(&self) -> bool {
        self.b.len() == READOUT
            && self.inverse.len() == READOUT
            && self
                .inverse
                .iter()
                .all(|r| r.len() == READOUT && r.iter().all(|v| v.is_finite()))
            && self.b.iter().all(|v| v.is_finite())
            && (0..READOUT).all(|i| self.inverse[i][i] > 0.)
    }
    fn product(&self, x: &Features) -> Features {
        std::array::from_fn(|i| self.inverse[i].iter().zip(x).map(|(a, b)| a * b).sum())
    }
    pub fn score(&self, x: &Features) -> f64 {
        let ax = self.product(x);
        ax.iter().zip(&self.b).map(|(a, b)| a * b).sum::<f64>()
            + 0.15
                * ax.iter()
                    .zip(x)
                    .map(|(a, b)| a * b)
                    .sum::<f64>()
                    .max(0.)
                    .sqrt()
    }
    pub fn update(&mut self, x: &Features, reward: f64) {
        let ax = self.product(x);
        let d = 1. + ax.iter().zip(x).map(|(a, b)| a * b).sum::<f64>();
        for i in 0..READOUT {
            for j in 0..READOUT {
                self.inverse[i][j] -= ax[i] * ax[j] / d;
            }
            self.b[i] += reward.clamp(-5., 5.) * x[i];
        }
        self.updates += 1;
    }
}
pub fn features(raw: &crate::bandit::Features, context: &[f64; POOLS]) -> Features {
    let mut out = [0.; READOUT];
    out[..10].copy_from_slice(raw);
    // Candidate-conditioned interactions; context-only terms cancel between candidates.
    for i in 10..READOUT {
        out[i] = context[(i - 10) % POOLS] * raw[1 + (i - 10) % 9];
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    fn empty(n: usize) -> Graph {
        Graph {
            offsets: vec![0; n + 1],
            targets: vec![],
            weights: vec![],
        }
    }
    #[test]
    fn exact_decay_and_simd_match_scalar() {
        let mut v = vec![-50.; 19];
        let mut g = vec![3.; 19];
        let mut until = vec![0; 19];
        until[2] = 10;
        let mut sv = v.clone();
        let mut sg = g.clone();
        integrate(&mut v, &mut g, &until, 0, false, &mut Vec::new());
        integrate(&mut sv, &mut sg, &until, 0, has_avx2(), &mut Vec::new());
        assert_eq!(v[2], -50.);
        assert_eq!(g[2], 3.);
        for i in 0..19 {
            assert!((v[i] - sv[i]).abs() < 1e-5);
            assert!((g[i] - sg[i]).abs() < 1e-6);
        }
        let exact =
            -52. + 2. * (-0.1_f64 / 20.).exp() + ((-0.1_f64 / 20.).exp() - (-0.1_f64 / 5.).exp());
        assert!((f64::from(v[0]) - exact).abs() < 1e-5);
    }
    #[test]
    fn silent_baseline_and_persistent_state() {
        let mut b = Brain::new(empty(19));
        b.observe(&[0.; INPUTS], 100);
        assert_eq!(b.last_spikes, 0);
        assert!(b.v.iter().all(|v| *v == -52.));
        b.g[0] = 4.;
        b.observe(&[0.; INPUTS], 1);
        assert!(b.v[0] > -52.);
        assert_eq!(b.tick, 101);
    }
    #[test]
    fn silent_conductances_do_not_get_stuck_as_subnormals() {
        for simd in [false, has_avx2()] {
            let mut v = vec![-52.; 19];
            let mut g = vec![SILENT_G * 0.5; 19];
            g[1] = -SILENT_G * 0.5;
            g[18] = f32::MIN_POSITIVE / 2.;
            integrate(&mut v, &mut g, &[0; 19], 0, simd, &mut Vec::new());
            assert!(g.iter().all(|x| *x == 0.));
            assert!(v.iter().all(|x| *x == -52.));
        }
    }
    #[test]
    fn signed_synapse_delay_and_refractory() {
        let mut b = Brain::new(Graph {
            offsets: vec![0, 2, 2, 2],
            targets: vec![1, 2],
            weights: vec![1., -2.],
        });
        b.input.clear();
        b.v[0] = -44.;
        let mut c = [0; POOLS];
        b.step(&[0.; INPUTS], &mut c, &[]);
        assert_eq!(b.v[0], -52.);
        for _ in 1..DELAY {
            b.step(&[0.; INPUTS], &mut c, &[]);
        }
        assert_eq!(b.g[1], 0.);
        b.step(&[0.; INPUTS], &mut c, &[]);
        assert_eq!(b.g[1], 1.);
        assert_eq!(b.g[2], -2.);
        assert_eq!(b.last_spikes, 1);
    }
    #[test]
    fn external_stimulation_targets_voltage_after_threshold_detection() {
        let mut b = Brain::new(empty(19));
        b.input = vec![0];
        let mut counts = [0; POOLS];
        b.step(&[0.; INPUTS], &mut counts, &[(0, 68.75)]);
        assert_eq!(b.v[0], 16.75);
        assert_eq!(b.g[0], 0.);
        assert!(b.fired.is_empty());
        b.step(&[0.; INPUTS], &mut counts, &[]);
        assert_eq!(b.fired, vec![0]);
        assert_eq!(b.v[0], -52.);
    }
    #[test]
    fn readout_learns_and_roundtrips() {
        let mut r = Readout::default();
        let mut x = [0.; READOUT];
        x[1] = 1.;
        let mut y = [0.; READOUT];
        y[2] = 1.;
        for _ in 0..20 {
            r.update(&x, 1.);
            r.update(&y, -1.);
        }
        assert!(r.score(&x) > r.score(&y) + 1.);
        let loaded: Readout =
            serde_json::from_str(&serde_json::to_string(&r).expect("serialize")).expect("restore");
        assert!(loaded.valid());
        assert_eq!(loaded.updates, 40);
    }
    #[test]
    fn readout_can_reverse_a_preference_when_brain_context_changes() {
        let mut left = [0.; 10];
        left[0] = 1.;
        left[1] = 1.;
        let mut right = [0.; 10];
        right[0] = 1.;
        right[2] = 1.;
        let mut context_a = [0.; POOLS];
        context_a[0] = 0.7;
        let context_b = [0.; POOLS];
        let la = features(&left, &context_a);
        let ra = features(&right, &context_a);
        let lb = features(&left, &context_b);
        let rb = features(&right, &context_b);
        let mut r = Readout::default();
        for _ in 0..100 {
            r.update(&la, 1.);
            r.update(&ra, -1.);
            r.update(&lb, -1.);
            r.update(&rb, 1.);
        }
        assert!(r.score(&la) > r.score(&ra));
        assert!(r.score(&rb) > r.score(&lb));
        assert!(r.valid());
    }
}
