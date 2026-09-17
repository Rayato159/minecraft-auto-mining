//! Two complete cells around every occupied/dug cell, including diagonals and height.
use crate::types::{Block, Pos};

pub const LIQUID_RADIUS: i32 = 2;

pub fn liquid_clear<'a>(get: &impl Fn(Pos) -> Option<&'a Block>, p: Pos) -> bool {
    (-LIQUID_RADIUS..=LIQUID_RADIUS).all(|dy| {
        (-LIQUID_RADIUS..=LIQUID_RADIUS).all(|dz| {
            (-LIQUID_RADIUS..=LIQUID_RADIUS).all(|dx| {
                get(p.offset(dx, dy, dz)).is_some_and(|b| !b.fluid && b.block != "unknown")
            })
        })
    })
}

/// Separable box dilation: O(15 * cells), rather than 125 lookups for every
/// Dijkstra edge. Unknown cells and the boundary outside a scan are unsafe.
pub struct LiquidMask {
    origin: Pos,
    size: Pos,
    blocked: Vec<bool>,
}
impl LiquidMask {
    pub fn new(world: &crate::world::World) -> Self {
        let size = world.scan.size;
        let mut blocked: Vec<_> = (0..world.scan.cells.len())
            .map(|i| {
                world
                    .get(world.pos(i))
                    .is_none_or(|b| b.fluid || b.block == "unknown")
            })
            .collect();
        for axis in 0..3 {
            let mut next = vec![false; blocked.len()];
            for (i, out) in next.iter_mut().enumerate() {
                let p = world.pos(i);
                *out = (-LIQUID_RADIUS..=LIQUID_RADIUS).any(|d| {
                    let q = match axis {
                        0 => p.offset(d, 0, 0),
                        1 => p.offset(0, d, 0),
                        _ => p.offset(0, 0, d),
                    };
                    world.index(q).is_none_or(|j| blocked[j])
                });
            }
            blocked = next;
        }
        Self {
            origin: world.scan.origin,
            size,
            blocked,
        }
    }
    pub fn clear(&self, p: Pos) -> bool {
        let (x, y, z) = (
            p.x - self.origin.x,
            p.y - self.origin.y,
            p.z - self.origin.z,
        );
        x >= 0
            && y >= 0
            && z >= 0
            && x < self.size.x
            && y < self.size.y
            && z < self.size.z
            && !self.blocked[(x + self.size.x * (z + self.size.z * y)) as usize]
    }
    /// A cached answer only when the entire safety cube is in this scan.
    /// At its boundary, navigation must also consult remembered terrain.
    pub fn interior_clear(&self, p: Pos) -> Option<bool> {
        let local = p.offset(-self.origin.x, -self.origin.y, -self.origin.z);
        (local.x >= LIQUID_RADIUS
            && local.y >= LIQUID_RADIUS
            && local.z >= LIQUID_RADIUS
            && local.x < self.size.x - LIQUID_RADIUS
            && local.y < self.size.y - LIQUID_RADIUS
            && local.z < self.size.z - LIQUID_RADIUS)
            .then(|| self.clear(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_cube_buffer_matches_dense_mask_including_unknown_and_boundaries() {
        let (mut world, _) = crate::planner::tests::fixture();
        let center = Pos { x: 4, y: 2, z: 4 };
        for id in ["minecraft:water", "minecraft:lava", "unknown"] {
            for offset in [(2, 0, 0), (2, 2, 2), (-2, -2, -2), (0, 2, 0), (0, -2, 0)] {
                world.patches.clear();
                let p = center.offset(offset.0, offset.1, offset.2);
                world.patches.insert(
                    p,
                    Block {
                        block: id.into(),
                        fluid: id != "unknown",
                        ..Default::default()
                    },
                );
                assert!(!liquid_clear(&|p| world.get(p), center), "{id} at {p:?}");
                let mask = LiquidMask::new(&world);
                for i in 0..world.scan.cells.len() {
                    let p = world.pos(i);
                    assert_eq!(
                        mask.clear(p),
                        liquid_clear(&|p| world.get(p), p),
                        "{id} at {p:?}"
                    );
                    if let Some(safe) = mask.interior_clear(p) {
                        assert_eq!(safe, liquid_clear(&|p| world.get(p), p));
                    }
                }
            }
        }
        world.patches.clear();
        world.patches.insert(
            center.offset(3, 0, 0),
            Block {
                block: "minecraft:water".into(),
                fluid: true,
                ..Default::default()
            },
        );
        assert!(liquid_clear(&|p| world.get(p), center));
        let mask = LiquidMask::new(&world);
        assert_eq!(mask.interior_clear(center), Some(true));
        assert_eq!(mask.interior_clear(world.scan.origin), None);
        assert_eq!(
            mask.interior_clear(world.scan.origin.offset(-1, 0, 0)),
            None
        );
    }
}
