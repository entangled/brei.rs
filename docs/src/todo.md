To do
=====

The plan for the rest of the port of Brei (`../brei`, the Python version) to Rust. Last updated 2026-11-18, after adding program resolution.

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

Remaining work
--------------

In the suggested order. Items 1 to 4 are done.

### 5. Loading programs

`data.rs` and `program_loader.md` have the serde structs. These should become `Deserialize` for `tasks::Program`, `Call`, `Argument`, `Join`, `RuleTemplate` and `Runner` (`program.md`), and `data.rs` can then go. The loader for includes is the function passed to `tasks::resolve`.

- Read TOML from a file, or from a section of `pyproject.toml` (`read_from_file` in `construct.py`).
- Refuse unknown fields (Python uses `forbid_unknown_fields`).
- Convert into `Rule` and `Runner`: there are two `Runner` types now, the serde one in `data.rs` and `tasks::Runner`.
- Variable names used both as `var(x)` and `listvar(x)` give a `TypeMismatch` from `Targets::intern`. Report it as a user error that points at the rule. `ProgramError::Rule` doesn't say which rule yet.

### 6. CLI

`src/main.rs` is still "Hello, world". Options, following `cli.py`:

- target names;
- `-i` input file;
- `-B` force (`TaskDb.force_run`);
- `-j` jobs (the size of the `WorkerPool`);
- `--list-runners`, `--version` and `--debug`.

Probably use `clap`. Run with `WorkerPool` and `tasks::build_all`. Load the history before resolving the program (resolution may run rules) and save it after the build, also when either fails. Resolve and build with the same kernel.

### 7. Logging

Python logs a short note per rule (its description, name or targets), and forwards each command's stdout and stderr line by line, prefixed with that note. Here, output now goes straight to the terminal (`Stdio::inherit`).

### 8. Tests against Python Brei

Port the examples and tests in `../brei/test` as integration tests, to check that we behave the same as the original.

### 9. Housekeeping

- Remove the unused `futures` dependency. `toml` will be needed for item 5.
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
