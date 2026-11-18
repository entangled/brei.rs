---
entangled:
  version: "2.7"
  namespace: brei
---

Brei
====

This chapter builds Brei on top of the [engine](./engine.md): the tasks that Brei runs, what they produce, and how they find each other. The code lives in the `tasks` module.

```rust
//| file: src/tasks/mod.rs
//! Brei tasks, running on the engine in `noodles`.

mod target;
mod rule;
mod db;
mod template;
mod history;
mod exec;
mod job;

#[cfg(test)]
mod tests;

pub use target::{Target, VarType, TargetId, Targets, Key, EmptyTarget, TypeMismatch};
pub use rule::{Rule, RuleError, Runner, Value, Failure, default_runners};
pub use db::TaskDb;
pub use template::RuleTemplate;
pub use history::HISTORY_FILE;
pub use job::{Job, build, build_all};
```

Targets
-------

Everything a task creates or requires is a *target*. There are three kinds of target:

- a **path**, a file on disk, written as the path itself: `data/output.csv`;
- a **phony** target, a name for a task that doesn't create a file, written with a hash: `#all`;
- a **variable**, a value computed by a task. A variable has a type, which is one of:
  - a **scalar**, a single string, written as `var(name)`. It can be used in templates as `${name}`;
  - a **list** of strings, written as `listvar(name)`. The output of a task with `listvar(name)` as `stdout` is split into lines. A list can't be used in a template directly; instead `splice(name)` inserts its elements into a list of dependencies or call arguments.

