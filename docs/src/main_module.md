Main Module
===========

The crate consists of a library and a binary. The binary is the `brei` command. The library is meant for other front ends, like the Python package: they can define a program, from a file or in code, and run it. The modules that make up Brei are private; the library exports only what is needed for that.

- A program is a `Program`, made of `RuleTemplate`s (tasks and templates), `Call`s and `Runner`s. These have public fields, and can be deserialized with `serde` from anything that looks like the [workflow format](./reference.md). `read_program` reads one from a file.
- `run` resolves a program and builds targets, with `Options` for the directory, the number of jobs, and so on. It fails with an `Error`, whose message is meant for people.
- `kill_all` kills the commands that are running, for when the front end is interrupted. `default_runners` gives the runners that come with Brei.

The library logs through the [`log`](https://docs.rs/log) crate; the front end decides how to show it.

The library
-----------

The crate documentation, the modules, and what is exported.

```rust
//| file: src/lib.rs
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

<<lib-options>>
<<lib-error>>
<<lib-targets>>
<<lib-run>>

#[cfg(test)]
mod tests {
    use super::*;

    <<lib-tests>>
}
```

### Options

The options of `run` are those of the command line.

```rust
//| id: lib-options
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
```

### Errors

Errors carry a message, so that the types behind them can stay private. Failures are listed one per line. A cycle is shown as the chain of targets that wait for each other.

```rust
//| id: lib-error
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
```

### Targets

Targets to build are usually the names of rules: `all` builds `#all`. A name that isn't the name of a rule is read as any other target, so `output.csv` builds a file, and `var(x)` computes a variable.

```rust
//| id: lib-targets
fn target(db: &TaskDb, name: &str) -> Result<TargetId, Error> {
    let phony = Target::Phony(name.to_string());
    let target = if db.creates(&phony) {
        phony
    } else {
        name.parse().map_err(|_| Error::Target("empty target name".into()))?
    };
    db.targets.intern(&target).map_err(|e| Error::Target(e.to_string()))
}
```

### Running a program

The history is loaded before resolving the program, since resolving may already run rules. It is saved afterwards, also when something failed: rules that succeeded don't need to run again. Resolving and building use the same kernel, so that what was computed during resolution is not computed again.

```rust
//| id: lib-run
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
```

A program can be defined in code, without a file.

```rust
//| id: lib-tests
fn hello() -> Program {
    let task = RuleTemplate {
        name: Some("hello".into()),
        stdout: Some("hello.txt".into()),
        script: Some("echo hello".into()),
        ..Default::default()
    };
    Program { task: vec![task], ..Default::default() }
}

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
```

The command line
----------------

The binary is the `brei` command-line program, on top of the library. It follows the options of the Python version, and adds `-k`, to keep going after a failure, like `make -k`.

```rust
//| file: src/main.rs
use std::io::IsTerminal;
use std::num::NonZeroUsize;
use std::path::Path;
use std::process::ExitCode;
use clap::Parser;
use brei::{default_runners, kill_all, read_program, run, Options};

/// Build one of the configured targets.
#[derive(Parser)]
#[command(name = "brei", disable_version_flag = true)]
struct Args {
    /// Names of the tasks to run, or other targets: files, or variables as `var(name)`.
    targets: Vec<String>,

    /// Brei TOML, YAML or JSON file; use a `[...]` suffix to indicate a section.
    #[arg(short, long)]
    input_file: Option<String>,

    /// Rebuild all dependencies.
    #[arg(short = 'B', long)]
    force_run: bool,

    /// Number of jobs to run at the same time [default: number of CPUs].
    #[arg(short, long)]
    jobs: Option<NonZeroUsize>,

    /// Keep going after a failure, building everything that doesn't depend on it.
    #[arg(short, long)]
    keep_going: bool,

    /// Print the version number and exit.
    #[arg(short, long)]
    version: bool,

    /// Show the default runners and exit.
    #[arg(long)]
    list_runners: bool,

    /// More verbose logging.
    #[arg(long)]
    debug: bool,
}

<<main-logger>>
<<main-run>>
```

### Logging

The library logs through the `log` crate, and this is where the log is shown, on standard error. When a rule runs, its note is shown in green. Every line of output of a command is shown with the note of its rule in yellow (the library passes it as the key-value pair `note`). Anything between backticks is shown in bold. Without `--debug`, debug messages are hidden. Colours are only used on a terminal, and not when `NO_COLOR` is set.

```rust
//| id: main-logger
struct Logger {
    level: log::LevelFilter,
    color: bool,
}

impl Logger {
    fn paint(&self, code: &str, text: &str) -> String {
        match self.color {
            true => format!("\x1b[{code}m{text}\x1b[0m"),
            false => text.to_string(),
        }
    }

    /// Show text in backticks in bold.
    fn highlight(&self, text: &str) -> String {
        let parts: Vec<&str> = text.split('`').collect();
        if !self.color || parts.len().is_multiple_of(2) {
            return text.to_string();
        }
        parts.iter().enumerate().map(|(i, part)| match i % 2 {
            1 => format!("\x1b[1m`{part}`\x1b[22m"),
            _ => part.to_string(),
        }).collect()
    }
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= self.level && metadata.target().starts_with("brei")
    }

    fn log(&self, record: &log::Record) {
        use log::Level;
        if !self.enabled(record.metadata()) {
            return;
        }
        let message = record.args().to_string();
        let line = match (record.level(), record.target()) {
            (Level::Info, "brei::rule") => self.paint("32", &message),
            (Level::Info, "brei::output") => {
                let note = record.key_values().get(log::kv::Key::from_str("note")).map(|v| v.to_string()).unwrap_or_default();
                format!("{} {message}", self.paint("33", &note))
            }
            (Level::Info, _) => self.highlight(&message),
            (Level::Debug | Level::Trace, _) => self.paint("2", &format!("debug: {message}")),
            (Level::Warn, _) => format!("{} {}", self.paint("33", "warning:"), self.highlight(&message)),
            (Level::Error, _) => format!("{} {}", self.paint("1;31", "error:"), self.highlight(&message)),
        };
        eprintln!("{line}");
    }

    fn flush(&self) {}
}

