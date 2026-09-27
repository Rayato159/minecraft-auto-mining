use crate::{
    planner::{Plan, VeinFocus},
    types::{Pos, State},
    world::clearance,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub fn due(state: &State, last: Option<Pos>) -> bool {
    let nether = crate::nether::active(state);
    state.has("torch")
        && state.capabilities.iter().any(|c| c == "rear_torches")
        && (nether || state.light_level <= 6)
        && last.is_none_or(|p| p.distance(state.feet()) >= if nether { 6. } else { 4. })
}

/// Placement is executed and ray-checked by Forge. Pass the intended digging
/// direction, not the camera yaw left behind by mining/eating/placing a torch.
pub fn command(
    state: &State,
    plan: &Plan,
    focus: Option<&VeinFocus>,
    dig: Option<Pos>,
) -> Option<Value> {
    let here = state.feet();
    let ahead = dig
        .filter(|p| p.x != here.x || p.z != here.z)
        .or_else(|| {
            plan.path
                .iter()
                .copied()
                .find(|p| p.x != here.x || p.z != here.z)
        })
        .unwrap_or(plan.target);
    let (x, z) = (ahead.x - here.x, ahead.z - here.z);
    let (dx, dz) = if x.abs() > z.abs() {
        (x.signum(), 0)
    } else {
        (0, z.signum())
    };
    if dx == 0 && dz == 0 {
        return None;
    }
    let mut avoid = BTreeSet::from([here, here.offset(0, -1, 0), plan.target]);
    avoid.extend(dig);
    if let Some(focus) = focus {
        avoid.extend(focus.pending().filter(|p| here.distance(*p) <= 6.));
    }
    let mut from = here;
    for to in &plan.path {
        avoid.extend(clearance(from, *to));
        from = *to;
    }
    // Only nearby cells can support an in-reach torch. The bound also keeps IPC small.
    let avoid: Vec<_> = avoid
        .into_iter()
        .filter(|p| here.distance(*p) <= 6.)
        .take(128)
        .collect();
    Some(json!({"action":"torch", "forwardX":dx, "forwardZ":dz, "avoid":avoid}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stair_torch_uses_route_direction_and_excludes_future_digs_and_vein() {
        let (world, mut state) = crate::planner::tests::fixture();
        state.yaw = 90.; // Looking sideways at a previous torch must not reverse the new one.
        let here = state.feet();
        let next = here.offset(0, -1, 1);
        let target = next.offset(1, 0, 0);
        let plan = Plan {
            target,
            stand: next,
            path: vec![next],
            seconds: 1.,
            vein: 1,
            ore: true,
            features: [0.; 10],
            score: 0.,
            brain_features: None,
        };
        let focus = VeinFocus::new(&world, "", target);
        let cmd =
            command(&state, &plan, Some(&focus), Some(next.offset(0, 2, 0))).expect("heading");
        assert_eq!(cmd["forwardX"], 0);
        assert_eq!(cmd["forwardZ"], 1);
        let avoid: Vec<Pos> = serde_json::from_value(cmd["avoid"].clone()).expect("positions");
        for p in [
            next,
            next.offset(0, 1, 0),
            next.offset(0, 2, 0),
            target,
            here.offset(0, -1, 0),
        ] {
            assert!(avoid.contains(&p), "must not anchor a torch on {p:?}");
        }
    }
}
