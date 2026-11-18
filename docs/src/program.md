---
entangled:
  version: "2.7"
  namespace: program
---

Programs
========

A Brei program is more than a list of rules. It also has:

- an `environment` of variables, whose values are template strings;
- `template`s: rules with variables, that are filled in by a `call`;
- `include`s of other programs, which may have to be built by a rule first;
- extra `runner`s.

Before we can build anything, these have to be *resolved* into recipes in the task database. This is the Python `resolve_tasks`. Reading programs from TOML comes later; here a program is a plain struct, and included programs are read by a function that is passed in.

```rust
//| file: src/tasks/program.rs
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use crate::noodles::kernel::Kernel;
use crate::noodles::task::{Error, Worker};
use crate::template_strings::Env;
use super::db::TaskDb;
use super::job::{Job, build, build_all};
use super::rule::{RuleError, Runner, Value};
use super::target::{Target, TypeMismatch, VarType};
use super::template::{RuleTemplate, escape, splice_name, substitute, variables};

<<program-types>>
<<program-error>>
<<program-call>>
<<program-resolve>>

#[cfg(test)]
mod tests {
    <<program-tests>>
}
```

The program
-----------

A call fills in a template with arguments. An argument is a string or a list of strings. With lists, the template is filled in once for each element. If there are several lists, they are either zipped (an *inner* join, the default), or every combination is used (an *outer* join). `collect` names a phony target that requires all targets of the call.

```rust
//| id: program-types
#[derive(Clone, Debug, Default)]
pub struct Program {
    pub task: Vec<RuleTemplate>,
    pub environment: BTreeMap<String, String>,
    pub template: HashMap<String, RuleTemplate>,
    pub call: Vec<Call>,
    pub include: Vec<String>,
    pub runner: HashMap<String, Runner>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Join {
    #[default]
    Inner,
    Outer,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Argument {
    Single(String),
    Multiple(Vec<String>),
}

#[derive(Clone, Debug, Default)]
pub struct Call {
    pub template: String,
    pub args: BTreeMap<String, Argument>,
    pub collect: Option<String>,
    pub join: Join,
}
```

Errors
------

Resolving a program can fail in many ways: a rule can be wrong, a template or include can be missing, or a rule that computes a variable we need can fail.

```rust
//| id: program-error
#[derive(Debug)]
pub enum ProgramError {
    Rule(RuleError),
    MissingTemplate(String),
    MissingInclude(PathBuf),
    Load(PathBuf, String),
    Call(String, String),
    NoValue(Target),
    Unresolvable(Vec<String>),
    Build(Error<Job>),
}

impl From<RuleError> for ProgramError {
    fn from(e: RuleError) -> Self {
        ProgramError::Rule(e)
    }
}

impl From<TypeMismatch> for ProgramError {
    fn from(e: TypeMismatch) -> Self {
        ProgramError::Rule(RuleError::TypeMismatch(e))
    }
}

impl From<Error<Job>> for ProgramError {
    fn from(e: Error<Job>) -> Self {
        ProgramError::Build(e)
    }
}

impl fmt::Display for ProgramError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProgramError::Rule(e) => write!(f, "{e}"),
            ProgramError::MissingTemplate(name) => write!(f, "template `{name}` not found"),
            ProgramError::MissingInclude(path) => write!(f, "include `{}` not found", path.display()),
            ProgramError::Load(path, msg) => write!(f, "can't read `{}`: {msg}", path.display()),
            ProgramError::Call(name, msg) => write!(f, "in call to template `{name}`: {msg}"),
            ProgramError::NoValue(t) => write!(f, "`{t}` has no value"),
            ProgramError::Unresolvable(targets) => write!(f, "rules have unresolvable targets: {}", targets.join(", ")),
            ProgramError::Build(Error::Jobs(failures)) => {
                let failures: Vec<String> = failures.iter().map(|e| e.to_string()).collect();
                write!(f, "{}", failures.join("; "))
            }
            ProgramError::Build(e) => write!(f, "{e:?}"),
        }
    }
}
```

Calls
-----

The arguments of a call are template strings themselves. For instance, an argument `"${dir}/output.txt"` uses the environment variable `dir`. So we fill in the arguments with `RuleTemplate::fill`, which leaves the other variables for later.

An inner join of lists with different lengths is an error. Lists can come from list variables, so silently dropping elements would hide mistakes.

