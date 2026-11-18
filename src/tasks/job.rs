// ~/~ begin <<docs/src/brei.md#brei::src/tasks/job.rs>>[init]
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

// ~/~ begin <<docs/src/brei.md#brei::job-expand-run>>[init]
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
// ~/~ end
// ~/~ end