Scalar and list variables share the same name space: `var(x)` and `listvar(x)` refer to the same variable, used in two incompatible ways. That is an error, which is caught when targets are [interned](#interning).

```rust
//| file: src/tasks/target.rs
use std::collections::HashMap;
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use std::sync::RwLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VarType {
    Scalar,
    List,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    Path(PathBuf),
    Phony(String),
    Variable(String, VarType),
}

<<target-parse>>
<<target-intern>>
<<target-key>>

#[cfg(test)]
mod tests {
    use super::*;

    <<target-tests>>
}
```

### Parsing

Targets are written as strings in a Brei program. A string that doesn't look like any of the other kinds is a path. Variable names can't contain white space or parentheses.

Paths are normalized a little bit, so that `./data.csv` and `data.csv` are the same target. We only remove `.` components; `..` is left alone, since `a/../b` is not the same as `b` when `a` is a symbolic link.

```rust
//| id: target-parse
pub(super) fn wrapped<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let name = s.strip_prefix(prefix)?.strip_prefix('(')?.strip_suffix(')')?;
    let valid = !name.is_empty() && !name.contains(|c: char| c.is_whitespace() || c == '(' || c == ')');
    valid.then_some(name)
}

fn normalize(path: &Path) -> PathBuf {
    let normal: PathBuf = path.components().filter(|c| *c != Component::CurDir).collect();
    if normal.as_os_str().is_empty() { PathBuf::from(".") } else { normal }
}

impl Target {
    /// A path target, normalized.
    pub fn path(path: impl AsRef<Path>) -> Self {
        Target::Path(normalize(path.as_ref()))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct EmptyTarget;

impl FromStr for Target {
    type Err = EmptyTarget;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(EmptyTarget);
        }
        Ok(if let Some(name) = s.strip_prefix('#') {
            Target::Phony(name.to_string())
        } else if let Some(name) = wrapped(s, "var") {
            Target::Variable(name.to_string(), VarType::Scalar)
        } else if let Some(name) = wrapped(s, "listvar") {
            Target::Variable(name.to_string(), VarType::List)
        } else {
            Target::path(s)
        })
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Target::Path(p) => write!(f, "{}", p.display()),
            Target::Phony(name) => write!(f, "#{}", name),
            Target::Variable(name, VarType::Scalar) => write!(f, "var({})", name),
            Target::Variable(name, VarType::List) => write!(f, "listvar({})", name),
        }
    }
}
```

```rust
//| id: target-tests
fn target(s: &str) -> Target {
    s.parse().unwrap()
}

#[test]
fn parse() {
    assert_eq!(target("data/output.csv"), Target::Path("data/output.csv".into()));
    assert_eq!(target("#all"), Target::Phony("all".into()));
    assert_eq!(target("var(name)"), Target::Variable("name".into(), VarType::Scalar));
    assert_eq!(target("listvar(files)"), Target::Variable("files".into(), VarType::List));
    assert_eq!("".parse::<Target>(), Err(EmptyTarget));
}

#[test]
fn parse_edge_cases() {
    // not a valid variable name, so these are paths
    assert_eq!(target("var(a b)"), Target::Path("var(a b)".into()));
    assert_eq!(target("var()"), Target::Path("var()".into()));
    assert_eq!(target("variable(x)"), Target::Path("variable(x)".into()));
    // normalization
    assert_eq!(target("./data.csv"), target("data.csv"));
    assert_eq!(target("data//output.csv"), target("data/output.csv"));
    assert_eq!(target("data/./output.csv"), target("data/output.csv"));
    assert_ne!(target("a/../b"), target("b"));
    assert_eq!(target("./"), Target::Path(".".into()));
}

#[test]
fn display_round_trip() {
    for s in ["data/output.csv", "#all", "var(name)", "listvar(files)"] {
        assert_eq!(target(s).to_string(), s);
    }
}
```

### Interning

The engine wants keys that are `Copy`, and they get copied a lot: into the memo, the waits-for graph and the delivery queue. A `Target` holds a string, so it isn't `Copy`. Instead, every target gets a number, a `TargetId`, from a table that is shared by all tasks. Looking up the same target twice gives the same id.

New targets can appear while the workflow is running, for instance when a list variable is spliced into a list of dependencies. Those tasks run on worker threads, so the table is behind a `RwLock`. Most of the time we are only reading.

The table indexes targets by their *name*, which for a variable leaves out the type. So `var(x)` and `listvar(x)` get the same id, and when a variable comes in with a different type than the one that is already in the table, `intern` gives a `TypeMismatch`. The type that is stored is the one that was seen first.

```rust
//| id: target-intern
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TargetId(u32);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Name {
    Path(PathBuf),
    Phony(String),
    Variable(String),
}

impl From<&Target> for Name {
    fn from(target: &Target) -> Self {
        match target {
            Target::Path(p) => Name::Path(p.clone()),
            Target::Phony(n) => Name::Phony(n.clone()),
            Target::Variable(n, _) => Name::Variable(n.clone()),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct TypeMismatch {
    pub name: String,
    pub known: VarType,
    pub found: VarType,
}

impl fmt::Display for TypeMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.known {
            VarType::List => write!(f, "`{}` is a list variable, it can only be used as `splice({})`", self.name, self.name),
            VarType::Scalar => write!(f, "`{}` is not a list variable, use `${{{}}}` instead", self.name, self.name),
        }
    }
}

#[derive(Default)]
struct Table {
    targets: Vec<Target>,
    index: HashMap<Name, TargetId>,
}

impl Table {
    /// Check that `target` agrees with the target that is known under `id`.
    fn check(&self, id: TargetId, target: &Target) -> Result<TargetId, TypeMismatch> {
        match (&self.targets[id.0 as usize], target) {
            (Target::Variable(name, known), Target::Variable(_, found)) if known != found =>
                Err(TypeMismatch { name: name.clone(), known: *known, found: *found }),
            _ => Ok(id),
        }
    }
}

#[derive(Default)]
pub struct Targets {
    table: RwLock<Table>,
}

impl Targets {
    pub fn new() -> Self {
        Self::default()
    }

    /// The id of `target`, adding it to the table if it is new.
    pub fn intern(&self, target: &Target) -> Result<TargetId, TypeMismatch> {
        let name = Name::from(target);
        {
            let table = self.table.read().unwrap();
            if let Some(id) = table.index.get(&name) {
                return table.check(*id, target);
            }
        }
        let mut table = self.table.write().unwrap();
        // Another thread may have added it in the mean time.
        if let Some(id) = table.index.get(&name) {
            return table.check(*id, target);
        }
        let id = TargetId(table.targets.len().try_into().expect("too many targets"));
        table.targets.push(target.clone());
        table.index.insert(name, id);
        Ok(id)
    }

    /// The id of `target`, if it is known (with the same type).
    pub fn lookup(&self, target: &Target) -> Option<TargetId> {
        let table = self.table.read().unwrap();
        let id = *table.index.get(&Name::from(target))?;
        table.check(id, target).ok()
    }

    pub fn get(&self, id: TargetId) -> Target {
        self.table.read().unwrap().targets[id.0 as usize].clone()
    }

    pub fn len(&self) -> usize {
        self.table.read().unwrap().targets.len()
    }
}
```

```rust
//| id: target-tests
#[test]
fn interning() {
    let targets = Targets::new();
    let a = targets.intern(&target("a.txt")).unwrap();
    let b = targets.intern(&target("#b")).unwrap();
    assert_ne!(a, b);
    assert_eq!(targets.intern(&target("./a.txt")), Ok(a));
    assert_eq!(targets.lookup(&target("var(c)")), None);
    assert_eq!(targets.get(b), target("#b"));
    assert_eq!(targets.len(), 2);
}

#[test]
fn interning_from_threads() {
    let targets = std::sync::Arc::new(Targets::new());
    let handles: Vec<_> = (0..8).map(|_| {
        let targets = targets.clone();
        std::thread::spawn(move || {
            (0..100).map(|i| targets.intern(&target(&format!("file{i}.txt"))).unwrap()).collect::<Vec<_>>()
        })
    }).collect();
    let ids: Vec<Vec<TargetId>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert!(ids.iter().all(|v| *v == ids[0]));
    assert_eq!(targets.len(), 100);
}

#[test]
fn variable_types() {
    let targets = Targets::new();
    let files = targets.intern(&target("listvar(files)")).unwrap();
    assert_eq!(targets.intern(&target("listvar(files)")), Ok(files));
    assert_eq!(targets.lookup(&target("var(files)")), None);

    let err = targets.intern(&target("var(files)")).unwrap_err();
    assert_eq!(err, TypeMismatch { name: "files".into(), known: VarType::List, found: VarType::Scalar });
    assert_eq!(err.to_string(), "`files` is a list variable, it can only be used as `splice(files)`");

    targets.intern(&target("var(name)")).unwrap();
    let err = targets.intern(&target("listvar(name)")).unwrap_err();
    assert_eq!(err.to_string(), "`name` is not a list variable, use `${name}` instead");
    assert_eq!(targets.len(), 2);
}
```

### Engine keys

Not every task in the engine is a target. Running a Brei task takes a few steps: first its dependencies need to be there, then we check whether the targets are out of date, and only then do we run the script. These intermediate steps live inside a single continuation, and get a local key. Targets are global: their tasks are memoized, so that every target is built at most once, and they can be shared between frames. Local keys are only unique within their frame.

```rust
//| id: target-key
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Target(TargetId),
    Local(u32),
}

impl Key {
    /// Whether tasks with this key should be memoized by the engine.
    pub fn is_global(&self) -> bool {
        matches!(self, Key::Target(_))
    }
}

impl From<TargetId> for Key {
    fn from(id: TargetId) -> Self {
        Key::Target(id)
    }
}
```

```rust
//| id: target-tests
#[test]
fn keys() {
    let targets = Targets::new();
    let key: Key = targets.intern(&target("a.txt")).unwrap().into();
    assert!(key.is_global());
    assert!(!Key::Local(0).is_global());
}
```

Rules
-----

A *rule* says how to create a set of targets from a set of dependencies. This is what the Python version calls a `Task`; here that name is taken by the engine. A rule has:

- `creates`, the targets it creates, and `requires`, the targets it needs;
- optionally a `name`, which makes the rule available as the phony target `#name`;
- something to run: a `script`, optionally with a `runner`, or a `path` to a script file, which needs a runner. Without a runner, every line of the script is a command. With a runner, the script is written to a file, and the runner is called with that file;
- optionally `stdin` and `stdout`, which redirect standard input and output to a file or variable. Standard input can only come from a file or a scalar variable; standard output can go to a file, a scalar variable or a list variable;
- a `description`, used for logging, and `force`, to always run the rule.

The targets in `stdout` and `name`, and the dependencies in `stdin` and `path`, are added to `creates` and `requires` when the rule is added to the database. A rule that doesn't create any file is always run, so that is the case for rules that only have phony or variable targets.

```rust
//| file: src/tasks/rule.rs
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;
use super::target::{Target, VarType, TypeMismatch};

<<rule-value>>
<<rule-failure>>
<<rule-runner>>
<<rule-rule>>
```

### Values

The engine passes values from one task to the next. For a file or phony target there's nothing to pass: whatever a rule produced is on disk. Variables carry a string or a list of strings.

```rust
//| id: rule-value
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Nothing,
    Scalar(String),
    List(Vec<String>),
}
```

### Failures

When a rule fails, the engine collects the error. These need to be `Clone`, so we store messages instead of the original errors.

```rust
//| id: rule-failure
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    Missing(Target),
    UnknownRunner(String),
    InvalidScript(String),
    Template(String),
    Io(String),
    Command { command: String, code: Option<i32> },
    NotAchieved(Vec<Target>),
    Cancelled,
}

impl From<std::io::Error> for Failure {
    fn from(e: std::io::Error) -> Self {
        Failure::Io(e.to_string())
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::Missing(t) => write!(f, "missing dependency: `{t}`"),
            Failure::UnknownRunner(r) => write!(f, "unknown runner: `{r}`"),
            Failure::InvalidScript(line) => write!(f, "can't parse command: `{line}`"),
            Failure::Template(msg) => write!(f, "error in template: {msg}"),
            Failure::Io(msg) => write!(f, "{msg}"),
            Failure::Command { command, code: Some(code) } => write!(f, "`{command}` failed with exit code {code}"),
            Failure::Command { command, code: None } => write!(f, "`{command}` was killed"),
            Failure::NotAchieved(targets) => {
                let targets: Vec<String> = targets.iter().map(|t| format!("`{t}`")).collect();
                write!(f, "rule didn't create {}", targets.join(", "))
            }
            Failure::Cancelled => write!(f, "cancelled"),
        }
    }
}
```

### Runners

A runner is a command that runs a script. Its arguments are template strings, where `${script}` is replaced with the path of the script. Brei comes with runners for Python and Bash; a program can add more.

```rust
//| id: rule-runner
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Runner {
    pub command: String,
    pub args: Vec<String>,
}

pub fn default_runners() -> HashMap<String, Runner> {
    let runner = |command: &str| Runner { command: command.into(), args: vec!["${script}".into()] };
    HashMap::from([
        ("python".into(), runner("python")),
        ("bash".into(), runner("bash")),
    ])
}
```

### The rule

The digest of a rule is a hash of its script. It is stored in the history after the rule was run, so that we notice when the script changed. We use MD5, like the Python version, to be able to read the same history files. Rules that run a script file don't have a digest; for those, the script file is a dependency.

```rust
//| id: rule-rule
#[derive(Clone, Debug, Default)]
pub struct Rule {
    pub creates: Vec<Target>,
    pub requires: Vec<Target>,
    pub name: Option<String>,
    pub runner: Option<String>,
    pub path: Option<PathBuf>,
    pub script: Option<String>,
    pub stdin: Option<Target>,
    pub stdout: Option<Target>,
    pub description: Option<String>,
    pub force: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RuleError {
    TypeMismatch(TypeMismatch),
    Duplicate(Target),
    Invalid(String),
}

impl From<TypeMismatch> for RuleError {
    fn from(e: TypeMismatch) -> Self {
        RuleError::TypeMismatch(e)
    }
}

impl fmt::Display for RuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuleError::TypeMismatch(e) => write!(f, "{e}"),
            RuleError::Duplicate(t) => write!(f, "`{t}` is created by more than one rule"),
            RuleError::Invalid(msg) => write!(f, "{msg}"),
        }
    }
}

fn dedup(targets: &mut Vec<Target>) {
    let mut seen = HashSet::new();
    targets.retain(|t| seen.insert(t.clone()));
}

impl Rule {
    pub fn target_paths(&self) -> impl Iterator<Item=&PathBuf> {
        self.creates.iter().filter_map(|t| match t { Target::Path(p) => Some(p), _ => None })
    }

    pub fn dependency_paths(&self) -> impl Iterator<Item=&PathBuf> {
        self.requires.iter().filter_map(|t| match t { Target::Path(p) => Some(p), _ => None })
    }

    pub fn digest(&self) -> Option<String> {
        self.script.as_ref().map(|s| format!("{:x}", md5::compute(s)))
    }

    pub fn always_run(&self) -> bool {
        self.force || self.target_paths().next().is_none()
    }

    /// Add implied targets and dependencies, and check that the rule makes sense.
    pub(super) fn complete(mut self) -> Result<Rule, RuleError> {
        if let Some(name) = &self.name {
            self.creates.push(Target::Phony(name.clone()));
        }
        if let Some(stdin) = &self.stdin {
            self.requires.push(stdin.clone());
        }
        if let Some(path) = &self.path {
            self.requires.push(Target::path(path));
        }
        if let Some(stdout) = &self.stdout {
            self.creates.push(stdout.clone());
        }
        dedup(&mut self.creates);
        dedup(&mut self.requires);

        let invalid = |msg: String| Err(RuleError::Invalid(msg));
        match &self.stdin {
            None | Some(Target::Path(_)) | Some(Target::Variable(_, VarType::Scalar)) => {}
            Some(t) => return invalid(format!("`stdin` can't be `{t}`")),
        }
        if let Some(t @ Target::Phony(_)) = &self.stdout {
            return invalid(format!("`stdout` can't be `{t}`"));
        }
        match (&self.runner, &self.script, &self.path) {
            (_, Some(_), Some(_)) => invalid("a rule can't have both `script` and `path`".into()),
            (None, None, Some(_)) => invalid("a rule with a `path` needs a `runner`".into()),
            (Some(_), None, None) => invalid("a rule with a `runner` needs a `script` or `path`".into()),
            (None, Some(script), None) if script.lines().count() > 1 && self.stdin.is_some() =>
                invalid("a script of more than one line can't have `stdin`, use a runner".into()),
            _ => Ok(self),
        }
    }
}
```

The task database
-----------------

The task database holds everything the tasks share while running: the table of targets, the recipes, and which recipe creates which target. Commands run in the `root` directory, and paths are relative to it.

A *recipe* is anything that creates targets. There are three kinds:

- a rule, ready to run;
- a [template](#templates): a rule that uses variables. It is expanded into a rule once the values of those variables are known;
- a variable from the environment, whose value is a template string.

Each recipe knows the targets it creates, and what it requires before it can be used. For a rule, those are its dependencies. For a template or an environment variable, they are only the variables it uses: the dependencies of a template rule are only known once it is expanded.

A rule is stored as an `Entry`, with its targets interned. Entries are shared through an `Arc`, because rules that are expanded from a template don't live in the database, only in the jobs that run them.

The history remembers the digest of the rule that last created each file. It is shared by all tasks, which may run on different threads, so it is behind a `Mutex`. It is a `BTreeMap`, so that it is written to disk in a stable order.

Recipes are added before running anything, so adding takes a `&mut` reference; once all recipes are in, the database goes into an `Arc` and is shared with the tasks. A target can only be created by one recipe.

```rust
//| file: src/tasks/db.rs
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use super::rule::{Rule, RuleError, Runner, default_runners};
use super::target::{Target, TargetId, Targets};
use super::template::RuleTemplate;

pub struct Entry {
    pub(super) rule: Rule,
    pub(super) creates: Vec<TargetId>,
    pub(super) requires: Vec<TargetId>,
    pub(super) stdin: Option<TargetId>,
}

impl Entry {
    pub(super) fn new(rule: Rule, targets: &Targets) -> Result<Self, RuleError> {
        let rule = rule.complete()?;
        let intern = |ts: &[Target]| ts.iter().map(|t| targets.intern(t)).collect::<Result<Vec<_>, _>>();
        let creates = intern(&rule.creates)?;
        let requires = intern(&rule.requires)?;
        let stdin = match &rule.stdin {
            Some(t @ Target::Variable(..)) => targets.lookup(t),
            _ => None,
        };
        Ok(Entry { rule, creates, requires, stdin })
    }
}

pub(super) enum Kind {
    Rule(Arc<Entry>),
    Template(RuleTemplate),
    Variable(String),
}

pub(super) struct Recipe {
    pub(super) creates: Vec<TargetId>,
    pub(super) requires: Vec<TargetId>,
    pub(super) kind: Kind,
}

pub struct TaskDb {
    pub root: PathBuf,
    pub targets: Targets,
    pub runners: HashMap<String, Runner>,
    pub force_run: bool,
    recipes: Vec<Recipe>,
    index: HashMap<TargetId, usize>,
    pub(super) history: Mutex<BTreeMap<PathBuf, String>>,
}

impl TaskDb {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        TaskDb {
            root: root.into(),
            targets: Targets::new(),
            runners: default_runners(),
            force_run: false,
            recipes: Vec::new(),
            index: HashMap::new(),
            history: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn add(&mut self, rule: Rule) -> Result<(), RuleError> {
        let entry = Entry::new(rule, &self.targets)?;
        self.insert(Recipe { creates: entry.creates.clone(), requires: entry.requires.clone(), kind: Kind::Rule(Arc::new(entry)) })
    }

    pub(super) fn insert(&mut self, recipe: Recipe) -> Result<(), RuleError> {
        if let Some(id) = recipe.creates.iter().find(|id| self.index.contains_key(id)) {
            return Err(RuleError::Duplicate(self.targets.get(*id)));
        }
        let n = self.recipes.len();
        for id in &recipe.creates {
            self.index.insert(*id, n);
        }
        self.recipes.push(recipe);
        Ok(())
    }

    pub(super) fn recipe_for(&self, target: TargetId) -> Option<usize> {
        self.index.get(&target).copied()
    }

    pub(super) fn recipe(&self, n: usize) -> &Recipe {
        &self.recipes[n]
    }
}
```

### Persistent history

The history is kept between runs in a JSON file, by default `.brei_history` in the root directory, that maps each path to its digest. A missing file is an empty history. Loading adds to the history in memory, so that loading twice does no harm.

The history should be saved after every run, also when the run failed: targets of failed rules have been forgotten, so they are built again next time, while the rules that did succeed don't need to. We write to a temporary file first and then rename it, so an interrupted write can't leave a broken history behind.

```rust
//| file: src/tasks/history.rs
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;
use super::db::TaskDb;

pub const HISTORY_FILE: &str = ".brei_history";

impl TaskDb {
    /// Read the history from a file, relative to the root.
    pub fn load_history(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let file = match File::open(self.root.join(path)) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        let data: BTreeMap<PathBuf, String> = serde_json::from_reader(BufReader::new(file))?;
        self.history.lock().unwrap().extend(data);
        Ok(())
    }

    /// Write the history to a file, relative to the root.
    pub fn save_history(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let path = self.root.join(path);
        let dir = path.parent().unwrap_or(&self.root);
        let mut tmp = NamedTempFile::new_in(dir)?;
        {
            let mut out = BufWriter::new(tmp.as_file_mut());
            serde_json::to_writer_pretty(&mut out, &*self.history.lock().unwrap())?;
            writeln!(out)?;
            out.flush()?;
        }
        tmp.persist(path)?;
        Ok(())
    }
}
```

Templates
---------

A rule in a Brei program is written with strings, which may contain variables: `${name}` is replaced with the value of the scalar variable `var(name)`. Additionally, `splice(name)` in `requires` is replaced with the elements of the list variable `listvar(name)`. Such a rule is a `RuleTemplate`, the Python `TaskProxy`. A template depends on the variables it uses. Once their values are known, it is expanded into a `Rule`, which then has its own dependencies.

A template string uses the [dollar syntax](./template_strings.md#dollar-syntax). `$$` is a literal dollar sign, also in a template that doesn't use any variables, so every template is expanded, even if it has no variables. One that has no variables and doesn't splice anything is expanded right away, and added as a rule. Otherwise it is added as a recipe of its own.

The targets of a template (`creates`, `stdout` and `name`) are fixed: we need to know which recipe creates a target before we can build it. Templates with variables in their targets are resolved in an earlier stage, when reading the program. List variables have their own place: `listvar(name)` can only be used as `stdout`, and `splice(name)` only in `requires`.

```rust
//| file: src/tasks/template.rs
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use crate::template_strings::{dollar, Env, Error, Format, Item};
use super::db::{Kind, Recipe, TaskDb};
use super::rule::{Failure, Rule, RuleError};
use super::target::{wrapped, Target, VarType};

#[derive(Clone, Debug, Default)]
pub struct RuleTemplate {
    pub creates: Vec<String>,
    pub requires: Vec<String>,
    pub name: Option<String>,
    pub runner: Option<String>,
    pub path: Option<String>,
    pub script: Option<String>,
    pub stdin: Option<String>,
    pub stdout: Option<String>,
    pub description: Option<String>,
    pub force: bool,
}

/// The values of list variables, by name.
pub(super) type Lists = HashMap<String, Vec<String>>;

/// Names of the variables in a template string.
pub(super) fn variables(s: &str) -> impl Iterator<Item=&str> {
    dollar(s).filter_map(|item| match item {
        Ok(Item::Variable(name)) => Some(name),
        _ => None,
    })
}

pub(super) fn substitute(s: &str, env: &Env) -> Result<String, Failure> {
    dollar(s).format(env).map_err(|e| Failure::Template(match e {
        Error::MissingVariable(name) => format!("variable `{name}` has no value"),
        Error::InvalidTemplate(msg) => msg,
    }))
}

fn splice_name(s: &str) -> Option<&str> {
    wrapped(s, "splice")
}

fn parse(s: String) -> Result<Target, Failure> {
    s.parse().map_err(|_| Failure::Template("empty target".into()))
}

impl RuleTemplate {
    /// The targets: `creates`, `stdout` and the phony target `#name`.
    fn targets(&self) -> Vec<String> {
        self.creates.iter().cloned()
            .chain(self.stdout.clone())
            .chain(self.name.as_ref().map(|n| format!("#{n}")))
            .collect()
    }

    fn strings(&self) -> impl Iterator<Item=&String> {
        let fields = [&self.name, &self.runner, &self.path, &self.script, &self.stdin, &self.stdout, &self.description];
        self.creates.iter().chain(&self.requires).chain(fields.into_iter().flatten())
    }

    /// Names of the variables used in the template.
    pub fn variables(&self) -> BTreeSet<&str> {
        self.strings().flat_map(|s| variables(s)).collect()
    }

    /// Names of the list variables spliced into `requires`.
    pub fn splices(&self) -> BTreeSet<&str> {
        self.requires.iter().filter_map(|s| splice_name(s)).collect()
    }

    fn check(&self) -> Result<(), RuleError> {
        let invalid = |msg: String| Err(RuleError::Invalid(msg));
        let targets = self.targets();
        if let Some(t) = targets.iter().find(|t| variables(t).next().is_some()) {
            return invalid(format!("the targets of a rule can't contain variables, found `{t}`"));
        }
        let inputs = || self.stdin.iter().chain(&self.path);
        if let Some(s) = self.creates.iter().chain(&self.requires).chain(inputs()).find(|s| wrapped(s, "listvar").is_some()) {
            return invalid(format!("`{s}` can only be used as `stdout` of a rule"));
        }
        if let Some(s) = targets.iter().chain(inputs()).find(|s| splice_name(s).is_some()) {
            return invalid(format!("`{s}` can only be used in `requires`"));
        }
        Ok(())
    }

    /// Substitute variables and splice lists, giving a rule.
    pub(super) fn expand(&self, env: &Env, lists: &Lists) -> Result<Rule, Failure> {
        let sub = |s: &Option<String>| s.as_deref().map(|s| substitute(s, env)).transpose();
        let target = |s: &str| parse(substitute(s, env)?);

        let mut requires = Vec::new();
        for s in &self.requires {
            match splice_name(s) {
                Some(name) => {
                    let items = lists.get(name)
                        .ok_or_else(|| Failure::Template(format!("list variable `{name}` has no value")))?;
                    for item in items {
                        requires.push(parse(item.clone())?);
                    }
                }
                None => requires.push(target(s)?),
            }
        }

        Ok(Rule {
            creates: self.creates.iter().map(|s| target(s)).collect::<Result<_, _>>()?,
            requires,
            name: sub(&self.name)?,
            runner: sub(&self.runner)?,
            path: sub(&self.path)?.map(PathBuf::from),
            script: sub(&self.script)?,
            stdin: sub(&self.stdin)?.map(parse).transpose()?,
            stdout: sub(&self.stdout)?.map(parse).transpose()?,
            description: sub(&self.description)?,
            force: self.force,
        })
    }
}

fn scalar(name: &str) -> Target {
    Target::Variable(name.to_string(), VarType::Scalar)
}

impl TaskDb {
    /// Add a rule that may use variables.
    pub fn add_template(&mut self, template: RuleTemplate) -> Result<(), RuleError> {
        template.check()?;
        let invalid = |f: Failure| RuleError::Invalid(f.to_string());
        let (vars, lists) = (template.variables(), template.splices());
        if vars.is_empty() && lists.is_empty() {
            let rule = template.expand(&Env::new(), &Lists::new()).map_err(invalid)?;
            return self.add(rule);
        }

        let requires = vars.iter().map(|v| scalar(v))
            .chain(lists.iter().map(|v| Target::Variable(v.to_string(), VarType::List)))
            .map(|t| self.targets.intern(&t))
            .collect::<Result<Vec<_>, _>>()?;
        let mut creates = Vec::new();
        for t in template.targets() {
            let t = parse(substitute(&t, &Env::new()).map_err(invalid)?).map_err(invalid)?;
            creates.push(self.targets.intern(&t)?);
        }
        self.insert(Recipe { creates, requires, kind: Kind::Template(template) })
    }

    /// Add a variable whose value is a template string.
    pub fn add_variable(&mut self, name: &str, template: &str) -> Result<(), RuleError> {
        let creates = vec![self.targets.intern(&scalar(name))?];
        let requires = variables(template).map(|v| self.targets.intern(&scalar(v))).collect::<Result<Vec<_>, _>>()?;
        self.insert(Recipe { creates, requires, kind: Kind::Variable(template.to_string()) })
    }
}
```

Running a rule
--------------

### Is a rule out of date?

A rule needs to run when one of its target files is missing, is older than one of the files it depends on, or was created by a different version of the script (according to the history).

```rust
//| file: src/tasks/exec.rs
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime};
use tempfile::{NamedTempFile, TempPath};
use crate::noodles::task::CancelToken;
use crate::template_strings::{dollar, Env, Format};
use super::db::TaskDb;
use super::rule::{Rule, Value, Failure};
use super::target::{Target, VarType};

impl TaskDb {
    fn modified(&self, path: &Path) -> Result<SystemTime, Failure> {
        Ok(fs::metadata(self.root.join(path))?.modified()?)
    }

    pub fn needs_run(&self, rule: &Rule) -> Result<bool, Failure> {
        if rule.target_paths().any(|p| !self.root.join(p).exists()) {
            return Ok(true);
        }
        let targets = rule.target_paths().map(|p| self.modified(p)).collect::<Result<Vec<_>, _>>()?;
        let deps = rule.dependency_paths().map(|p| self.modified(p)).collect::<Result<Vec<_>, _>>()?;
        if targets.iter().any(|t| deps.iter().any(|d| t < d)) {
            return Ok(true);
        }
        let digest = rule.digest();
        let history = self.history.lock().unwrap();
        Ok(rule.target_paths().any(|p| history.get(p) != digest.as_ref()))
    }

    fn remember(&self, rule: &Rule) {
        let mut history = self.history.lock().unwrap();
        for p in rule.target_paths() {
            match rule.digest() {
                Some(digest) => history.insert(p.clone(), digest),
                None => history.remove(p),
            };
        }
    }

    /// Targets of a rule that failed may be written partially, so we forget them, and they are built again next time.
    fn forget(&self, rule: &Rule) {
        let mut history = self.history.lock().unwrap();
        for p in rule.target_paths() {
            history.remove(p);
        }
    }

    <<exec-commands>>
    <<exec-execute>>
}

<<exec-run-command>>
```

### Commands

Without a runner, each line of the script is a command, split into arguments like a shell would. Note that this is not a shell: there are no pipes, redirections or variables. With a runner, the script is written to a temporary file (unless the rule has a `path`), and there is a single command: the runner with its arguments. The temporary file is kept until the rule is done.

```rust
//| id: exec-commands
fn commands(&self, rule: &Rule, script_file: &mut Option<TempPath>) -> Result<Vec<Vec<String>>, Failure> {
    let Some(name) = &rule.runner else {
        let script = rule.script.as_deref().unwrap_or("");
        return script.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| shlex::split(line).ok_or_else(|| Failure::InvalidScript(line.to_string())))
            .collect();
    };

    let runner = self.runners.get(name).ok_or_else(|| Failure::UnknownRunner(name.clone()))?;
    let path: PathBuf = match (&rule.path, &rule.script) {
        (Some(path), _) => self.root.join(path),
        (None, Some(script)) => {
            let mut file = NamedTempFile::new()?;
            file.write_all(script.as_bytes())?;
            // Close the file, so that it can be opened by the runner on any platform.
            let temp = file.into_temp_path();
            let path = temp.to_path_buf();
            *script_file = Some(temp);
            path
        }
        (None, None) => return Err(Failure::InvalidScript(String::new())),
    };

    let mut env = Env::new();
    env.insert("script".into(), path.to_string_lossy().into_owned());
    let args = runner.args.iter()
        .map(|arg| dollar(arg.as_str()).format(&env).map_err(|e| Failure::Template(format!("{e:?}"))))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(vec![std::iter::once(runner.command.clone()).chain(args).collect()])
}
```

### Executing

To execute a rule, we first check if it needs to run at all. A rule without a script (only dependencies) is done once its dependencies are.

Standard input comes from a file, from the value of a variable, or is empty. Unlike the Python version, a command never reads from the terminal: with many commands running at the same time that would be confusing. Standard output goes to a file, is captured into a variable, or goes to the terminal. Standard error always goes to the terminal. If there are several commands, they share the same input and output files.

After running, we store the digest in the history, and check that the rule actually did what it promised: if it still needs to run, something is wrong. The value of a rule is its standard output, if it was captured: trimmed for a scalar, and split into non-empty lines for a list.

```rust
//| id: exec-execute
pub(super) fn execute(&self, rule: &Rule, input: Option<&str>, cancel: &CancelToken) -> Result<Value, Failure> {
    if !(self.force_run || rule.always_run()) && !self.needs_run(rule)? {
        return Ok(Value::Nothing);
    }
    if rule.runner.is_none() && rule.script.is_none() {
        return Ok(Value::Nothing);
    }

    let mut script_file = None;
    let commands = self.commands(rule, &mut script_file)?;
    let stdin = match &rule.stdin {
        Some(Target::Path(p)) => Some(File::open(self.root.join(p))?),
        _ => None,
    };
    let stdout = match &rule.stdout {
        Some(Target::Path(p)) => Some(File::create(self.root.join(p))?),
        _ => None,
    };
    let capture = matches!(rule.stdout, Some(Target::Variable(..)));

    let mut output = Vec::new();
    for command in &commands {
        let mut cmd = Command::new(&command[0]);
        cmd.args(&command[1..]).current_dir(&self.root).stderr(Stdio::inherit());
        cmd.stdin(match &stdin {
            Some(file) => Stdio::from(file.try_clone()?),
            None if input.is_some() => Stdio::piped(),
            None => Stdio::null(),
        });
        cmd.stdout(match &stdout {
            Some(file) => Stdio::from(file.try_clone()?),
            None if capture => Stdio::piped(),
            None => Stdio::inherit(),
        });
        match run_command(cmd, &command.join(" "), input, cancel) {
            Ok(out) => output.extend(out),
            Err(failure) => {
                self.forget(rule);
                return Err(failure);
            }
        }
    }
    drop(script_file);

    self.remember(rule);
    if self.needs_run(rule)? {
        return Err(Failure::NotAchieved(rule.creates.clone()));
    }

    let text = String::from_utf8_lossy(&output);
    Ok(match &rule.stdout {
        Some(Target::Variable(_, VarType::List)) =>
            Value::List(text.lines().filter(|l| !l.is_empty()).map(String::from).collect()),
        Some(Target::Variable(_, VarType::Scalar)) => Value::Scalar(text.trim().to_string()),
        _ => Value::Nothing,
    })
}
```

### Running a command

Input and captured output go through pipes. Each pipe gets its own thread, so that a command that writes a lot of output before reading its input can't block. Meanwhile, we poll the command to see if it has finished, and check the cancel token. A cancelled command is killed.

Killing only stops the command itself. If the command started processes of its own, like a runner script that calls other programs, those keep running. Fixing that needs process groups, which is platform specific.

```rust
//| id: exec-run-command
fn run_command(mut cmd: Command, display: &str, input: Option<&str>, cancel: &CancelToken) -> Result<Vec<u8>, Failure> {
    let mut child = cmd.spawn().map_err(|e| Failure::Io(format!("can't run `{display}`: {e}")))?;

    let writer = match (child.stdin.take(), input) {
        (Some(mut stdin), Some(data)) => {
            let data = data.as_bytes().to_vec();
            // The command may exit without reading all of its input; that's fine.
            Some(thread::spawn(move || { let _ = stdin.write_all(&data); }))
        }
        _ => None,
    };
    let reader = child.stdout.take().map(|mut stdout| thread::spawn(move || {
        let mut buffer = Vec::new();
        stdout.read_to_end(&mut buffer).map(|_| buffer)
    }));

    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Failure::Cancelled);
        }
        thread::sleep(Duration::from_millis(10));
    };

    if let Some(writer) = writer {
        let _ = writer.join();
    }
    let output = match reader {
        Some(reader) => reader.join().expect("reader thread panicked")?,
        None => Vec::new(),
    };
    if !status.success() {
        return Err(Failure::Command { command: display.to_string(), code: status.code() });
    }
    Ok(output)
}
```

Engine tasks
------------

Now we can put rules on the engine. There are five kinds of jobs:

- `Build` a target, with the target as its key. This job is memoized, so every target is built at most once. If no recipe creates the target, it should be a file that already exists. Otherwise, the result is a continuation: building each dependency of the recipe, followed by running the rule, or expanding the template;
- `Expand` a template or an environment variable, once the variables it uses are built. A variable gives its value right away. A template gives a rule, so the result is again a continuation: building the dependencies of that rule, followed by running it;
- `Run` a rule, once its dependencies are built. This has a local key, and receives the values of all dependencies. The only value it uses is that of a variable on `stdin`;
- `Alias` a target to another. A recipe can create several targets, but we want to use it only once. So the first target is the main one: building any other target of the recipe gives a continuation that builds the main target, followed by an alias that passes on its value;
- `All` waits for several targets, so that we can build them in one go. Its value is `Nothing`.

```rust
//| file: src/tasks/job.rs
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use crate::noodles::task::{Task, Response, Promise, CancelToken, Error, Worker};
use crate::noodles::kernel::Kernel;
use crate::template_strings::Env;
use super::db::{Entry, Kind, TaskDb};
use super::rule::{Value, Failure};
use super::target::{Target, TargetId, Key};
use super::template::{substitute, Lists};