```rust
//| id: program-call
impl Call {
    /// The arguments for each time the template is filled in.
    fn instances(&self) -> Result<Vec<Env>, ProgramError> {
        let mut single = Env::new();
        let mut lists = Vec::new();
        for (key, arg) in &self.args {
            match arg {
                Argument::Single(s) => { single.insert(key.clone(), s.clone()); }
                Argument::Multiple(l) => lists.push((key, l)),
            }
        }

        match self.join {
            Join::Inner => {
                let lengths: BTreeSet<usize> = lists.iter().map(|(_, l)| l.len()).collect();
                if lengths.len() > 1 {
                    let lengths: Vec<String> = lists.iter().map(|(k, l)| format!("{k}: {}", l.len())).collect();
                    let msg = format!("the arguments of an inner join should have the same length, got {}", lengths.join(", "));
                    return Err(ProgramError::Call(self.template.clone(), msg));
                }
                let n = lengths.first().copied().unwrap_or(1);
                Ok((0..n).map(|i| {
                    let mut env = single.clone();
                    env.extend(lists.iter().map(|(k, l)| (k.to_string(), l[i].clone())));
                    env
                }).collect())
            }
            Join::Outer => {
                let mut envs = vec![single];
                for (key, list) in lists {
                    envs = envs.into_iter().flat_map(|env| list.iter().map(move |v| {
                        let mut env = env.clone();
                        env.insert(key.clone(), v.clone());
                        env
                    })).collect();
                }
                Ok(envs)
            }
        }
    }

    /// Names of the list variables spliced into the arguments.
    fn splices(&self) -> BTreeSet<&str> {
        self.args.values().flat_map(|arg| match arg {
            Argument::Single(s) => std::slice::from_ref(s),
            Argument::Multiple(l) => l.as_slice(),
        }).filter_map(|s| splice_name(s)).collect()
    }

    /// The rules from filling in `template`, plus the collecting rule.
    fn expand(&self, template: &RuleTemplate) -> Result<Vec<RuleTemplate>, ProgramError> {
        let mut rules: Vec<RuleTemplate> = self.instances()?.iter().map(|env| template.fill(env)).collect();
        if let Some(name) = &self.collect {
            let requires = rules.iter().flat_map(|r| r.targets()).collect();
            rules.push(RuleTemplate { name: Some(name.clone()), requires, ..Default::default() });
        }
        Ok(rules)
    }
}
```

Resolving
---------

Resolving a program needs to run tasks, so the resolver holds a kernel and a worker. The kernel remembers the values it computed, so when the same kernel is used to build the targets afterwards, the rules that were run during resolution aren't run again. That matters: a rule like `mktemp -d` gives a different value every time.

The order of resolution follows the Python version:

1. Add the environment and the runners.
2. Fill in the calls. A call has to wait if its template isn't known yet (it may be in an include), or if it splices list variables.
3. Add the rules. A rule with variables in its targets has to wait: we can't know what it creates before we know the values of those variables.
4. Resolve the waiting rules whose target variables are created by a known recipe. These variables are computed, the values are filled in, and the rule is added.
5. Read the includes, and resolve them in turn. An include can be built by a rule.
6. Fill in the calls that had to wait, and add or delay their rules.
7. Resolve the waiting rules again. Any rule that is still waiting is an error.

The variables in the targets of a delayed rule should be computable from rules that don't have variables in their targets themselves. That way, the order of the rules doesn't matter.

```rust
//| id: program-resolve
struct Resolver<'a, W: Worker<Job>> {
    db: &'a Arc<TaskDb>,
    kernel: &'a mut Kernel<Job>,
    worker: &'a W,
    load: &'a dyn Fn(&Path) -> Result<Program, String>,
    templates: HashMap<String, RuleTemplate>,
}

fn scalar(name: &str) -> Target {
    Target::Variable(name.to_string(), VarType::Scalar)
}

/// Add the recipes of `program` to the database. Included programs are read with `load`.
pub fn resolve<W: Worker<Job>>(
    db: &Arc<TaskDb>, kernel: &mut Kernel<Job>, worker: &W, program: Program,
    load: &dyn Fn(&Path) -> Result<Program, String>,
) -> Result<(), ProgramError> {
    Resolver { db, kernel, worker, load, templates: HashMap::new() }.resolve(program)
}

impl<'a, W: Worker<Job>> Resolver<'a, W> {
    <<resolver-values>>
    <<resolver-resolve>>
}
```

To get the values of variables, we build them all at once, and then ask for them one by one. The second time, the values come straight from the memo of the kernel. Variables have to have a value: a rule that creates a variable but didn't need to run gives `Nothing`.

