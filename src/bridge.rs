use crate::types::{Outcome, Scan, State};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub static STOP: AtomicBool = AtomicBool::new(false);
/// Normal cancellation is distinct from failed IPC, invalid state and timeouts.
#[derive(Debug)]
pub struct Cancelled(pub &'static str);
impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for Cancelled {}
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}
#[cfg(windows)]
pub fn signals() -> Result<()> {
    unsafe extern "system" {
        fn SetConsoleCtrlHandler(
            handler: Option<unsafe extern "system" fn(u32) -> i32>,
            add: i32,
        ) -> i32;
    }
    unsafe extern "system" fn handler(event: u32) -> i32 {
        if event <= 2 {
            STOP.store(true, Ordering::SeqCst);
            1
        } else {
            0
        }
    }
    // Handler only sets an atomic flag; the controller owns all I/O and cleanup.
    if unsafe { SetConsoleCtrlHandler(Some(handler), 1) } == 0 {
        bail!("Cannot register Ctrl+C handler");
    }
    Ok(())
}
#[cfg(not(windows))]
pub fn signals() -> Result<()> {
    Ok(())
}

pub fn atomic_json(path: &Path, data: &impl Serialize) -> Result<()> {
    let tmp = path.with_extension("tmp");
    // Terrain checkpoints contain millions of JSON fragments. Buffer them before
    // reaching Windows/the filesystem; unbuffered serde writes stalled the loop
    // for seconds after every scan, also delaying heartbeat and stop handling.
    let mut file = BufWriter::with_capacity(256 * 1024, File::create(&tmp)?);
    serde_json::to_writer(&mut file, data)?;
    file.flush()?;
    drop(file);
    fs::rename(&tmp, path).with_context(|| format!("Publish {}", path.display()))
}

pub struct Bridge {
    pub dir: PathBuf,
    pub runtime: PathBuf,
    serial: u64,
    heartbeat: Instant,
    pub deadline: Option<Instant>,
    pub remaining: Option<u64>,
    pub brain_enabled: bool,
}
impl Bridge {
    pub fn new(instance: PathBuf, runtime: PathBuf) -> Result<Self> {
        let dir = instance.join("config/flyminer");
        if !dir.is_dir() {
            bail!("Bridge directory missing: {}", dir.display());
        }
        fs::create_dir_all(&runtime)?;
        Ok(Self {
            dir,
            runtime,
            serial: 0,
            heartbeat: Instant::now() - Duration::from_secs(5),
            deadline: None,
            remaining: None,
            brain_enabled: true,
        })
    }
    pub fn lock(&self) -> Result<File> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.dir.join("rust-controller.lock"))?;
        file.try_lock()
            .context("Another Rust controller is already running")?;
        Ok(file)
    }
    pub fn state(&self) -> Result<State> {
        let s: State = serde_json::from_slice(
            &fs::read(self.dir.join("state.json"))
                .context("Start Minecraft with the Mining Bridge first")?,
        )?;
        if s.updated_at > now() + 1000 || now().saturating_sub(s.updated_at) > 5000 {
            bail!(
                "Game state is stale at {}; open the Minecraft profile selected by MC_INSTANCE or --instance (the game may be closed or frozen)",
                self.dir.display()
            );
        }
        if s.protocol != 8 {
            bail!(
                "Bridge {} uses protocol {}; install 0.7.0 and restart Minecraft",
                s.bridge_version,
                s.protocol
            );
        }
        if !s.position.x.is_finite()
            || !s.position.y.is_finite()
            || !s.position.z.is_finite()
            || !s.health.is_finite()
        {
            bail!("Invalid game state");
        }
        Ok(s)
    }
    pub fn stopped(&self) -> bool {
        STOP.load(Ordering::Relaxed)
            || self.runtime.join("stop-request.json").exists()
            || self.deadline.is_some_and(|t| Instant::now() >= t)
    }
    pub fn beat(&mut self) -> Result<()> {
        if self.heartbeat.elapsed() >= Duration::from_millis(500) {
            fs::write(self.dir.join("heartbeat"), now().to_string())?;
            self.heartbeat = Instant::now();
        }
        Ok(())
    }
    fn publish(&mut self, mut command: Value) -> Result<String> {
        self.serial += 1;
        let id = format!("rust-{}-{}-{}", std::process::id(), now(), self.serial);
        command["id"] = json!(id);
        command["createdAt"] = json!(now());
        let tmp = self.dir.join(format!("{id}.tmp"));
        fs::write(&tmp, serde_json::to_vec(&command)?)?;
        // Hard-link publication is atomic and refuses to overwrite another command.
        let result = fs::hard_link(&tmp, self.dir.join("command.json"));
        let _ = fs::remove_file(&tmp);
        result.context("Command slot occupied; another controller may be running")?;
        Ok(id)
    }
    pub fn stop(&mut self) -> Result<()> {
        match fs::remove_file(self.dir.join("command.json")) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let id = self.publish(json!({"action":"stop"}))?;
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(3) {
            if self
                .state()
                .is_ok_and(|s| s.last_result.id == id && s.action == "idle")
            {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(50));
        }
        bail!("Stop was queued, but game did not acknowledge it; bridge heartbeat will expire")
    }
    pub fn command(&mut self, command: Value) -> Result<(Outcome, State)> {
        let before = self.state()?;
        if self.stopped() {
            return Err(Cancelled("Stop requested").into());
        }
        if self.remaining == Some(0) {
            return Err(Cancelled("Bounded run reached its action limit").into());
        }
        require_enabled(&before)?;
        if before.screen_open {
            bail!(
                "FlyMiner is enabled, but a game menu/chat is open. Close it before sending an action"
            );
        }
        if before.health <= 0. {
            bail!("Player is dead; respawn and enable FlyMiner again");
        }
        let action = command["action"].as_str().unwrap_or("").to_string();
        let travel = command["travel"].as_bool().unwrap_or(false);
        let threat_retreat = action == "escape" && command["avoidThreats"] == true;
        if before.oxygen.needs_escape && action != "breathe" {
            return Ok((
                Outcome {
                    status: "interrupted".into(),
                    message: "Oxygen recovery preempts work".into(),
                    ..Outcome::default()
                },
                before,
            ));
        }
        self.beat()?;
        let id = self.publish(command)?;
        if let Some(remaining) = &mut self.remaining {
            *remaining = remaining.saturating_sub(1);
        }
        let started = Instant::now();
        loop {
            if self.stopped() {
                self.stop()?;
                return Err(Cancelled("Stop requested").into());
            }
            self.beat()?;
            let state = self.state()?;
            if state.last_result.id == id && state.last_result.status != "running" {
                return Ok((state.last_result.clone(), state));
            }
            if state.session_key() != before.session_key() || !state.enabled {
                self.stop()?;
                return Err(Cancelled("Session changed or control disabled").into());
            }
            if state.oxygen.needs_escape && action != "breathe" {
                self.stop()?;
                return Ok((
                    Outcome {
                        id,
                        status: "interrupted".into(),
                        message: "Oxygen recovery preempts work".into(),
                    },
                    self.state()?,
                ));
            }
            if state.goal_key() != before.goal_key()
                || state.home.revision != before.home.revision
                || state.screen_open
                    && !(matches!(action.as_str(), "store" | "craft_pickaxe")
                        && state.container_owned)
            {
                self.stop()?;
                return Ok((
                    Outcome {
                        id,
                        status: "interrupted".into(),
                        message: "Settings changed or menu opened".into(),
                    },
                    self.state()?,
                ));
            }
            let threatened = state
                .hostiles
                .iter()
                .any(|e| e.visible && e.reach_distance() < 3.5);
            if matches!(
                action.as_str(),
                "mine"
                    | "traverse"
                    | "route"
                    | "scan"
                    | "center"
                    | "store"
                    | "craft_pickaxe"
                    | "open_passage"
                    | "escape"
                    | "torch"
            ) && ((threatened && !threat_retreat)
                || action != "escape" && crate::nether::interrupts_work(&state)
                || state.health < (if travel && action != "mine" { 9. } else { 18. })
                || state.in_lava)
            {
                self.stop()?;
                return Ok((
                    Outcome {
                        id,
                        status: "interrupted".into(),
                        message: "Survival interrupt".into(),
                    },
                    self.state()?,
                ));
            }
            if started.elapsed()
                > Duration::from_secs(match action.as_str() {
                    "store" => 75,
                    "craft_pickaxe" => 180,
                    "breathe" => 30,
                    _ => 25,
                })
            {
                self.stop()?;
                bail!("Game action timed out");
            }
            thread::sleep(Duration::from_millis(40));
        }
    }
    pub fn scan(&mut self) -> Result<(Scan, State)> {
        let (result, state) = self.command(json!({"action":"scan"}))?;
        if result.status != "done" {
            bail!("Scan interrupted: {}", result.message);
        }
        let scan: Scan = serde_json::from_slice(&fs::read(self.dir.join("scan.json"))?)?;
        if scan.id != result.id || now().saturating_sub(scan.updated_at) > 5000 {
            bail!("Mismatched scan result");
        }
        Ok((scan, state))
    }
}