pub enum Job {
    Build { db: Arc<TaskDb>, target: TargetId },
    Expand { db: Arc<TaskDb>, recipe: usize, args: HashMap<TargetId, Value> },
    Run { db: Arc<TaskDb>, entry: Arc<Entry>, args: HashMap<TargetId, Value> },
    Alias { target: TargetId, value: Option<Value> },
    All { targets: Vec<TargetId>, done: HashSet<TargetId> },
}

impl Job {
    pub fn build(db: &Arc<TaskDb>, target: TargetId) -> Self {
        Job::Build { db: db.clone(), target }
    }
}

/// Build the dependencies, then `last`.
fn after(db: &Arc<TaskDb>, deps: &[TargetId], last: Job) -> Response<Job> {
    let tasks = deps.iter().map(|dep| Job::build(db, *dep)).chain([last]);
    Response::Continuation(Box::new(Promise::new(Key::Local(0), tasks)))
}

fn build_target(db: Arc<TaskDb>, target: TargetId) -> Response<Job> {
    let Some(n) = db.recipe_for(target) else {
        return match db.targets.get(target) {
            Target::Path(p) if db.root.join(&p).exists() => Response::Value(Value::Nothing),
            t => Response::Error(Failure::Missing(t)),
        };
    };

    let recipe = db.recipe(n);
    let main = recipe.creates[0];
    if main != target {
        return after(&db, &[main], Job::Alias { target: main, value: None });
    }
    let last = match &recipe.kind {
        Kind::Rule(entry) => Job::Run { db: db.clone(), entry: entry.clone(), args: HashMap::new() },
        Kind::Template(_) | Kind::Variable(_) => Job::Expand { db: db.clone(), recipe: n, args: HashMap::new() },
    };
    after(&db, &recipe.requires, last)
}