```rust
//| id: resolver-values
fn values(&mut self, targets: &[Target]) -> Result<Vec<Value>, ProgramError> {
    let ids = targets.iter().map(|t| self.db.targets.intern(t)).collect::<Result<Vec<_>, _>>()?;
    build_all(self.db, self.kernel, self.worker, &ids)?;
    ids.iter().map(|id| Ok(build(self.db, self.kernel, self.worker, *id)?)).collect()
}

/// The values of scalar variables.
fn env<'s>(&mut self, names: impl IntoIterator<Item=&'s str>) -> Result<Env, ProgramError> {
    let targets: Vec<Target> = names.into_iter().map(scalar).collect();
    let values = self.values(&targets)?;
    targets.into_iter().zip(values).map(|(t, v)| match (t, v) {
        (Target::Variable(name, _), Value::Scalar(s)) => Ok((name, s)),
        (t, _) => Err(ProgramError::NoValue(t)),
    }).collect()
}

/// The values of list variables.
fn lists<'s>(&mut self, names: impl IntoIterator<Item=&'s str>) -> Result<HashMap<String, Vec<String>>, ProgramError> {
    let targets: Vec<Target> = names.into_iter().map(|n| Target::Variable(n.to_string(), VarType::List)).collect();
    let values = self.values(&targets)?;
    targets.into_iter().zip(values).map(|(t, v)| match (t, v) {
        (Target::Variable(name, _), Value::List(l)) => Ok((name, l)),
        (t, _) => Err(ProgramError::NoValue(t)),
    }).collect()
}
```

The values of variables are literal strings, but filling them into a rule gives a template again, so they are escaped.

```rust
//| id: resolver-resolve
/// Whether the variables in the targets of `rule` can be computed.
fn resolvable(&self, rule: &RuleTemplate) -> bool {
    rule.target_variables().into_iter().all(|v| self.db.creates(&scalar(v)))
}

/// Add the rules with resolvable targets, and give back the others.
fn resolve_delayed(&mut self, delayed: Vec<RuleTemplate>) -> Result<Vec<RuleTemplate>, ProgramError> {
    let (ready, waiting): (Vec<_>, Vec<_>) = delayed.into_iter().partition(|r| self.resolvable(r));
    let names: BTreeSet<String> = ready.iter().flat_map(|r| r.target_variables()).map(String::from).collect();
    let env: Env = self.env(names.iter().map(String::as_str))?.into_iter().map(|(k, v)| (k, escape(&v))).collect();
    for rule in ready {
        self.db.add_template(rule.fill_targets(&env))?;
    }
    Ok(waiting)
}

/// Add rules, or delay them if their targets have variables.
fn add(&mut self, rules: Vec<RuleTemplate>, delayed: &mut Vec<RuleTemplate>) -> Result<(), ProgramError> {
    for rule in rules {
        if rule.target_variables().is_empty() {
            self.db.add_template(rule)?;
        } else {
            delayed.push(rule);
        }
    }
    Ok(())
}

/// Replace `splice(name)` in the arguments with the elements of the list variable.
fn splice(&mut self, call: Call) -> Result<Call, ProgramError> {
    let lists = self.lists(call.splices())?;
    let expand = |s: &String| match splice_name(s) {
        Some(name) => lists[name].clone(),
        None => vec![s.clone()],
    };
    let args = call.args.iter().map(|(k, arg)| (k.clone(), match arg {
        Argument::Single(s) if splice_name(s).is_some() => Argument::Multiple(expand(s)),
        Argument::Single(s) => Argument::Single(s.clone()),
        Argument::Multiple(l) => Argument::Multiple(l.iter().flat_map(expand).collect()),
    })).collect();
    Ok(Call { args, ..call })
}

fn include(&mut self, include: &str) -> Result<(), ProgramError> {
    let env = self.env(variables(include))?;
    let path = PathBuf::from(substitute(include, &env).expect("all variables are known"));
    let target = Target::path(&path);
    if self.db.creates(&target) {
        self.values(&[target])?;
    }
    let full = self.db.root.join(&path);
    if !full.exists() {
        return Err(ProgramError::MissingInclude(path));
    }
    let program = (self.load)(&full).map_err(|e| ProgramError::Load(path, e))?;
    self.resolve(program)
}

fn resolve(&mut self, program: Program) -> Result<(), ProgramError> {
    for (name, template) in &program.environment {
        self.db.add_variable(name, template)?;
    }
    for (name, runner) in program.runner {
        self.db.add_runner(&name, runner);
    }
    self.templates.extend(program.template);

    let mut rules = program.task;
    let mut delayed_calls = Vec::new();
    for call in program.call {
        match self.templates.get(&call.template) {
            Some(template) if call.splices().is_empty() => rules.extend(call.expand(template)?),
            _ => delayed_calls.push(call),
        }
    }
    let mut delayed = Vec::new();
    self.add(rules, &mut delayed)?;
    delayed = self.resolve_delayed(delayed)?;

    for include in &program.include {
        self.include(include)?;
    }

    for call in delayed_calls {
        let Some(template) = self.templates.get(&call.template).cloned() else {
            return Err(ProgramError::MissingTemplate(call.template));
        };
        let rules = self.splice(call)?.expand(&template)?;
        self.add(rules, &mut delayed)?;
    }

    delayed = self.resolve_delayed(delayed)?;
    if !delayed.is_empty() {
        let targets = delayed.iter().flat_map(|r| r.targets()).collect();
        return Err(ProgramError::Unresolvable(targets));
    }
    Ok(())
}
```

