//! Persistent terrain for trips longer than a scan. Unknown cells are never traversed.
use crate::{
    bridge::atomic_json,
    safety::{LIQUID_RADIUS, LiquidMask},
    types::{Block, Home, Pos, State},
    world::{World, clearance},
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Default)]
pub struct Atlas {
    cells: HashMap<Pos, usize>,
    palette: Vec<Block>,
    ids: HashMap<String, usize>,
    session: String,
}
#[derive(Serialize, Deserialize)]
struct Saved {
    version: u32,
    session: String,
    palette: Vec<Block>,
    cells: Vec<(Pos, usize)>,
}
impl Atlas {
    fn path(runtime: &Path, session: &str) -> PathBuf {
        let hash = session.bytes().fold(0xcbf29ce484222325_u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
        });
        runtime.join(format!("terrain-{hash:016x}.json"))
    }
    pub fn load(runtime: &Path, state: &State) -> Result<Self> {
        let session = state.session_key();
        let path = Self::path(runtime, &session);
        let mut out = Self {
            session: session.clone(),
            ..Self::default()
        };
        if !path.exists() {
            return Ok(out);
        }
        let saved: Saved =
            serde_json::from_slice(&fs::read(&path)?).context("Read saved terrain")?;
        if saved.version == 1 {
            // Old scans treated doors/stair tops as solid walls. Preserve the
            // old file, then rescan geometry instead of guessing from block IDs.
            let backup = path.with_extension("v1.json");
            if !backup.exists() {
                fs::copy(&path, backup)?;
            }
            return Ok(out);
        }
        if saved.version != 2
            || saved.session != session
            || saved.cells.len() > 2_000_000
            || saved.cells.iter().any(|(_, i)| *i >= saved.palette.len())
            || saved
                .palette
                .iter()
                .any(|b| !b.seconds.is_finite() || b.seconds < 0.)
        {
            bail!("Invalid saved terrain; move its terrain-*.json file aside before retrying");
        }
        out.palette = saved.palette;
        for (i, b) in out.palette.iter().enumerate() {
            out.ids.insert(serde_json::to_string(b)?, i);
        }
        out.cells = saved.cells.into_iter().collect();
        Ok(out)
    }
    pub fn save(&self, runtime: &Path) -> Result<()> {
        atomic_json(
            &Self::path(runtime, &self.session),
            &Saved {
                version: 2,
                session: self.session.clone(),
                palette: self.palette.clone(),
                cells: self.cells.iter().map(|(p, i)| (*p, *i)).collect(),
            },
        )
    }
    pub fn ingest(&mut self, world: &World) -> Result<()> {
        let mut indices = Vec::new();
        for b in &world.scan.palette {
            indices.push(self.intern(b)?);
        }
        for (i, id) in world.scan.cells.iter().enumerate() {
            if world.scan.palette[*id].block != "unknown" {
                self.cells.insert(world.pos(i), indices[*id]);
            }
        }
        for (p, b) in &world.patches {
            self.put(*p, b)?;
        }
        if self.cells.len() > 2_000_000 {
            bail!("Terrain memory limit reached; stopped before discarding the return map");
        }
        Ok(())
    }
    fn intern(&mut self, b: &Block) -> Result<usize> {
        let key = serde_json::to_string(b)?;
        if let Some(i) = self.ids.get(&key) {
            return Ok(*i);
        }
        let i = self.palette.len();
        self.palette.push(b.clone());
        self.ids.insert(key, i);
        Ok(i)
    }
    pub fn put(&mut self, p: Pos, b: &Block) -> Result<()> {
        let id = self.intern(b)?;
        self.cells.insert(p, id);
        Ok(())
    }
    fn get<'a>(&'a self, world: &'a World, p: Pos) -> Option<&'a Block> {
        world
            .get(p)
            .or_else(|| self.cells.get(&p).and_then(|i| self.palette.get(*i)))
    }
    pub fn route(
        &self,
        world: &World,
        state: &State,
        goals: &[Pos],
        dig: bool,
        blocked: &HashSet<Pos>,
        visits: &HashMap<Pos, u32>,
    ) -> Option<TripPath> {
        self.search(
            world,
            state,
            goals,
            dig,
            blocked,
            visits,
            Some(Duration::from_millis(150)),
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn search(
        &self,
        world: &World,
        state: &State,
        goals: &[Pos],
        dig: bool,
        blocked: &HashSet<Pos>,
        visits: &HashMap<Pos, u32>,
        budget: Option<Duration>,
    ) -> Option<TripPath> {
        let start = state.position.cell();
        if goals.is_empty() {
            return None;
        }
        let began = Instant::now();
        let local_liquids = LiquidMask::new(world);
        let heuristic = |p: Pos| {
            goals
                .iter()
                .map(|q| {
                    let horizontal = (i64::from(p.x) - i64::from(q.x)).abs()
                        + (i64::from(p.z) - i64::from(q.z)).abs();
                    horizontal.max((i64::from(p.y) - i64::from(q.y)).abs()) as u64 * 500
                })
                .min()
                .unwrap_or(0)
        };
        let get = |p| self.get(world, p);
        let liquid_cache = std::cell::RefCell::new(HashMap::new());
        let dry = |p| {
            if let Some(safe) = local_liquids.interior_clear(p) {
                return safe;
            }
            if let Some(safe) = liquid_cache.borrow().get(&p).copied() {
                return safe;
            }
            let safe = crate::safety::liquid_clear(&get, p);
            liquid_cache.borrow_mut().insert(p, safe);
            safe
        };
        let mut costs = HashMap::from([(start, 0_u64)]);
        let mut parents = HashMap::new();
        let mut heap = BinaryHeap::from([Reverse((heuristic(start), 0_u64, start))]);
        let mut frontier: Option<(u64, Pos)> = None;
        let mut progress: Option<(u64, Pos)> = None;
        let mut short_progress: Option<(u64, Pos)> = None;
        let mut budget_exhausted = false;
        let mut expanded = 0;
        while let Some(Reverse((_, cost, p))) = heap.pop() {
            if costs.get(&p) != Some(&cost) {
                continue;
            }
            if goals.contains(&p) {
                return Some(path(start, p, cost, RouteEnd::Destination, &parents));
            }
            // Every popped node has a checked parent chain. Keep useful progress
            // for a bounded search, but never use it to escape a proven dead end.
            let score =
                cost + heuristic(p) + u64::from(visits.get(&p).copied().unwrap_or(0)) * 30_000;
            if p != start {
                let progress_score = heuristic(p)
                    + cost / 4
                    + u64::from(visits.get(&p).copied().unwrap_or(0)) * 30_000;
                if short_progress.is_none_or(|(best, _)| progress_score < best) {
                    short_progress = Some((progress_score, p));
                }
                if start.distance(p) >= 4. && progress.is_none_or(|(best, _)| progress_score < best)
                {
                    progress = Some((progress_score, p));
                }
            }
            expanded += 1;
            if expanded > if budget.is_some() { 20_000 } else { 120_000 }
                || expanded % 128 == 0 && budget.is_some_and(|b| began.elapsed() > b)
            {
                budget_exhausted = true;
                break;
            }
            // Advance to a known, supported point near the remembered map edge, then refresh.
            // Allow detours away from the goal, but discourage reusing a fruitless frontier.
            let near_unknown = start.distance(p) >= 6.
                && [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(x, z)| {
                    [0, 1].iter().any(|y| {
                        get(p.offset(*x * (LIQUID_RADIUS + 1), *y, *z * (LIQUID_RADIUS + 1)))
                            .is_none_or(|b| b.block == "unknown")
                    })
                })
                || start.distance(p) >= 8.
                    // Jump headroom reaches three cells above the standing
                    // point, then still needs the full liquid/unknown buffer.
                    && [-(LIQUID_RADIUS + 1), LIQUID_RADIUS + 3]
                        .iter()
                        .any(|y| get(p.offset(0, *y, 0)).is_none_or(|b| b.block == "unknown"));
            if near_unknown && frontier.is_none_or(|(best, _)| score < best) {
                frontier = Some((score, p));
            }
            for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                for dy in -1..=1 {
                    let to = p.offset(dx, dy, dz);
                    if blocked.contains(&to)
                        || clearance(p, to).iter().any(|q| blocked.contains(q))
                        || state
                            .hostiles
                            .iter()
                            .any(|e| e.visible && to.distance(e.position.cell()) < 4.)
                    {
                        continue;
                    }
                    let Some(edge) = travel_cost_with(&get, &state.home, p, to, dig, &dry) else {
                        continue;
                    };
                    let next = cost + edge;
                    if costs.get(&to).is_none_or(|v| next < *v) {
                        costs.insert(to, next);
                        parents.insert(to, p);
                        heap.push(Reverse((next + heuristic(to), next, to)));
                    }
                }
            }
        }
        let (end, reason) = if let Some((_, end)) = frontier {
            (end, RouteEnd::ScanFrontier)
        } else if budget_exhausted {
            (progress.or(short_progress)?.1, RouteEnd::SearchBudget)
        } else {
            return None;
        };
        Some(path(start, end, *costs.get(&end)?, reason, &parents))
    }
}