<<job-expand-run>>
```

To expand a template, we collect the values of the variables it uses. Variables that have no value (because the rule that creates them didn't need to run) are left out, so using them gives an error. The rule that comes out of a template is interned at run time; its dependencies may be new targets, for instance paths from a list variable.

```rust
//| id: job-expand-run
fn expand(db: Arc<TaskDb>, n: usize, args: HashMap<TargetId, Value>) -> Response<Job> {
    let mut env = Env::new();
    let mut lists = Lists::new();
    for (id, value) in args {
        let Target::Variable(name, _) = db.targets.get(id) else { continue };
        match value {
            Value::Scalar(s) => { env.insert(name, s); }
            Value::List(l) => { lists.insert(name, l); }
            Value::Nothing => {}
        }
    }

    let result = match &db.recipe(n).kind {
        Kind::Variable(template) => substitute(template, &env).map(|s| Response::Value(Value::Scalar(s))),
        Kind::Template(template) => template.expand(&env, &lists)
            .and_then(|rule| Entry::new(rule, &db.targets).map_err(|e| Failure::Template(e.to_string())))
            .map(|entry| {
                let entry = Arc::new(entry);
                after(&db, &entry.requires, Job::Run { db: db.clone(), entry: entry.clone(), args: HashMap::new() })
            }),
        Kind::Rule(_) => unreachable!("rules are not expanded"),
    };
    result.unwrap_or_else(Response::Error)
}

