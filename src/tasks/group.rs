// ~/~ begin <<docs/src/brei.md#brei::src/tasks/group.rs>>[init]
use std::collections::BTreeSet;
use std::io;
use std::process::{Child, Command};
use std::sync::Mutex;

struct Running {
    stopping: bool,
    groups: BTreeSet<u32>,
}

static RUNNING: Mutex<Running> = Mutex::new(Running { stopping: false, groups: BTreeSet::new() });

/// Start a command in a process group of its own.
pub(super) fn spawn(cmd: &mut Command) -> io::Result<Child> {
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(cmd, 0);
    let mut running = RUNNING.lock().unwrap();
    if running.stopping {
        return Err(io::Error::new(io::ErrorKind::Interrupted, "Brei is stopping"));
    }
    let child = cmd.spawn()?;
    running.groups.insert(child.id());
    Ok(child)
}

/// Forget about a command, after waiting for it.
pub(super) fn done(child: &Child) {
    RUNNING.lock().unwrap().groups.remove(&child.id());
}

/// Kill a command, and everything it started. Do this before waiting for it.
pub(super) fn kill(child: &mut Child) {
    #[cfg(unix)]
    kill_group(child.id());
    let _ = child.kill();
}

#[cfg(unix)]
fn kill_group(id: u32) {
    // SAFETY: `kill` only sends a signal; it has no memory safety requirements.
    unsafe {
        libc::kill(-(id as libc::pid_t), libc::SIGKILL);
    }
}

/// Kill all running commands, and don't start new ones. For when Brei is interrupted.
pub fn kill_all() {
    let mut running = RUNNING.lock().unwrap();
    running.stopping = true;
    #[cfg(unix)]
    for id in &running.groups {
        kill_group(*id);
    }
}
// ~/~ end
