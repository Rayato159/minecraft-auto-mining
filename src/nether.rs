use crate::types::{Block, Enemy, Pos, State};

pub const DIMENSION: &str = "minecraft:the_nether";

pub fn active(state: &State) -> bool {
    state.dimension == DIMENSION
}

/// Only debris has a specialized vanilla height policy. Other ores, including
/// modded ores, explore the current stratum instead of guessing their worldgen.
pub fn exploration_y(state: &State) -> Option<i32> {
    active(state).then(|| {
        if state.mining_target == "minecraft:ancient_debris" {
            15
        } else {
            state.feet().y.clamp(8, 112)
        }
    })
}

pub fn threat_radius(state: &State, enemy: &Enemy) -> f64 {
    if active(state) && enemy.ranged {
        24.
    } else if active(state) && enemy.avoid_only {
        8.
    } else {
        4.
    }
}

pub fn interrupts_work(state: &State) -> bool {
    active(state)
        && !state
            .hostiles
            .iter()
            .any(|e| !e.avoid_only && e.visible && e.reach_distance() <= 3.)
        && (state.on_fire && !state.fire_resistant
            || state.hostiles.iter().any(|e| {
                e.visible
                    && (e.ranged || e.avoid_only)
                    && e.distance < threat_radius(state, e)
                    // In-place melee defense still takes priority against hostiles.
                    && (e.avoid_only || e.reach_distance() > 3.)
            }))
}

