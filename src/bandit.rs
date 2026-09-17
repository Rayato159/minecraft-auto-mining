use crate::bridge::atomic_json;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::Path};

pub const N: usize = 10;
pub type Features = [f64; N];
#[derive(Clone, Serialize, Deserialize)]
pub struct Model {
    pub updates: u64,
    inverse: [[f64; N]; N],
    b: Features,
}
impl Default for Model {
    fn default() -> Self {
        let mut inverse = [[0.; N]; N];
        for (i, row) in inverse.iter_mut().enumerate() {
            row[i] = 1.;
        }
        Self {
            updates: 0,
            inverse,
            b: [0.; N],
        }
    }
}
fn dot(a: &Features, b: &Features) -> f64 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}
impl Model {
    fn product(&self, x: &Features) -> Features {
        std::array::from_fn(|i| dot(&self.inverse[i], x))
    }
    pub fn score(&self, x: &Features) -> f64 {
        let ax = self.product(x);
        dot(&ax, &self.b) + 0.15 * dot(x, &ax).max(0.).sqrt()
    }
    pub fn update(&mut self, x: &Features, reward: f64) {
        let ax = self.product(x);
        let denominator = 1. + dot(x, &ax);
        for i in 0..N {
            for j in 0..N {
                self.inverse[i][j] -= ax[i] * ax[j] / denominator;
            }
            self.b[i] += reward.clamp(-5., 5.) * x[i];
        }
        self.updates += 1;
    }
    fn valid(&self) -> bool {
        self.inverse
            .iter()
            .flatten()
            .chain(&self.b)
            .all(|v| v.is_finite())
            && (0..N).all(|i| self.inverse[i][i] > 0.)
    }
}
#[derive(Default, Serialize, Deserialize)]
pub struct Book {
    pub version: u32,
    pub profiles: BTreeMap<String, Model>,
    #[serde(default)]
    pub fly_profiles: BTreeMap<String, crate::brain::Readout>,
    #[serde(skip)]
    pub brain: Option<crate::brain::Brain>,
}
impl Book {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self {
                version: 1,
                ..Self::default()
            });
        }
        let book: Self = serde_json::from_slice(&fs::read(path)?)?;
        if book.version != 1
            || book.profiles.values().any(|m| !m.valid())
            || book.fly_profiles.values().any(|m| !m.valid())
        {
            bail!("Invalid bandit checkpoint; preserve it and start a new checkpoint");
        }
        Ok(book)
    }
    pub fn model(&mut self, key: &str) -> &mut Model {
        self.profiles.entry(key.into()).or_default()
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        atomic_json(path, self)
    }
    pub fn enable_brain(&mut self, runtime: &Path) -> Result<()> {
        let graph = crate::brain::Graph::load(
            Path::new("references/Drosophila_brain_model"),
            &runtime.join("flywire-v783.bin"),
        )?;
        let brain = crate::brain::Brain::new(graph);
        println!(
            "Fly brain online: persistent LIF, {} integration, learned readout",
            if brain.simd { "AVX2" } else { "scalar" }
        );
        self.brain = Some(brain);
        Ok(())
    }
    pub fn choose(
        &mut self,
        mut candidates: Vec<crate::planner::Plan>,
        state: &crate::types::State,
    ) -> Option<crate::planner::Plan> {
        if let Some(brain) = &mut self.brain
            && !candidates.is_empty()
        {
            let context = brain.context(state);
            let model = self.fly_profiles.entry(state.goal_key()).or_default();
            for p in &mut candidates {
                let x = crate::brain::features(&p.features, &context);
                p.score += 0.6 * model.score(&x).clamp(-2., 2.);
                p.brain_features = Some(x);
            }
            println!(
                "Brain: {:.2}ms / {} spikes; readout updates={}",
                brain.last_ms, brain.last_spikes, model.updates
            );
        }
        candidates
            .into_iter()
            .max_by(|a, b| a.score.total_cmp(&b.score))
    }
}
pub fn reward(points: f64, seconds: f64, damage: f64, wear: f64, failed: bool) -> f64 {
    // No reward for mere survival, walking, or picking up an unrelated item.
    (points / 100. - seconds * 0.005 - damage * 0.15 - wear * 0.002 - if failed { 0.4 } else { 0. })
        .clamp(-5., 5.)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn learning_prefers_productive_context() {
        let mut m = Model::default();
        let mut good = [0.; N];
        good[0] = 1.;
        let mut bad = [0.; N];
        bad[1] = 1.;
        for _ in 0..20 {
            m.update(&good, 1.);
            m.update(&bad, -1.);
        }
        assert!(m.score(&good) > m.score(&bad) + 1.);
        assert!(m.valid());
        assert_eq!(m.updates, 40);
    }
    #[test]
    fn no_positive_reward_for_waiting_or_failing() {
        assert!(reward(0., 30., 0., 0., false) < 0.);
        assert!(reward(100., 1., 10., 1., true) < 0.);
        assert!(reward(100., 1., 0., 1., false) > reward(100., 100., 0., 1., false));
    }
    #[test]
    fn old_checkpoint_loads_and_fly_learning_survives_restart() {
        let path =
            std::env::temp_dir().join(format!("miner-learning-{}.json", crate::bridge::now()));
        let mut legacy = Model::default();
        legacy.update(&[0.5; N], 1.);
        atomic_json(
            &path,
            &serde_json::json!({"version":1,"profiles":{"world":legacy}}),
        )
        .expect("legacy save");
        let mut book = Book::load(&path).expect("migration");
        assert_eq!(book.model("world").updates, 1);
        book.fly_profiles
            .entry("world".into())
            .or_default()
            .update(&[0.25; crate::brain::READOUT], 1.);
        book.save(&path).expect("save");
        let restored = Book::load(&path).expect("restore");
        assert_eq!(restored.profiles["world"].updates, 1);
        assert_eq!(restored.fly_profiles["world"].updates, 1);
        assert!(restored.brain.is_none());
        fs::remove_file(path).expect("cleanup");
    }
}