fn init_logger(debug: bool) {
    let level = if debug { log::LevelFilter::Debug } else { log::LevelFilter::Info };
    let color = std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    if log::set_logger(Box::leak(Box::new(Logger { level, color }))).is_ok() {
        log::set_max_level(level);
    }
}
```

### Main

Programs are read relative to the current directory, and so is the history. When Brei is interrupted (Ctrl-C, or `SIGTERM`), it kills the commands that are running, which run in [process groups](./brei.md#process-groups) of their own and wouldn't stop otherwise, and exits with code 130. The history isn't saved then.

```rust
//| id: main-run
fn list_runners() {
    let runners = default_runners();
    let mut names: Vec<&String> = runners.keys().collect();
    names.sort();
    for name in names {
        let runner = &runners[name];
        println!("{name:10} {} {}", runner.command, runner.args.join(" "));
    }
}

fn main() -> ExitCode {
    let args = Args::parse();
    if args.version {
        println!("Brei {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if args.list_runners {
        list_runners();
        return ExitCode::SUCCESS;
    }
    init_logger(args.debug);
    let interrupted = ctrlc::set_handler(|| {
        kill_all();
        log::error!("interrupted");
        std::process::exit(130);
    });
    if let Err(e) = interrupted {
        log::warn!("can't handle interrupts: {e}");
    }
    let options = Options { root: ".".into(), jobs: args.jobs, force_run: args.force_run, keep_going: args.keep_going };
    let result = read_program(Path::new("."), args.input_file.as_deref())
        .map_err(|e| e.to_string())
        .and_then(|program| run(program, &args.targets, &options).map_err(|e| e.to_string()));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            log::error!("{msg}");
            ExitCode::FAILURE
        }
    }
}
```

Tests
-----

These tests run the `brei` binary in a temporary directory.

```rust
//| file: tests/cli.rs
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn brei(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_brei")).args(args).current_dir(dir).output().unwrap()
}

fn setup(program: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("brei.toml"), program).unwrap();
    dir
}

fn read(dir: &TempDir, path: &str) -> String {
    fs::read_to_string(dir.path().join(path)).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

<<cli-tests>>
```

Building targets by name, or as a file. The second time, nothing runs, thanks to the history; with `-B` everything runs again.

```rust
//| id: cli-tests
const HELLO: &str = r#"
[[task]]
name = "hello"
creates = ["hello.txt"]
runner = "bash"
script = "echo run >> log.txt; echo hello > hello.txt"
"#;

#[test]
fn build() {
    let dir = setup(HELLO);
    assert!(brei(dir.path(), &["hello"]).status.success());
    assert_eq!(read(&dir, "hello.txt"), "hello\n");
    assert!(dir.path().join(".brei_history").exists());

    assert!(brei(dir.path(), &["hello.txt"]).status.success());
    assert_eq!(read(&dir, "log.txt"), "run\n");

    assert!(brei(dir.path(), &["-B", "hello"]).status.success());
    assert_eq!(read(&dir, "log.txt"), "run\nrun\n");
}
```

A failure gives an error message and exit code. With `-k`, the other targets are still built.

```rust
//| id: cli-tests
const FAILING: &str = r#"
[[task]]
name = "fail"
script = "false"

[[task]]
creates = ["ok.txt"]
script = "touch ok.txt"
"#;

#[test]
fn failure() {
    let dir = setup(FAILING);
    let output = brei(dir.path(), &["-k", "-j", "1", "fail", "ok.txt"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("`false` failed with exit code 1"), "{}", stderr(&output));
    assert!(dir.path().join("ok.txt").exists());

    let output = brei(dir.path(), &["nothere"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("missing dependency: `nothere`"), "{}", stderr(&output));
}
```

The output of commands is logged, with the note of the rule. With `--debug`, we also see which commands run.

```rust
//| id: cli-tests
#[test]
fn logging() {
    let dir = setup(r#"
        [[task]]
        name = "greet"
        script = "echo hello"

        [[task]]
        name = "complain"
        description = "complaining"
        runner = "bash"
        script = "echo oops >&2"
    "#);
    let output = brei(dir.path(), &["greet", "--debug"]);
    let log = stderr(&output);
    assert!(log.starts_with("#greet\n") && log.contains("#greet hello\n"), "{log}");
    assert!(log.contains("debug: #greet: running `echo hello`"), "{log}");
    assert!(output.stdout.is_empty());

    let output = brei(dir.path(), &["complain"]);
    assert!(stderr(&output).contains("complaining oops"), "{}", stderr(&output));
}
```

Input files, with a section, and errors in reading them.

```rust
//| id: cli-tests
#[test]
fn input_file() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("workflow.toml"), format!("[brei]\n{}", HELLO.replace("[[task]]", "[[brei.task]]"))).unwrap();
    assert!(brei(dir.path(), &["-i", "workflow.toml[brei]", "hello"]).status.success());
    assert_eq!(read(&dir, "hello.txt"), "hello\n");

    let output = brei(dir.path(), &["hello"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("no input file given"), "{}", stderr(&output));
}

#[test]
fn info() {
    let dir = TempDir::new().unwrap();
    let output = brei(dir.path(), &["--list-runners"]);
    assert!(String::from_utf8_lossy(&output.stdout).contains("bash"));
    let output = brei(dir.path(), &["-v"]);
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("Brei "));
}
```