Tests
-----

```rust
//| id: program-tests
use std::fs;
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;
use crate::noodles::kernel::Kernel;
use crate::noodles::single_worker::SingleWorker;
use crate::tasks::{TaskDb, RuleTemplate, Value, build_all, build};
use super::*;

fn strings(ss: &[&str]) -> Vec<String> {
    ss.iter().map(|s| s.to_string()).collect()
}

fn rule(creates: &[&str], requires: &[&str], script: &str) -> RuleTemplate {
    RuleTemplate { creates: strings(creates), requires: strings(requires), script: Some(script.into()), ..Default::default() }
}

fn bash(creates: &[&str], requires: &[&str], script: &str) -> RuleTemplate {
    RuleTemplate { runner: Some("bash".into()), ..rule(creates, requires, script) }
}

fn call(template: &str, args: &[(&str, Argument)]) -> Call {
    let args = args.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
    Call { template: template.into(), args, ..Default::default() }
}

fn single(s: &str) -> Argument {
    Argument::Single(s.into())
}

fn multiple(ss: &[&str]) -> Argument {
    Argument::Multiple(strings(ss))
}

fn no_includes(path: &Path) -> Result<Program, String> {
    Err(format!("unexpected include {}", path.display()))
}

struct Setup {
    dir: TempDir,
    db: Arc<TaskDb>,
    kernel: Kernel<Job>,
    worker: SingleWorker<Job>,
}

impl Setup {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let db = Arc::new(TaskDb::new(dir.path()));
        Setup { dir, db, kernel: Kernel::new(), worker: SingleWorker::new() }
    }

    fn resolve(&mut self, program: Program, load: &dyn Fn(&Path) -> Result<Program, String>) -> Result<(), ProgramError> {
        resolve(&self.db, &mut self.kernel, &self.worker, program, load)
    }

    fn build(&mut self, targets: &[&str]) {
        let ids: Vec<_> = targets.iter().map(|t| self.db.targets.lookup(&t.parse().unwrap()).unwrap()).collect();
        build_all(&self.db, &mut self.kernel, &self.worker, &ids).unwrap();
    }

    fn value(&mut self, target: &str) -> Value {
        let id = self.db.targets.lookup(&target.parse().unwrap()).unwrap();
        build(&self.db, &mut self.kernel, &self.worker, id).unwrap()
    }

    fn read(&self, path: &str) -> String {
        fs::read_to_string(self.dir.path().join(path)).unwrap()
    }
}
```

Calls fill in templates. Arguments can use variables from the environment, and lists give a rule for each element. With `collect`, a phony target builds them all.

```rust
//| id: program-tests
#[test]
fn calls() {
    let mut s = Setup::new();
    let program = Program {
        environment: [("ext".to_string(), "txt".to_string())].into(),
        template: [("write".to_string(), rule(&["${name}.${ext}"], &[], "touch ${name}.${ext}"))].into(),
        call: vec![
            Call { collect: Some("letters".into()), ..call("write", &[("name", multiple(&["a", "b"]))]) },
            call("write", &[("name", single("c"))]),
        ],
        ..Default::default()
    };
    s.resolve(program, &no_includes).unwrap();
    s.build(&["#letters", "c.txt"]);
    for f in ["a.txt", "b.txt", "c.txt"] {
        assert!(s.dir.path().join(f).exists());
    }
}

#[test]
fn joins() {
    let args = [("x", multiple(&["1", "2"])), ("y", multiple(&["a", "b"])), ("z", single("-"))];
    let inner = call("t", &args).instances().unwrap();
    let pairs: Vec<String> = inner.iter().map(|e| format!("{}{}{}", e["x"], e["y"], e["z"])).collect();
    assert_eq!(pairs, ["1a-", "2b-"]);

    let outer = Call { join: Join::Outer, ..call("t", &args) }.instances().unwrap();
    let pairs: Vec<String> = outer.iter().map(|e| format!("{}{}", e["x"], e["y"])).collect();
    assert_eq!(pairs, ["1a", "1b", "2a", "2b"]);

    let uneven = call("t", &[("x", multiple(&["1", "2"])), ("y", multiple(&["a"]))]);
    assert!(matches!(uneven.instances(), Err(ProgramError::Call(..))));
}
```

