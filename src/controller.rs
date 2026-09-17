use crate::{
    bandit::{self, Book},
    bridge::{Bridge, Cancelled, atomic_json, now},
    navigation::{self, Atlas},
    planner::{self, Plan},
    types::{Pos, State, matches, points},
    world::World,
};
use anyhow::{Result, bail};
use serde::Serialize;
use serde_json::json;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::{self, OpenOptions},
    io::Write,
    thread,
    time::{Duration, Instant},
};

#[derive(Debug)]
struct Paused(String);
impl std::fmt::Display for Paused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Paused {}
fn pause(message: impl Into<String>) -> anyhow::Error {
    Paused(message.into()).into()
}

#[derive(Default, Serialize)]
struct Scoreboard {
    started_at: u64,
    updated_at: u64,
    ore_blocks: BTreeMap<String, u64>,
    points: f64,
    actions: u64,
    bandit_updates: u64,
    brain_updates: u64,
    mode: String,
    message: String,
    health: f64,
    food: u32,
    position: Pos,
    world: String,
}
struct Attempt {
    plan: Plan,
    key: String,
    start: Instant,
    points: f64,
    damage: f64,
    wear: f64,
    moves: u32,
}
impl Attempt {
    fn new(plan: Plan, key: String) -> Self {
        Self {
            plan,
            key,
            start: Instant::now(),
            points: 0.,
            damage: 0.,
            wear: 0.,
            moves: 0,
        }
    }
}
fn finish(
    bridge: &Bridge,
    book: &mut Book,
    attempt: &mut Option<Attempt>,
    failed: bool,
    board: &mut Scoreboard,
) -> Result<()> {
    let Some(a) = attempt.take() else {
        return Ok(());
    };
    let seconds = a.start.elapsed().as_secs_f64();
    let reward = bandit::reward(a.points, seconds, a.damage, a.wear, failed);
    let model = book.model(&a.key);
    model.update(&a.plan.features, reward);
    board.bandit_updates = model.updates;
    if let Some(x) = &a.plan.brain_features {
        let readout = book.fly_profiles.entry(a.key.clone()).or_default();
        readout.update(x, reward);
        board.brain_updates = readout.updates;
    }
    book.save(&bridge.runtime.join("bandit.json"))?;
    let event = json!({"at":now(),"profile":a.key,"target":a.plan.target,"ore":a.plan.ore,"vein":a.plan.vein,"estimatedSeconds":a.plan.seconds,"seconds":seconds,"points":a.points,"damage":a.damage,"toolUses":a.wear,"failed":failed,"reward":reward,"updates":board.bandit_updates,"brainUpdates":board.brain_updates,"brainFeatures":a.plan.brain_features});
    writeln!(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(bridge.runtime.join("attempts.jsonl"))?,
        "{event}"
    )?;
    println!(
        "Learn: reward={reward:.3}, ore points={:.0}, bandit={}, fly readout={}",
        a.points, board.bandit_updates, board.brain_updates
    );
    Ok(())
}
fn status(bridge: &Bridge, board: &mut Scoreboard, mode: &str, message: &str) -> Result<()> {
    if board.mode != mode || board.message != message {
        println!("{mode}: {message}");
    }
    board.mode = mode.into();
    board.message = message.into();
    board.updated_at = now();
    atomic_json(&bridge.runtime.join("scoreboard.json"), board)
}
fn action(
    bridge: &mut Bridge,
    board: &mut Scoreboard,
    command: serde_json::Value,
) -> Result<(bool, State)> {
    let before = bridge.state()?;
    let (outcome, state) = bridge.command(command.clone())?;
    board.actions += 1;
    let event = json!({"at":now(),"command":command,"before":before.position,"after":state.position,"result":outcome,
        "originSafe":state.navigation.origin_safe,"centered":state.navigation.centered,"freeSlots":state.free_slots});
    writeln!(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(bridge.runtime.join("actions.jsonl"))?,
        "{event}"
    )?;
    if outcome.status != "done"
        && state.free_slots == 0
        && command["action"] == "mine"
        && state.home.home.is_none()
    {
        return Err(pause(
            "Inventory full; empty it and run again. Learning saved.",
        ));
    }
    if outcome.status != "done" {
        println!("Action {}: {}", outcome.status, outcome.message);
    }
    Ok((outcome.status == "done", state))
}
fn retreat(bridge: &mut Bridge, board: &mut Scoreboard, state: &State) -> Result<bool> {
    let here = state.position.cell();
    let safe = state
        .navigation
        .routes
        .iter()
        .filter(|r| r.level == 0 && r.clear && r.safe_floor)
        .filter(|r| {
            state
                .hostiles
                .iter()
                .filter(|e| e.visible)
                .all(|e| r.pos.distance(e.position.cell()) > here.distance(e.position.cell()))
        })
        .max_by(|a, b| {
            let nearest = |p: Pos| {
                state
                    .hostiles
                    .iter()
                    .map(|e| p.distance(e.position.cell()))
                    .fold(30., f64::min)
            };
            nearest(a.pos).total_cmp(&nearest(b.pos))
        });
    let Some(route) = safe else {
        return Ok(false);
    };
    let dx = route.pos.x - here.x;
    let dz = route.pos.z - here.z;
    let yaw = (-f64::from(dx)).atan2(f64::from(dz)).to_degrees();
    action(bridge, board, json!({"action":"look","yaw":yaw,"pitch":0}))?;
    Ok(action(
        bridge,
        board,
        json!({"action":"step","direction":"forward","ticks":4}),
    )?
    .0)
}

