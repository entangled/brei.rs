To do
=====

The plan for the rest of the port of Brei (`../brei`, the Python version) to Rust. Last updated 2026-11-18, after porting the Python tests.

Done so far
-----------

- **Engine** (`engine.md`, `src/noodles/`): a kernel that walks the dependency graph with explicit continuations instead of async futures.
  - Frames, with tail-call optimisation.
  - Memoization of global keys (`Task::memoize`), with a queue for passing values on.
  - Cycle detection (`Error::Cycle`).
  - Fail-fast by default, or `Kernel::keep_going()` (like `make -k`), which returns `Error::Jobs` with all errors.
  - Cancellation through `CancelToken` and `Worker::cancel`.
  - Workers: `SingleWorker` and the thread pool `WorkerPool`.
  - Mock integer tasks and tests in `src/noodles/mock.rs`.
- **Targets** (`brei.md`, `src/tasks/target.rs`):
  - `Target::{Path, Phony, Variable(name, VarType)}`, where `VarType` is `Scalar` (`var(x)`) or `List` (`listvar(x)`).
  - Targets are interned in a shared `Targets` table, giving a `TargetId` that is `Copy`. Variables are indexed by name only, so using one name as both a scalar and a list gives a `TypeMismatch`.
  - Engine keys are `Key::{Target(TargetId), Local(u32)}`. Only target keys are memoized.
- **Rules and tasks** (`brei.md`, `src/tasks/`):
  - `Rule` (the Python `Task`) and the `TaskDb`, which holds the rules, the targets, the runners and the history.
  - `needs_run`, running commands (with or without a runner, and `stdin`/`stdout` to files or variables), and killing them on cancel.
  - Engine jobs: `Build` (memoized per target), `Run` and `Alias`. A rule with several targets runs once.
  - The entry point is `tasks::build`.
- **Persistent history** (`brei.md`, `src/tasks/history.rs`): `TaskDb::load_history` and `save_history` read and write a JSON file (`HISTORY_FILE`, `.brei_history`) that maps paths to digests. A missing file is an empty history; saving writes a temporary file and renames it. This is not compatible with Python history files that contain `null` digests.
- **Several targets** (`brei.md`, `src/tasks/job.rs`): `tasks::build_all` builds a list of targets in one run, through a local `Job::All` that waits for all of them. Its result is `()`; use `build` for the value of a single target.
- **Variables and templates** (`brei.md`, `src/tasks/template.rs`, `template_strings.md`):
  - The dollar parser now follows Python's `safe_substitute`: `$1`, `$(...)` and `${a b}` are left alone, so parsing never fails.
  - The database holds *recipes*: a rule (`Arc<Entry>`), a template (`RuleTemplate`, the Python `TaskProxy`), or an environment variable (`TaskDb::add_variable`).
  - `TaskDb::add_template` checks the template. One without variables or splices is expanded right away (so `$$` becomes `$`) and added as a rule.
  - A template's targets must be fixed; variables in targets are an error here, to be handled by program resolution (item 4).
  - The `Expand` job substitutes, splices, interns the new rule at run time and continues with building its dependencies and running it. Expanded rules never enter the database, which answers the open question about interior mutability: the rule index stays frozen.
- **Program resolution** (`program.md`, `src/tasks/program.rs`): `tasks::resolve` adds a `Program` (environment, runners, rules, templates, calls, includes) to the database, following Python's `resolve_tasks`.
  - Recipes and runners in `TaskDb` are behind a `RwLock` (`add*` take `&self`), so resolution can add recipes to a database that is already in an `Arc`.
  - Resolution builds variables and includes on the caller's kernel. Use the *same* kernel for the build afterwards: its memo keeps the values, so rules (think `mktemp -d`) don't run twice.
  - Delayed rules only get their targets filled in during resolution; the rest of the rule stays a template that is expanded when it runs.
  - `fill` substitutes some variables and leaves the others, keeping `$$` escapes intact. Python unescapes `$$` once per substitution, so a `$$` in a called template ends up as a variable there.
  - Included programs are read by a function `&dyn Fn(&Path) -> Result<Program, String>` that the caller passes in.
