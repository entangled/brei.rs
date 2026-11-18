Main Module
===========

The crate consists of a library and a binary. The library exports the modules that make up Brei, so that their public items form its API.

```rust
//| file: src/lib.rs
pub mod template_strings;
pub mod noodles;
pub mod tasks;
pub mod loader;
```

The command line
----------------

The binary is the `brei` command-line program. It follows the options of the Python version, and adds `-k`, to keep going after a failure, like `make -k`.

```rust
//| file: src/main.rs
use std::io::IsTerminal;
use std::num::NonZeroUsize;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;
use clap::Parser;
use brei::loader;
use brei::noodles::kernel::Kernel;
use brei::noodles::task::Error;
use brei::noodles::worker_pool::WorkerPool;
use brei::tasks::{self, Job, Key, Target, TargetId, TaskDb, HISTORY_FILE};

/// Build one of the configured targets.
#[derive(Parser)]
#[command(name = "brei", disable_version_flag = true)]
struct Args {
    /// Names of the tasks to run, or other targets: files, or variables as `var(name)`.
    targets: Vec<String>,

    /// Brei TOML or JSON file; use a `[...]` suffix to indicate a section.
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
<<main-targets>>
<<main-errors>>
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
        if !self.color || parts.len() % 2 == 0 {
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

### Targets

On the command line, targets are usually the names of rules: `brei all` builds `#all`. A name that isn't the name of a rule is read as any other target, so `brei output.csv` builds a file, and `brei 'var(x)'` computes a variable.

```rust
//| id: main-targets
fn target(db: &TaskDb, name: &str) -> Result<TargetId, String> {
    let phony = Target::Phony(name.to_string());
    let target = if db.creates(&phony) {
        phony
    } else {
        name.parse().map_err(|_| "empty target name".to_string())?
    };
    db.targets.intern(&target).map_err(|e| e.to_string())
}
```

### Errors

Failures are listed one per line. A cycle is shown as the chain of targets that wait for each other.

```rust
//| id: main-errors
fn describe(db: &TaskDb, error: Error<Job>) -> String {
    match error {
        Error::Jobs(failures) => {
            let lines: Vec<String> = failures.iter().map(|f| format!("  - {f}")).collect();
            format!("some jobs have failed:\n{}", lines.join("\n"))
        }
        Error::Cycle(keys) => {
            let targets: Vec<String> = keys.iter().filter_map(|k| match k {
                Key::Target(id) => Some(format!("`{}`", db.targets.get(*id))),
                Key::Local(_) => None,
            }).collect();
            format!("cycle in dependencies: {}", targets.join(" -> "))
        }
        Error::Internal(e) => format!("internal error: {e:?}"),
    }
}
```

### Running

Programs are read relative to the current directory, and so is the history. The history is loaded before resolving the program, since resolving may already run rules. It is saved afterwards, also when something failed: rules that succeeded don't need to run again. Resolving and building use the same kernel, so that what was computed during resolution is not computed again.

```rust
//| id: main-run
fn run(args: &Args) -> Result<(), String> {
    let root = Path::new(".");
    let program = loader::read_program(root, args.input_file.as_deref()).map_err(|e| e.to_string())?;
    let mut db = TaskDb::new(root);
    db.force_run = args.force_run;
    let db = Arc::new(db);
    db.load_history(HISTORY_FILE).map_err(|e| format!("can't read history: {e}"))?;

    let jobs = args.jobs.or_else(|| thread::available_parallelism().ok()).map_or(1, NonZeroUsize::get);
    let worker = WorkerPool::new(jobs);
    let mut kernel = Kernel::new();
    if args.keep_going {
        kernel = kernel.keep_going();
    }

    let result = tasks::resolve(&db, &mut kernel, &worker, program, &loader::load_include)
        .map_err(|e| e.to_string())
        .and_then(|()| args.targets.iter().map(|t| target(&db, t)).collect::<Result<Vec<_>, _>>())
        .and_then(|ids| tasks::build_all(&db, &mut kernel, &worker, &ids).map_err(|e| describe(&db, e)));

    let saved = db.save_history(HISTORY_FILE).map_err(|e| format!("can't write history: {e}"));
    result.and(saved)
}

fn list_runners() {
    let db = TaskDb::new(".");
    for name in db.runner_names() {
        let runner = db.runner(&name).expect("runner exists");
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
    match run(&args) {
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
