mod bandit;
mod brain;
mod bridge;
mod controller;
mod navigation;
mod planner;
mod safety;
mod types;
mod world;
use anyhow::{Context, Result, bail};
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

fn instance() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("MC_INSTANCE") {
        return Ok(path.into());
    }
    if let Ok(text) = fs::read_to_string(".env") {
        for line in text.lines() {
            if let Some(value) = line.trim().strip_prefix("MC_INSTANCE=") {
                return Ok(PathBuf::from(value.trim().trim_matches(['\'', '"'])));
            }
        }
    }
    bail!("Set MC_INSTANCE in .env or use --instance <folder>")
}
fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let verb = args.next().unwrap_or_else(|| "help".into());
    if verb == "nav-bench" {
        return navigation::benchmark(std::path::Path::new("runtime"), &instance()?);
    }
    if verb == "brain-trace" {
        return brain::Brain::reference_trace(std::path::Path::new("runtime"));
    }
    if verb == "brain-bench" {
        return brain::Brain::benchmark(
            std::path::Path::new("references/Drosophila_brain_model"),
            std::path::Path::new("runtime"),
        );
    }
    if matches!(verb.as_str(), "help" | "--help" | "-h") {
        println!(
            "Rust Mining Controller 0.7.1\n\nminer run       Continuous mining / survival / home storage and restocking\nminer craft     Craft the best available pickaxe only\nminer brain-bench  Benchmark real FlyWire graph, SIMD vs scalar\nminer brain-trace  Export independent LIF validation fixture\nminer status    Current game status\nminer scan      Scan and preview best target without moving or mining\nminer stop      Stop a running controller\n\nOptions: --instance <folder> --max-actions <N> --max-seconds <N> --no-brain (baseline comparison)\nIn game: /flyminer enable | stop | target <ore-id> (Tab for suggestions) | home [set X Y Z | radius N | search N | status | clear] | mine set [X Y Z]"
        );
        return Ok(());
    }
    let mut location = None;
    let mut max_actions = None;
    let mut max_seconds = None;
    let mut brain_enabled = true;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--no-brain" => brain_enabled = false,
            "--instance" => {
                location = Some(PathBuf::from(args.next().context("Missing instance path")?))
            }
            "--max-actions" => {
                max_actions = Some(
                    args.next()
                        .context("Missing action count")?
                        .parse::<u64>()?,
                )
            }
            "--max-seconds" => {
                max_seconds = Some(args.next().context("Missing seconds")?.parse::<u64>()?)
            }
            _ => bail!("Unknown option: {arg}"),
        }
    }
    bridge::signals()?;
    let runtime = std::env::current_dir()?.join("runtime");
    let mut bridge = bridge::Bridge::new(location.map_or_else(instance, Ok)?, runtime)?;
    bridge.remaining = max_actions;
    bridge.brain_enabled = brain_enabled;
    bridge.deadline = max_seconds.map(|s| Instant::now() + Duration::from_secs(s));
    match verb.as_str() {
        "status" => {
            println!("{}", serde_json::to_string_pretty(&bridge.state()?)?);
            Ok(())
        }
        "run" => controller::run(&mut bridge),
        "craft" => {
            let _lock = bridge.lock()?;
            if bridge.runtime.join("stop-request.json").exists() {
                fs::remove_file(bridge.runtime.join("stop-request.json"))?;
            }
            let result = bridge.command(serde_json::json!({"action":"craft_pickaxe"}));
            let stopped = bridge.stop();
            let (outcome, _) = result?;
            println!("{}: {}", outcome.status, outcome.message);
            stopped?;
            if outcome.status != "done" {
                bail!("Crafting did not complete: {}", outcome.message);
            }
            Ok(())
        }
        "stop" => {
            bridge::atomic_json(
                &bridge.runtime.join("stop-request.json"),
                &serde_json::json!({"at":bridge::now()}),
            )?;
            bridge.stop()?;
            println!("Stopped.");
            Ok(())
        }
        "scan" => {
            let _lock = bridge.lock()?;
            if bridge.runtime.join("stop-request.json").exists() {
                fs::remove_file(bridge.runtime.join("stop-request.json"))?;
            }
            let result = (|| -> Result<()> {
                let (scan, state) = bridge.scan()?;
                let map = world::World::new(scan, &state)?;
                let mut book = bandit::Book::load(&bridge.runtime.join("bandit.json"))?;
                let plan = planner::plan(
                    &map,
                    &state,
                    book.model(&state.goal_key()),
                    &Default::default(),
                    &Default::default(),
                );
                println!(
                    "Loaded {} cells, {} block states. Preview: {plan:#?}",
                    map.scan.cells.len(),
                    map.scan.palette.len()
                );
                Ok(())
            })();
            let _ = bridge.stop();
            result
        }
        _ => bail!("Unknown command: {verb}; use --help"),
    }
}