fn run_rule(db: Arc<TaskDb>, entry: Arc<Entry>, args: HashMap<TargetId, Value>, cancel: &CancelToken) -> Response<Job> {
    let input = entry.stdin.map(|id| match args.get(&id) {
        Some(Value::Scalar(s)) => s.as_str(),
        _ => "",
    });
    match db.execute(&entry.rule, input, cancel) {
        Ok(value) => Response::Value(value),
        Err(failure) => Response::Error(failure),
    }
}

impl Task for Job {
    type Value = Value;
    type Key = Key;
    type Error = Failure;

    fn key(&self) -> Key {
        match self {
            Job::Build { target, .. } => Key::Target(*target),
            Job::Expand { .. } | Job::Run { .. } | Job::Alias { .. } | Job::All { .. } => Key::Local(0),
        }
    }

    fn dependencies(&self) -> impl Iterator<Item=Key> {
        let deps: Vec<TargetId> = match self {
            Job::Build { .. } => Vec::new(),
            Job::Expand { db, recipe, .. } => db.recipe(*recipe).requires.clone(),
            Job::Run { entry, .. } => entry.requires.clone(),
            Job::Alias { target, .. } => vec![*target],
            Job::All { targets, .. } => targets.clone(),
        };
        deps.into_iter().map(Key::Target)
    }

