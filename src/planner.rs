use crate::{
    bandit::{Features, Model},
    types::{Pos, State, matches, points},
    world::{World, clearance},
};
use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap, HashSet, VecDeque},
};

#[derive(Clone, Debug)]
pub struct Plan {
    pub target: Pos,
    pub stand: Pos,
    pub path: Vec<Pos>,
    pub seconds: f64,
    pub vein: usize,
    pub ore: bool,
    pub features: Features,
    pub score: f64,
    pub brain_features: Option<crate::brain::Features>,
}
fn blocked_by_enemy(p: Pos, state: &State) -> bool {
    state
        .hostiles
        .iter()
        .any(|e| e.visible && p.distance(e.position.cell()) < 4.)
}
fn vein_neighbors(p: Pos) -> impl Iterator<Item = Pos> {
    (-1..=1).flat_map(move |dy| {
        (-1..=1).flat_map(move |dz| {
            (-1..=1).filter_map(move |dx| {
                (dx != 0 || dy != 0 || dz != 0).then_some(p.offset(dx, dy, dz))
            })
        })
    })
}

/// Remember every member, including mined connectors, so breaking the center of
/// a vein cannot turn its remaining branches into unrelated targets.
pub struct VeinFocus {
    members: HashSet<Pos>,
    pending: HashSet<Pos>,
}
impl VeinFocus {
    pub fn new(world: &World, goal: &str, seed: Pos) -> Self {
        let mut out = Self {
            members: HashSet::from([seed]),
            pending: HashSet::from([seed]),
        };
        out.refresh(world, goal);
        out
    }
    pub fn refresh(&mut self, world: &World, goal: &str) {
        self.pending.retain(|p| {
            world
                .get(*p)
                .is_none_or(|b| b.block == "unknown" || b.ore && matches(&b.block, goal))
        });
        let mut queue: VecDeque<_> = self.members.iter().copied().collect();
        while let Some(p) = queue.pop_front() {
            for q in vein_neighbors(p) {
                if world
                    .get(q)
                    .is_some_and(|b| b.ore && matches(&b.block, goal))
                {
                    self.pending.insert(q);
                    if self.members.insert(q) {
                        queue.push_back(q);
                    }
                }
            }
        }
    }
    pub fn contains(&self, p: Pos) -> bool {
        self.members.contains(&p)
    }
    pub fn remaining(&self, world: &World, goal: &str) -> usize {
        self.pending
            .iter()
            .filter(|p| {
                world
                    .get(**p)
                    .is_none_or(|b| b.block == "unknown" || b.ore && matches(&b.block, goal))
            })
            .count()
    }
}
pub fn plan(
    world: &World,
    state: &State,
    model: &Model,
    blocked: &HashSet<Pos>,
    visits: &HashMap<Pos, u32>,
) -> Option<Plan> {
    candidates(world, state, model, blocked, visits)
        .into_iter()
        .max_by(|a, b| a.score.total_cmp(&b.score))
}
pub fn candidates(
    world: &World,
    state: &State,
    model: &Model,
    blocked: &HashSet<Pos>,
    visits: &HashMap<Pos, u32>,
) -> Vec<Plan> {
    candidates_for(world, state, model, blocked, visits, None)
}
pub fn candidates_for(
    world: &World,
    state: &State,
    model: &Model,
    blocked: &HashSet<Pos>,
    visits: &HashMap<Pos, u32>,
    focus: Option<&VeinFocus>,
) -> Vec<Plan> {
    let start = state.position.cell();
    let Some(root) = world.index(start) else {
        return Vec::new();
    };
    if !world.supported(start) {
        return Vec::new();
    }
    let count = world.scan.cells.len();
    let liquids = crate::safety::LiquidMask::new(world);
    let dry = |p| liquids.clear(p);
    if !dry(start) || !dry(start.offset(0, 1, 0)) {
        return Vec::new();
    }
    let mut distance = vec![u64::MAX; count];
    let mut parent = vec![None; count];
    distance[root] = 0;
    let mut heap = BinaryHeap::from([Reverse((0, root))]);
    // One Dijkstra traversal scores all reachable ore veins, instead of one A* per ore.
    while let Some(Reverse((cost, index))) = heap.pop() {
        if cost != distance[index] {
            continue;
        }
        let from = world.pos(index);
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            for dy in -1..=1 {
                let to = from.offset(dx, dy, dz);
                let Some(next) = world.index(to) else {
                    continue;
                };
                if blocked.contains(&to)
                    || blocked_by_enemy(to, state)
                    || clearance(from, to).iter().any(|p| blocked.contains(p))
                {
                    continue;
                }
                let Some(seconds) = world.cost_with(from, to, &dry) else {
                    continue;
                };
                let total = cost + (seconds * 1000.).ceil() as u64;
                if total < distance[next] {
                    distance[next] = total;
                    parent[next] = Some(index);
                    heap.push(Reverse((total, next)));
                }
            }
        }
    }
    let mut candidates = Vec::new();
    let mut seen = HashSet::new();
    for i in 0..count {
        let pos = world.pos(i);
        let Some(block) = world.get(pos) else {
            continue;
        };
        if !block.ore
            || !matches(&block.block, &state.mining_target)
            || focus.is_some_and(|f| !f.contains(pos))
            || !world.can_clear_with(pos, &dry)
            || blocked.contains(&pos)
            || !seen.insert(pos)
        {
            continue;
        }
        let mut vein = Vec::new();
        let mut pending = VecDeque::from([pos]);
        while let Some(p) = pending.pop_front() {
            vein.push(p);
            for q in vein_neighbors(p) {
                if world
                    .get(q)
                    .is_some_and(|b| b.ore && matches(&b.block, &state.mining_target))
                    && focus.is_none_or(|f| f.contains(q))
                    && !blocked.contains(&q)
                    && seen.insert(q)
                {
                    pending.push_back(q);
                }
            }
        }
        let mut best: Option<(u64, Pos, usize)> = None;
        for p in &vein {
            if !world.can_clear_with(*p, &dry) {
                continue;
            }
            for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                for dy in [-1, 0] {
                    let stand = p.offset(dx, dy, dz);
                    let Some(j) = world.index(stand) else {
                        continue;
                    };
                    if distance[j] == u64::MAX {
                        continue;
                    }
                    let cost =
                        distance[j] + (world.get(*p).map_or(3600., |b| b.seconds) * 1000.) as u64;
                    if best.as_ref().is_none_or(|v| cost < v.0) {
                        best = Some((cost, *p, j));
                    }
                }
            }
        }
        if let Some((cost, p, j)) = best {
            let value = points(
                world.get(p).map_or("", |b| b.block.as_str()),
                &state.mining_target,
            );
            candidates.push(make_plan(
                world,
                state,
                &parent,
                root,
                j,
                p,
                cost,
                vein.len(),
                value,
                visits,
                model,
            ));
        }
    }
    if candidates.is_empty() && focus.is_none() {
        // Descend in checked stair segments whenever possible. Neural/bandit scores
        // rank only this class of choices and cannot replace it with level roaming.
        let lowest = distance
            .iter()
            .enumerate()
            .filter(|(j, cost)| {
                **cost != u64::MAX && visits.get(&world.pos(*j)).copied().unwrap_or(0) <= 3
            })
            .map(|(j, _)| world.pos(j).y)
            .min()
            .unwrap_or(start.y);
        let descent_y = lowest.max(start.y - 6);
        for (j, cost) in distance.iter().enumerate() {
            if *cost == u64::MAX {
                continue;
            }
            let p = world.pos(j);
            if blocked.contains(&p)
                || if lowest < start.y {
                    p.y > descent_y
                } else {
                    p.y != start.y || start.distance(p) < 6.
                }
            {
                continue;
            }
            let steps = visits.get(&p).copied().unwrap_or(0);
            if steps > 3 {
                continue;
            }
            candidates.push(make_plan(
                world, state, &parent, root, j, p, *cost, 0, 0., visits, model,
            ));
        }
    }
    candidates.sort_by(|a, b| b.score.total_cmp(&a.score));
    candidates.truncate(32);
    candidates
}
#[allow(clippy::too_many_arguments)]
fn make_plan(
    world: &World,
    state: &State,
    parent: &[Option<usize>],
    root: usize,
    index: usize,
    target: Pos,
    cost: u64,
    vein: usize,
    value: f64,
    visits: &HashMap<Pos, u32>,
    model: &Model,
) -> Plan {
    let mut path = Vec::new();
    let mut current = index;
    while current != root {
        path.push(world.pos(current));
        if let Some(p) = parent[current] {
            current = p;
        } else {
            break;
        }
    }
    path.reverse();
    let seconds = cost as f64 / 1000.;
    let stand = world.pos(index);
    let mut previous = state.position.cell();
    let mut digs: f64 = 0.;
    for p in &path {
        for cell in clearance(previous, *p) {
            if world.get(cell).is_some_and(|b| !b.clear) {
                digs += 1.;
            }
        }
        previous = *p;
    }
    let novelty = 1. / (1. + f64::from(visits.get(&stand).copied().unwrap_or(0)));
    let durability = state
        .inventory
        .iter()
        .filter(|s| s.pickaxe)
        .map(|s| s.durability)
        .max()
        .unwrap_or(0)
        .clamp(0, 1000) as f64
        / 1000.;
    let features = [
        1.,
        (value / 100.).min(1.),
        (vein as f64 / 10.).min(1.),
        (seconds / 60.).min(1.),
        (digs / 30.).min(1.),
        f64::from((stand.y - state.position.cell().y).abs()) / 16.,
        state.health / 20.,
        state.food as f64 / 20.,
        durability,
        novelty,
    ];
    let base = if vein > 0 {
        4. * (1. + value).ln() / 101_f64.ln() / (1. + seconds / 10.)
    } else {
        0.4 * novelty + 0.2 / (1. + seconds / 10.)
    };
    Plan {
        target,
        stand,
        path,
        seconds,
        vein,
        ore: vein > 0,
        features,
        score: base + 0.3 * model.score(&features).clamp(-2., 2.),
        brain_features: None,
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::types::{Block, Point, Scan};
    pub fn fixture() -> (World, State) {
        let state = State {
            protocol: 8,
            position: Point {
                x: 3.5,
                y: 1.,
                z: 3.5,
            },
            health: 20.,
            food: 20,
            ..State::default()
        };
        let stone = Block {
            block: "minecraft:stone".into(),
            support: true,
            diggable: true,
            seconds: 0.2,
            ..Block::default()
        };
        let air = Block {
            block: "minecraft:air".into(),
            clear: true,
            ..Block::default()
        };
        let ore = Block {
            block: "minecraft:iron_ore".into(),
            ore: true,
            ..stone.clone()
        };
        let scan = Scan {
            protocol: 8,
            id: "test".into(),
            updated_at: 0,
            server: String::new(),
            username: String::new(),
            dimension: String::new(),
            origin: Pos { x: 0, y: -3, z: 0 },
            size: Pos { x: 9, y: 12, z: 9 },
            palette: vec![stone, air, ore],
            cells: vec![0; 9 * 12 * 9],
            chests: Vec::new(),
        };
        let mut world = World::new(scan, &state).expect("world");
        for y in 1..=2 {
            world.clear(Pos { x: 3, y, z: 3 });
        }
        (world, state)
    }
    #[test]
    fn targets_buried_ore_with_digging_path() {
        let (mut world, state) = fixture();
        let ore = Pos { x: 6, y: 1, z: 3 };
        world.patches.insert(ore, world.scan.palette[2].clone());
        let p = plan(
            &world,
            &state,
            &Model::default(),
            &HashSet::new(),
            &HashMap::new(),
        )
        .expect("reachable ore");
        assert_eq!(p.target, ore);
        assert!(p.ore);
        assert!(!p.path.is_empty());
    }
    #[test]
    fn stairs_and_hazards_have_real_constraints() {
        let (mut world, state) = fixture();
        let start = state.position.cell();
        assert!(world.cost(start, start.offset(1, 1, 0)).is_some());
        let down = start.offset(1, -1, 0);
        world.clear(down.offset(0, -1, 0));
        assert!(world.cost(start, down).is_none()); // no landing support
        let landing = start.offset(1, 0, 0);
        world.patches.insert(
            landing.offset(0, -1, 0),
            Block {
                block: "minecraft:lava".into(),
                danger: true,
                fluid: true,
                ..Block::default()
            },
        );
        assert!(world.cost(start, landing).is_none());
        assert!(!world.can_clear(landing));
    }
    #[test]
    fn selected_ore_and_blocked_target_are_respected() {
        let (mut world, mut state) = fixture();
        let iron = Pos { x: 6, y: 1, z: 3 };
        let diamond = Pos { x: 3, y: 1, z: 6 };
        world.patches.insert(iron, world.scan.palette[2].clone());
        world.patches.insert(
            diamond,
            Block {
                block: "minecraft:diamond_ore".into(),
                ..world.scan.palette[2].clone()
            },
        );
        state.mining_target = "minecraft:deepslate_diamond_ore".into();
        let p = plan(
            &world,
            &state,
            &Model::default(),
            &HashSet::new(),
            &HashMap::new(),
        )
        .expect("diamond");
        assert_eq!(p.target, diamond);
        let p = plan(
            &world,
            &state,
            &Model::default(),
            &HashSet::from([diamond]),
            &HashMap::new(),
        );
        assert!(p.is_none_or(|p| !p.ore));
    }

    #[test]
    fn full_scan_plans_without_a_search_per_ore() {
        let (mut world, mut state) = fixture();
        world.scan.size = Pos {
            x: 33,
            y: 33,
            z: 33,
        };
        world.scan.cells = vec![0; 33 * 33 * 33];
        world.patches.clear();
        state.position = Point {
            x: 16.5,
            y: 16.,
            z: 16.5,
        };
        world.clear(state.position.cell());
        world.clear(state.position.cell().offset(0, 1, 0));
        for x in [6, 12, 22, 27] {
            for y in [7, 14, 23] {
                let pos = Pos { x, y, z: 22 };
                let index = world.index(pos).expect("index");
                world.scan.cells[index] = 2;
            }
        }
        let began = std::time::Instant::now();
        let plan = super::plan(
            &world,
            &state,
            &Model::default(),
            &HashSet::new(),
            &HashMap::new(),
        )
        .expect("plan");
        println!(
            "35937-cell / 12-ore planning: {} ms",
            began.elapsed().as_millis()
        );
        assert!(plan.ore);
        assert!(!plan.path.is_empty());
    }

    #[test]
    fn descent_clears_three_blocks_and_keeps_landing_floor() {
        let (mut world, _) = fixture();
        let from = Pos { x: 3, y: 3, z: 3 };
        let to = from.offset(1, -1, 0);
        world
            .patches
            .insert(from.offset(0, -1, 0), world.scan.palette[0].clone());
        world.clear(from);
        world.clear(from.offset(0, 1, 0));
        assert!(world.cost(from, to).is_some());
        let cells = clearance(from, to);
        assert_eq!(cells.len(), 3);
        assert!(!cells.contains(&from.offset(0, -1, 0)));
        assert!(!cells.contains(&to.offset(0, -1, 0)));
        world.patches.insert(
            to.offset(0, 3, 0),
            crate::types::Block {
                block: "minecraft:gravel".into(),
                falling: true,
                ..world.scan.palette[0].clone()
            },
        );
        assert!(world.cost(from, to).is_none());
    }

    #[test]
    fn open_corridor_next_to_liquid_and_unknown_are_not_safe() {
        let (mut world, state) = fixture();
        let to = state.position.cell().offset(1, 0, 0);
        world.clear(to);
        world.clear(to.offset(0, 1, 0));
        world.patches.insert(
            to.offset(1, 0, 0),
            crate::types::Block {
                block: "minecraft:water".into(),
                fluid: true,
                danger: true,
                ..Default::default()
            },
        );
        assert!(world.cost(state.position.cell(), to).is_none());
        world.patches.insert(
            to.offset(1, 0, 0),
            crate::types::Block {
                block: "unknown".into(),
                ..Default::default()
            },
        );
        assert!(world.cost(state.position.cell(), to).is_none());
    }

    #[test]
    fn fresh_origin_floor_prevents_planning_from_an_air_center() {
        let (mut world, mut state) = fixture();
        let floor = state.position.cell().offset(0, -1, 0);
        let destination = state.position.cell().offset(1, 0, 0);
        assert!(world.cost(state.position.cell(), destination).is_some());
        state.navigation.source_floor = Some(crate::types::LocalBlock {
            pos: floor,
            kind: world.scan.palette[1].clone(),
            ..Default::default()
        });
        world.patch(&state);
        assert!(world.cost(state.position.cell(), destination).is_none());
        assert!(
            plan(
                &world,
                &state,
                &Model::default(),
                &HashSet::new(),
                &HashMap::new()
            )
            .is_none()
        );
    }

    #[test]
    fn focus_keeps_split_and_diagonal_members_and_never_switches_to_other_veins() {
        let (mut world, state) = fixture();
        let left = Pos { x: 4, y: 1, z: 3 };
        let connector = Pos { x: 5, y: 1, z: 3 };
        let right = Pos { x: 6, y: 1, z: 3 };
        let unrelated = Pos { x: 3, y: 1, z: 6 };
        for p in [left, connector, right, unrelated] {
            world.patches.insert(p, world.scan.palette[2].clone());
        }
        let mut focus = VeinFocus::new(&world, "minecraft:iron_ore", connector);
        assert_eq!(focus.remaining(&world, "minecraft:iron_ore"), 3);
        world.clear(connector);
        let diagonal = right.offset(0, 1, 1);
        world
            .patches
            .insert(diagonal, world.scan.palette[2].clone());
        focus.refresh(&world, "minecraft:iron_ore");
        assert!(focus.contains(diagonal));
        assert!(!focus.contains(unrelated));
        let choices = candidates_for(
            &world,
            &state,
            &Model::default(),
            &HashSet::new(),
            &HashMap::new(),
            Some(&focus),
        );
        assert!(!choices.is_empty());
        assert!(choices.iter().all(|p| p.ore && focus.contains(p.target)));
        let denied = HashSet::from([left, right, diagonal]);
        assert!(
            candidates_for(
                &world,
                &state,
                &Model::default(),
                &denied,
                &HashMap::new(),
                Some(&focus)
            )
            .is_empty(),
            "Cannot abandon blocked focus for an unrelated vein or exploration"
        );
        for p in [left, right, diagonal] {
            world.clear(p);
        }
        focus.refresh(&world, "minecraft:iron_ore");
        assert_eq!(focus.remaining(&world, "minecraft:iron_ore"), 0);
        let mut moved_scan = world.scan.clone();
        moved_scan.origin.x += 100;
        let moved_world = World::new(moved_scan, &state).expect("later scan");
        focus.refresh(&moved_world, "minecraft:iron_ore");
        assert_eq!(
            focus.remaining(&moved_world, "minecraft:iron_ore"),
            0,
            "Mined connectors must not reappear outside the scan"
        );
        assert_eq!(
            plan(
                &world,
                &state,
                &Model::default(),
                &HashSet::new(),
                &HashMap::new()
            )
            .expect("next vein")
            .target,
            unrelated
        );
    }

    #[test]
    fn no_target_ore_forces_safe_descent_before_neural_scoring() {
        let (mut world, mut state) = fixture();
        state.position = Point {
            x: 4.5,
            y: 5.,
            z: 4.5,
        };
        state.mining_target = "minecraft:diamond_ore".into();
        world.clear(state.position.cell());
        world.clear(state.position.cell().offset(0, 1, 0));
        world
            .patches
            .insert(Pos { x: 5, y: 5, z: 4 }, world.scan.palette[2].clone()); // not selected
        let choices = candidates(
            &world,
            &state,
            &Model::default(),
            &HashSet::new(),
            &HashMap::new(),
        );
        assert!(!choices.is_empty());
        for choice in choices {
            assert!(!choice.ore);
            assert!(choice.stand.y <= state.position.cell().y - 6);
            let mut from = state.position.cell();
            for to in choice.path {
                assert_eq!((to.x - from.x).abs() + (to.z - from.z).abs(), 1);
                assert!((to.y - from.y).abs() <= 1);
                assert!(world.cost(from, to).is_some());
                from = to;
            }
        }
    }

    #[test]
    fn target_ore_inside_two_block_liquid_buffer_is_not_mined() {
        let (mut world, state) = fixture();
        let ore = Pos { x: 6, y: 1, z: 4 };
        world.patches.insert(ore, world.scan.palette[2].clone());
        world.patches.insert(
            ore.offset(2, 2, 2),
            Block {
                block: "minecraft:lava".into(),
                fluid: true,
                danger: true,
                ..Default::default()
            },
        );
        assert!(!world.can_clear(ore));
        let focus = VeinFocus::new(&world, "", ore);
        assert!(
            candidates_for(
                &world,
                &state,
                &Model::default(),
                &HashSet::new(),
                &HashMap::new(),
                Some(&focus)
            )
            .is_empty()
        );
    }
}
