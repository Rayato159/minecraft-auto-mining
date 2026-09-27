use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord,
)]
pub struct Pos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}
impl Pos {
    pub fn offset(self, x: i32, y: i32, z: i32) -> Self {
        Self {
            x: self.x + x,
            y: self.y + y,
            z: self.z + z,
        }
    }
    pub fn distance(self, other: Self) -> f64 {
        ((f64::from(self.x) - f64::from(other.x)).powi(2)
            + (f64::from(self.y) - f64::from(other.y)).powi(2)
            + (f64::from(self.z) - f64::from(other.z)).powi(2))
        .sqrt()
    }
}
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}
impl Point {
    pub fn cell(self) -> Pos {
        Pos {
            x: self.x.floor() as i32,
            y: self.y.floor() as i32,
            z: self.z.floor() as i32,
        }
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Block {
    pub water: bool,
    pub block: String,
    pub clear: bool,
    pub openable: bool,
    pub support: bool,
    pub diggable: bool,
    pub ore: bool,
    pub danger: bool,
    pub falling: bool,
    pub fluid: bool,
    pub seconds: f64,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Home {
    pub home: Option<Pos>,
    pub mine: Option<Pos>,
    pub radius: u32,
    pub search_radius: u32,
    pub revision: u64,
    pub request: u64,
    pub acknowledged: u64,
}
impl Home {
    pub fn protected(&self, p: Pos) -> bool {
        self.home.is_some_and(|h| {
            let dx = i64::from(p.x) - i64::from(h.x);
            let dz = i64::from(p.z) - i64::from(h.z);
            dx * dx + dz * dz <= i64::from(self.radius).pow(2)
        })
    }
    pub fn chest_in_area(&self, p: Pos) -> bool {
        self.home
            .is_some_and(|h| h.distance(p) <= f64::from(self.search_radius))
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LocalBlock {
    #[serde(flatten)]
    pub pos: Pos,
    #[serde(flatten)]
    pub kind: Block,
    pub mineable: bool,
    pub reason: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Route {
    pub name: String,
    pub level: i32,
    #[serde(flatten)]
    pub pos: Pos,
    pub safe_floor: bool,
    pub clear: bool,
    pub blocks: Vec<LocalBlock>,
    pub floor: Option<LocalBlock>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Navigation {
    pub feet: Option<Pos>,
    pub routes: Vec<Route>,
    pub ores: Vec<LocalBlock>,
    pub source_floor: Option<LocalBlock>,
    pub origin_safe: bool,
    pub centered: bool,
    pub anchor: Option<Pos>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Stack {
    pub crafting_reserve: bool,
    pub item: String,
    pub count: u32,
    pub pickaxe: bool,
    pub sword: bool,
    pub food: bool,
    pub torch: bool,
    pub durability: i32,
    pub harvests_target: Option<bool>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Enemy {
    pub ranged: bool,
    pub avoid_only: bool,
    pub id: i32,
    pub uuid: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(flatten)]
    pub position: Point,
    pub distance: f64,
    pub attack_distance: Option<f64>,
    pub health: f64,
    pub visible: bool,
    pub exploding: bool,
}
impl Enemy {
    pub fn reach_distance(&self) -> f64 {
        // A large modded mob can have its center far away while its hitbox is
        // already within melee range. Use the same geometry as the bridge.
        self.attack_distance
            .filter(|d| d.is_finite() && *d >= 0.)
            .unwrap_or(self.distance)
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Outcome {
    pub id: String,
    pub status: String,
    pub message: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Crafting {
    pub possible: bool,
    pub tier: String,
    pub reason: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Oxygen {
    pub known: bool,
    pub quality: String,
    pub breathable: bool,
    pub air: i32,
    pub max_air: i32,
    pub tank_air: f64,
    pub tank_capacity: f64,
    pub diving_gear: bool,
    pub submerged: bool,
    pub needs_escape: bool,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct State {
    pub mining_profile: MiningProfile,
    pub oxygen: Oxygen,
    pub on_fire: bool,
    pub fire_resistant: bool,
    pub capabilities: Vec<String>,
    pub liquid_safe: Option<bool>,
    pub crafting: Crafting,
    pub updated_at: u64,
    pub bridge_version: String,
    pub protocol: u32,
    pub enabled: bool,
    pub expected_session: bool,
    pub screen_open: bool,
    pub action: String,
    pub last_result: Outcome,
    pub mining_target: String,
    pub target_harvestable: Option<bool>,
    pub server: String,
    pub username: String,
    pub dimension: String,
    pub position: Point,
    pub yaw: f64,
    pub health: f64,
    pub food: u32,
    pub free_slots: u32,
    pub light_level: u32,
    pub on_ground: bool,
    pub in_water: bool,
    pub in_lava: bool,
    pub held_item: String,
    pub inventory: Vec<Stack>,
    pub navigation: Navigation,
    pub hostiles: Vec<Enemy>,
    pub home: Home,
    pub container_owned: bool,
}
impl State {
    /// Route-space cell supplied by the bridge; soul sand has a 7/8-block surface.
    /// Keep the physical position intact for distance/oxygen telemetry.
    pub fn feet(&self) -> Pos {
        let raw = self.position.cell();
        self.navigation
            .feet
            .filter(|p| {
                p.x == raw.x
                    && p.z == raw.z
                    && (p.y == raw.y
                        || self.on_ground
                            && p.y == raw.y + 1
                            && (f64::from(p.y) - self.position.y).abs() <= 0.15)
            })
            .unwrap_or(raw)
    }
    pub fn reserve_pickaxe(&self) -> bool {
        self.inventory.iter().any(|s| {
            s.count > 0
                && s.pickaxe
                && s.harvests_target != Some(false)
                && (s.durability < 0 || s.durability > 32)
        })
    }
    pub fn has(&self, kind: &str) -> bool {
        self.inventory.iter().any(|s| {
            s.count > 0
                && match kind {
                    "pickaxe" => s.pickaxe,
                    "sword" => s.sword,
                    "food" => s.food,
                    "torch" => s.torch,
                    _ => false,
                }
        })
    }
    pub fn holds(&self, kind: &str) -> bool {
        self.inventory.iter().any(|s| {
            s.item == self.held_item
                && match kind {
                    "pickaxe" => s.pickaxe,
                    "sword" => s.sword,
                    _ => false,
                }
        })
    }
    pub fn goal_key(&self) -> String {
        format!("{}|{}", self.session_key(), family(&self.mining_target))
    }
    pub fn session_key(&self) -> String {
        let base = format!("{}|{}|{}", self.server, self.username, self.dimension);
        if self.mining_profile.name == "dwarf" {
            format!("{base}|dwarf-v1")
        } else {
            base
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MiningProfile {
    pub name: String,
    pub can_swim: bool,
    pub requires_cover: bool,
    pub under_cover: bool,
}
impl Default for MiningProfile {
    fn default() -> Self {
        Self {
            name: "standard".into(),
            can_swim: true,
            requires_cover: false,
            under_cover: true,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scan {
    pub protocol: u32,
    pub id: String,
    pub updated_at: u64,
    pub server: String,
    pub username: String,
    pub dimension: String,
    pub origin: Pos,
    pub size: Pos,
    pub palette: Vec<Block>,
    pub cells: Vec<usize>,
    #[serde(default)]
    pub chests: Vec<Pos>,
}
pub fn family(id: &str) -> String {
    id.replace("minecraft:deepslate_", "minecraft:")
}

pub fn matches(block: &str, goal: &str) -> bool {
    goal.is_empty() || family(block) == family(goal)
}
pub fn points(block: &str, goal: &str) -> f64 {
    if !goal.is_empty() && matches(block, goal) {
        return 100.;
    }
    [
        ("ancient_debris", 40.),
        ("emerald", 25.),
        ("diamond", 20.),
        ("gold", 8.),
        ("lapis", 6.),
        ("redstone", 5.),
        ("iron", 4.),
        ("zinc", 4.),
        ("sulfur", 4.),
        ("copper", 2.),
        ("coal", 1.),
    ]
    .iter()
    .find_map(|(name, p)| block.contains(name).then_some(*p))
    .unwrap_or(4.)
}

#[cfg(test)]
mod standing_tests {
    use super::*;
    #[test]
    fn old_state_defaults_to_standard_and_dwarf_learning_is_separate() {
        let mut s: State = serde_json::from_str(r#"{"server":"world","username":"player","dimension":"minecraft:overworld","miningTarget":"minecraft:iron_ore"}"#).expect("old state");
        assert!(s.mining_profile.can_swim && s.mining_profile.under_cover);
        assert!(!s.mining_profile.requires_cover);
        assert_eq!(
            s.goal_key(),
            "world|player|minecraft:overworld|minecraft:iron_ore"
        );
        let old = s.goal_key();
        s.mining_profile.name = "dwarf".into();
        assert_ne!(old, s.goal_key());
        assert!(s.session_key().ends_with("|dwarf-v1"));
    }
    #[test]
    fn standing_cell_requires_nearby_grounded_bridge_geometry() {
        let mut s = State {
            position: Point {
                x: 67.5,
                y: 111.875,
                z: -5.45,
            },
            on_ground: true,
            ..State::default()
        };
        let top = Pos {
            x: 67,
            y: 112,
            z: -6,
        };
        assert_eq!(s.feet().y, 111, "old bridge remains physical");
        s.navigation.feet = Some(top);
        assert_eq!(s.feet(), top);
        s.on_ground = false;
        assert_eq!(s.feet().y, 111, "airborne positions are never rounded up");
        s.on_ground = true;
        s.position.y = 111.5;
        assert_eq!(s.feet().y, 111, "half slabs are not silently supported");
        s.position.y = 111.875;
        s.navigation.feet = Some(top.offset(1, 0, 0));
        assert_eq!(
            s.feet().x,
            67,
            "stale/foreign cell cannot redirect navigation"
        );
    }
}