    fn memoize(&self) -> bool {
        self.key().is_global()
    }

    fn run(self, cancel: &CancelToken) -> Response<Self> {
        match self {
            Job::Build { db, target } => build_target(db, target),
            Job::Expand { db, recipe, args } => expand(db, recipe, args),
            Job::Run { db, entry, args } => run_rule(db, entry, args, cancel),
            Job::Alias { value, .. } => Response::Value(value.expect("alias is ready")),
            Job::All { .. } => Response::Value(Value::Nothing),
        }
    }

    fn assoc(&mut self, key: Key, value: Value) {
        match (self, key) {
            (Job::Expand { args, .. } | Job::Run { args, .. }, Key::Target(id)) => { args.insert(id, value); }
            (Job::Alias { value: v, .. }, _) => *v = Some(value),
            (Job::All { done, .. }, Key::Target(id)) => { done.insert(id); }
            _ => {}
        }
    }

    fn ready(&self) -> bool {
        match self {
            Job::Build { .. } => true,
            Job::Expand { db, recipe, args } => args.len() == db.recipe(*recipe).requires.len(),
            Job::Run { entry, args, .. } => args.len() == entry.requires.len(),
            Job::Alias { value, .. } => value.is_some(),
            Job::All { targets, done } => done.len() == targets.len(),
        }
    }
}

/// Build a target, giving its value.
pub fn build<W: Worker<Job>>(db: &Arc<TaskDb>, kernel: &mut Kernel<Job>, worker: &W, target: TargetId) -> Result<Value, Error<Job>> {
    kernel.run(worker, Box::new(Promise::new(Key::Target(target), [Job::build(db, target)])))
}

/// Build several targets at once.
pub fn build_all<W: Worker<Job>>(db: &Arc<TaskDb>, kernel: &mut Kernel<Job>, worker: &W, targets: &[TargetId]) -> Result<(), Error<Job>> {
    let mut targets = targets.to_vec();
    targets.sort();
    targets.dedup();
    let mut jobs: Vec<Job> = targets.iter().map(|t| Job::build(db, *t)).collect();
    jobs.push(Job::All { targets, done: HashSet::new() });
    kernel.run(worker, Box::new(Promise::new(Key::Local(0), jobs))).map(|_| ())
}
```

Tests
-----

The tests run real commands in a temporary directory, so they need `bash`, `cat`, `tr` and a few other common Unix tools.

```rust
//| file: src/tasks/tests.rs
use std::fs;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tempfile::TempDir;
use crate::noodles::kernel::Kernel;
use crate::noodles::single_worker::SingleWorker;
use crate::noodles::worker_pool::WorkerPool;
use crate::noodles::task::Error;
use super::*;

fn target(s: &str) -> Target {
    s.parse().unwrap()
}

fn targets(ts: &[&str]) -> Vec<Target> {
    ts.iter().map(|t| target(t)).collect()
}

fn rule(creates: &[&str], requires: &[&str], script: &str) -> Rule {
    Rule { creates: targets(creates), requires: targets(requires), script: Some(script.into()), ..Default::default() }
}

fn bash(creates: &[&str], requires: &[&str], script: &str) -> Rule {
    Rule { runner: Some("bash".into()), ..rule(creates, requires, script) }
}

fn run(db: &Arc<TaskDb>, t: &str) -> Result<Value, Error<Job>> {
    let id = db.targets.intern(&target(t)).unwrap();
    build(db, &mut Kernel::new(), &SingleWorker::new(), id)
}

fn read(dir: &TempDir, path: &str) -> String {
    fs::read_to_string(dir.path().join(path)).unwrap()
}

fn failures(result: Result<Value, Error<Job>>) -> Vec<Failure> {
    match result {
        Err(Error::Jobs(failures)) => failures,
        Err(e) => panic!("expected failures, got {e:?}"),
        Ok(v) => panic!("expected failures, got {v:?}"),
    }
}

<<task-tests>>
```

A simple rule, and a chain of rules. The second time, nothing needs to be done. When a dependency changes, only the rules that depend on it run again.

```rust
//| id: task-tests
#[test]
fn hello() {
    let dir = TempDir::new().unwrap();
    let mut db = TaskDb::new(dir.path());
    db.add(Rule { stdout: Some(target("hello.txt")), ..rule(&[], &[], "echo hello") }).unwrap();
    assert_eq!(run(&Arc::new(db), "hello.txt").unwrap(), Value::Nothing);
    assert_eq!(read(&dir, "hello.txt"), "hello\n");
}