A rule with a variable in its targets waits until the variable is known. The rule that computes the variable runs only once, also when its value is needed again to build the targets.

```rust
//| id: program-tests
#[test]
fn delayed_targets() {
    let mut s = Setup::new();
    let program = Program {
        task: vec![
            RuleTemplate { stdout: Some("var(dir)".into()), ..bash(&[], &[], "echo run >> log.txt; mkdir -p out; echo out") },
            RuleTemplate { name: Some("all".into()), ..rule(&[], &["${dir}/msg.txt"], "") },
        ],
        template: [("echo".to_string(), RuleTemplate { stdout: Some("${out}".into()), ..rule(&[], &[], "echo ${text}") })].into(),
        call: vec![call("echo", &[("out", single("${dir}/msg.txt")), ("text", single("$$hello"))])],
        ..Default::default()
    };
    s.resolve(program, &no_includes).unwrap();
    s.build(&["#all"]);
    assert_eq!(s.read("out/msg.txt"), "$hello\n");
    assert_eq!(s.read("log.txt"), "run\n");
    assert_eq!(s.value("var(dir)"), Value::Scalar("out".into()));
}
```

Includes can be built by a rule, and their name can use variables. A call can use a template from an include.

```rust
//| id: program-tests
#[test]
fn includes() {
    let mut s = Setup::new();
    let program = Program {
        environment: [("name".to_string(), "generated".to_string())].into(),
        task: vec![RuleTemplate { stdout: Some("generated.txt".into()), ..rule(&[], &[], "echo hello") }],
        include: vec!["${name}.txt".into()],
        call: vec![call("greet", &[("who", single("world"))])],
        ..Default::default()
    };
    let load = |path: &Path| -> Result<Program, String> {
        let greeting = fs::read_to_string(path).map_err(|e| e.to_string())?;
        let greet = RuleTemplate { stdout: Some("var(greeting)".into()), ..rule(&[], &[], &format!("echo {} ${{who}}", greeting.trim())) };
        Ok(Program { template: [("greet".to_string(), greet)].into(), ..Default::default() })
    };
    s.resolve(program, &load).unwrap();
    assert_eq!(s.value("var(greeting)"), Value::Scalar("hello world".into()));
}
```

A call can splice list variables into its arguments.

```rust
//| id: program-tests
#[test]
fn splice_arguments() {
    let mut s = Setup::new();
    let program = Program {
        task: vec![RuleTemplate { stdout: Some("listvar(names)".into()), ..rule(&[], &[], r"printf 'a\nb\n'") }],
        template: [("touch".to_string(), rule(&["${name}.txt"], &[], "touch ${name}.txt"))].into(),
        call: vec![Call {
            collect: Some("all".into()),
            ..call("touch", &[("name", multiple(&["splice(names)", "c"]))])
        }],
        ..Default::default()
    };
    s.resolve(program, &no_includes).unwrap();
    s.build(&["#all"]);
    for f in ["a.txt", "b.txt", "c.txt"] {
        assert!(s.dir.path().join(f).exists());
    }
}
```

Missing templates, missing includes and targets that can't be resolved are errors.

```rust
//| id: program-tests
#[test]
fn errors() {
    let missing_template = Program { call: vec![call("nothere", &[])], ..Default::default() };
    assert!(matches!(Setup::new().resolve(missing_template, &no_includes), Err(ProgramError::MissingTemplate(t)) if t == "nothere"));

    let missing_include = Program { include: vec!["nothere.toml".into()], ..Default::default() };
    assert!(matches!(Setup::new().resolve(missing_include, &no_includes), Err(ProgramError::MissingInclude(_))));

    let unresolvable = Program { task: vec![rule(&["${x}.txt"], &[], "true")], ..Default::default() };
    let err = Setup::new().resolve(unresolvable, &no_includes).unwrap_err();
    assert_eq!(err.to_string(), "rules have unresolvable targets: ${x}.txt");

    let failing = Program {
        task: vec![
            RuleTemplate { stdout: Some("var(x)".into()), ..rule(&[], &[], "false") },
            rule(&["${x}.txt"], &[], "true"),
        ],
        ..Default::default()
    };
    let err = Setup::new().resolve(failing, &no_includes).unwrap_err();
    assert_eq!(err.to_string(), "`false` failed with exit code 1");
}
```
