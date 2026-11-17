To do
=====

The plan for the rest of the port of Brei (`../brei`, the Python version) to Rust. Last updated 2026-11-17, after commit `fd5cd35`.

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

Remaining work
--------------

In the suggested order.

### 1. Persistent history

Read and write the history file (JSON, mapping path to MD5 digest). The format should match Python's `TaskDb.read_history` and `write_history` in `../brei/brei/task.py`, so existing history files keep working. Right now `TaskDb.history` is a `Mutex<HashMap<PathBuf, String>>` that lives in memory only. Load it before a run and save it afterwards, also when the run fails: targets of failed rules have already been forgotten.

### 2. Building several targets

`tasks::build` takes a single target. The CLI needs to build several at once. Give the top-level promise one `Build` per target, plus a local root that depends on all of them. Until then, a phony target that requires the others does the same.

### 3. Variables and templates

From `TemplateVariable` and `TemplateTask` in `task.py`:

- An `environment` entry in a program is a variable whose value is a template string. It depends on the variables used in that template.
- A rule with `${...}` in its fields is a template task. It depends on the variables it uses, and when it runs, it substitutes them and becomes a real rule. This fits continuations naturally: the job substitutes, then continues with building the new rule's dependencies and running it.
- `splice(name)` in `requires` (or in call arguments) inserts the elements of a list variable. Spliced paths are new targets, interned at run time from worker threads. The `RwLock` on `Targets` is there for this.
- The `template_strings` module (`dollar`, `Env`, `Format`) does the substitution.
- Open question: rules are added to `TaskDb` through `&mut` before running, and the database is frozen in an `Arc` while running. Template tasks create rules at run time, so the rule index needs interior mutability (a `RwLock`), or template rules need to be kept apart from the frozen rules.

### 4. Program resolution

From `resolve_tasks` in `program.py`:

- `call`: expand templates, with `join` being inner or outer (`TemplateCall.all_args`).
- `include`: read further programs. An include may have to be built by a task first.
- Delayed calls, and templates with templated targets, are resolved once their variables are known.
- An error if targets remain unresolvable.

In Python this is a separate async pass before the build. It could also run as an engine computation, with includes and delayed templates as continuations.

### 5. Loading programs

`data.rs` and `program_loader.md` have the serde structs.

- Read TOML from a file, or from a section of `pyproject.toml` (`read_from_file` in `construct.py`).
- Refuse unknown fields (Python uses `forbid_unknown_fields`).
- Convert into `Rule` and `Runner`: there are two `Runner` types now, the serde one in `data.rs` and `tasks::Runner`.
- Variable names used both as `var(x)` and `listvar(x)` give a `TypeMismatch` from `Targets::intern`. Report it as a user error that points at the rule.

### 6. CLI

`src/main.rs` is still "Hello, world". Options, following `cli.py`:

- target names;
- `-i` input file;
- `-B` force (`TaskDb.force_run`);
- `-j` jobs (the size of the `WorkerPool`);
- `--list-runners`, `--version` and `--debug`.

Probably use `clap`. Run with `WorkerPool`.

### 7. Logging

Python logs a short note per rule (its description, name or targets), and forwards each command's stdout and stderr line by line, prefixed with that note. Here, output now goes straight to the terminal (`Stdio::inherit`).

### 8. Tests against Python Brei

Port the examples and tests in `../brei/test` as integration tests, to check that we behave the same as the original.

### 9. Housekeeping

- Remove the unused `futures` dependency. `serde_json` and `toml` will be needed for items 1 and 5.
- Maybe move to edition 2024, which allows `let` chains in the kernel.

Known limitations
-----------------

- **Killing commands:** a cancelled command is killed, but processes it started itself keep running. For example, cancelling a `bash` runner leaves its `sleep` running. Fixing this needs process groups, which is platform specific.
- **Local cycles:** cycles between local tasks within one promise aren't detected; the run ends with `NoResult`. Cycles through targets are detected.
- **Standard input:** a command without `stdin` reads from `/dev/null`, not from the terminal. This differs from Python, on purpose.
- **Empty variables:** a scalar variable with empty output is `""`, where Python gives `None`.
- **Skipped variable rules:** a rule that writes to a variable *and* creates files gives the value `Value::Nothing` when it is up to date and doesn't run. Python has the same problem.

Conventions
-----------

- The project is literate: edit the Markdown in `docs/src/`, never the files in `src/` directly. `entangled watch` tangles the code; run `uv run entangled tangle` if the watcher isn't running. Each chapter's code blocks use `//| file:` and `//| id:`, and chapters with a namespace in their front matter (`brei`, `template-strings`) keep their ids apart.
- Tests live in the chapters, next to the code they test.
