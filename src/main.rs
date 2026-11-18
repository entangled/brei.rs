// ~/~ begin <<docs/src/main_module.md#src/main.rs>>[init]
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

// ~/~ begin <<docs/src/main_module.md#main-logger>>[init]
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
// ~/~ end
// ~/~ begin <<docs/src/main_module.md#main-run>>[init]
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
// ~/~ end
// ~/~ end
