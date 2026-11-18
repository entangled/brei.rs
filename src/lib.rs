// ~/~ begin <<docs/src/main_module.md#src/lib.rs>>[init]
//! Brei is a small but versatile workflow system. Workflows are written in TOML, YAML or JSON:
//! a list of tasks, which create targets from other targets by running a script. Tasks run
//! lazily and in parallel. See the [book](https://entangled.github.io/brei.rs) for the details.
//!
//! This library is for front ends: define a [`Program`], or read one with [`read_program`],
//! and [`run`] it. Brei logs through the [`log`](https://docs.rs/log) crate.
//!
//! ```no_run
//! use brei::{read_program, run, Options};
//!
//! let options = Options::default();
//! let program = read_program(&options.root, Some("brei.toml")).unwrap();
//! run(program, &["all"], &options).unwrap();
//! ```

#![warn(missing_docs)]

mod template_strings;
mod noodles;
mod tasks;
mod loader;

use std::fmt;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
use noodles::kernel::Kernel;
use noodles::task::Error as EngineError;
use noodles::worker_pool::WorkerPool;
use tasks::{Job, Key, Target, TargetId, TaskDb, HISTORY_FILE};

pub use loader::{read_program, LoadError};
pub use tasks::{Program, Call, Argument, Join, RuleTemplate, Runner, default_runners, kill_all};

// ~/~ begin <<docs/src/main_module.md#lib-options>>[init]
/// How to run a program.
#[derive(Clone, Debug)]
pub struct Options {
    /// The directory to run in. Paths in the program, and the history file, are relative to it.
    pub root: PathBuf,
    /// The number of jobs to run at the same time; by default, the number of CPUs.
    pub jobs: Option<NonZeroUsize>,
    /// Run all tasks, also those that are up to date.
    pub force_run: bool,
    /// Keep going after a failure, building everything that doesn't depend on it.
    pub keep_going: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { root: PathBuf::from("."), jobs: None, force_run: false, keep_going: false }
    }
}
// ~/~ end
// ~/~ begin <<docs/src/main_module.md#lib-error>>[init]
/// Why running a program failed. Each kind carries a message for people.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The history file can't be read or written.
    History(String),
    /// The program is wrong: a mistake in a task, a missing template or include, and so on.
    Program(String),
    /// A target to build isn't valid.
    Target(String),
    /// Some tasks failed, or wait for each other.
    Build(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::History(msg) | Error::Program(msg) | Error::Target(msg) | Error::Build(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for Error {}

fn describe(db: &TaskDb, error: EngineError<Job>) -> Error {
    Error::Build(match error {
        EngineError::Jobs(failures) => {
            let lines: Vec<String> = failures.iter().map(|f| format!("  - {f}")).collect();
            format!("some jobs have failed:\n{}", lines.join("\n"))
        }
        EngineError::Cycle(keys) => {
            let targets: Vec<String> = keys.iter().filter_map(|k| match k {
                Key::Target(id) => Some(format!("`{}`", db.targets.get(*id))),
                Key::Local(_) => None,
            }).collect();
            format!("cycle in dependencies: {}", targets.join(" -> "))
        }
        EngineError::Internal(e) => format!("internal error: {e}"),
    })
}
// ~/~ end
// ~/~ begin <<docs/src/main_module.md#lib-targets>>[init]
fn target(db: &TaskDb, name: &str) -> Result<TargetId, Error> {
    let phony = Target::Phony(name.to_string());
    let target = if db.creates(&phony) {
        phony
    } else {
        name.parse().map_err(|_| Error::Target("empty target name".into()))?
    };
    db.targets.intern(&target).map_err(|e| Error::Target(e.to_string()))
}
// ~/~ end
// ~/~ begin <<docs/src/main_module.md#lib-run>>[init]
/// Resolve `program`, and build `targets`: names of tasks, or other targets, like files or `var(name)`.
pub fn run(program: Program, targets: &[impl AsRef<str>], options: &Options) -> Result<(), Error> {
    let mut db = TaskDb::new(&options.root);
    db.force_run = options.force_run;
    let db = Arc::new(db);
    db.load_history(HISTORY_FILE).map_err(|e| Error::History(format!("can't read history: {e}")))?;

    let worker = match options.jobs {
        Some(jobs) => WorkerPool::new(jobs.get()),
        None => WorkerPool::with_available_parallelism(),
    };
    let mut kernel = Kernel::new();
    if options.keep_going {
        kernel = kernel.keep_going();
    }

    let result = tasks::resolve(&db, &mut kernel, &worker, program, &loader::load_include)
        .map_err(|e| Error::Program(e.to_string()))
        .and_then(|()| targets.iter().map(|t| target(&db, t.as_ref())).collect::<Result<Vec<_>, _>>())
        .and_then(|ids| tasks::build_all(&db, &mut kernel, &worker, &ids).map_err(|e| describe(&db, e)));

    let saved = db.save_history(HISTORY_FILE).map_err(|e| Error::History(format!("can't write history: {e}")));
    result.and(saved)
}
// ~/~ end

#[cfg(test)]
mod tests {
    use super::*;

    // ~/~ begin <<docs/src/main_module.md#lib-tests>>[init]
    fn hello() -> Program {
        let task = RuleTemplate {
            name: Some("hello".into()),
            stdout: Some("hello.txt".into()),
            script: Some("echo hello".into()),
            ..Default::default()
        };
        Program { task: vec![task], ..Default::default() }
    }

    #[cfg_attr(windows, ignore = "uses Unix tools")]
    #[test]
    fn run_program() {
        let dir = tempfile::TempDir::new().unwrap();
        let options = Options { root: dir.path().into(), ..Default::default() };
        run(hello(), &["hello"], &options).unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("hello.txt")).unwrap(), "hello\n");
        assert!(dir.path().join(HISTORY_FILE).exists());

        assert!(matches!(run(hello(), &["nothere"], &options), Err(Error::Build(_))));
        assert!(matches!(run(hello(), &[""], &options), Err(Error::Target(_))));
        let broken = Program { include: vec!["nothere.toml".into()], ..Default::default() };
        assert_eq!(run(broken, &["hello"], &options), Err(Error::Program("include `nothere.toml` not found".into())));
    }
    // ~/~ end
}
// ~/~ end