#[test]
fn up_to_date() {
    let dir = TempDir::new().unwrap();
    let mut db = TaskDb::new(dir.path());
    db.add(bash(&["a.txt"], &[], "echo a >> log.txt; echo a > a.txt")).unwrap();
    db.add(bash(&["b.txt"], &["a.txt"], "echo b >> log.txt; cat a.txt > b.txt")).unwrap();
    let db = Arc::new(db);

    run(&db, "b.txt").unwrap();
    assert_eq!(read(&dir, "log.txt"), "a\nb\n");
    assert_eq!(read(&dir, "b.txt"), "a\n");

    run(&db, "b.txt").unwrap();
    assert_eq!(read(&dir, "log.txt"), "a\nb\n");

    // make `b.txt` older than `a.txt`
    let earlier = SystemTime::now() - Duration::from_secs(10);
    fs::File::options().write(true).open(dir.path().join("b.txt")).unwrap().set_modified(earlier).unwrap();
    run(&db, "b.txt").unwrap();
    assert_eq!(read(&dir, "log.txt"), "a\nb\nb\n");
}
```

Variables pass values between rules, through standard output and input.

```rust
//| id: task-tests
#[test]
fn variables() {
    let dir = TempDir::new().unwrap();
    let mut db = TaskDb::new(dir.path());
    db.add(Rule { stdout: Some(target("var(greeting)")), ..rule(&[], &[], "echo hello world") }).unwrap();
    db.add(Rule {
        stdin: Some(target("var(greeting)")),
        stdout: Some(target("shout.txt")),
        ..rule(&[], &[], "tr a-z A-Z")
    }).unwrap();
    db.add(Rule { stdout: Some(target("listvar(files)")), ..rule(&[], &[], r"printf 'a\nb\n\nc\n'") }).unwrap();
    let db = Arc::new(db);

    assert_eq!(run(&db, "var(greeting)").unwrap(), Value::Scalar("hello world".into()));
    run(&db, "shout.txt").unwrap();
    assert_eq!(read(&dir, "shout.txt"), "HELLO WORLD");
    assert_eq!(run(&db, "listvar(files)").unwrap(), Value::List(vec!["a".into(), "b".into(), "c".into()]));
}
```

A rule that creates several targets runs once, even when its targets are built in parallel. Its value can be reached through any of its targets.

```rust
//| id: task-tests
#[test]
fn several_targets() {
    let dir = TempDir::new().unwrap();
    let mut db = TaskDb::new(dir.path());
    db.add(bash(&["a.txt", "b.txt", "c.txt"], &[], "echo run >> log.txt; touch a.txt b.txt c.txt")).unwrap();
    db.add(Rule { name: Some("all".into()), requires: targets(&["a.txt", "b.txt", "c.txt"]), ..Default::default() }).unwrap();
    db.add(Rule { name: Some("answer".into()), stdout: Some(target("var(answer)")), ..rule(&[], &[], "echo 42") }).unwrap();
    let db = Arc::new(db);

    let id = db.targets.lookup(&target("#all")).unwrap();
    build(&db, &mut Kernel::new(), &WorkerPool::new(4), id).unwrap();
    assert_eq!(read(&dir, "log.txt"), "run\n");

    // `#answer` is the main target, `var(answer)` is an alias.
    assert_eq!(run(&db, "var(answer)").unwrap(), Value::Scalar("42".into()));
}
```

Dependencies that are files without a rule are fine, as long as they exist.

```rust
//| id: task-tests
#[test]
fn missing_dependency() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("input.txt"), "data").unwrap();
    let mut db = TaskDb::new(dir.path());
    db.add(rule(&["copy.txt"], &["input.txt"], "cp input.txt copy.txt")).unwrap();
    db.add(rule(&["other.txt"], &["nothere.txt"], "touch other.txt")).unwrap();
    let db = Arc::new(db);

    run(&db, "copy.txt").unwrap();
    assert_eq!(read(&dir, "copy.txt"), "data");
    assert_eq!(failures(run(&db, "other.txt")), [Failure::Missing(target("nothere.txt"))]);
    assert_eq!(failures(run(&db, "#nothing")), [Failure::Missing(target("#nothing"))]);
}
```

A failing command fails the rule, and so does a rule that doesn't create its targets.

```rust
//| id: task-tests
#[test]
fn failing_rules() {
    let dir = TempDir::new().unwrap();
    let mut db = TaskDb::new(dir.path());
    db.add(rule(&["x.txt"], &[], "false")).unwrap();
    db.add(rule(&["y.txt"], &[], "true")).unwrap();
    db.add(rule(&["z.txt"], &[], "no-such-command")).unwrap();
    let db = Arc::new(db);

    assert_eq!(failures(run(&db, "x.txt")), [Failure::Command { command: "false".into(), code: Some(1) }]);
    assert_eq!(failures(run(&db, "y.txt")), [Failure::NotAchieved(targets(&["y.txt"]))]);
    assert!(matches!(&failures(run(&db, "z.txt"))[..], [Failure::Io(msg)] if msg.contains("no-such-command")));
}
```

When one rule fails, a slow command that runs in parallel is killed.

```rust
//| id: task-tests
#[test]
fn cancel() {
    let dir = TempDir::new().unwrap();
    let mut db = TaskDb::new(dir.path());
    db.add(rule(&["slow.txt"], &[], "sleep 10")).unwrap();
    db.add(rule(&["fail.txt"], &[], "false")).unwrap();
    db.add(Rule { name: Some("all".into()), requires: targets(&["slow.txt", "fail.txt"]), ..Default::default() }).unwrap();
    let db = Arc::new(db);

    let start = Instant::now();
    let id = db.targets.lookup(&target("#all")).unwrap();
    assert!(build(&db, &mut Kernel::new(), &WorkerPool::new(2), id).is_err());
    assert!(start.elapsed() < Duration::from_secs(5));
}
```

Mistakes in rules are found when they are added.

```rust
//| id: task-tests
#[test]
fn invalid_rules() {
    let mut db = TaskDb::new(".");
    db.add(rule(&["a.txt"], &[], "true")).unwrap();
    assert_eq!(db.add(rule(&["a.txt"], &[], "true")), Err(RuleError::Duplicate(target("a.txt"))));

    db.add(Rule { stdout: Some(target("listvar(x)")), ..rule(&[], &[], "true") }).unwrap();
    assert!(matches!(db.add(Rule { stdin: Some(target("var(x)")), ..rule(&["b.txt"], &[], "cat") }),
        Err(RuleError::TypeMismatch(_))));

    let invalid = [
        Rule { stdin: Some(target("#a")), ..rule(&["c.txt"], &[], "cat") },
        Rule { stdout: Some(target("#a")), ..rule(&[], &[], "true") },
        Rule { path: Some("script.py".into()), ..Rule::default() },
        Rule { runner: Some("bash".into()), ..Rule::default() },
        Rule { stdin: Some(target("in.txt")), ..rule(&["d.txt"], &[], "cat\ncat") },
    ];
    for r in invalid {
        assert!(matches!(db.add(r), Err(RuleError::Invalid(_))));
    }
}
```


The history carries over to a new database, through the history file. A rule whose script changed runs again, and so does a rule that failed last time.

```rust
//| id: task-tests
#[test]
fn persistent_history() {
    let dir = TempDir::new().unwrap();
    let rules = |script: &str| {
        let mut db = TaskDb::new(dir.path());
        db.add(bash(&["a.txt"], &[], &format!("echo {script} >> log.txt; echo {script} > a.txt"))).unwrap();
        db.add(bash(&["b.txt"], &[], "echo b >> log.txt; [ -e ok ] && touch b.txt")).unwrap();
        db.load_history(HISTORY_FILE).unwrap();
        Arc::new(db)
    };

    let db = rules("a");
    run(&db, "a.txt").unwrap();
    assert!(run(&db, "b.txt").is_err());
    db.save_history(HISTORY_FILE).unwrap();
    assert_eq!(read(&dir, "log.txt"), "a\nb\n");

    // `a.txt` is up to date, `b.txt` failed so it runs again.
    fs::write(dir.path().join("ok"), "").unwrap();
    let db = rules("a");
    run(&db, "a.txt").unwrap();
    run(&db, "b.txt").unwrap();
    db.save_history(HISTORY_FILE).unwrap();
    assert_eq!(read(&dir, "log.txt"), "a\nb\nb\n");

    // a different script for `a.txt`
    let db = rules("A");
    run(&db, "a.txt").unwrap();
    run(&db, "b.txt").unwrap();
    assert_eq!(read(&dir, "log.txt"), "a\nb\nb\nA\n");

    fs::write(dir.path().join(HISTORY_FILE), "not json").unwrap();
    assert!(TaskDb::new(dir.path()).load_history(HISTORY_FILE).is_err());
}
```

Several targets are built in one run, in parallel, sharing their dependencies. Naming a target twice is fine. With `keep_going`, a failing target doesn't stop the others.

```rust
//| id: task-tests
#[test]
fn build_several() {
    let dir = TempDir::new().unwrap();
    let mut db = TaskDb::new(dir.path());
    db.add(bash(&["a.txt"], &[], "echo a >> log.txt; touch a.txt")).unwrap();
    db.add(bash(&["b.txt"], &["a.txt"], "touch b.txt")).unwrap();
    db.add(bash(&["c.txt"], &["a.txt"], "touch c.txt")).unwrap();
    db.add(rule(&["x.txt"], &[], "false")).unwrap();
    let db = Arc::new(db);
    let ids = |ts: &[&str]| ts.iter().map(|t| db.targets.lookup(&target(t)).unwrap()).collect::<Vec<_>>();

    build_all(&db, &mut Kernel::new(), &WorkerPool::new(4), &ids(&["b.txt", "c.txt", "b.txt"])).unwrap();
    assert_eq!(read(&dir, "log.txt"), "a\n");
    assert!(dir.path().join("b.txt").exists() && dir.path().join("c.txt").exists());
    build_all(&db, &mut Kernel::new(), &SingleWorker::new(), &[]).unwrap();

    fs::remove_file(dir.path().join("b.txt")).unwrap();
    fs::remove_file(dir.path().join("c.txt")).unwrap();
    let result = build_all(&db, &mut Kernel::new().keep_going(), &SingleWorker::new(), &ids(&["x.txt", "b.txt", "c.txt"]));
    assert_eq!(failures(result.map(|_| Value::Nothing)), [Failure::Command { command: "false".into(), code: Some(1) }]);
    assert!(dir.path().join("b.txt").exists() && dir.path().join("c.txt").exists());
}
```

### Templates

A variable from the environment is a template string, which may use other variables. Templates use variables in any field, and are expanded when they run.

```rust
//| id: task-tests
fn template(creates: &[&str], requires: &[&str], script: &str) -> RuleTemplate {
    let strings = |ts: &[&str]| ts.iter().map(|t| t.to_string()).collect();
    RuleTemplate { creates: strings(creates), requires: strings(requires), script: Some(script.into()), ..Default::default() }
}