- **Loading programs** (`program_loader.md`, `src/loader.rs`): `Program`, `Call`, `Argument`, `Join`, `RuleTemplate` and `Runner` derive `Deserialize` and refuse unknown fields. `loader::read_program(dir, input)` reads `file[section]` (TOML or JSON), or finds `brei.toml` or `[tool.brei]` in `pyproject.toml`. `loader::load_include` is the include loader for `tasks::resolve`. Rule errors during resolution name the rule (`RuleTemplate::label`). `data.rs` is gone. All examples in `../brei/examples` deserialize.
- **CLI** (`main_module.md`, `src/main.rs`, `tests/cli.rs`): `clap`, with targets, `-i`, `-B`, `-j` (default: number of CPUs), `-k` (keep going), `-v`, `--list-runners` and `--debug`. A target name is `#name` if a rule has that name, and otherwise a file or `var(...)`. History is loaded before resolution and saved after the build, also on failure. Errors are logged, with exit code 1. The examples `rot13`, `include-gen`, `list_variables`, `template_multiplexing` and `hello-includes` run.
- **Logging** (`brei.md`, `main_module.md`): the library logs through the `log` crate (with the `kv` feature).
  - A rule that runs logs its note (description, `#name`, or ``creating `targets` ``) at target `brei::rule`. Each line of a command's stdout (unless redirected or captured) and stderr is logged at target `brei::output`, with the note as key-value `note`.
  - Debug messages: rules that are up to date, each command that runs, and steps in program resolution.
  - Failures from running or expanding a rule are wrapped in `Failure::Rule(note, failure)`, so error messages say which rule failed.
  - The binary has a small logger on stderr: notes in colour, backticks in bold, `--debug` for debug messages, and no colour off a terminal or with `NO_COLOR`.
- **Tests against Python Brei** (`compat.md`, `tests/python.rs`): the tests from `test_program.py`, `test_history.py`, `test_construct.py` and the file-level tests of `test_lazy.py`, and the examples from the Python documentation (except `force_run`, which needs `coverage`), run the `brei` binary. They need `python` and `git`; the Lua example is skipped without `lua`. The chapter lists the deliberate differences from Python. The `versioned_output` example makes a broken `output/latest` link; that is a mistake in the example itself, not in Brei.

Remaining work
--------------

In the suggested order. Items 1 to 8 are done.

### 9. Housekeeping

- Remove the unused `futures` dependency.
- Maybe move to edition 2024, which allows `let` chains in the kernel.

Known limitations
-----------------

- **Killing commands:** a cancelled command is killed, but processes it started itself keep running. For example, cancelling a `bash` runner leaves its `sleep` running. Fixing this needs process groups, which is platform specific.
- **Local cycles:** cycles between local tasks within one promise aren't detected; the run ends with `NoResult`. Cycles through targets are detected.
- **Standard input:** a command without `stdin` reads from `/dev/null`, not from the terminal. This differs from Python, on purpose.
- **Empty variables:** a scalar variable with empty output is `""`, where Python gives `None`.
- **Variables without a value:** a template that uses a variable whose value is `Value::Nothing` (see below) fails with "variable has no value".
- **Skipped variable rules:** a rule that writes to a variable *and* creates files gives the value `Value::Nothing` when it is up to date and doesn't run. Python has the same problem.

Conventions
-----------

- The project is literate: edit the Markdown in `docs/src/`, never the files in `src/` directly. `entangled watch` tangles the code; run `uv run entangled tangle` if the watcher isn't running. Each chapter's code blocks use `//| file:` and `//| id:`, and chapters with a namespace in their front matter (`brei`, `template-strings`) keep their ids apart.
- Tests live in the chapters, next to the code they test.
