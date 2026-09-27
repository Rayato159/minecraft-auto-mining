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

#[derive(Default)]
struct RouteFailures(HashMap<Pos, (u32, Instant)>);
impl RouteFailures {
    fn reject(&mut self, to: Pos, at: Instant) -> Instant {
        self.0
            .retain(|_, (_, seen)| at.saturating_duration_since(*seen) < Duration::from_secs(600));
        let (count, seen) = self.0.entry(to).or_insert((0, at));
        *count = count.saturating_add(1);
        *seen = at;
        at + Duration::from_secs(30 * (1_u64 << (*count - 1).min(2)))
    }
    fn reached(&mut self, to: Pos) {
        self.0.remove(&to);
    }
}
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
fn idle_retry(bridge: &mut Bridge, board: &mut Scoreboard, message: &str) -> Result<()> {
    status(bridge, board, "reroute", message)?;
    let began = Instant::now();
    let before = bridge.state()?;
    let nearest = |state: &State| {
        state
            .hostiles
            .iter()
            .filter(|e| e.visible)
            .map(|e| e.reach_distance())
            .fold(4.5_f64, f64::min)
    };
    let previous_threat = nearest(&before);
    while began.elapsed() < Duration::from_secs(3) && !bridge.stopped() {
        bridge.beat()?;
        let state = bridge.state()?;
        if !state.enabled
            || state.screen_open
            || state.home.revision != before.home.revision
            || state.goal_key() != before.goal_key()
            || state.health < before.health
            || nearest(&state) < previous_threat
            || state.oxygen.needs_escape
            || crate::nether::interrupts_work(&state) && !crate::nether::interrupts_work(&before)
        {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    Ok(())
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
    let here = state.feet();
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
pub enum StartMode {
    Resume,
    MineHere,
    MineWaypoint,
}

#[derive(Debug, Clone, Copy, PartialEq)]
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
impl MineResume {
    fn clear_for_session(runtime: &std::path::Path, state: &State) -> Result<()> {
        let path = runtime.join("resume-mine.json");
        match fs::read(&path) {
            Ok(bytes) => {
                let saved: Self = serde_json::from_slice(&bytes)?;
                if saved.session == state.session_key() {
                    fs::remove_file(path)?;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }
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
    fn save_mine_resume(&self, runtime: &std::path::Path, state: &State) -> Result<()> {
        let mine = state.home.mine.ok_or_else(|| {
            pause("Mine point is missing; set /flyminer mine set or use miner mine to mine here")
        })?;
        atomic_json(
            &runtime.join("resume-mine.json"),
            &MineResume {
                session: state.session_key(),
                home_revision: state.home.revision,
                request: self.request,
                mine,
            },
        )
    }
    fn reconfigure(self, runtime: &std::path::Path, state: &State) -> Result<Option<Self>> {
        if state.home.home.is_none() {
            MineResume::clear_for_session(runtime, state)?;
            return Ok(None);
        }
        // Waypoint/radius edits invalidate geometry, not the purpose of the trip.
        // Only a new, unacknowledged home request redirects a mine trip home.
        let mut trip = Self::new(state);
        if self.phase == TripPhase::Mine && state.home.request <= state.home.acknowledged {
            trip.phase = TripPhase::Mine;
            trip.save_mine_resume(runtime, state)?;
        }
        Ok(Some(trip))
    }
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
        let here = state.feet();
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
                && after.reserve_pickaxe()
                && (!crate::nether::active(&after) || after.has("torch"));
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
            self.save_mine_resume(&bridge.runtime, &after)?;
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
                idle_retry(
                    bridge,
                    board,
                    if !state.has("pickaxe") {
                        "No safe checked passage to destination; no usable pickaxe, so digging is unavailable"
                    } else if !can_dig {
                        "No safe checked passage to destination; digging is paused until health reaches 18"
                    } else {
                        "No safe checked route after rescanning; a pickaxe is available. Check for a connected entrance, protected blocks, or the two-block liquid/unknown buffer"
                    },
                )?;
                if self.empty_scans.is_multiple_of(10) {
                    self.denied.clear();
                    self.visits.clear();
                }
                return Ok(TripStep::Refresh);
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
                self.denied.insert(end);
                idle_retry(
                    bridge,
                    board,
                    "Travel endpoint is not progressing; checking another route",
                )?;
                return Ok(TripStep::Refresh);
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
                self.failures = 0;
                idle_retry(
                    bridge,
                    board,
                    "Route changed repeatedly; rescanning before the next detour",
                )?;
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
                if let Some(end) = self.path.iter().position(|p| *p == after.feet()) {
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
                self.failures = 0;
                idle_retry(
                    bridge,
                    board,
                    "Movement blocked; checking alternative travel routes",
                )?;
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

fn start_trip(
    bridge: &mut Bridge,
    board: &mut Scoreboard,
    state: &mut State,
    mode: StartMode,
) -> Result<Option<Trip>> {
    if mode == StartMode::Resume {
        if state.home.request > state.home.acknowledged {
            println!(
                "Pending home request: {:.0}s old. run resumes it; use miner mine to cancel it and mine here, or miner mine --waypoint to go to the saved mine.",
                now().saturating_sub(state.home.request) as f64 / 1000.
            );
        }
        return Trip::resume(&bridge.runtime, state);
    }
    if mode == StartMode::MineWaypoint && (state.home.home.is_none() || state.home.mine.is_none()) {
        bail!(
            "Set /flyminer home set and /flyminer mine set first, or use miner mine to mine here"
        );
    }
    if state.home.request > state.home.acknowledged {
        let request = state.home.request;
        let (ok, after) = action(
            bridge,
            board,
            json!({"action":"home_ack","request":request}),
        )?;
        if !ok || after.home.request != request || after.home.acknowledged != request {
            return Err(pause(
                "Home request changed or cancellation failed; inspect status and retry",
            ));
        }
        *state = after;
    }
    MineResume::clear_for_session(&bridge.runtime, state)?;
    if mode == StartMode::MineWaypoint {
        let mut trip = Trip::new(state);
        trip.phase = TripPhase::Mine;
        trip.save_mine_resume(&bridge.runtime, state)?;
        return Ok(Some(trip));
    }
    status(
        bridge,
        board,
        "mine",
        "Old trip cancelled; mining here. Home protection and automatic supply returns remain active",
    )?;
    Ok(None)
}

fn await_ready(bridge: &Bridge) -> Result<Option<State>> {
    let mut waiting = false;
    loop {
        if bridge.stopped() {
            return Ok(None);
        }
        let state = bridge.state()?;
        crate::bridge::require_enabled(&state)?;
        if !state.screen_open {
            return Ok(Some(state));
        }
        if !waiting {
            println!(
                "menu: FlyMiner is enabled on {}; close chat/Esc/inventory and return to the game to start. Ctrl+C cancels",
                state.server
            );
            waiting = true;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

pub fn run(bridge: &mut Bridge, mode: StartMode) -> Result<()> {
    let _lock = bridge.lock()?;
    if bridge.runtime.join("stop-request.json").exists() {
        fs::remove_file(bridge.runtime.join("stop-request.json"))?;
    }
    println!("Bridge folder: {}", bridge.dir.display());
    let Some(mut state) = await_ready(bridge)? else {
        return Ok(());
    };
    println!(
        "Connected: {} as {} (Bridge {}; control enabled)",
        state.server, state.username, state.bridge_version
    );
    if state.action != "idle" {
        bail!("Bridge is busy; stop the other controller first");
    }
    if crate::nether::active(&state)
        && (!state.capabilities.iter().any(|c| c == "nether_mining")
            || !state.capabilities.iter().any(|c| c == "fractional_floor"))
    {
        bail!(
            "Nether mining requires Bridge 0.10.1 (Soul Sand movement fix); close Minecraft and install the new bridge first"
        );
    }
    if let Some(y) = crate::nether::exploration_y(&state) {
        println!(
            "Nether: covered mining routes; exploration near Y={y}; rear trail torches about every 6 blocks; home stays in this dimension"
        );
    }
    if !state.capabilities.iter().any(|c| c == "rear_torches") {
        println!(
            "Bridge 0.8.0 is needed for rear torch placement and automatic hazard retreat; older bridges skip torch placement."
        );
    }
    let mut board = Scoreboard {
        started_at: now(),
        health: state.health,
        food: state.food,
        position: state.feet(),
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
        // Brain/atlas loading can take time; never act on the startup snapshot.
        state = bridge.state()?;
        if state.session_key() != session {
            return Err(Cancelled("World/session changed while starting").into());
        }
        let mut world: Option<World> = None;
        let mut scan_center = state.feet();
        let mut scanned = Instant::now();
        let mut visited = HashMap::<Pos, u32>::new();
        let mut denied = HashMap::<Pos, Instant>::new();
        let mut route_failures = RouteFailures::default();
        let mut last_cell = None;
        let mut goal_key = state.goal_key();
        let mut health = state.health;
        let mut torch_at: Option<Pos> = None;
        let mut torch_attempt = Instant::now() - Duration::from_secs(60);
        let mut torch_retry = Duration::from_secs(8);
        let mut escape_attempt = Instant::now() - Duration::from_secs(60);
        let mut equipped = false;
        let mut combat_since: Option<Instant> = None;
        let mut no_plan = 0;
        let mut recovery: Option<(f64, Instant)> = None;
        let mut trip = start_trip(bridge, &mut board, &mut state, mode)?;
        if trip.is_some() {
            status(
                bridge,
                &mut board,
                "return_to_mine",
                "Travelling to the configured mine point before continuous mining",
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
            board.position = state.feet();
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
            if state.oxygen.needs_escape {
                finish(bridge, &mut book, &mut attempt, false, &mut board)?;
                world = None;
                if let Some(t) = &mut trip {
                    t.path.clear();
                }
                equipped = false;
                if !state.capabilities.iter().any(|c| c == "oxygen_escape") {
                    return Err(pause(
                        "Low oxygen; install a bridge with oxygen escape before continuing",
                    ));
                }
                status(
                    bridge,
                    &mut board,
                    "oxygen",
                    "Low oxygen: finding breathable air before resuming work",
                )?;
                let (ok, _) = action(bridge, &mut board, json!({"action":"breathe"}))?;
                if !ok {
                    thread::sleep(Duration::from_millis(150));
                }
                continue;
            }
            if state.goal_key() != goal_key {
                focus = None;
                attempt = None;
                world = None;
                denied.clear();
                goal_key = state.goal_key();
                route_failures = RouteFailures::default();
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
                if let Some(current) = trip.take() {
                    trip = current.reconfigure(&bridge.runtime, &state)?;
                }
            }
            if state.home.home.is_some()
                && (trip.is_none() || trip.as_ref().is_some_and(|t| t.phase == TripPhase::Mine))
                && let Some(reason) = navigation::return_reason(&state)
                && !(state.crafting.possible
                    && state.free_slots >= 2
                    && !craft_failed
                    && state.has("food")
                    && (!crate::nether::active(&state) || state.has("torch"))
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
            let cell = state.feet();
            if last_cell != Some(cell) {
                *visited.entry(cell).or_default() += 1;
                last_cell = Some(cell);
            }
            if crate::nether::interrupts_work(&state) {
                finish(bridge, &mut book, &mut attempt, false, &mut board)?;
                world = None;
                if let Some(t) = &mut trip {
                    t.path.clear();
                }
                if escape_attempt.elapsed() >= Duration::from_secs(1) {
                    escape_attempt = Instant::now();
                    status(
                        bridge,
                        &mut board,
                        "retreat",
                        "Nether danger: retreating on checked ground away from ranged mobs or piglins",
                    )?;
                    let (ok, _) = action(
                        bridge,
                        &mut board,
                        json!({"action":"escape","travel":true,"avoidThreats":true}),
                    )?;
                    if ok {
                        continue;
                    }
                }
                if state.food < 18 && state.has("food") {
                    action(bridge, &mut board, json!({"action":"eat"}))?;
                    equipped = false;
                }
                idle_retry(
                    bridge,
                    &mut board,
                    "Nether danger: no checked retreat; waiting for danger to clear",
                )?;
                continue;
            }
            // Defend in-place against an enemy already in melee range before
            // attempting a retreat whose corridor would reject that enemy.
            if (state.in_lava || state.in_water || state.liquid_safe == Some(false))
                && !state
                    .hostiles
                    .iter()
                    .any(|e| !e.avoid_only && e.visible && e.reach_distance() <= 3.)
            {
                if attempt.is_some() {
                    finish(bridge, &mut book, &mut attempt, false, &mut board)?;
                }
                world = None;
                if let Some(t) = &mut trip {
                    t.path.clear();
                }
                if state.capabilities.iter().any(|c| c == "hazard_recovery")
                    && escape_attempt.elapsed() >= Duration::from_secs(3)
                {
                    escape_attempt = Instant::now();
                    status(
                        bridge,
                        &mut board,
                        "retreat",
                        "Finding supported dry ground away from the fluid buffer",
                    )?;
                    let (ok, _) =
                        action(bridge, &mut board, json!({"action":"escape","travel":true}))?;
                    if ok {
                        continue;
                    }
                }
                if state.food < 18 && state.has("food") {
                    action(bridge, &mut board, json!({"action":"eat"}))?;
                    equipped = false;
                    continue;
                }
                idle_retry(
                    bridge,
                    &mut board,
                    "No checked dry escape yet; holding position and retrying (hazard recovery needs Bridge 0.8.0)",
                )?;
                continue;
            }
            let threats: Vec<_> = state
                .hostiles
                .iter()
                .filter(|e| !e.avoid_only && e.visible && e.reach_distance() < 4.5)
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
            if crate::nether::active(&state) && !state.has("torch") && trip.is_none() {
                return Err(pause(
                    "Nether trail torches exhausted; refill torches or set a home chest in this dimension",
                ));
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
            if state.mining_profile.requires_cover && !state.mining_profile.under_cover {
                if attempt.as_ref().is_some_and(|a| a.points > 0.) {
                    finish(bridge, &mut book, &mut attempt, false, &mut board)?;
                } else {
                    attempt = None;
                }
                world = None;
                if let Some(t) = &mut trip {
                    t.path.clear();
                }
                equipped = false;
                status(
                    bridge,
                    &mut board,
                    "shelter",
                    "Dwarf profile: move under a roof or into a tunnel; mining and home routes require continuous cover",
                )?;
                thread::sleep(Duration::from_millis(250));
                continue;
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
                    if state.capabilities.iter().any(|c| c == "hazard_recovery")
                        && escape_attempt.elapsed() >= Duration::from_secs(3)
                    {
                        escape_attempt = Instant::now();
                        let (ok, _) =
                            action(bridge, &mut board, json!({"action":"escape","travel":true}))?;
                        if ok {
                            world = None;
                            attempt = None;
                            continue;
                        }
                    }
                    idle_retry(
                        bridge,
                        &mut board,
                        "Checking for a supported way off this edge; no blind drops",
                    )?;
                    world = None;
                    continue;
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
                    idle_retry(
                        bridge,
                        &mut board,
                        "Standing geometry changed; checking another safe anchor",
                    )?;
                    world = None;
                    continue;
                }
                if after.feet() != cell {
                    if attempt.as_ref().is_some_and(|a| a.points > 0.) {
                        finish(bridge, &mut book, &mut attempt, false, &mut board)?;
                    } else {
                        attempt = None;
                    }
                    world = None;
                }
                continue;
            }
            if !state.capabilities.iter().any(|c| c == "block_tools")
                && state.has("pickaxe")
                && (!equipped || !state.holds("pickaxe"))
            {
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
                scan_center = after.feet();
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
                    if no_plan >= 2 {
                        if let Some(locked) = focus.take() {
                            for p in locked.pending() {
                                denied.insert(p, Instant::now() + Duration::from_secs(90));
                            }
                            status(
                                bridge,
                                &mut board,
                                "detour",
                                "Vein temporarily inaccessible; trying other ore or a safe detour, then retrying it later",
                            )?;
                            no_plan = 0;
                        } else {
                            if no_plan % 10 == 0 {
                                visited.clear();
                            }
                            idle_retry(
                                bridge,
                                &mut board,
                                if crate::nether::active(&state) {
                                    "No checked covered Nether route; looking for a dry tunnel entrance away from lava and ranged mobs"
                                } else {
                                    "No checked mining route yet; rescanning for a dry detour or staircase"
                                },
                            )?;
                        }
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
                                route_failures.reached(to);
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
                                let until = route_failures.reject(to, Instant::now());
                                denied.insert(to, until);
                                status(
                                    bridge,
                                    &mut board,
                                    "detour",
                                    "Walking route failed; excluding that destination while checking another path",
                                )?;
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
            if crate::lighting::due(&state, torch_at)
                && torch_attempt.elapsed() > torch_retry
                && let Some(a) = &attempt
                && let Some(command) = crate::lighting::command(
                    &state,
                    &a.plan,
                    focus.as_ref(),
                    dig.as_ref().map(|d| d.0),
                )
            {
                torch_attempt = Instant::now();
                status(
                    bridge,
                    &mut board,
                    "torch",
                    "Placing a torch behind the digging direction, outside planned cuts",
                )?;
                let (ok, _) = action(bridge, &mut board, command)?;
                torch_retry = Duration::from_secs(if ok { 8 } else { 30 });
                if ok {
                    torch_at = Some(cell);
                }
                equipped = false;
                continue;
            }
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
                    let obstructed = after
                        .last_result
                        .message
                        .contains("hidden behind another block");
                    // Retry the same ore from a different standing point. Occlusion
                    // says nothing about whether the ore itself is unreachable.
                    denied.insert(
                        if obstructed { cell } else { pos },
                        Instant::now() + Duration::from_secs(30),
                    );
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
    #[test]
    fn startup_waits_for_enabled_menu_to_close_without_publishing_actions() {
        let root = std::env::temp_dir().join(format!("miner-start-menu-{}", now()));
        let dir = root.join("config/flyminer");
        fs::create_dir_all(&dir).expect("dir");
        let mut state = State {
            protocol: 8,
            updated_at: now(),
            enabled: true,
            expected_session: true,
            screen_open: true,
            server: "new.example:27920".into(),
            ..Default::default()
        };
        atomic_json(&dir.join("state.json"), &state).expect("state");
        let mut bridge = Bridge::new(root.clone(), root.join("runtime")).expect("bridge");
        bridge.deadline = Some(Instant::now() + Duration::from_secs(3));
        let worker = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            assert!(
                !dir.join("command.json").exists(),
                "Menu waiting must not control the character"
            );
            state.screen_open = false;
            state.updated_at = now();
            atomic_json(&dir.join("state.json"), &state).expect("close menu");
        });
        let ready = await_ready(&bridge)
            .expect("enabled menu is not an error")
            .expect("ready");
        worker.join().expect("worker");
        assert!(ready.enabled && !ready.screen_open);
        assert_eq!(ready.server, "new.example:27920");
        assert!(!bridge.dir.join("command.json").exists());
        fs::remove_dir_all(root).expect("cleanup");
    }
    #[test]
    fn startup_menu_wait_can_be_cancelled_without_enabling_or_moving() {
        let root = std::env::temp_dir().join(format!("miner-cancel-menu-{}", now()));
        let dir = root.join("config/flyminer");
        fs::create_dir_all(&dir).expect("dir");
        let state = State {
            protocol: 8,
            updated_at: now(),
            enabled: true,
            expected_session: true,
            screen_open: true,
            ..Default::default()
        };
        atomic_json(&dir.join("state.json"), &state).expect("state");
        let mut bridge = Bridge::new(root.clone(), root.join("runtime")).expect("bridge");
        bridge.deadline = Some(Instant::now() + Duration::from_millis(100));
        assert!(await_ready(&bridge).expect("cancel").is_none());
        assert!(!dir.join("command.json").exists());
        fs::remove_dir_all(root).expect("cleanup");
    }
    #[test]
    fn failed_walking_cells_back_off_then_reset_after_verified_success() {
        let mut failures = RouteFailures::default();
        let p = Pos {
            x: 67,
            y: 112,
            z: -6,
        };
        let now = Instant::now();
        for (step, seconds) in [30, 60, 120, 120].into_iter().enumerate() {
            let at = now + Duration::from_secs(step as u64 * 130);
            assert_eq!(
                failures.reject(p, at).duration_since(at),
                Duration::from_secs(seconds)
            );
        }
        failures.reached(p);
        let at = now + Duration::from_secs(520);
        assert_eq!(
            failures.reject(p, at).duration_since(at),
            Duration::from_secs(30)
        );
        let at = now + Duration::from_secs(1200);
        assert_eq!(
            failures.reject(p, at).duration_since(at),
            Duration::from_secs(30)
        );
    }
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

    #[test]
    fn waypoint_edits_preserve_mine_phase_and_restart_checkpoint_but_new_home_preempts() {
        let root = std::env::temp_dir().join(format!("miner-retarget-{}", now()));
        fs::create_dir_all(&root).expect("directory");
        let (_, mut state) = crate::planner::tests::fixture();
        state.home.home = Some(Pos { x: 6, y: 1, z: 3 });
        state.home.mine = Some(Pos { x: 3, y: 1, z: 3 });
        state.home.request = 10;
        state.home.acknowledged = 10;
        let mut trip = Trip::new(&state);
        trip.phase = TripPhase::Mine;
        trip.path.push(Pos { x: 5, y: 1, z: 3 });
        state.home.mine = Some(Pos { x: 4, y: 1, z: 3 });
        state.home.radius = 2;
        state.home.revision += 1;
        let trip = trip
            .reconfigure(&root, &state)
            .expect("replan")
            .expect("trip");
        assert_eq!(trip.phase, TripPhase::Mine);
        assert!(trip.path.is_empty());
        assert_eq!(
            Trip::resume(&root, &state)
                .expect("restart")
                .expect("saved")
                .phase,
            TripPhase::Mine
        );
        state.home.request = 11;
        state.home.revision += 1;
        let trip = trip
            .reconfigure(&root, &state)
            .expect("new request")
            .expect("trip");
        assert_eq!(trip.phase, TripPhase::Home);
        assert_eq!(trip.request, 11);
        assert!(
            Trip::resume(&root, &state)
                .expect("old checkpoint")
                .is_none()
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Scenario {
        Dwarf,
        BlockTools,
        Oxygen,
        Normal,
        Edge,
        FullAfterOre,
        Craft,
        Unknown,
        Focus,
        BlockedFocus,
        LiquidBuffer,
        LiquidCombat,
        TorchSkipped,
        NetherThreat,
    }

    #[test]
    fn dwarf_eats_while_exposed_then_resumes_only_after_shelter() {
        exercise_loop(20., 10, Scenario::Dwarf);
    }
    #[test]
    fn block_tool_bridge_keeps_control_of_equipment_between_digs() {
        exercise_loop(20., 20, Scenario::BlockTools);
    }
    #[test]
    fn continuous_ipc_survives_combat_food_and_darkness_without_waiting_for_drops() {
        exercise_loop(20., 20, Scenario::Normal);
    }
    #[test]
    fn underground_oxygen_escape_preempts_mining_and_then_resumes() {
        exercise_loop(20., 20, Scenario::Oxygen);
    }
    #[test]
    fn vein_lock_survives_reward_updates_and_combat_without_grabbing_other_exposed_ore() {
        exercise_loop(20., 20, Scenario::Focus);
    }
    #[test]
    fn blocked_vein_is_deferred_and_other_ore_is_mined_without_exiting() {
        exercise_loop(20., 20, Scenario::BlockedFocus);
    }
    #[test]
    fn unsafe_liquid_buffer_retreats_before_resuming_mining() {
        exercise_loop(20., 20, Scenario::LiquidBuffer);
    }
    #[test]
    fn melee_defense_remains_active_while_waiting_for_a_hazard_retreat() {
        exercise_loop(20., 20, Scenario::LiquidCombat);
    }
    #[test]
    fn missing_rear_torch_support_backs_off_and_keeps_mining() {
        exercise_loop(20., 20, Scenario::TorchSkipped);
    }
    #[test]
    fn nether_retreats_before_work_and_places_a_rear_torch_even_in_bright_light() {
        exercise_loop(20., 20, Scenario::NetherThreat);
    }
    #[test]
    fn old_bridge_cannot_begin_nether_control_or_publish_actions() {
        let root = std::env::temp_dir().join(format!("miner-nether-version-{}", now()));
        let dir = root.join("config/flyminer");
        fs::create_dir_all(&dir).expect("directory");
        let state = State {
            dimension: crate::nether::DIMENSION.into(),
            protocol: 8,
            enabled: true,
            expected_session: true,
            action: "idle".into(),
            health: 20.,
            updated_at: now(),
            ..Default::default()
        };
        atomic_json(&dir.join("state.json"), &state).expect("state");
        let mut bridge = Bridge::new(root.clone(), root.join("runtime")).expect("bridge");
        let error = run(&mut bridge, StartMode::Resume).expect_err("old bridge must be rejected");
        assert!(error.to_string().contains("Bridge 0.10.1"), "{error:#}");
        assert!(!dir.join("command.json").exists());
        fs::remove_dir_all(root).expect("cleanup");
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
        NoNetherTorches,
        EmptyChest,
        CancelHere,
        CancelWaypoint,
        RejectCancel,
        RetargetMine,
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
    fn nether_torch_exhaustion_returns_restocks_and_resumes_mining() {
        exercise_home(HomeCase::NoNetherTorches);
    }
    #[test]
    fn insufficient_chest_supplies_pause_at_home_without_mining() {
        exercise_home(HomeCase::EmptyChest);
    }
    #[test]
    fn mine_here_cancels_stale_home_and_checkpoint_then_honors_a_new_home_request() {
        exercise_home(HomeCase::CancelHere);
    }
    #[test]
    fn mine_waypoint_bypasses_storage_and_starts_at_the_saved_mine() {
        exercise_home(HomeCase::CancelWaypoint);
    }
    #[test]
    fn failed_home_cancellation_does_not_start_mining_or_clear_checkpoint() {
        exercise_home(HomeCase::RejectCancel);
    }
    #[test]
    fn editing_mine_after_storage_does_not_send_player_home_again() {
        exercise_home(HomeCase::RetargetMine);
    }

    fn exercise_home(case: HomeCase) {
        use crate::types::{Point, Route};
        let root = std::env::temp_dir().join(format!("miner-home-{}-{case:?}", now()));
        let directory = root.join("config/flyminer");
        fs::create_dir_all(&directory).expect("dir");
        let (mut world, mut state) = crate::planner::tests::fixture();
        let home = Pos { x: 6, y: 1, z: 3 };
        let mine = state.feet();
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
        state.free_slots = if matches!(case, HomeCase::Full | HomeCase::RetargetMine) {
            0
        } else {
            1
        };
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
        if case == HomeCase::NoNetherTorches {
            state.dimension = crate::nether::DIMENSION.into();
            world.scan.dimension = state.dimension.clone();
            state.capabilities = vec!["nether_mining".into(), "fractional_floor".into()];
            state.inventory.retain(|s| !s.torch);
        }
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
        let explicit_mine = matches!(
            case,
            HomeCase::CancelHere | HomeCase::CancelWaypoint | HomeCase::RejectCancel
        );
        if explicit_mine {
            state.home.request = 10;
            state.home.acknowledged = 9;
            fs::create_dir_all(root.join("runtime")).expect("runtime");
            atomic_json(
                &root.join("runtime/resume-mine.json"),
                &MineResume {
                    session: state.session_key(),
                    home_revision: 0,
                    request: 9,
                    mine: home, // An obsolete checkpoint must not send mine-here back home.
                },
            )
            .expect("old checkpoint");
        }
        if case == HomeCase::CancelWaypoint {
            state.position.x = f64::from(home.x) + 0.5;
        }
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
                        "home_request" => state.home.request += 1,
                        "home_ack" => {
                            assert_eq!(command["request"].as_u64(), Some(state.home.request));
                            if case == HomeCase::RejectCancel {
                                status = "failed";
                            } else {
                                state.home.acknowledged = state.home.request;
                            }
                            if case == HomeCase::RetargetMine {
                                state.home.mine = Some(Pos { x: 4, y: 1, z: 3 });
                                state.home.revision += 1;
                            }
                        }
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
                                        state.feet(),
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
                            assert!(state.feet().distance(chest) <= 1.5);
                            state.screen_open = true;
                            state.container_owned = true;
                            state.action = "store".into();
                            store_ticks = 12;
                            store_id = id.clone();
                            status = "running";
                        }
                        "mine" => {
                            if !stored {
                                assert!(
                                    matches!(
                                        case,
                                        HomeCase::Manual
                                            | HomeCase::CancelHere
                                            | HomeCase::CancelWaypoint
                                    ),
                                    "must store/refill before mining"
                                );
                                if explicit_mine {
                                    assert_eq!(state.home.request, state.home.acknowledged);
                                    assert_eq!(state.feet(), mine);
                                    assert_eq!(state.home.home, Some(home));
                                    assert_eq!(state.home.radius, 1);
                                }
                                if case == HomeCase::CancelWaypoint {
                                    resumed = true;
                                }
                            } else {
                                assert_eq!(
                                    Some(state.feet()),
                                    state.home.mine,
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
                            if matches!(case, HomeCase::Manual | HomeCase::CancelHere) && !triggered
                            {
                                triggered = true;
                                state.home.request += 1;
                                state.home.revision += 1;
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
                        let to = state.feet().offset(dx, dy, dz);
                        let blocks: Vec<_> = crate::world::clearance(state.feet(), to)
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
            (calls, stored, resumed, state.feet())
        });
        let mut bridge = Bridge::new(root.clone(), root.join("runtime")).expect("bridge");
        bridge.remaining = Some(60);
        bridge.deadline = Some(Instant::now() + Duration::from_secs(12));
        let mode = match case {
            HomeCase::CancelHere | HomeCase::RejectCancel => StartMode::MineHere,
            HomeCase::CancelWaypoint => StartMode::MineWaypoint,
            _ => StartMode::Resume,
        };
        let result = run(&mut bridge, mode);
        shutdown.store(true, Ordering::Relaxed);
        let (calls, stored, resumed, last) = worker.join().expect("worker");
        if case == HomeCase::RejectCancel {
            assert!(result.is_ok(), "cancellation failure pauses: {result:?}");
            assert_eq!(calls, ["home_ack", "stop"]);
            assert!(bridge.runtime.join("resume-mine.json").exists());
        } else if case == HomeCase::CancelWaypoint {
            assert!(!stored, "explicit mine must bypass old storage trip");
            assert!(resumed, "must reach mine and mine: {calls:?} {result:?}");
            assert!(!bridge.runtime.join("resume-mine.json").exists());
        } else if case == HomeCase::EmptyChest {
            assert!(stored, "must try chest before supply pause");
            assert!(result.is_ok(), "resource pause: {result:?}");
            assert!(!resumed);
            assert!(last.distance(home) <= 2.);
            assert!(!calls.iter().any(|c| c == "mine"));
        } else {
            assert!(stored, "store not reached: {case:?} {calls:?} {result:?}");
            assert!(resumed, "must mine again: {case:?} {calls:?} {result:?}");
            assert!(calls.iter().any(|c| c == "home_ack"));
            assert_eq!(
                calls.iter().filter(|c| *c == "store").count(),
                1,
                "must not loop back to storage"
            );
            if case == HomeCase::CancelHere {
                assert_eq!(calls.first().map(String::as_str), Some("home_ack"));
                assert_eq!(calls.iter().filter(|c| *c == "home_ack").count(), 2);
                assert!(!bridge.runtime.join("resume-mine.json").exists());
            }
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
        state.capabilities = vec!["hazard_recovery".into(), "rear_torches".into()];
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
        if scenario == Scenario::Dwarf {
            state.mining_profile.name = "dwarf".into();
            state.mining_profile.can_swim = false;
            state.mining_profile.requires_cover = true;
            state.mining_profile.under_cover = false;
        }
        if scenario == Scenario::BlockTools {
            state.capabilities.push("block_tools".into());
            state.held_item = "minecraft:iron_shovel".into();
        }
        if scenario == Scenario::Oxygen {
            state.oxygen.needs_escape = true;
            state.oxygen.air = 100;
            state.capabilities.push("oxygen_escape".into());
        }
        if scenario == Scenario::NetherThreat {
            state.dimension = crate::nether::DIMENSION.into();
            world.scan.dimension = state.dimension.clone();
            state.capabilities.push("nether_mining".into());
            state.capabilities.push("fractional_floor".into());
            state.hostiles.push(Enemy {
                kind: "minecraft:ghast".into(),
                ranged: true,
                visible: true,
                distance: 18.,
                ..Default::default()
            });
        }
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
        if matches!(scenario, Scenario::LiquidBuffer | Scenario::LiquidCombat) {
            state.liquid_safe = Some(false);
        }
        if scenario == Scenario::LiquidCombat {
            state.hostiles.push(Enemy {
                id: 1,
                uuid: "mob".into(),
                visible: true,
                distance: 2.,
                health: 10.,
                ..Default::default()
            });
        }
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
        if matches!(scenario, Scenario::Focus | Scenario::BlockedFocus) {
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
        let scoreboard = root.join("runtime/scoreboard.json");
        let worker = thread::spawn(move || {
            let mut calls = Vec::<String>::new();
            let mut mined = 0;
            let mut saw_shelter = false;
            while !finished.load(Ordering::Relaxed) {
                if scenario == Scenario::Dwarf
                    && !saw_shelter
                    && let Ok(bytes) = fs::read(&scoreboard)
                    && let Ok(board) = serde_json::from_slice::<serde_json::Value>(&bytes)
                    && board["mode"] == "shelter"
                {
                    saw_shelter = true;
                    state.mining_profile.under_cover = true;
                }
                if let Ok(bytes) = fs::read(directory.join("command.json")) {
                    let command: serde_json::Value =
                        serde_json::from_slice(&bytes).expect("command");
                    fs::remove_file(directory.join("command.json")).expect("consume");
                    let name = command["action"].as_str().expect("action");
                    calls.push(name.to_string());
                    if matches!(name, "mine" | "traverse" | "scan") {
                        assert!(
                            state.mining_profile.under_cover,
                            "Work must wait for shelter"
                        );
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
                    let mut result_status = "done";
                    match name {
                        "breathe" => {
                            state.oxygen.needs_escape = false;
                            state.oxygen.air = 300;
                            state.oxygen.breathable = true;
                        }
                        "escape" => {
                            state.liquid_safe = Some(true);
                            if scenario == Scenario::NetherThreat {
                                assert_eq!(command["avoidThreats"], true);
                                state.hostiles.clear();
                            }
                        }
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
                            if matches!(scenario, Scenario::Focus | Scenario::BlockedFocus)
                                && mined <= 2
                            {
                                assert_eq!(
                                    pos,
                                    if scenario == Scenario::BlockedFocus && mined == 2 {
                                        Pos { x: 3, y: 1, z: 5 }
                                    } else {
                                        Pos {
                                            x: if mined == 1 { 5 } else { 6 },
                                            y: 1,
                                            z: 3,
                                        }
                                    },
                                    "Must finish committed vein even after another becomes cheaper/closer"
                                );
                                world.scan.palette[3].seconds = 0.01;
                                for b in &mut state.navigation.ores {
                                    if b.pos == (Pos { x: 3, y: 1, z: 5 }) {
                                        b.kind.seconds = 0.01;
                                    }
                                }
                                if scenario == Scenario::BlockedFocus && mined == 1 {
                                    let blocked = Pos { x: 6, y: 1, z: 3 };
                                    world.scan.palette.push(crate::types::Block {
                                        diggable: false,
                                        ..world.scan.palette[2].clone()
                                    });
                                    let i = world.index(blocked).expect("blocked ore");
                                    world.scan.cells[i] = world.scan.palette.len() - 1;
                                    state.navigation.ores.retain(|b| b.pos != blocked);
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
                            assert!(
                                command["forwardX"].as_i64().expect("heading x").abs()
                                    + command["forwardZ"].as_i64().expect("heading z").abs()
                                    == 1
                            );
                            assert!(command["avoid"].is_array());
                            if scenario == Scenario::TorchSkipped {
                                result_status = "skipped";
                            } else {
                                state.light_level = 14;
                            }
                            state.held_item = "torch".into();
                        }
                        _ => {}
                    }
                    state.last_result = Outcome {
                        id,
                        status: result_status.into(),
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
            if scenario == Scenario::Dwarf {
                assert!(saw_shelter, "Must wait for shelter before resuming");
            }
            calls
        });
        let mut bridge = Bridge::new(root.clone(), root.join("runtime")).expect("bridge");
        bridge.remaining = Some(
            if matches!(
                scenario,
                Scenario::BlockedFocus
                    | Scenario::LiquidBuffer
                    | Scenario::LiquidCombat
                    | Scenario::TorchSkipped
            ) {
                20
            } else {
                12
            },
        );
        bridge.deadline = Some(Instant::now() + Duration::from_secs(8));
        let result = run(&mut bridge, StartMode::Resume);
        shutdown.store(true, Ordering::Relaxed);
        let calls = worker.join().expect("worker");
        if scenario == Scenario::Dwarf {
            assert_eq!(calls.first().map(String::as_str), Some("eat"));
        }
        if scenario == Scenario::BlockTools {
            assert!(
                !calls.iter().any(|c| c == "equip"),
                "Rust must not force a pickaxe between per-block tool choices: {calls:?}"
            );
        }
        if scenario == Scenario::Oxygen {
            assert_eq!(calls.first().map(String::as_str), Some("breathe"));
            assert!(
                calls.iter().any(|c| c == "mine"),
                "Mining must resume after oxygen recovery"
            );
        }
        if scenario == Scenario::NetherThreat {
            assert_eq!(calls.first().map(String::as_str), Some("escape"));
            assert!(
                calls
                    .iter()
                    .position(|c| c == "torch")
                    .expect("bright Nether torch")
                    < calls.iter().position(|c| c == "mine").expect("mining")
            );
        }
        if scenario == Scenario::LiquidBuffer {
            assert_eq!(calls.first().map(String::as_str), Some("escape"));
        }
        if scenario == Scenario::LiquidCombat {
            assert_eq!(calls.first().map(String::as_str), Some("attack"));
            assert!(
                calls.iter().position(|c| c == "escape").expect("retreat")
                    < calls.iter().position(|c| c == "mine").expect("mine")
            );
        }
        if scenario == Scenario::TorchSkipped {
            assert_eq!(
                calls.iter().filter(|c| *c == "torch").count(),
                1,
                "skip must back off"
            );
        }
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