#[test]
fn environment() {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("data")).unwrap();
    fs::write(dir.path().join("data/input.txt"), "hello").unwrap();
    let mut db = TaskDb::new(dir.path());
    db.add_variable("dir", "data").unwrap();
    db.add_variable("input", "${dir}/input.txt").unwrap();
    db.add(Rule { stdout: Some(target("var(output)")), ..rule(&[], &[], "echo output.txt") }).unwrap();
    db.add_template(template(&["output.txt"], &["${input}"], "cp ${input} ${output}")).unwrap();
    let db = Arc::new(db);

    assert_eq!(run(&db, "var(input)").unwrap(), Value::Scalar("data/input.txt".into()));
    run(&db, "output.txt").unwrap();
    assert_eq!(read(&dir, "output.txt"), "hello");
}
```

The output of a template rule can go to a variable, and the values of variables are substituted in `stdin` too. A literal dollar sign is written `$$`, also in a rule without variables, and a dollar sign that doesn't start a variable stays as it is.

```rust
//| id: task-tests
#[test]
fn template_variables() {
    let dir = TempDir::new().unwrap();
    let mut db = TaskDb::new(dir.path());
    db.add_variable("name", "greeting").unwrap();
    db.add(Rule { stdout: Some(target("var(greeting)")), ..rule(&[], &[], "echo hello") }).unwrap();
    db.add_template(RuleTemplate {
        stdin: Some("var(${name})".into()), stdout: Some("var(shout)".into()), ..template(&[], &[], "tr a-z A-Z")
    }).unwrap();
    db.add_template(RuleTemplate { stdout: Some("var(dollars)".into()), ..template(&[], &[], "echo $$1 $1 $(x)") }).unwrap();
    let db = Arc::new(db);

    assert_eq!(run(&db, "var(shout)").unwrap(), Value::Scalar("HELLO".into()));
    assert_eq!(run(&db, "var(dollars)").unwrap(), Value::Scalar("$1 $1 $(x)".into()));
}
```

Splicing a list variable into `requires` makes all its elements dependencies. These targets are only known at run time.

```rust
//| id: task-tests
#[test]
fn splice() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("c.txt"), "c\n").unwrap();
    let mut db = TaskDb::new(dir.path());
    db.add(Rule { stdout: Some(target("listvar(files)")), ..rule(&[], &[], r"printf 'a.txt\nb.txt\nc.txt\n'") }).unwrap();
    db.add(bash(&["a.txt"], &[], "echo a > a.txt")).unwrap();
    db.add(bash(&["b.txt"], &[], "echo b > b.txt")).unwrap();
    db.add_template(RuleTemplate {
        runner: Some("bash".into()),
        ..template(&["all.txt"], &["splice(files)"], "cat a.txt b.txt c.txt > all.txt")
    }).unwrap();
    db.add_template(template(&["d.txt"], &["splice(files)", "nothere.txt"], "touch d.txt")).unwrap();
    let db = Arc::new(db);

    run(&db, "all.txt").unwrap();
    assert_eq!(read(&dir, "all.txt"), "a\nb\nc\n");
    assert_eq!(failures(run(&db, "d.txt")), [Failure::Missing(target("nothere.txt"))]);
}
```

Templates fail when the variables they use are missing. Mistakes are found when a template is added, where possible: variables in targets, list variables in the wrong places, or a variable that is used both as a scalar and as a list.

```rust
//| id: task-tests
#[test]
fn invalid_templates() {
    let dir = TempDir::new().unwrap();
    let mut db = TaskDb::new(dir.path());
    db.add_template(template(&["a.txt"], &[], "touch ${unknown}")).unwrap();
    db.add_variable("x", "${y}").unwrap();
    assert_eq!(db.add_variable("x", "1"), Err(RuleError::Duplicate(target("var(x)"))));
    db.add(Rule { stdout: Some(target("listvar(list)")), ..rule(&[], &[], "true") }).unwrap();
    assert!(matches!(db.add_template(template(&["b.txt"], &[], "echo ${list}")), Err(RuleError::TypeMismatch(_))));
    assert!(matches!(db.add_template(template(&["b.txt"], &["splice(x)"], "true")), Err(RuleError::TypeMismatch(_))));

    let invalid = [
        template(&["${x}.txt"], &[], "true"),
        RuleTemplate { name: Some("${x}".into()), ..template(&[], &[], "true") },
        template(&["c.txt"], &["listvar(list)"], "true"),
        template(&["listvar(list)"], &[], "true"),
        template(&["splice(list)"], &[], "true"),
        RuleTemplate { stdin: Some("splice(list)".into()), ..template(&["c.txt"], &[], "cat") },
    ];
    for t in invalid {
        assert!(matches!(db.add_template(t), Err(RuleError::Invalid(_))));
    }

    let db = Arc::new(db);
    assert_eq!(failures(run(&db, "a.txt")), [Failure::Missing(target("var(unknown)"))]);
    assert_eq!(failures(run(&db, "var(x)")), [Failure::Missing(target("var(y)"))]);
}
```