/// Require a stable roof for mining routes. This does not claim protection from
/// explosions; home travel may still use existing open passages.
pub fn covered<'a>(get: &impl Fn(Pos) -> Option<&'a Block>, feet: Pos) -> bool {
    [2, 3].iter().any(|dy| {
        get(feet.offset(0, *dy, 0))
            .is_some_and(|b| b.support && !b.clear && !b.fluid && !b.danger && !b.falling)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bandit::Model,
        planner,
        types::{Point, Stack},
        world::World,
    };
    use std::collections::{HashMap, HashSet};

    fn fixture(y: i32) -> (World, State) {
        let (mut world, mut state) = planner::tests::fixture();
        state.dimension = DIMENSION.into();
        state.mining_target = "minecraft:ancient_debris".into();
        state.position = Point {
            x: 0.5,
            y: f64::from(y),
            z: 0.5,
        };
        world.scan.dimension = state.dimension.clone();
        world.scan.origin = Pos {
            x: -16,
            y: y - 16,
            z: -16,
        };
        world.scan.size = Pos {
            x: 33,
            y: 33,
            z: 33,
        };
        world.scan.cells = vec![0; 33 * 33 * 33];
        world.scan.palette[0].block = "minecraft:netherrack".into();
        world.scan.palette[2].block = state.mining_target.clone();
        world.patches.clear();
        world.clear(state.feet());
        world.clear(state.feet().offset(0, 1, 0));
        (world, state)
    }
    #[test]
    fn descending_route_cannot_count_its_own_planned_cut_as_roof_cover() {
        let (mut world, state) = fixture(21);
        let next = state.feet().offset(1, -1, 0);
        world.clear(next.offset(0, 3, 0));
        assert!(covered(&|p| world.get(p), next), "pre-cut roof exists");
        assert!(
            world.cost(state.feet(), next).is_none(),
            "the only roof would be removed to make headroom"
        );
    }
    #[test]
    fn soul_sand_uses_checked_standing_cell_and_remains_diggable_for_descent() {
        let (mut world, mut state) = fixture(112);
        state.position.y = 111.875;
        state.on_ground = true;
        state.navigation.feet = Some(Pos { x: 0, y: 112, z: 0 });
        let mut soul = world.scan.palette[0].clone();
        soul.block = "minecraft:soul_sand".into();
        soul.seconds = 0.15;
        world
            .patches
            .insert(Pos { x: 0, y: 111, z: 0 }, soul.clone());
        world.patches.insert(Pos { x: 1, y: 111, z: 0 }, soul);
        assert_eq!(
            state.position.cell().y,
            111,
            "physical position remains untouched"
        );
        assert_eq!(state.feet().y, 112);
        let plan = planner::plan(
            &world,
            &state,
            &Model::default(),
            &HashSet::new(),
            &HashMap::new(),
        )
        .expect("checked staircase out of soul sand");
        assert!(plan.stand.y < state.feet().y);
        let mut from = state.feet();
        for to in &plan.path {
            assert!(world.cost(from, *to).is_some());
            assert!(!crate::world::clearance(from, *to).contains(&from.offset(0, -1, 0)));
            from = *to;
        }
    }
    #[test]
    fn debris_descends_or_ascends_towards_fifteen_then_explores_horizontally() {
        for y in [9, 15, 21] {
            let (world, state) = fixture(y);
            let plans = planner::candidates(
                &world,
                &state,
                &Model::default(),
                &HashSet::new(),
                &HashMap::new(),
            );
            assert!(!plans.is_empty());
            assert!(plans.iter().all(|p| !p.ore && p.stand.y == 15), "from {y}");
            for plan in plans {
                let mut previous = state.feet();
                for next in plan.path {
                    assert!(world.cost(previous, next).is_some());
                    assert!(covered(&|p| world.get(p), next));
                    previous = next;
                }
            }
        }
    }
    #[test]
    fn other_nether_ores_keep_current_level_without_assuming_mod_worldgen() {
        let (world, mut state) = fixture(46);
        for ore in [
            "minecraft:nether_quartz_ore",
            "minecraft:nether_gold_ore",
            "mod:unknown_ore",
            "",
        ] {
            state.mining_target = ore.into();
            let plans = planner::candidates(
                &world,
                &state,
                &Model::default(),
                &HashSet::new(),
                &HashMap::new(),
            );
            assert!(!plans.is_empty());
            assert!(plans.iter().all(|p| p.stand.y == 46));
        }
    }
    #[test]
    fn visible_debris_preempts_height_policy_but_lava_pocket_and_wrong_tool_block_it() {
        let (mut world, state) = fixture(21);
        let ore = Pos { x: 6, y: 21, z: 0 };
        world.patches.insert(ore, world.scan.palette[2].clone());
        let choose = |w: &World| {
            planner::plan(
                w,
                &state,
                &Model::default(),
                &HashSet::new(),
                &HashMap::new(),
            )
            .expect("route")
        };
        assert_eq!(choose(&world).target, ore);
        world.patches.insert(
            ore.offset(2, 0, 0),
            Block {
                block: "minecraft:lava".into(),
                fluid: true,
                danger: true,
                ..Default::default()
            },
        );
        assert!(!choose(&world).ore, "lava hidden behind debris");
        world.patches.remove(&ore.offset(2, 0, 0));
        world.patches.get_mut(&ore).expect("ore").diggable = false;
        assert!(
            !choose(&world).ore,
            "a non-harvestable ore must not be targeted"
        );
    }
    #[test]
    fn open_cavern_is_not_a_mining_route_and_fire_magma_or_ghast_block_routes() {
        let (mut world, mut state) = fixture(15);
        for i in 0..world.scan.cells.len() {
            if world.pos(i).y >= 15 {
                world.scan.cells[i] = 1;
            }
        }
        assert!(
            planner::candidates(
                &world,
                &state,
                &Model::default(),
                &HashSet::new(),
                &HashMap::new()
            )
            .is_empty()
        );
        let (mut enclosed, s) = fixture(15);
        let next = s.feet().offset(1, 0, 0);
        for id in [
            "minecraft:fire",
            "minecraft:soul_fire",
            "minecraft:magma_block",
        ] {
            enclosed.patches.insert(
                next.offset(0, -1, 0),
                Block {
                    block: id.into(),
                    support: true,
                    danger: true,
                    ..Default::default()
                },
            );
            assert!(enclosed.cost(s.feet(), next).is_none());
        }
        state = s;
        state.hostiles.push(Enemy {
            position: Point {
                x: 10.,
                y: 15.,
                z: 0.,
            },
            distance: 10.,
            visible: true,
            ranged: true,
            ..Default::default()
        });
        assert!(
            planner::candidates(
                &enclosed,
                &state,
                &Model::default(),
                &HashSet::new(),
                &HashMap::new()
            )
            .is_empty()
        );
    }
    #[test]
    fn nether_torches_ignore_brightness_space_the_trail_and_trigger_restock() {
        let (_, mut state) = fixture(15);
        state.free_slots = 20;
        state.capabilities.push("rear_torches".into());
        state.light_level = 15;
        state.inventory = vec![
            Stack {
                pickaxe: true,
                count: 1,
                durability: 500,
                ..Default::default()
            },
            Stack {
                food: true,
                count: 32,
                ..Default::default()
            },
            Stack {
                torch: true,
                count: 64,
                ..Default::default()
            },
        ];
        assert!(crate::lighting::due(&state, None));
        assert!(!crate::lighting::due(
            &state,
            Some(state.feet().offset(5, 0, 0))
        ));
        assert!(crate::lighting::due(
            &state,
            Some(state.feet().offset(6, 0, 0))
        ));
        state.inventory.retain(|s| !s.torch);
        assert!(!crate::lighting::due(&state, None));
        assert_eq!(
            crate::navigation::return_reason(&state),
            Some("Nether trail torches exhausted")
        );
        state.dimension = "minecraft:overworld".into();
        assert_eq!(crate::navigation::return_reason(&state), None);
    }
    #[test]
    fn ranged_and_neutral_mobs_interrupt_without_disabling_melee_defense() {
        let mut state = State {
            dimension: DIMENSION.into(),
            ..Default::default()
        };
        state.hostiles.push(Enemy {
            ranged: true,
            visible: true,
            distance: 18.,
            ..Default::default()
        });
        assert!(interrupts_work(&state));
        state.hostiles[0].attack_distance = Some(2.);
        state.on_fire = true;
        assert!(!interrupts_work(&state));
        state.on_fire = false;
        state.hostiles[0].ranged = false;
        state.hostiles[0].avoid_only = true;
        state.hostiles[0].distance = 2.;
        assert!(interrupts_work(&state));
        state.dimension = "minecraft:overworld".into();
        assert!(!interrupts_work(&state));
    }
}
