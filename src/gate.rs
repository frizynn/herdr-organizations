//! `gate run`: one queue per machine for heavy commands (full test suites,
//! production builds, browser runs), so parallel threads on one computer take
//! turns instead of fighting over memory. A slot is an exclusive lock on a
//! file under `<root>/.gates/`; the operating system frees it when the holder
//! exits, so a killed thread never leaves the queue stuck. Before starting,
//! the command also waits while the kernel reports memory pressure.

use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::paths::Ctx;
use crate::runner::{Cmd, Runner};

const POLL: Duration = Duration::from_secs(2);

/// What the kernel says about memory: only `Pressure` holds a gate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Memory {
    Normal,
    Pressure,
    Unknown,
}

/// macOS `kern.memorystatus_vm_pressure_level`: 1 normal, 2 warn, 4 critical.
pub fn parse_macos_level(text: &str) -> Memory {
    match text.trim() {
        "1" => Memory::Normal,
        "2" | "4" => Memory::Pressure,
        _ => Memory::Unknown,
    }
}

/// Linux PSI (`/proc/pressure/memory`): pressure once tasks stalled on
/// memory for more than `threshold` percent of the last ten seconds.
pub fn parse_linux_psi(text: &str, threshold: f64) -> Memory {
    let avg10 = text
        .lines()
        .find(|l| l.starts_with("some "))
        .and_then(|l| l.split_whitespace().find_map(|f| f.strip_prefix("avg10=")))
        .and_then(|v| v.parse::<f64>().ok());
    match avg10 {
        Some(v) if v > threshold => Memory::Pressure,
        Some(_) => Memory::Normal,
        None => Memory::Unknown,
    }
}

pub fn memory(runner: &dyn Runner) -> Memory {
    if cfg!(target_os = "macos") {
        let cmd = Cmd::new("sysctl", Duration::from_secs(5))
            .args(["-n", "kern.memorystatus_vm_pressure_level"]);
        return match runner.run(&cmd) {
            Ok(out) if out.success() => parse_macos_level(&out.stdout),
            _ => Memory::Unknown,
        };
    }
    std::fs::read_to_string("/proc/pressure/memory")
        .map(|text| parse_linux_psi(&text, 10.0))
        .unwrap_or(Memory::Unknown)
}

fn slot_path(dir: &Path, name: &str, slot: usize) -> PathBuf {
    dir.join(format!("{name}-{slot}.lock"))
}

/// Takes the first free slot, or `None` when all are held.
fn try_slot(dir: &Path, name: &str, slots: usize) -> Result<Option<File>> {
    for slot in 0..slots {
        let path = slot_path(dir, name, slot);
        let file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| format!("could not open {}", path.display()))?;
        if file.try_lock().is_ok() {
            return Ok(Some(file));
        }
    }
    Ok(None)
}

/// Who holds the slots, from what each holder wrote into its lock file.
fn holders(dir: &Path, name: &str, slots: usize) -> String {
    (0..slots)
        .filter_map(|slot| std::fs::read_to_string(slot_path(dir, name, slot)).ok())
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}

/// Runs `command` once a slot of the `name` queue is free and memory is not
/// under pressure, and returns its exit code. Waiting is announced once on
/// standard error, so a thread's log says why it was slow.
pub fn run(ctx: &Ctx, name: &str, slots: usize, command: &[String]) -> Result<i32> {
    if slots == 0 {
        bail!("--slots must be at least 1");
    }
    crate::project::validate_slug(name).context("--name must be a short slug")?;
    let Some((program, args)) = command.split_first() else {
        bail!("nothing to run: pass the command after `--`");
    };
    let dir = ctx.root.join(".gates");
    std::fs::create_dir_all(&dir).with_context(|| format!("could not create {}", dir.display()))?;

    let mut said_queue = false;
    let mut file = loop {
        if let Some(taken) = try_slot(&dir, name, slots)? {
            break taken;
        }
        if !said_queue {
            eprintln!(
                "gate {name}: waiting for one of {slots} slot(s), held by: {}",
                holders(&dir, name, slots)
            );
            said_queue = true;
        }
        std::thread::sleep(POLL);
    };
    let mut said_memory = false;
    while memory(ctx.runner) == Memory::Pressure {
        if !said_memory {
            eprintln!("gate {name}: waiting while the system reports memory pressure");
            said_memory = true;
        }
        std::thread::sleep(POLL);
    }
    let holder = format!(
        "pid {} since {} in {}: {}",
        std::process::id(),
        crate::project::now(),
        std::env::current_dir().unwrap_or_default().display(),
        command.join(" ")
    );
    file.set_len(0)?;
    file.write_all(holder.as_bytes())?;

    // The gated command streams to the caller's terminal, so it is spawned
    // directly rather than through `Runner`, which captures output.
    let status = std::process::Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("could not run {program}"))?;
    let _ = file.set_len(0);
    Ok(status.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_signals_parse_and_only_pressure_holds() {
        assert_eq!(parse_macos_level("1\n"), Memory::Normal);
        assert_eq!(parse_macos_level("2"), Memory::Pressure);
        assert_eq!(parse_macos_level("4"), Memory::Pressure);
        assert_eq!(parse_macos_level(""), Memory::Unknown);
        let psi = "some avg10=12.50 avg60=3.00 avg300=1.00 total=1\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=0\n";
        assert_eq!(parse_linux_psi(psi, 10.0), Memory::Pressure);
        assert_eq!(parse_linux_psi(psi, 20.0), Memory::Normal);
        assert_eq!(parse_linux_psi("garbage", 10.0), Memory::Unknown);
    }

    #[test]
    fn a_held_slot_is_skipped_and_freed_when_its_holder_goes() {
        let dir = tempfile::tempdir().unwrap();
        let first = try_slot(dir.path(), "heavy", 2).unwrap().unwrap();
        let second = try_slot(dir.path(), "heavy", 2).unwrap().unwrap();
        assert!(try_slot(dir.path(), "heavy", 2).unwrap().is_none());
        drop(first);
        // Another test's child process can share the descriptor for the
        // instant between its fork and exec, so the slot frees promptly, not
        // necessarily at once.
        let freed = (0..50).any(|_| {
            let taken = try_slot(dir.path(), "heavy", 2).unwrap().is_some();
            if !taken {
                std::thread::sleep(Duration::from_millis(20));
            }
            taken
        });
        assert!(freed);
        drop(second);
    }
}