#[derive(Clone, Copy, PartialEq)]
enum TripPhase {
    Home,
    Mine,
}
#[derive(Serialize, serde::Deserialize)]
struct MineResume {
    session: String,
    home_revision: u64,
    request: u64,
    mine: Pos,
}
struct Trip {
    phase: TripPhase,
    path: Vec<Pos>,
    destination: Option<Pos>,
    chest: Option<Pos>,
    tried_chests: HashSet<Pos>,
    denied: HashSet<Pos>,
    visits: HashMap<Pos, u32>,
    request: u64,
    failures: u32,
    empty_scans: u32,
}
impl Trip {
    fn resume(runtime: &std::path::Path, state: &State) -> Result<Option<Self>> {
        let path = runtime.join("resume-mine.json");
        if !path.exists() {
            return Ok(None);
        }
        let saved: MineResume = serde_json::from_slice(&fs::read(path)?)?;
        if saved.session != state.session_key()
            || saved.home_revision != state.home.revision
            || saved.request != state.home.request
            || saved.request != state.home.acknowledged
            || state.home.mine != Some(saved.mine)
            || state.home.home.is_none()
        {
            return Ok(None);
        }
        let mut trip = Self::new(state);
        trip.phase = TripPhase::Mine;
        Ok(Some(trip))
    }
    fn new(state: &State) -> Self {
        Self {
            phase: TripPhase::Home,
            path: Vec::new(),
            destination: None,
            chest: None,
            tried_chests: HashSet::new(),
            denied: HashSet::new(),
            visits: HashMap::new(),
            request: state.home.request,
            failures: 0,
            empty_scans: 0,
        }
    }
    // One checked action per iteration lets survival, menus, and new home commands preempt a trip.
    fn step(
        &mut self,
        bridge: &mut Bridge,
        board: &mut Scoreboard,
        state: &State,
        map: &mut World,
        atlas: &mut Atlas,
    ) -> Result<TripStep> {
        let here = state.position.cell();
        let returning = self.phase == TripPhase::Home;
        let home = state
            .home
            .home
            .ok_or_else(|| pause("Home was cleared; trip paused"))?;
        let goal = if returning {
            home
        } else {
            state.home.mine.ok_or_else(|| {
                pause("Deposited at home. Set /flyminer mine set before restarting")
            })?
        };
        if !returning && here == goal {
            return Ok(TripStep::Finished);
        }
        let can_dig = state.has("pickaxe") && state.health >= 18.;
        if returning
            && self.chest.is_none()
            && here.distance(home) <= f64::from(state.home.search_radius) + 4.
        {
            let mut chests: Vec<_> = map
                .scan
                .chests
                .iter()
                .copied()
                .filter(|p| state.home.chest_in_area(*p) && !self.tried_chests.contains(p))
                .collect();
            chests.sort_by(|a, b| a.distance(here).total_cmp(&b.distance(here)));
            for chest in chests {
                let goals: Vec<_> = [(1, 0), (-1, 0), (0, 1), (0, -1)]
                    .iter()
                    .flat_map(|(dx, dz)| [-1, 0].map(|dy| chest.offset(*dx, dy, *dz)))
                    .filter(|p| map.supported(*p))
                    .collect();
                if let Some(route) =
                    atlas.route(map, state, &goals, false, &self.denied, &self.visits)
                    && route.complete
                {
                    self.destination = route.path.last().copied().or(Some(here));
                    self.path = route.path;
                    self.chest = Some(chest);
                    break;
                }
            }
            if self.chest.is_none() && here.distance(home) <= 2. {
                self.empty_scans += 1;
                if self.empty_scans < 2 {
                    return Ok(TripStep::Refresh);
                }
                return Err(pause(
                    "At home: no remaining reachable chest with enough space/supplies. Check the chest and restart",
                ));
            }
        }
        while self.path.first() == Some(&here) {
            self.path.remove(0);
        }
        if returning
            && self.chest.is_some()
            && self.destination == Some(here)
            && self.path.is_empty()
        {
            let chest = self.chest.ok_or_else(|| pause("Chest selection lost"))?;
            status(
                bridge,
                board,
                "store",
                &format!(
                    "Depositing cargo and refilling at {},{},{}",
                    chest.x, chest.y, chest.z
                ),
            )?;
            let (mut ok, mut after) = action(
                bridge,
                board,
                json!({"action":"store","x":chest.x,"y":chest.y,"z":chest.z,"travel":true}),
            )?;
            if ok
                && !after.reserve_pickaxe()
                && after.crafting.possible
                && after.health >= 18.
                && !after.screen_open
            {
                status(
                    bridge,
                    board,
                    "craft",
                    &format!("Making {} from chest supplies", after.crafting.tier),
                )?;
                let crafted = action(
                    bridge,
                    board,
                    json!({"action":"craft_pickaxe","travel":true}),
                )?;
                if !crafted.0
                    && (crafted.1.screen_open
                        || crafted.1.health < 18.
                        || crafted
                            .1
                            .hostiles
                            .iter()
                            .any(|e| e.visible && e.reach_distance() < 4.5))
                {
                    return Ok(TripStep::Continue);
                }
                if !crafted.0 {
                    return Err(pause(format!(
                        "Crafting paused: {}",
                        crafted.1.last_result.message
                    )));
                }
                (ok, after) = action(
                    bridge,
                    board,
                    json!({"action":"store","x":chest.x,"y":chest.y,"z":chest.z,"travel":true}),
                )?;
            }
            if ok && !after.reserve_pickaxe() && after.crafting.possible && after.health < 18. {
                return Ok(TripStep::Continue);
            }
            if after.screen_open
                || after.home.revision != state.home.revision
                || after
                    .hostiles
                    .iter()
                    .any(|e| e.visible && e.reach_distance() < 4.5)
            {
                return Ok(TripStep::Continue);
            }
            self.tried_chests.insert(chest);
            self.chest = None;
            self.destination = None;
            if !ok {
                return Err(pause(format!(
                    "Chest transfer stopped: {}. Inventory left for inspection",
                    after.last_result.message
                )));
            }
            let stocked = after.has("pickaxe")
                && after.has("food")
                && after.target_harvestable != Some(false)
                && after.reserve_pickaxe();
            if navigation::cargo(&after) || after.free_slots == 0 || !stocked {
                status(
                    bridge,
                    board,
                    "store",
                    "Chest lacks space or supplies; checking the next nearest reachable chest",
                )?;
                return Ok(TripStep::Refresh);
            }
            let (acked, _) = action(
                bridge,
                board,
                json!({"action":"home_ack","request":self.request}),
            )?;
            if !acked {
                return Ok(TripStep::Refresh);
            }
            atlas.save(&bridge.runtime)?;
            if after.home.mine.is_none() {
                return Err(pause(
                    "Cargo deposited and supplies checked. Set /flyminer mine set, then run again",
                ));
            }
            self.phase = TripPhase::Mine;
            if let Some(mine) = after.home.mine {
                atomic_json(
                    &bridge.runtime.join("resume-mine.json"),
                    &MineResume {
                        session: after.session_key(),
                        home_revision: after.home.revision,
                        request: self.request,
                        mine,
                    },
                )?;
            }
            self.denied.clear();
            self.visits.clear();
            self.empty_scans = 0;
            status(
                bridge,
                board,
                "return_to_mine",
                "Storage finished; returning to your mine standing point",
            )?;
            return Ok(TripStep::Refresh);
        }
        if self.path.is_empty() {
            let began = Instant::now();
            let target = self.destination.unwrap_or(goal);
            let next = atlas.route(map, state, &[target], can_dig, &self.denied, &self.visits);
            let Some(route) = next else {
                self.empty_scans += 1;
                if self.empty_scans < 2 {
                    return Ok(TripStep::Refresh);
                }
                return Err(pause(if !state.has("pickaxe") {
                    "No safe checked passage to destination; no usable pickaxe, so digging is unavailable"
                } else if !can_dig {
                    "No safe checked passage to destination; digging is paused until health reaches 18"
                } else {
                    "No safe checked route after rescanning; a pickaxe is available. Check for a connected entrance, protected blocks, or the two-block liquid/unknown buffer"
                }));
            };
            self.empty_scans = 0;
            if route.path.is_empty() {
                return Ok(TripStep::Refresh);
            }
            let end = route
                .path
                .last()
                .copied()
                .ok_or_else(|| pause("Empty travel route"))?;
            *self.visits.entry(end).or_default() += 1;
            if self.visits[&end] > 3 {
                return Err(pause(
                    "Return route is not progressing; open a connected entrance and restart",
                ));
            }
            status(
                bridge,
                board,
                if returning { "home" } else { "return_to_mine" },
                &format!(
                    "{} route: {} steps, {:.1}s estimated; planned in {}ms",
                    route.end.label(),
                    route.path.len(),
                    route.seconds,
                    began.elapsed().as_millis()
                ),
            )?;
            self.path = route.path;
        }
        let Some(to) = self.path.first().copied() else {
            return Ok(TripStep::Refresh);
        };
        // Cached terrain guides the route; fresh local geometry always authorizes the next step.
        let valid =
            navigation::travel_cost(&|p| map.get(p), &state.home, here, to, can_dig).is_some();
        if !valid {
            self.denied.insert(to);
            self.path.clear();
            self.failures += 1;
            if self.failures >= 12 {
                return Err(pause(
                    "Route changed repeatedly; return paused on supported ground",
                ));
            }
            return Ok(TripStep::Refresh);
        }
        let obstacle = crate::world::clearance(here, to)
            .into_iter()
            .find(|p| map.get(*p).is_some_and(|b| !b.clear));
        let opening = obstacle.filter(|p| map.get(*p).is_some_and(|b| b.openable));
        let dig = obstacle.filter(|_| opening.is_none());
        let command = if let Some(p) = opening {
            json!({"action":"open_passage","x":p.x,"y":p.y,"z":p.z,"travel":true})
        } else if let Some(p) = dig {
            json!({"action":"mine","x":p.x,"y":p.y,"z":p.z,"travel":true,"returning":returning})
        } else {
            let batch = navigation::walking_batch(map, state, &self.path);
            if batch.len() > 1 {
                json!({"action":"route","path":batch,"travel":true})
            } else {
                json!({"action":"traverse","x":to.x,"y":to.y,"z":to.z,"travel":true})
            }
        };
        let (ok, after) = action(bridge, board, command)?;
        if ok {
            self.failures = 0;
            if let Some(p) = opening {
                map.patch(&after);
                atlas.ingest(map)?;
                if map.get(p).is_none_or(|b| !b.clear) {
                    return Ok(TripStep::Refresh);
                }
            } else if let Some(p) = dig {
                if let Some(b) = map.get(p).filter(|b| b.ore) {
                    let earned = points(&b.block, &state.mining_target);
                    *board.ore_blocks.entry(b.block.clone()).or_default() += 1;
                    board.points += earned;
                }
                map.clear(p);
                if let Some(b) = map.get(p) {
                    atlas.put(p, b)?;
                }
            } else {
                if let Some(end) = self.path.iter().position(|p| *p == after.position.cell()) {
                    self.path.drain(..=end);
                } else {
                    self.path.clear();
                    return Ok(TripStep::Refresh);
                }
            }
            if self.path.is_empty() {
                return Ok(TripStep::Refresh);
            }
        } else if after.home.revision == state.home.revision
            && !after.screen_open
            && !after
                .hostiles
                .iter()
                .any(|e| e.visible && e.reach_distance() < 4.5)
        {
            self.denied.insert(to);
            self.path.clear();
            self.failures += 1;
            if self.failures >= 12 {
                return Err(pause(
                    "Repeated movement failure on return route; inspect actions.jsonl",
                ));
            }
            return Ok(TripStep::Refresh);
        }
        Ok(TripStep::Continue)
    }
}
enum TripStep {
    Continue,
    Refresh,
    Finished,
}

