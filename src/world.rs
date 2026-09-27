use crate::types::{Block, Home, Pos, Scan, State};
use anyhow::{Result, bail};
use std::collections::HashMap;

pub struct World {
    pub scan: Scan,
    pub patches: HashMap<Pos, Block>,
    pub home: Home,
}
impl World {
    pub fn new(scan: Scan, state: &State) -> Result<Self> {
        let s = scan.size;
        if scan.protocol != 8
            || scan.dimension != state.dimension
            || scan.server != state.server
            || scan.username != state.username
            || !(1..=35).contains(&s.x)
            || !(1..=35).contains(&s.y)
            || !(1..=35).contains(&s.z)
            || scan.cells.len() != (s.x * s.y * s.z) as usize
            || scan.cells.iter().any(|i| *i >= scan.palette.len())
            || scan
                .palette
                .iter()
                .any(|b| !b.seconds.is_finite() || b.seconds < 0.)
        {
            bail!("Invalid or mismatched scan snapshot");
        }
        Ok(Self {
            scan,
            patches: HashMap::new(),
            home: state.home.clone(),
        })
    }
    pub fn index(&self, p: Pos) -> Option<usize> {
        let o = self.scan.origin;
        let s = self.scan.size;
        let (x, y, z) = (p.x - o.x, p.y - o.y, p.z - o.z);
        if x < 0 || y < 0 || z < 0 || x >= s.x || y >= s.y || z >= s.z {
            return None;
        }
        Some((x + s.x * (z + s.z * y)) as usize)
    }
    pub fn pos(&self, index: usize) -> Pos {
        let s = self.scan.size;
        let n = index as i32;
        self.scan
            .origin
            .offset(n % s.x, n / (s.x * s.z), (n / s.x) % s.z)
    }
    pub fn get(&self, p: Pos) -> Option<&Block> {
        self.patches.get(&p).or_else(|| {
            self.index(p)
                .and_then(|i| self.scan.palette.get(self.scan.cells[i]))
        })
    }
    pub fn patch(&mut self, state: &State) {
        self.home = state.home.clone();
        if let Some(floor) = &state.navigation.source_floor {
            self.patches.insert(floor.pos, floor.kind.clone());
        }
        for floor in state
            .navigation
            .routes
            .iter()
            .filter_map(|r| r.floor.as_ref())
        {
            self.patches.insert(floor.pos, floor.kind.clone());
        }
        for b in state
            .navigation
            .routes
            .iter()
            .flat_map(|r| &r.blocks)
            .chain(&state.navigation.ores)
        {
            self.patches.insert(b.pos, b.kind.clone());
        }
    }
    pub fn clear(&mut self, p: Pos) {
        self.patches.insert(
            p,
            Block {
                block: "minecraft:air".into(),
                clear: true,
                ..Block::default()
            },
        );
    }
    pub fn supported(&self, p: Pos) -> bool {
        self.get(p.offset(0, -1, 0))
            .is_some_and(|b| b.support && !b.danger && !b.falling)
    }
    pub fn can_clear(&self, p: Pos) -> bool {
        self.can_clear_with(p, &|p| crate::safety::liquid_clear(&|q| self.get(q), p))
    }
    pub fn can_clear_with(&self, p: Pos, dry: &impl Fn(Pos) -> bool) -> bool {
        let Some(b) = self.get(p) else {
            return false;
        };
        if b.danger || b.fluid || b.block == "unknown" || !dry(p) {
            return false;
        }
        if b.clear {
            return true;
        }
        !self.home.protected(p)
            && b.diggable
            && !b.danger
            && !b.falling
            && b.seconds <= 18.
            && self.get(p.offset(0, 1, 0)).is_some_and(|v| !v.falling)
    }
    pub fn cost(&self, from: Pos, to: Pos) -> Option<f64> {
        self.cost_with(from, to, &|p| {
            crate::safety::liquid_clear(&|q| self.get(q), p)
        })
    }
    pub fn cost_with(&self, from: Pos, to: Pos, dry: &impl Fn(Pos) -> bool) -> Option<f64> {
        if (to.x - from.x).abs() + (to.z - from.z).abs() != 1 || (to.y - from.y).abs() > 1 {
            return None;
        }
        if !self.supported(from) || !self.supported(to) || !dry(from) || !dry(from.offset(0, 1, 0))
        {
            return None;
        }
        let blocks = clearance(from, to);
        if self.scan.dimension == crate::nether::DIMENSION
            && !crate::nether::covered(
                &|p| {
                    if blocks.contains(&p) {
                        None
                    } else {
                        self.get(p)
                    }
                },
                to,
            )
        {
            return None;
        }
        if to.y > from.y && (!dry(from.offset(0, 3, 0)) || !dry(to.offset(0, 2, 0))) {
            return None; // The head rises above its final standing height during a jump.
        }
        if blocks.iter().any(|p| !self.can_clear_with(*p, dry)) {
            return None;
        }
        let mut seconds = if to.y > from.y { 0.8 } else { 0.5 };
        for p in blocks {
            let b = self.get(p)?;
            if !b.clear {
                seconds += b.seconds.max(0.1) + 0.25;
            }
        }
        Some(seconds)
    }
}
pub struct Clearance {
    cells: [Pos; 3],
    len: usize,
}
impl std::ops::Deref for Clearance {
    type Target = [Pos];
    fn deref(&self) -> &Self::Target {
        &self.cells[..self.len]
    }
}
impl IntoIterator for Clearance {
    type Item = Pos;
    type IntoIter = std::iter::Take<std::array::IntoIter<Pos, 3>>;
    fn into_iter(self) -> Self::IntoIter {
        self.cells.into_iter().take(self.len)
    }
}
pub fn clearance(from: Pos, to: Pos) -> Clearance {
    if to.y > from.y {
        Clearance {
            cells: [from.offset(0, 2, 0), to.offset(0, 1, 0), to],
            len: 3,
        }
    } else if to.y < from.y {
        Clearance {
            cells: [to.offset(0, 2, 0), to.offset(0, 1, 0), to],
            len: 3,
        }
    } else {
        Clearance {
            cells: [to.offset(0, 1, 0), to, to],
            len: 2,
        }
    }
}