/// Replay the last recorded home trip without issuing any game commands.
pub fn benchmark(runtime: &Path, instance: &Path) -> Result<()> {
    let board: serde_json::Value =
        serde_json::from_slice(&fs::read(runtime.join("scoreboard.json"))?)?;
    let profiles: HashMap<String, Home> =
        serde_json::from_slice(&fs::read(instance.join("config/flyminer/home.json"))?)?;
    let world = board["world"].as_str().context("No recorded world")?;
    let (session, home) = profiles
        .iter()
        .find(|(k, _)| k.starts_with(&format!("{world}|")))
        .context("No matching home profile")?;
    let parts: Vec<_> = session.split('|').collect();
    anyhow::ensure!(parts.len() == 3, "Invalid session key");
    let start: Pos = serde_json::from_value(board["position"].clone())?;
    let state = State {
        server: parts[0].into(),
        username: parts[1].into(),
        dimension: parts[2].into(),
        position: crate::types::Point {
            x: f64::from(start.x) + 0.5,
            y: f64::from(start.y),
            z: f64::from(start.z) + 0.5,
        },
        home: home.clone(),
        health: 20.,
        ..Default::default()
    };
    let atlas = Atlas::load(runtime, &state)?;
    let mut palette = atlas.palette.clone();
    let unknown = palette.len();
    palette.push(Block {
        block: "unknown".into(),
        ..Default::default()
    });
    let origin = start.offset(-16, -16, -16);
    let cells = (0..33 * 33 * 33)
        .map(|i| {
            *atlas
                .cells
                .get(&origin.offset(i % 33, i / (33 * 33), (i / 33) % 33))
                .unwrap_or(&unknown)
        })
        .collect();
    let world = World::new(
        crate::types::Scan {
            protocol: 8,
            id: "replay".into(),
            updated_at: 0,
            server: state.server.clone(),
            username: state.username.clone(),
            dimension: state.dimension.clone(),
            origin,
            size: Pos {
                x: 33,
                y: 33,
                z: 33,
            },
            palette,
            cells,
            chests: vec![],
        },
        &state,
    )?;
    let target = home.home.context("Home unset")?;
    let mut reports = Vec::new();
    for (name, budget) in [
        ("previous_search_limit", None),
        ("minimal_time_slice", Some(Duration::ZERO)),
        ("bounded_search", Some(Duration::from_millis(150))),
    ] {
        let began = Instant::now();
        let route = atlas.search(
            &world,
            &state,
            &[target],
            true,
            &HashSet::new(),
            &HashMap::new(),
            budget,
        );
        reports.push(serde_json::json!({"mode":name,"planning_ms":began.elapsed().as_secs_f64()*1000.,"steps":route.as_ref().map(|r|r.path.len()),"reaches_home":route.as_ref().map(|r|r.complete),"estimated_seconds":route.as_ref().map(|r|r.seconds),"reason":route.as_ref().map(|r|r.end.label()),"endpoint":route.as_ref().and_then(|r|r.path.last())}));
    }
    let report = serde_json::json!({"source":"saved terrain + last scoreboard; no game actions","start":start,"home":target,"results":reports});
    println!("{}", serde_json::to_string_pretty(&report)?);
    atomic_json(&runtime.join("navigation-benchmark.json"), &report)
}
/// Batch only straight, level, freshly checked walking cells. No digging or blind corners.
pub fn walking_batch(world: &World, state: &State, path: &[Pos]) -> Vec<Pos> {
    let mut from = state.position.cell();
    let Some(first) = path.first() else {
        return Vec::new();
    };
    let direction = (first.x - from.x, first.z - from.z);
    let mut result = Vec::new();
    for &to in path.iter().take(8) {
        if to.y != from.y
            || (to.x - from.x, to.z - from.z) != direction
            || travel_cost(&|p| world.get(p), &state.home, from, to, false).is_none()
            || clearance(from, to)
                .iter()
                .any(|p| world.get(*p).is_none_or(|b| !b.clear))
        {
            break;
        }
        result.push(to);
        from = to;
    }
    result
}
#[derive(Debug)]
pub struct TripPath {
    pub path: Vec<Pos>,
    pub complete: bool,
    pub seconds: f64,
    pub end: RouteEnd,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteEnd {
    Destination,
    ScanFrontier,
    SearchBudget,
}
impl RouteEnd {
    pub fn label(self) -> &'static str {
        match self {
            Self::Destination => "Known destination",
            Self::ScanFrontier => "Scan frontier",
            Self::SearchBudget => "Checked partial progress",
        }
    }
}
fn path(
    start: Pos,
    end: Pos,
    cost: u64,
    reason: RouteEnd,
    parents: &HashMap<Pos, Pos>,
) -> TripPath {
    let mut p = end;
    let mut path = Vec::new();
    while p != start {
        path.push(p);
        let Some(previous) = parents.get(&p) else {
            break;
        };
        p = *previous;
    }
    path.reverse();
    TripPath {
        path,
        complete: reason == RouteEnd::Destination,
        seconds: cost as f64 / 1000.,
        end: reason,
    }
}
pub fn travel_cost<'a>(
    get: &impl Fn(Pos) -> Option<&'a Block>,
    home: &Home,
    from: Pos,
    to: Pos,
    dig: bool,
) -> Option<u64> {
    travel_cost_with(get, home, from, to, dig, &|p| {
        crate::safety::liquid_clear(get, p)
    })
}
fn travel_cost_with<'a>(
    get: &impl Fn(Pos) -> Option<&'a Block>,
    home: &Home,
    from: Pos,
    to: Pos,
    dig: bool,
    dry: &impl Fn(Pos) -> bool,
) -> Option<u64> {
    if (to.x - from.x).abs() + (to.z - from.z).abs() != 1 || (to.y - from.y).abs() > 1 {
        return None;
    }
    for p in [from, to] {
        if !get(p.offset(0, -1, 0)).is_some_and(|b| b.support && !b.danger && !b.falling) {
            return None;
        }
    }
    if !dry(from) || !dry(from.offset(0, 1, 0)) {
        return None;
    }
    if to.y > from.y && (!dry(from.offset(0, 3, 0)) || !dry(to.offset(0, 2, 0))) {
        return None;
    }
    let mut cost = if to.y > from.y { 800 } else { 500 };
    for p in clearance(from, to) {
        let b = get(p)?;
        if b.danger || b.fluid || b.block == "unknown" || !dry(p) {
            return None;
        }
        if !b.clear {
            if b.openable {
                cost += 500;
                continue;
            }
            if !dig
                || home.protected(p)
                || !b.diggable
                || b.falling
                || b.seconds > 18.
                || get(p.offset(0, 1, 0)).is_none_or(|v| v.falling)
            {
                return None;
            }
            cost += ((b.seconds.max(0.1) + 0.25) * 1000.).ceil() as u64;
        }
    }
    Some(cost)
}

