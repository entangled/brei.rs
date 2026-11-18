// ~/~ begin <<docs/src/main_module.md#src/main.rs>>[init]
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

// ~/~ begin <<docs/src/main_module.md#main-targets>>[init]
fn target(db: &TaskDb, name: &str) -> Result<TargetId, String> {
    let phony = Target::Phony(name.to_string());
    let target = if db.creates(&phony) {
        phony
    } else {
        name.parse().map_err(|_| "empty target name".to_string())?
    };
    db.targets.intern(&target).map_err(|e| e.to_string())
}
// ~/~ end
// ~/~ begin <<docs/src/main_module.md#main-errors>>[init]
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
// ~/~ end
// ~/~ begin <<docs/src/main_module.md#main-run>>[init]
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
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("brei: {msg}");
            ExitCode::FAILURE
        }
    }
}
// ~/~ end
// ~/~ end