pub fn run(bridge: &mut Bridge) -> Result<()> {
    let _lock = bridge.lock()?;
    if bridge.runtime.join("stop-request.json").exists() {
        fs::remove_file(bridge.runtime.join("stop-request.json"))?;
    }
    let mut state = bridge.state()?;
    if !state.enabled || !state.expected_session || state.screen_open {
        bail!("Open a world or join the server, /flyminer enable, then close the menu");
    }
    if state.action != "idle" {
        bail!("Bridge is busy; stop the other controller first");
    }
    let mut board = Scoreboard {
        started_at: now(),
        health: state.health,
        food: state.food,
        position: state.position.cell(),
        world: state.server.clone(),
        ..Scoreboard::default()
    };
    let mut book = Book::load(&bridge.runtime.join("bandit.json"))?;
    if bridge.brain_enabled && !cfg!(test) {
        book.enable_brain(&bridge.runtime)?;
    }
    println!(
        "Target: {}; learning profile has {} updates",
        if state.mining_target.is_empty() {
            "any ore"
        } else {
            &state.mining_target
        },
        book.model(&state.goal_key()).updates
    );
    let mut attempt: Option<Attempt> = None;
    let mut atlas = Atlas::load(&bridge.runtime, &state)?;
    let session = state.session_key();
    let result = (|| -> Result<()> {
        let mut world: Option<World> = None;
        let mut scan_center = state.position.cell();
        let mut scanned = Instant::now();
        let mut visited = HashMap::<Pos, u32>::new();
        let mut denied = HashMap::<Pos, Instant>::new();
        let mut last_cell = None;
        let mut goal_key = state.goal_key();
        let mut health = state.health;
        let mut torch_at: Option<Pos> = None;
        let mut torch_attempt = Instant::now() - Duration::from_secs(60);
        let mut equipped = false;
        let mut combat_since: Option<Instant> = None;
        let mut no_plan = 0;
        let mut recovery: Option<(f64, Instant)> = None;
        let mut trip = Trip::resume(&bridge.runtime, &state)?;
        if trip.is_some() {
            status(
                bridge,
                &mut board,
                "return_to_mine",
                "Resuming the stored return to your mine point",
            )?;
        }
        let mut home_revision = state.home.revision;
        let mut menu_paused = false;
        let mut craft_failed = false;
        let mut focus: Option<planner::VeinFocus> = None;
        loop {
            if bridge.stopped() {
                break;
            }
            bridge.beat()?;
            state = bridge.state()?;
            // A disconnected client omits health (deserialized as zero); that is not a death.
            if !state.expected_session || state.session_key() != session {
                return Err(
                    Cancelled("World/session ended; stopping without a death penalty").into(),
                );
            }
            board.health = state.health;
            board.food = state.food;
            board.position = state.position.cell();
            board.world = state.server.clone();
            if let Some(a) = &mut attempt {
                a.damage += (health - state.health).max(0.);
            }
            health = state.health;
            if state.health <= 0. {
                finish(bridge, &mut book, &mut attempt, true, &mut board)?;
                bail!("Player died; control stopped");
            }
            if !state.enabled {
                return Err(Cancelled("Game disabled control").into());
            }
            if state.screen_open {
                if !menu_paused {
                    if attempt.as_ref().is_some_and(|a| a.points > 0.) {
                        finish(bridge, &mut book, &mut attempt, false, &mut board)?;
                    } else {
                        attempt = None;
                    }
                }
                menu_paused = true;
                status(
                    bridge,
                    &mut board,
                    "menu",
                    "Inputs paused; close the menu/chat to continue",
                )?;
                thread::sleep(Duration::from_millis(100));
                continue;
            }
            if menu_paused {
                menu_paused = false;
                if let Some(map) = &world {
                    atlas.ingest(map)?;
                }
                world = None;
                if let Some(t) = &mut trip {
                    t.path.clear();
                }
            }
            if state.goal_key() != goal_key {
                focus = None;
                attempt = None;
                world = None;
                denied.clear();
                goal_key = state.goal_key();
                equipped = false;
                recovery = None;
            }
            if state.home.revision != home_revision {
                home_revision = state.home.revision;
                if let Some(map) = &world {
                    atlas.ingest(map)?;
                }
                world = None;
                denied.clear();
                if attempt.as_ref().is_some_and(|a| a.points > 0.) {
                    finish(bridge, &mut book, &mut attempt, false, &mut board)?;
                } else {
                    attempt = None;
                }
                if trip.is_some() {
                    trip = state.home.home.map(|_| Trip::new(&state));
                }
            }
            if state.home.home.is_some()
                && (trip.is_none() || trip.as_ref().is_some_and(|t| t.phase == TripPhase::Mine))
                && let Some(reason) = navigation::return_reason(&state)
                && !(state.crafting.possible
                    && state.free_slots >= 2
                    && !craft_failed
                    && state.has("food")
                    && state.home.request <= state.home.acknowledged)
            {
                if attempt.as_ref().is_some_and(|a| a.points > 0.) {
                    finish(bridge, &mut book, &mut attempt, false, &mut board)?;
                } else {
                    attempt = None;
                }
                if let Some(map) = &world {
                    atlas.ingest(map)?;
                }
                world = None;
                if state.home.request <= state.home.acknowledged {
                    let (ok, after) = action(bridge, &mut board, json!({"action":"home_request"}))?;
                    if !ok {
                        continue;
                    }
                    state = after;
                }
                trip = Some(Trip::new(&state));
                focus = None; // Supplies/manual home requests preempt mining commitments.
                status(bridge, &mut board, "home", reason)?;
            }
            let cell = state.position.cell();
            if last_cell != Some(cell) {
                *visited.entry(cell).or_default() += 1;
                last_cell = Some(cell);
            }
            if state.in_lava || state.in_water {
                finish(bridge, &mut book, &mut attempt, true, &mut board)?;
                bail!("Player entered liquid; stopped for manual recovery");
            }
            if state.liquid_safe == Some(false) {
                return Err(pause(
                    "Water/lava or unknown terrain within the two-block body buffer; move to safe dry ground and restart",
                ));
            }
            let threats: Vec<_> = state
                .hostiles
                .iter()
                .filter(|e| e.visible && e.reach_distance() < 4.5)
                .collect();
            if !threats.is_empty() {
                let since = combat_since.get_or_insert_with(Instant::now);
                status(
                    bridge,
                    &mut board,
                    "combat",
                    &format!("{} nearby hostile(s)", threats.len()),
                )?;
                if state.health <= 8. || threats.iter().any(|e| e.exploding && e.distance < 5.) {
                    if !retreat(bridge, &mut board, &state)? {
                        finish(bridge, &mut book, &mut attempt, true, &mut board)?;
                        bail!(
                            "No safe retreat from immediate danger (HP {:.1}, food {}/20)",
                            state.health,
                            state.food
                        );
                    }
                    continue;
                }
                if !state.has("sword") {
                    finish(bridge, &mut book, &mut attempt, true, &mut board)?;
                    bail!("No usable sword; refill and restart");
                }
                if since.elapsed() > Duration::from_secs(90) {
                    finish(bridge, &mut book, &mut attempt, true, &mut board)?;
                    bail!("Combat is not clearing; stopped for manual recovery");
                }
                if let Some(enemy) = threats
                    .iter()
                    .filter(|e| e.reach_distance() <= 3.)
                    .min_by(|a, b| a.reach_distance().total_cmp(&b.reach_distance()))
                {
                    action(
                        bridge,
                        &mut board,
                        json!({"action":"attack","entityId":enemy.id,"uuid":enemy.uuid}),
                    )?;
                    equipped = false;
                } else {
                    // One checked level step toward a visible enemy; never chase into unknown terrain.
                    let approach = threats
                        .iter()
                        .min_by(|a, b| a.reach_distance().total_cmp(&b.reach_distance()))
                        .and_then(|enemy| {
                            state
                                .navigation
                                .routes
                                .iter()
                                .filter(|r| r.level == 0 && r.clear && r.safe_floor)
                                .filter(|r| {
                                    r.pos.distance(enemy.position.cell())
                                        < cell.distance(enemy.position.cell())
                                })
                                .min_by(|a, b| {
                                    a.pos
                                        .distance(enemy.position.cell())
                                        .total_cmp(&b.pos.distance(enemy.position.cell()))
                                })
                                .map(|r| (*enemy, r.pos))
                        });
                    if let Some((enemy, p)) = approach {
                        action(
                            bridge,
                            &mut board,
                            json!({"action":"approach","entityId":enemy.id,"uuid":enemy.uuid,"x":p.x,"y":p.y,"z":p.z}),
                        )?;
                        equipped = false;
                        continue;
                    }
                    if state.food < 18 && state.has("food") {
                        action(bridge, &mut board, json!({"action":"eat"}))?;
                        equipped = false;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                continue;
            }
            if combat_since.take().is_some() {
                world = None;
                equipped = false;
            }
            // With no nearby visible threat, restore food and wait for healing even at low HP.
            // Mining/traversal still require HP >= 18 in both Rust and the Forge bridge.
            if state.food < 18 || (state.health < 18. && state.food < 20) {
                if !state.has("food") && trip.is_none() {
                    finish(bridge, &mut book, &mut attempt, true, &mut board)?;
                    bail!(
                        "No safe food (HP {:.1}, food {}/20); add food and restart",
                        state.health,
                        state.food
                    );
                }
                if state.has("food") {
                    status(
                        bridge,
                        &mut board,
                        "eat",
                        &format!(
                            "HP {:.1}, food {}/20; restoring hunger before mining",
                            state.health, state.food
                        ),
                    )?;
                    action(bridge, &mut board, json!({"action":"eat"}))?;
                    equipped = false;
                    continue;
                }
            }
            if state.health <= 8. && trip.is_some() && !state.has("food") {
                return Err(pause(
                    "Too little health to return safely without food; bring supplies to this position",
                ));
            }
            if state.health < 18. && (trip.is_none() || state.has("food") || state.food >= 18) {
                let (last_health, last_progress) =
                    recovery.get_or_insert((state.health, Instant::now()));
                if state.health > *last_health {
                    *last_progress = Instant::now();
                }
                *last_health = state.health;
                if last_progress.elapsed() >= Duration::from_secs(60) {
                    bail!(
                        "Health has not recovered for 60s (HP {:.1}, food {}/20); check regeneration, heal, then restart",
                        state.health,
                        state.food
                    );
                }
                status(
                    bridge,
                    &mut board,
                    "recover",
                    &format!(
                        "HP {:.1}, food {}/20; waiting to reach 18 HP before mining",
                        state.health, state.food
                    ),
                )?;
                thread::sleep(Duration::from_millis(150));
                continue;
            }
            recovery = None;
            if !state.on_ground {
                status(bridge, &mut board, "wait", "Waiting for stable ground")?;
                thread::sleep(Duration::from_millis(150));
                continue;
            }
            if state.reserve_pickaxe() {
                craft_failed = false;
            }
            if !state.reserve_pickaxe()
                && state.crafting.possible
                && !craft_failed
                && state.free_slots >= 2
                && trip.is_none()
            {
                if attempt.as_ref().is_some_and(|a| a.points > 0.) {
                    finish(bridge, &mut book, &mut attempt, false, &mut board)?;
                } else {
                    attempt = None;
                }
                status(
                    bridge,
                    &mut board,
                    "craft",
                    &format!("Preparing {} (diamond > iron > stone)", state.crafting.tier),
                )?;
                let (ok, after) = action(bridge, &mut board, json!({"action":"craft_pickaxe"}))?;
                if !ok
                    && !after.screen_open
                    && !after
                        .hostiles
                        .iter()
                        .any(|e| e.visible && e.reach_distance() < 4.5)
                {
                    craft_failed = true;
                    if after.home.home.is_none() {
                        return Err(pause(format!(
                            "Crafting paused: {}",
                            after.last_result.message
                        )));
                    }
                }
                equipped = false;
                world = None;
                continue;
            }
            if !state.has("pickaxe") && trip.is_none() {
                finish(bridge, &mut book, &mut attempt, true, &mut board)?;
                return Err(pause(format!(
                    "No usable pickaxe. {}",
                    state.crafting.reason
                )));
            }
            if state.free_slots == 0 && trip.is_none() {
                return Err(pause(
                    "Inventory full; empty it and run again. Learning saved.",
                ));
            }
            if state.target_harvestable == Some(false)
                && !state.mining_target.is_empty()
                && trip.is_none()
            {
                return Err(pause(format!(
                    "No usable pickaxe can harvest {}; bring a suitable tool and run again",
                    state.mining_target
                )));
            }
            if !state.navigation.origin_safe || !state.navigation.centered {
                if state.navigation.anchor.is_none() {
                    return Err(pause(
                        "No supported standing anchor nearby; move onto a solid block and run again",
                    ));
                }
                status(
                    bridge,
                    &mut board,
                    "center",
                    "Moving onto the center of a continuously supported floor",
                )?;
                let (ok, after) = action(
                    bridge,
                    &mut board,
                    json!({"action":"center","travel":trip.is_some()}),
                )?;
                if !ok {
                    if after.screen_open
                        || after.home.revision != state.home.revision
                        || after
                            .hostiles
                            .iter()
                            .any(|e| e.visible && e.reach_distance() < 4.5)
                    {
                        continue;
                    }
                    return Err(pause(
                        "Could not center safely; move onto solid ground and run again",
                    ));
                }
                if after.position.cell() != cell {
                    if attempt.as_ref().is_some_and(|a| a.points > 0.) {
                        finish(bridge, &mut book, &mut attempt, false, &mut board)?;
                    } else {
                        attempt = None;
                    }
                    world = None;
                }
                continue;
            }
            if trip.is_none()
                && state.light_level <= 6
                && torch_attempt.elapsed() > Duration::from_secs(8)
                && torch_at.is_none_or(|p| p.distance(cell) >= 4.)
            {
                torch_attempt = Instant::now();
                if state.has("torch") {
                    status(
                        bridge,
                        &mut board,
                        "torch",
                        "Dark area: placing a torch at a random checked spot",
                    )?;
                    let (ok, _) = action(bridge, &mut board, json!({"action":"torch"}))?;
                    if ok {
                        torch_at = Some(cell);
                    }
                    equipped = false;
                    continue;
                } else {
                    println!("No torches left; mining continues without placing lights.");
                }
            }
            if state.has("pickaxe") && (!equipped || !state.holds("pickaxe")) {
                let (_, after) = action(
                    bridge,
                    &mut board,
                    json!({"action":"equip","kind":"pickaxe"}),
                )?;
                if !after.holds("pickaxe") {
                    bail!("Cannot equip a usable pickaxe");
                }
                equipped = true;
                state = after;
            }
            let mut fresh_scan = false;
            if world.is_none()
                || scan_center.distance(cell) >= 8.
                || scanned.elapsed() > Duration::from_secs(30)
            {
                status(
                    bridge,
                    &mut board,
                    "scan",
                    "Refreshing nearby loaded terrain",
                )?;
                let (outcome, after) = action(
                    bridge,
                    &mut board,
                    json!({"action":"scan","travel":trip.is_some()}),
                )?;
                if !outcome {
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }
                let snapshot: crate::types::Scan =
                    serde_json::from_slice(&fs::read(bridge.dir.join("scan.json"))?)?;
                if snapshot.id != after.last_result.id
                    || now().saturating_sub(snapshot.updated_at) > 5000
                {
                    bail!("Scan result does not match the completed command");
                }
                if let Some(old) = &world {
                    atlas.ingest(old)?;
                }
                world = Some(World::new(snapshot, &after)?);
                if let Some(map) = &world {
                    atlas.ingest(map)?;
                }
                if state.home.home.is_some() {
                    atlas.save(&bridge.runtime)?;
                }
                scan_center = after.position.cell();
                scanned = Instant::now();
                state = after;
                fresh_scan = true;
            }
            let Some(map) = world.as_mut() else {
                continue;
            };
            map.patch(&state);
            if let Some(t) = &mut trip {
                match t.step(bridge, &mut board, &state, map, &mut atlas)? {
                    TripStep::Continue => {}
                    TripStep::Refresh => {
                        atlas.ingest(map)?;
                        world = None;
                    }
                    TripStep::Finished => {
                        let resume = bridge.runtime.join("resume-mine.json");
                        if resume.exists() {
                            fs::remove_file(resume)?;
                        }
                        trip = None;
                        world = None;
                        equipped = false;
                        denied.clear();
                        no_plan = 0;
                        status(
                            bridge,
                            &mut board,
                            "mine",
                            "Reached configured mine point; continuous mining resumed",
                        )?;
                    }
                }
                continue;
            }
            denied.retain(|_, until| *until > Instant::now());
            let blocked: HashSet<_> = denied.keys().copied().collect();
            if let Some(locked) = &mut focus {
                locked.refresh(map, &state.mining_target);
                if locked.remaining(map, &state.mining_target) == 0 {
                    focus = None;
                    finish(bridge, &mut book, &mut attempt, false, &mut board)?;
                    status(
                        bridge,
                        &mut board,
                        "focus",
                        "Target vein exhausted; selecting the next vein",
                    )?;
                }
            }
            // New scans or newly exposed ore preempt a descent segment immediately.
            if attempt.as_ref().is_some_and(|a| !a.plan.ore)
                && (state
                    .navigation
                    .ores
                    .iter()
                    .any(|b| b.mineable && matches(&b.kind.block, &state.mining_target))
                    || fresh_scan
                        && (0..map.scan.cells.len()).any(|i| {
                            map.get(map.pos(i))
                                .is_some_and(|b| b.ore && matches(&b.block, &state.mining_target))
                        }))
            {
                finish(bridge, &mut book, &mut attempt, false, &mut board)?;
            }
            if attempt
                .as_ref()
                .is_some_and(|a| a.moves >= 12 || a.start.elapsed() > Duration::from_secs(90))
            {
                finish(bridge, &mut book, &mut attempt, false, &mut board)?;
            }
            if attempt.is_none() {
                let began = Instant::now();
                let candidates = planner::candidates_for(
                    map,
                    &state,
                    book.model(&goal_key),
                    &blocked,
                    &visited,
                    focus.as_ref(),
                );
                let next = book.choose(candidates, &state);
                let Some(plan) = next else {
                    no_plan += 1;
                    if no_plan >= 3 {
                        return Err(pause(if focus.is_some() {
                            "Locked target vein still has ore, but no safe reachable member remains; check liquids, protected terrain or blocked access"
                        } else {
                            "No safe target vein or descending/detour route with two-block liquid clearance; relocate and run again"
                        }));
                    }
                    world = None;
                    thread::sleep(Duration::from_millis(300));
                    continue;
                };
                no_plan = 0;
                if plan.ore && focus.is_none() {
                    let locked = planner::VeinFocus::new(map, &state.mining_target, plan.target);
                    status(
                        bridge,
                        &mut board,
                        "focus",
                        &format!(
                            "Locked target vein: {} remaining blocks",
                            locked.remaining(map, &state.mining_target)
                        ),
                    )?;
                    focus = Some(locked);
                }
                status(
                    bridge,
                    &mut board,
                    "plan",
                    &format!(
                        "{} at {},{},{}; {:.1}s path; {} blocks in vein; planning {}ms",
                        if plan.ore {
                            "ore"
                        } else if plan.stand.y < cell.y {
                            "descend"
                        } else {
                            "detour"
                        },
                        plan.target.x,
                        plan.target.y,
                        plan.target.z,
                        plan.seconds,
                        plan.vein,
                        began.elapsed().as_millis()
                    ),
                )?;
                attempt = Some(Attempt::new(plan, goal_key.clone()));
            }
            let Some(a) = attempt.as_mut() else {
                continue;
            };
            // Opportunistic mining must stay inside the committed vein.
            let exposed = state
                .navigation
                .ores
                .iter()
                .filter(|b| {
                    b.mineable
                        && matches(&b.kind.block, &state.mining_target)
                        && !blocked.contains(&b.pos)
                        && focus.as_ref().is_some_and(|f| f.contains(b.pos))
                        && map.can_clear(b.pos)
                })
                .min_by(|a, b| a.pos.distance(cell).total_cmp(&b.pos.distance(cell)));
            let dig = if let Some(ore) = exposed {
                Some((ore.pos, ore.kind.block.clone(), true))
            } else {
                while a.plan.path.first() == Some(&cell) {
                    a.plan.path.remove(0);
                }
                if cell == a.plan.stand && a.plan.ore {
                    let p = a.plan.target;
                    map.get(p)
                        .filter(|b| b.ore)
                        .map(|b| (p, b.block.clone(), true))
                } else if let Some(to) = a.plan.path.first().copied() {
                    if (to.x - cell.x).abs() + (to.z - cell.z).abs() != 1
                        || (to.y - cell.y).abs() > 1
                        || map.cost(cell, to).is_none()
                    {
                        finish(bridge, &mut book, &mut attempt, true, &mut board)?;
                        world = None;
                        continue;
                    }
                    let route = state.navigation.routes.iter().find(|r| r.pos == to);
                    if let Some(route) = route {
                        if !route.safe_floor {
                            denied.insert(to, Instant::now() + Duration::from_secs(30));
                            finish(bridge, &mut book, &mut attempt, true, &mut board)?;
                            continue;
                        }
                        let remaining: Vec<_> =
                            route.blocks.iter().filter(|b| !b.kind.clear).collect();
                        if remaining.is_empty() {
                            let (ok, after) = action(
                                bridge,
                                &mut board,
                                json!({"action":"traverse","x":to.x,"y":to.y,"z":to.z}),
                            )?;
                            if ok {
                                if let Some(a) = &mut attempt {
                                    a.moves += 1;
                                }
                            } else if after.screen_open
                                || !after.enabled
                                || after.goal_key() != state.goal_key()
                                || after.home.revision != state.home.revision
                            {
                                if attempt.as_ref().is_some_and(|a| a.points > 0.) {
                                    finish(bridge, &mut book, &mut attempt, false, &mut board)?;
                                } else {
                                    attempt = None;
                                }
                                world = None;
                            } else {
                                denied.insert(to, Instant::now() + Duration::from_secs(10));
                                finish(bridge, &mut book, &mut attempt, true, &mut board)?;
                                world = None;
                            }
                            continue;
                        }
                        if let Some(b) = remaining
                            .iter()
                            .find(|b| b.mineable && map.can_clear(b.pos))
                        {
                            Some((b.pos, b.kind.block.clone(), b.kind.ore))
                        } else {
                            denied.insert(to, Instant::now() + Duration::from_secs(30));
                            finish(bridge, &mut book, &mut attempt, true, &mut board)?;
                            world = None;
                            continue;
                        }
                    } else {
                        finish(bridge, &mut book, &mut attempt, true, &mut board)?;
                        world = None;
                        continue;
                    }
                } else {
                    finish(bridge, &mut book, &mut attempt, false, &mut board)?;
                    continue;
                }
            };
            if let Some((pos, block, ore)) = dig {
                let (ok, after) = action(
                    bridge,
                    &mut board,
                    json!({"action":"mine","x":pos.x,"y":pos.y,"z":pos.z}),
                )?;
                if let Some(a) = &mut attempt {
                    a.damage += (health - after.health).max(0.);
                    if ok {
                        a.wear += 1.;
                    }
                }
                health = after.health;
                if ok {
                    map.clear(pos);
                    if let Some(b) = map.get(pos) {
                        atlas.put(pos, b)?;
                    }
                    if ore {
                        let earned = points(&block, &state.mining_target);
                        *board.ore_blocks.entry(block.clone()).or_default() += 1;
                        board.points += earned;
                        if let Some(a) = &mut attempt {
                            a.points += earned;
                        }
                        println!("Mined {block}: +{earned:.0}; total {:.0}", board.points);
                    }
                    let completed = attempt.as_ref().is_some_and(|a| a.plan.target == pos);
                    if completed {
                        finish(bridge, &mut book, &mut attempt, false, &mut board)?;
                    }
                } else {
                    if after.home.home.is_some() && navigation::return_reason(&after).is_some()
                        || after.screen_open
                        || !after.enabled
                        || after.goal_key() != state.goal_key()
                        || after.home.revision != state.home.revision
                    {
                        if attempt.as_ref().is_some_and(|a| a.points > 0.) {
                            finish(bridge, &mut book, &mut attempt, false, &mut board)?;
                        } else {
                            attempt = None;
                        }
                        world = None;
                        continue;
                    }
                    denied.insert(pos, Instant::now() + Duration::from_secs(30));
                    finish(bridge, &mut book, &mut attempt, true, &mut board)?;
                    world = None;
                }
                board.updated_at = now();
                atomic_json(&bridge.runtime.join("scoreboard.json"), &board)?;
            } else {
                finish(bridge, &mut book, &mut attempt, false, &mut board)?;
            }
        }
        Ok(())
    })();
    // Every exit releases game inputs, including failures and Ctrl+C.
    let stop = bridge.stop();
    if state.home.home.is_some() {
        atlas.save(&bridge.runtime)?;
    }
    // User cancellation/menu changes are censored attempts, not negative training labels.
    if attempt.as_ref().is_some_and(|a| a.points > 0.) {
        finish(bridge, &mut book, &mut attempt, false, &mut board)?;
    }
    let message = match &result {
        Ok(()) => "Stopped".to_string(),
        Err(e) => e.to_string(),
    };
    let paused = result.as_ref().is_err_and(|e| e.is::<Paused>());
    let cancelled = result.as_ref().is_err_and(|e| e.is::<Cancelled>());
    status(
        bridge,
        &mut board,
        if paused { "paused" } else { "stopped" },
        &message,
    )?;
    if let Err(e) = stop {
        eprintln!("Stop acknowledgement: {e}");
    }
    if paused || cancelled { Ok(()) } else { result }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Enemy, LocalBlock, Outcome, Stack};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    #[test]
    fn mine_resume_is_persistent_and_rejects_changed_world_or_settings() {
        let root = std::env::temp_dir().join(format!("miner-resume-{}", now()));
        fs::create_dir_all(&root).expect("directory");
        let mut s = State {
            server: "test-world".into(),
            ..Default::default()
        };
        s.home.home = Some(Pos { x: 1, y: 2, z: 3 });
        s.home.mine = Some(Pos { x: 10, y: 2, z: 3 });
        s.home.revision = 10;
        s.home.request = 20;
        s.home.acknowledged = 20;
        atomic_json(
            &root.join("resume-mine.json"),
            &MineResume {
                session: s.session_key(),
                home_revision: 10,
                request: 20,
                mine: s.home.mine.expect("mine"),
            },
        )
        .expect("save");
        assert!(
            Trip::resume(&root, &s)
                .expect("restore")
                .is_some_and(|t| t.phase == TripPhase::Mine)
        );
        s.home.revision += 1;
        assert!(Trip::resume(&root, &s).expect("changed settings").is_none());
        s.home.revision = 10;
        s.server = "other-world".into();
        assert!(Trip::resume(&root, &s).expect("changed world").is_none());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Scenario {
        Normal,
        Edge,
        FullAfterOre,
        Craft,
        Unknown,
        Focus,
    }

    #[test]
    fn continuous_ipc_survives_combat_food_and_darkness_without_waiting_for_drops() {
        exercise_loop(20., 20, Scenario::Normal);
    }
    #[test]
    fn vein_lock_survives_reward_updates_and_combat_without_grabbing_other_exposed_ore() {
        exercise_loop(20., 20, Scenario::Focus);
    }

    #[test]
    fn critically_low_health_eats_and_recovers_before_mining() {
        exercise_loop(6., 10, Scenario::Normal);
    }

    #[test]
    fn standing_over_air_recenters_before_scan_and_resumes_mining() {
        exercise_loop(20., 20, Scenario::Edge);
    }

    #[test]
    fn inventory_full_pauses_cleanly_and_does_not_penalize_a_productive_route() {
        exercise_loop(20., 20, Scenario::FullAfterOre);
    }
    #[test]
    fn missing_pickaxe_crafts_before_mining_and_then_resumes_survival() {
        exercise_loop(20., 20, Scenario::Craft);
    }
    #[test]
    fn unrecognized_ore_and_large_modded_monster_use_generic_interfaces() {
        exercise_loop(20., 20, Scenario::Unknown);
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    enum HomeCase {
        Full,
        Manual,
        NoPickaxe,
        NoFood,
        EmptyChest,
    }
    #[test]
    fn full_inventory_deposits_keeps_supplies_returns_to_mine_and_resumes() {
        exercise_home(HomeCase::Full);
    }
    #[test]
    fn in_game_home_command_interrupts_mining_without_ending_the_controller() {
        exercise_home(HomeCase::Manual);
    }
    #[test]
    fn exhausted_pickaxe_uses_open_passage_and_restocks_before_mining() {
        exercise_home(HomeCase::NoPickaxe);
    }
    #[test]
    fn exhausted_food_returns_home_and_restocks() {
        exercise_home(HomeCase::NoFood);
    }
    #[test]
    fn insufficient_chest_supplies_pause_at_home_without_mining() {
        exercise_home(HomeCase::EmptyChest);
    }

    fn exercise_home(case: HomeCase) {
        use crate::types::{Point, Route};
        let root = std::env::temp_dir().join(format!("miner-home-{}-{case:?}", now()));
        let directory = root.join("config/flyminer");
        fs::create_dir_all(&directory).expect("dir");
        let (mut world, mut state) = crate::planner::tests::fixture();
        let home = Pos { x: 6, y: 1, z: 3 };
        let mine = state.position.cell();
        let chest = Pos { x: 6, y: 1, z: 4 };
        state.home.home = Some(home);
        state.home.mine = Some(mine);
        state.home.radius = 1;
        state.home.search_radius = 4;
        state.enabled = true;
        state.expected_session = true;
        state.action = "idle".into();
        state.on_ground = true;
        state.health = 20.;
        state.food = 20;
        state.light_level = 15;
        state.free_slots = if case == HomeCase::Full { 0 } else { 1 };
        state.navigation.origin_safe = true;
        state.navigation.centered = true;
        let supplies = vec![
            Stack {
                item: "pick".into(),
                count: 1,
                pickaxe: true,
                durability: 200,
                ..Default::default()
            },
            Stack {
                item: "food".into(),
                count: 16,
                food: true,
                ..Default::default()
            },
            Stack {
                item: "sword".into(),
                count: 1,
                sword: true,
                durability: 200,
                ..Default::default()
            },
            Stack {
                item: "torch".into(),
                count: 16,
                torch: true,
                ..Default::default()
            },
        ];
        state.inventory = supplies.clone();
        if matches!(case, HomeCase::NoPickaxe | HomeCase::EmptyChest) {
            state.inventory.retain(|s| !s.pickaxe);
        }
        if case == HomeCase::NoFood {
            state.inventory.retain(|s| !s.food);
            state.food = 15;
            state.health = 15.;
        }
        state.inventory.push(Stack {
            item: "minecraft:cobblestone".into(),
            count: 64,
            ..Default::default()
        });
        for x in 3..=6 {
            for y in 1..=2 {
                let p = Pos { x, y, z: 3 };
                let i = world.index(p).expect("cell");
                world.scan.cells[i] = 1;
            }
        }
        for p in [Pos { x: 3, y: 1, z: 5 }, Pos { x: 4, y: 1, z: 5 }] {
            let i = world.index(p).expect("ore");
            world.scan.cells[i] = 2;
            state.navigation.ores.push(LocalBlock {
                pos: p,
                kind: world.scan.palette[2].clone(),
                mineable: true,
                ..Default::default()
            });
        }
        world.scan.chests = vec![chest];
        state.updated_at = now();
        atomic_json(&directory.join("state.json"), &state).expect("state");
        let shutdown = Arc::new(AtomicBool::new(false));
        let finished = shutdown.clone();
        let worker = thread::spawn(move || {
            let mut calls = Vec::new();
            let mut stored = false;
            let mut resumed = false;
            let mut chat_ticks = 0;
            let mut store_ticks = 0;
            let mut store_id = String::new();
            let mut triggered = false;
            while !finished.load(Ordering::Relaxed) {
                if let Ok(bytes) = fs::read(directory.join("command.json")) {
                    let command: serde_json::Value =
                        serde_json::from_slice(&bytes).expect("command");
                    fs::remove_file(directory.join("command.json")).expect("consume");
                    let name = command["action"].as_str().expect("action");
                    calls.push(name.to_string());
                    let id = command["id"].as_str().expect("id").to_string();
                    let mut status = "done";
                    match name {
                        "home_request" => state.home.request = 1,
                        "home_ack" => state.home.acknowledged = state.home.request,
                        "scan" => {
                            world.scan.id = id.clone();
                            world.scan.updated_at = now();
                            atomic_json(&directory.join("scan.json"), &world.scan).expect("scan");
                        }
                        "equip" => state.held_item = "pick".into(),
                        "eat" => {
                            state.food = 20;
                            state.health = 20.;
                        }
                        "traverse" | "route" => {
                            let path: Vec<Pos> = if name == "route" {
                                serde_json::from_value(command["path"].clone()).expect("route")
                            } else {
                                vec![serde_json::from_value(command.clone()).expect("destination")]
                            };
                            for to in path {
                                assert!(
                                    navigation::travel_cost(
                                        &|p| world.get(p),
                                        &state.home,
                                        state.position.cell(),
                                        to,
                                        false
                                    )
                                    .is_some(),
                                    "walk only in this fixture"
                                );
                                if state.health < 18. {
                                    assert_eq!(command["travel"], true);
                                }
                                state.position = Point {
                                    x: f64::from(to.x) + 0.5,
                                    y: f64::from(to.y),
                                    z: f64::from(to.z) + 0.5,
                                };
                            }
                        }
                        "store" => {
                            assert!(state.position.cell().distance(chest) <= 1.5);
                            state.screen_open = true;
                            state.container_owned = true;
                            state.action = "store".into();
                            store_ticks = 12;
                            store_id = id.clone();
                            status = "running";
                        }
                        "mine" => {
                            if !stored {
                                assert_eq!(
                                    case,
                                    HomeCase::Manual,
                                    "must store/refill before mining"
                                );
                            } else {
                                assert_eq!(
                                    state.position.cell(),
                                    mine,
                                    "must return to configured point first"
                                );
                                resumed = true;
                            }
                            let p = Pos {
                                x: command["x"].as_i64().expect("x") as i32,
                                y: command["y"].as_i64().expect("y") as i32,
                                z: command["z"].as_i64().expect("z") as i32,
                            };
                            assert!(!state.home.protected(p));
                            state.navigation.ores.retain(|b| b.pos != p);
                            let i = world.index(p).expect("mined cell");
                            world.scan.cells[i] = 1;
                            if case == HomeCase::Manual && !triggered {
                                triggered = true;
                                state.home.request = 1;
                                state.home.revision = 1;
                                state.screen_open = true;
                                chat_ticks = 20;
                            }
                        }
                        "stop" => {
                            state.screen_open = false;
                            state.container_owned = false;
                            state.action = "idle".into();
                        }
                        _ => {}
                    }
                    state.last_result = Outcome {
                        id,
                        status: status.into(),
                        message: String::new(),
                    };
                }
                if chat_ticks > 0 {
                    chat_ticks -= 1;
                    if chat_ticks == 0 {
                        state.screen_open = false;
                    }
                }
                if store_ticks > 0 {
                    store_ticks -= 1;
                    if store_ticks == 0 {
                        stored = true;
                        state.free_slots = 1;
                        state.inventory = supplies.clone();
                        state.inventory.push(Stack {
                            item: "minecraft:cobblestone".into(),
                            count: 64,
                            crafting_reserve: true,
                            ..Default::default()
                        });
                        if case == HomeCase::EmptyChest {
                            state.inventory.retain(|s| !s.pickaxe);
                        }
                        state.screen_open = false;
                        state.container_owned = false;
                        state.action = "idle".into();
                        state.last_result = Outcome {
                            id: store_id.clone(),
                            status: "done".into(),
                            message: "Mock server confirmed transfer".into(),
                        };
                    }
                }
                if resumed {
                    state.enabled = false;
                }
                state.updated_at = now();
                atomic_json(&directory.join("state.json"), &state).expect("publish");
                // Navigation patches from the current position must agree with the scanned corridor.
                state.navigation.routes.clear();
                for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    for dy in -1..=1 {
                        let to = state.position.cell().offset(dx, dy, dz);
                        let blocks: Vec<_> = crate::world::clearance(state.position.cell(), to)
                            .into_iter()
                            .filter_map(|p| {
                                world.get(p).map(|b| LocalBlock {
                                    pos: p,
                                    kind: b.clone(),
                                    mineable: !state.home.protected(p),
                                    ..Default::default()
                                })
                            })
                            .collect();
                        state.navigation.routes.push(Route {
                            pos: to,
                            level: dy,
                            safe_floor: world.supported(to),
                            clear: blocks.iter().all(|b| b.kind.clear),
                            blocks,
                            ..Default::default()
                        });
                    }
                }
                thread::sleep(Duration::from_millis(5));
            }
            (calls, stored, resumed, state.position.cell())
        });
        let mut bridge = Bridge::new(root.clone(), root.join("runtime")).expect("bridge");
        bridge.remaining = Some(60);
        bridge.deadline = Some(Instant::now() + Duration::from_secs(12));
        let result = run(&mut bridge);
        shutdown.store(true, Ordering::Relaxed);
        let (calls, stored, resumed, last) = worker.join().expect("worker");
        assert!(stored, "store not reached: {case:?} {calls:?} {result:?}");
        if case == HomeCase::EmptyChest {
            assert!(result.is_ok(), "resource pause: {result:?}");
            assert!(!resumed);
            assert!(last.distance(home) <= 2.);
            assert!(!calls.iter().any(|c| c == "mine"));
        } else {
            assert!(resumed, "must mine again: {case:?} {calls:?} {result:?}");
            assert!(calls.iter().any(|c| c == "home_ack"));
        }
        assert_eq!(calls.last().map(String::as_str), Some("stop"));
        fs::remove_dir_all(root).expect("cleanup");
    }

    fn exercise_loop(initial_health: f64, initial_food: u32, scenario: Scenario) {
        let root = std::env::temp_dir().join(format!(
            "miner-loop-{}-{}-{scenario:?}",
            now(),
            initial_health
        ));
        let directory = root.join("config/flyminer");
        fs::create_dir_all(&directory).expect("mkdir");
        let (mut world, mut state) = crate::planner::tests::fixture();
        state.enabled = true;
        state.expected_session = true;
        state.on_ground = true;
        state.navigation.origin_safe = true;
        state.navigation.centered = true;
        state.free_slots = 1;
        state.light_level = 15;
        state.action = "idle".into();
        state.inventory = vec![
            Stack {
                item: "pick".into(),
                count: 1,
                pickaxe: true,
                durability: 200,
                ..Stack::default()
            },
            Stack {
                item: "sword".into(),
                count: 1,
                sword: true,
                durability: 200,
                ..Stack::default()
            },
            Stack {
                item: "food".into(),
                count: 8,
                food: true,
                ..Stack::default()
            },
            Stack {
                item: "torch".into(),
                count: 8,
                torch: true,
                ..Stack::default()
            },
        ];
        state.held_item = "pick".into();
        if scenario == Scenario::Craft {
            state.inventory.retain(|s| !s.pickaxe);
            state.free_slots = 3;
            state.crafting.possible = true;
            state.crafting.tier = "minecraft:diamond_pickaxe".into();
        }
        if scenario == Scenario::Unknown {
            world.scan.palette[2].block = "unseen_mod:glowing_crystal_ore".into();
        }
        state.updated_at = now();
        state.health = initial_health;
        state.food = initial_food;
        if scenario == Scenario::Edge {
            state.position.x = 3.8;
            state.navigation.origin_safe = false;
            state.navigation.centered = false;
            state.navigation.anchor = Some(Pos { x: 4, y: 1, z: 3 });
            let floor = Pos { x: 3, y: 0, z: 3 };
            let index = world.index(floor).expect("floor");
            world.scan.cells[index] = 1;
            state.navigation.source_floor = Some(LocalBlock {
                pos: floor,
                kind: world.scan.palette[1].clone(),
                ..Default::default()
            });
            for y in [1, 2] {
                let index = world
                    .index(Pos { x: 4, y, z: 3 })
                    .expect("anchor clearance");
                world.scan.cells[index] = 1;
            }
        }
        for pos in [Pos { x: 5, y: 1, z: 3 }, Pos { x: 3, y: 1, z: 5 }] {
            let i = world.index(pos).expect("index");
            world.scan.cells[i] = 2;
            state.navigation.ores.push(LocalBlock {
                pos,
                kind: world.scan.palette[2].clone(),
                mineable: true,
                ..LocalBlock::default()
            });
        }
        if scenario == Scenario::Focus {
            let member = Pos { x: 6, y: 1, z: 3 };
            let i = world.index(member).expect("vein member");
            world.scan.cells[i] = 2;
            state.navigation.ores.push(LocalBlock {
                pos: member,
                kind: world.scan.palette[2].clone(),
                mineable: true,
                ..Default::default()
            });
            let other = Pos { x: 3, y: 1, z: 5 };
            let i = world.index(other).expect("other vein");
            world.scan.palette.push(crate::types::Block {
                seconds: 12.,
                ..world.scan.palette[2].clone()
            });
            world.scan.cells[i] = 3;
            state
                .navigation
                .ores
                .iter_mut()
                .find(|b| b.pos == other)
                .expect("other ore")
                .kind = world.scan.palette[3].clone();
        }
        atomic_json(&directory.join("state.json"), &state).expect("state");
        let shutdown = Arc::new(AtomicBool::new(false));
        let finished = shutdown.clone();
        let worker = thread::spawn(move || {
            let mut calls = Vec::<String>::new();
            let mut mined = 0;
            while !finished.load(Ordering::Relaxed) {
                if let Ok(bytes) = fs::read(directory.join("command.json")) {
                    let command: serde_json::Value =
                        serde_json::from_slice(&bytes).expect("command");
                    fs::remove_file(directory.join("command.json")).expect("consume");
                    let name = command["action"].as_str().expect("action");
                    calls.push(name.to_string());
                    if matches!(name, "mine" | "traverse" | "scan") {
                        assert!(
                            state.health >= 18.,
                            "Cannot {name} before HP recovers: {}",
                            state.health
                        );
                        assert!(
                            state.navigation.origin_safe && state.navigation.centered,
                            "Must recenter before {name}"
                        );
                    }
                    let id = command["id"].as_str().expect("id").to_string();
                    match name {
                        "center" => {
                            state.position.x = 4.5;
                            state.position.z = 3.5;
                            state.navigation.origin_safe = true;
                            state.navigation.centered = true;
                            state.navigation.source_floor = Some(LocalBlock {
                                pos: Pos { x: 4, y: 0, z: 3 },
                                kind: world.scan.palette[0].clone(),
                                ..Default::default()
                            });
                        }
                        "scan" => {
                            world.scan.id = id.clone();
                            world.scan.updated_at = now();
                            atomic_json(&directory.join("scan.json"), &world.scan).expect("scan");
                        }
                        "equip" => state.held_item = "pick".into(),
                        "craft_pickaxe" => {
                            assert_eq!(scenario, Scenario::Craft);
                            state.inventory.push(Stack {
                                item: "pick".into(),
                                count: 1,
                                pickaxe: true,
                                durability: 1561,
                                ..Default::default()
                            });
                            state.crafting.possible = false;
                        }
                        "mine" => {
                            mined += 1;
                            let pos = Pos {
                                x: command["x"].as_i64().expect("x") as i32,
                                y: command["y"].as_i64().expect("y") as i32,
                                z: command["z"].as_i64().expect("z") as i32,
                            };
                            if let Some(index) = world.index(pos) {
                                world.scan.cells[index] = 1;
                            }
                            state.navigation.ores.retain(|b| b.pos != pos);
                            // No inventory gains: an ore break must not terminate the loop.
                            if scenario == Scenario::Focus && mined <= 2 {
                                assert_eq!(
                                    pos,
                                    Pos {
                                        x: if mined == 1 { 5 } else { 6 },
                                        y: 1,
                                        z: 3
                                    },
                                    "Must finish committed vein even after another becomes cheaper/closer"
                                );
                                world.scan.palette[3].seconds = 0.01;
                                for b in &mut state.navigation.ores {
                                    if b.pos == (Pos { x: 3, y: 1, z: 5 }) {
                                        b.kind.seconds = 0.01;
                                    }
                                }
                            }
                            if mined == 1 && scenario == Scenario::FullAfterOre {
                                state.free_slots = 0;
                            } else if mined == 1 {
                                state.hostiles.push(Enemy {
                                    id: 1,
                                    uuid: "mob".into(),
                                    distance: if scenario == Scenario::Unknown {
                                        6.
                                    } else {
                                        2.
                                    },
                                    attack_distance: Some(2.),
                                    kind: "unseen_mod:large_hostile".into(),
                                    visible: true,
                                    health: 10.,
                                    ..Enemy::default()
                                });
                                state.food = 15;
                                state.light_level = 0;
                            }
                        }
                        "attack" => {
                            state.hostiles.clear();
                            state.held_item = "sword".into();
                        }
                        "eat" => {
                            state.food = 20;
                            state.held_item = "food".into();
                        }
                        "torch" => {
                            state.light_level = 14;
                            state.held_item = "torch".into();
                        }
                        _ => {}
                    }
                    state.last_result = Outcome {
                        id,
                        status: "done".into(),
                        message: String::new(),
                    };
                }
                if state.food == 20 && state.health < 20. {
                    state.health = (state.health + 2.).min(20.);
                }
                state.updated_at = now();
                atomic_json(&directory.join("state.json"), &state).expect("state update");
                thread::sleep(Duration::from_millis(10));
            }
            calls
        });
        let mut bridge = Bridge::new(root.clone(), root.join("runtime")).expect("bridge");
        bridge.remaining = Some(12);
        bridge.deadline = Some(Instant::now() + Duration::from_secs(8));
        let result = run(&mut bridge);
        shutdown.store(true, Ordering::Relaxed);
        let calls = worker.join().expect("worker");
        if scenario == Scenario::Craft {
            assert!(
                calls
                    .iter()
                    .position(|s| s == "craft_pickaxe")
                    .expect("craft")
                    < calls.iter().position(|s| s == "mine").expect("mine")
            );
        }
        if scenario == Scenario::Edge {
            assert_eq!(
                calls.first().map(String::as_str),
                Some("center"),
                "calls={calls:?}"
            );
        }
        if scenario == Scenario::FullAfterOre {
            assert!(
                result.is_ok(),
                "inventory full is a pause, not a process failure: {result:?}"
            );
            assert_eq!(calls.iter().filter(|c| *c == "mine").count(), 1);
            assert_eq!(calls.last().map(String::as_str), Some("stop"));
            let attempts =
                fs::read_to_string(bridge.runtime.join("attempts.jsonl")).expect("attempts");
            assert!(!attempts.is_empty());
            for line in attempts.lines() {
                let event: serde_json::Value = serde_json::from_str(line).expect("event");
                assert_eq!(event["failed"], false);
            }
            fs::remove_dir_all(root).expect("cleanup");
            return;
        }
        if initial_health <= 8. {
            assert_eq!(
                calls.first().map(String::as_str),
                Some("eat"),
                "low HP must eat before taking any mining action: {calls:?}"
            );
        }
        assert!(
            result.is_ok(),
            "bounded cancellation must exit cleanly: {result:?}"
        );
        assert!(
            calls.iter().filter(|c| *c == "mine").count() >= 2,
            "calls={calls:?}"
        );
        for action in ["attack", "eat", "torch", "stop"] {
            assert!(
                calls.iter().any(|c| c == action),
                "missing {action}: {calls:?}"
            );
        }
        assert_eq!(calls.last().map(String::as_str), Some("stop"));
        let book = Book::load(&bridge.runtime.join("bandit.json")).expect("saved learning");
        assert!(book.profiles.values().any(|m| m.updates > 0));
        fs::remove_dir_all(root).expect("cleanup");
    }
}