pub fn require_enabled(state: &State) -> Result<()> {
    if !state.expected_session {
        bail!("Minecraft has no active world/server. Join the world in the selected profile first");
    }
    if !state.enabled {
        bail!(
            "FlyMiner control is disabled for {}. Run /flyminer enable in this session (rejoining, changing dimension or dying requires enabling again)",
            state.server
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn startup_diagnostics_distinguish_disconnected_disabled_and_open_menu() {
        let mut state = State::default();
        assert!(
            require_enabled(&state)
                .expect_err("disconnected")
                .to_string()
                .contains("no active world")
        );
        state.expected_session = true;
        state.server = "new.example:27920".into();
        assert!(
            require_enabled(&state)
                .expect_err("disabled")
                .to_string()
                .contains("disabled for new.example:27920")
        );
        state.enabled = true;
        state.screen_open = true;
        require_enabled(&state).expect("Menu must not be reported as missing enable");
    }
    #[test]
    fn publication_and_lock_are_exclusive() {
        let root = std::env::temp_dir().join(format!("miner-ipc-{}", now()));
        fs::create_dir_all(root.join("config/flyminer")).expect("directory");
        let mut bridge = Bridge::new(root.clone(), root.join("runtime")).expect("bridge");
        let lock = bridge.lock().expect("first lock");
        assert!(bridge.lock().is_err());
        bridge
            .publish(json!({"action":"scan"}))
            .expect("first command");
        assert!(bridge.publish(json!({"action":"mine"})).is_err());
        assert_eq!(
            serde_json::from_slice::<Value>(
                &fs::read(bridge.dir.join("command.json")).expect("read")
            )
            .expect("json")["action"],
            "scan"
        );
        let path = root.join("checkpoint.json");
        atomic_json(&path, &json!({"a":1})).expect("save");
        atomic_json(&path, &json!({"a":2})).expect("replace");
        drop(lock);
        fs::remove_dir_all(root).expect("cleanup");
    }
}