pub fn cargo(state: &State) -> bool {
    state.inventory.iter().any(|s| {
        s.count > 0 && !s.pickaxe && !s.sword && !s.food && !s.torch && !s.crafting_reserve
    })
}
pub fn return_reason(state: &State) -> Option<&'static str> {
    if state.home.request > state.home.acknowledged {
        Some("Return requested in game")
    } else if state.free_slots == 0 {
        Some("Inventory full")
    } else if !state.has("pickaxe") {
        Some("Pickaxes exhausted")
    } else if !state.has("food") {
        Some("Food exhausted")
    } else if state.target_harvestable == Some(false) {
        Some("Need a pickaxe suitable for the selected ore")
    } else if !state.reserve_pickaxe() {
        Some("Pickaxe reserve is low")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn climbing_frontier_respects_jump_headroom_plus_liquid_buffer() {
        let (base, mut state) = fixture();
        let mut scan = base.scan.clone();
        scan.origin = Pos::default();
        scan.size = Pos {
            x: 33,
            y: 33,
            z: 33,
        };
        scan.palette.push(Block {
            block: "minecraft:bedrock".into(),
            support: true,
            ..Default::default()
        });
        scan.cells = (0..33 * 33 * 33)
            .map(|i| {
                let x = i % 33;
                let z = (i / 33) % 33;
                if (14..=18).contains(&x) && (14..=18).contains(&z) {
                    0
                } else {
                    3
                }
            })
            .collect();
        state.position = crate::types::Point {
            x: 16.5,
            y: 8.,
            z: 16.5,
        };
        let mut world = World::new(scan, &state).expect("shaft");
        world.clear(state.position.cell());
        world.clear(state.position.cell().offset(0, 1, 0));
        let mut atlas = Atlas::default();
        atlas.ingest(&world).expect("atlas");
        let route = atlas
            .search(
                &world,
                &state,
                &[Pos {
                    x: 16,
                    y: 80,
                    z: 16,
                }],
                true,
                &HashSet::new(),
                &HashMap::new(),
                None,
            )
            .expect("A supported staircase frontier must be returned before unknown jump headroom");
        assert!(!route.complete);
        assert_eq!(route.path.last().expect("end").y, 28);
        let mut from = state.position.cell();
        for to in route.path {
            assert!(travel_cost(&|p| world.get(p), &state.home, from, to, true).is_some());
            from = to;
        }
    }

    #[test]
    fn search_budget_returns_checked_progress_instead_of_claiming_no_route() {
        let (base, mut state) = fixture();
        let mut scan = base.scan.clone();
        scan.origin = Pos::default();
        scan.size = Pos {
            x: 33,
            y: 33,
            z: 33,
        };
        scan.cells = vec![0; 33 * 33 * 33];
        state.position = crate::types::Point {
            x: 16.5,
            y: 8.,
            z: 16.5,
        };
        let mut world = World::new(scan, &state).expect("world");
        world.clear(state.position.cell());
        world.clear(state.position.cell().offset(0, 1, 0));
        let mut atlas = Atlas::default();
        atlas.ingest(&world).expect("atlas");
        let goal = Pos {
            x: 16,
            y: 80,
            z: 16,
        };
        let route = atlas
            .search(
                &world,
                &state,
                &[goal],
                true,
                &HashSet::new(),
                &HashMap::new(),
                Some(Duration::ZERO),
            )
            .expect("A time slice ending before the scan edge is not proof of no route");
        assert!(!route.complete);
        assert_eq!(route.end, RouteEnd::SearchBudget);
        assert!(
            route.path.last().expect("end").distance(goal) < state.position.cell().distance(goal)
        );
        let mut from = state.position.cell();
        for to in route.path {
            assert!(travel_cost(&|p| world.get(p), &state.home, from, to, true).is_some());
            from = to;
        }
    }
    #[test]
    fn fully_searched_sealed_corridor_does_not_return_partial_progress() {
        let (base, mut state) = fixture();
        let mut scan = base.scan.clone();
        scan.origin = Pos::default();
        scan.size = Pos {
            x: 33,
            y: 33,
            z: 33,
        };
        scan.palette.push(Block {
            block: "minecraft:bedrock".into(),
            support: true,
            ..Default::default()
        });
        scan.cells = vec![3; 33 * 33 * 33];
        state.position = crate::types::Point {
            x: 16.5,
            y: 8.,
            z: 16.5,
        };
        let mut world = World::new(scan, &state).expect("sealed world");
        for x in 10..=22 {
            for y in 8..=9 {
                world.clear(Pos { x, y, z: 16 });
            }
        }
        let mut atlas = Atlas::default();
        atlas.ingest(&world).expect("atlas");
        assert!(
            atlas
                .search(
                    &world,
                    &state,
                    &[Pos {
                        x: 16,
                        y: 80,
                        z: 16
                    }],
                    true,
                    &HashSet::new(),
                    &HashMap::new(),
                    Some(Duration::from_secs(10))
                )
                .is_none()
        );
    }
    #[test]
    fn reserved_crafting_stacks_are_supplies_but_excess_stacks_remain_cargo() {
        let mut state = crate::types::State::default();
        state.inventory.push(crate::types::Stack {
            item: "minecraft:cobblestone".into(),
            count: 64,
            crafting_reserve: true,
            ..Default::default()
        });
        assert!(!super::cargo(&state));
        state.inventory.push(crate::types::Stack {
            item: "minecraft:cobblestone".into(),
            count: 64,
            ..Default::default()
        });
        assert!(super::cargo(&state));
    }
    use super::*;
    use crate::planner::tests::fixture;
    #[test]
    fn protected_hand_operated_door_is_opened_not_dug_or_batched() {
        let (mut w, mut s) = fixture();
        let from = s.position.cell();
        let to = from.offset(1, 0, 0);
        s.home.home = Some(to);
        s.home.radius = 10;
        for p in [to, to.offset(0, 1, 0)] {
            w.patches.insert(
                p,
                Block {
                    block: "unseen_mod:wood_door".into(),
                    openable: true,
                    ..Default::default()
                },
            );
        }
        assert!(travel_cost(&|p| w.get(p), &s.home, from, to, false).is_some());
        assert!(walking_batch(&w, &s, &[to]).is_empty());
        for p in [to, to.offset(0, 1, 0)] {
            w.patches.get_mut(&p).expect("door").openable = false;
        }
        assert!(travel_cost(&|p| w.get(p), &s.home, from, to, true).is_none());
    }
    #[test]
    fn batches_stop_before_corners_blocks_and_liquids() {
        let (mut w, s) = fixture();
        let path = vec![
            Pos { x: 4, y: 1, z: 3 },
            Pos { x: 5, y: 1, z: 3 },
            Pos { x: 5, y: 1, z: 4 },
        ];
        for &p in &path {
            w.clear(p);
            w.clear(p.offset(0, 1, 0));
        }
        assert_eq!(walking_batch(&w, &s, &path), path[..2]);
        w.patches.insert(
            Pos { x: 5, y: 1, z: 2 },
            Block {
                block: "minecraft:lava".into(),
                fluid: true,
                danger: true,
                ..Default::default()
            },
        );
        assert!(walking_batch(&w, &s, &path).is_empty());
    }
    #[test]
    fn protection_covers_all_heights_and_allows_existing_corridors() {
        let (mut w, mut s) = fixture();
        s.home.home = Some(Pos { x: 4, y: 64, z: 3 });
        s.home.radius = 2;
        w.home = s.home.clone();
        assert!(s.home.protected(Pos { x: 4, y: -60, z: 3 }));
        assert!(!w.can_clear(Pos { x: 4, y: 1, z: 3 }));
        let to = Pos { x: 4, y: 1, z: 3 };
        assert!(travel_cost(&|p| w.get(p), &s.home, s.position.cell(), to, true).is_none());
        for y in [1, 2] {
            w.clear(Pos { x: 4, y, z: 3 });
        }
        assert!(travel_cost(&|p| w.get(p), &s.home, s.position.cell(), to, false).is_some());
    }
    #[test]
    fn a_star_finds_shortest_known_route_and_refuses_digs_without_tool() {
        let (w, s) = fixture();
        let mut atlas = Atlas::default();
        atlas.ingest(&w).expect("map");
        let target = Pos { x: 5, y: 1, z: 3 };
        let r = atlas
            .route(&w, &s, &[target], true, &HashSet::new(), &HashMap::new())
            .expect("route");
        assert!(r.complete);
        assert_eq!(r.path.len(), 2);
        assert!(
            atlas
                .route(&w, &s, &[target], false, &HashSet::new(), &HashMap::new())
                .is_none()
        );
    }
    #[test]
    fn terrain_survives_restart_but_isolated_by_world_and_dimension() {
        let (w, mut s) = fixture();
        let root = std::env::temp_dir().join(format!("miner-atlas-{}", crate::bridge::now()));
        fs::create_dir_all(&root).expect("dir");
        let mut atlas = Atlas::load(&root, &s).expect("load");
        atlas.ingest(&w).expect("ingest");
        atlas.save(&root).expect("save");
        assert!(!Atlas::load(&root, &s).expect("restore").cells.is_empty());
        s.dimension = "another".into();
        assert!(Atlas::load(&root, &s).expect("other").cells.is_empty());
        fs::remove_dir_all(root).expect("clean");
    }

    #[test]
    fn route_joins_multiple_scans_and_never_steps_into_unknown_terrain() {
        let (base, mut state) = fixture();
        let mut atlas = Atlas::default();
        let mut current = None;
        for origin in [0, 6, 12, 18, 24] {
            let mut scan = base.scan.clone();
            scan.origin.x = origin;
            for y in [1, 2] {
                for x in 0..9 {
                    scan.cells[(x + 9 * (3 + 9 * (y - scan.origin.y))) as usize] = 1;
                }
            }
            let w = World::new(scan, &state).expect("world");
            atlas.ingest(&w).expect("remember");
            if origin == 0 {
                current = Some(w);
            }
        }
        let world = current.expect("current");
        state.position.x = 3.5;
        let target = Pos { x: 29, y: 1, z: 3 };
        let route = atlas
            .route(
                &world,
                &state,
                &[target],
                false,
                &HashSet::new(),
                &HashMap::new(),
            )
            .expect("long route");
        assert!(route.complete);
        assert_eq!(route.path.len(), 26);
        assert_eq!(route.seconds, 13.);
        let unknown = Pos { x: 50, ..target };
        let route = atlas
            .route(
                &world,
                &state,
                &[unknown],
                false,
                &HashSet::new(),
                &HashMap::new(),
            )
            .expect("advance through old scans to the next frontier");
        assert!(!route.complete);
        assert!(route.path.last().is_some_and(|p| p.x > 8));
        assert!(
            route
                .path
                .iter()
                .all(|p| atlas.get(&world, *p).is_some_and(|b| b.block != "unknown"))
        );
    }

    #[test]
    fn protected_obstacle_is_detoured_and_no_direct_vertical_edge_exists() {
        let (mut world, mut state) = fixture();
        state.home.home = Some(Pos { x: 4, y: 40, z: 3 });
        state.home.radius = 1;
        world.home = state.home.clone();
        // Open a longer corridor around a protected solid obstacle.
        for (x, z) in [(3, 3), (3, 4), (3, 5), (4, 5), (5, 5), (5, 4), (5, 3)] {
            for y in [1, 2] {
                world.clear(Pos { x, y, z });
            }
        }
        let mut atlas = Atlas::default();
        atlas.ingest(&world).expect("atlas");
        let goal = Pos { x: 5, y: 1, z: 3 };
        let path = atlas
            .route(
                &world,
                &state,
                &[goal],
                true,
                &HashSet::new(),
                &HashMap::new(),
            )
            .expect("detour");
        assert!(path.complete);
        assert_eq!(path.path.len(), 6);
        assert!(!path.path.contains(&Pos { x: 4, y: 1, z: 3 }));
        assert!(
            travel_cost(
                &|p| world.get(p),
                &state.home,
                state.position.cell(),
                state.position.cell().offset(0, 1, 0),
                true
            )
            .is_none()
        );
    }
}
