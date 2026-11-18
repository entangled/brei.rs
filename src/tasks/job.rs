// ~/~ begin <<docs/src/brei.md#brei::src/tasks/job.rs>>[init]
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use crate::noodles::task::{Task, Response, Promise, CancelToken, Error, Worker};
use crate::noodles::kernel::Kernel;
use super::db::TaskDb;
use super::rule::{Value, Failure};
use super::target::{Target, TargetId, Key};

pub enum Job {
    Build { db: Arc<TaskDb>, target: TargetId },
    Run { db: Arc<TaskDb>, rule: usize, deps: Vec<TargetId>, args: HashMap<TargetId, Value> },
    Alias { target: TargetId, value: Option<Value> },
    All { targets: Vec<TargetId>, done: HashSet<TargetId> },
}

impl Job {
    pub fn build(db: &Arc<TaskDb>, target: TargetId) -> Self {
        Job::Build { db: db.clone(), target }
    }
}

fn build_target(db: Arc<TaskDb>, target: TargetId) -> Response<Job> {
    let Some(n) = db.rule_for(target) else {
        return match db.targets.get(target) {
            Target::Path(p) if db.root.join(&p).exists() => Response::Value(Value::Nothing),
            t => Response::Error(Failure::Missing(t)),
        };
    };

    let entry = db.entry(n);
    let main = entry.creates[0];
    let tasks = if main != target {
        vec![Job::build(&db, main), Job::Alias { target: main, value: None }]
    } else {
        let mut tasks: Vec<Job> = entry.requires.iter().map(|dep| Job::build(&db, *dep)).collect();
        tasks.push(Job::Run { db: db.clone(), rule: n, deps: entry.requires.clone(), args: HashMap::new() });
        tasks
    };
    Response::Continuation(Box::new(Promise::new(Key::Local(0), tasks)))
}

fn run_rule(db: Arc<TaskDb>, n: usize, args: HashMap<TargetId, Value>, cancel: &CancelToken) -> Response<Job> {
    let entry = db.entry(n);
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
            Job::Run { .. } | Job::Alias { .. } | Job::All { .. } => Key::Local(0),
        }
    }

    fn dependencies(&self) -> impl Iterator<Item=Key> {
        let deps: Vec<Key> = match self {
            Job::Build { .. } => Vec::new(),
            Job::Run { deps, .. } => deps.iter().map(|d| Key::Target(*d)).collect(),
            Job::Alias { target, .. } => vec![Key::Target(*target)],
            Job::All { targets, .. } => targets.iter().map(|t| Key::Target(*t)).collect(),
        };
        deps.into_iter()
    }

    fn memoize(&self) -> bool {
        self.key().is_global()
    }

    fn run(self, cancel: &CancelToken) -> Response<Self> {
        match self {
            Job::Build { db, target } => build_target(db, target),
            Job::Run { db, rule, args, .. } => run_rule(db, rule, args, cancel),
            Job::Alias { value, .. } => Response::Value(value.expect("alias is ready")),
            Job::All { .. } => Response::Value(Value::Nothing),
        }
    }

    fn assoc(&mut self, key: Key, value: Value) {
        match (self, key) {
            (Job::Run { args, .. }, Key::Target(id)) => { args.insert(id, value); }
            (Job::Alias { value: v, .. }, _) => *v = Some(value),
            (Job::All { done, .. }, Key::Target(id)) => { done.insert(id); }
            _ => {}
        }
    }

    fn ready(&self) -> bool {
        match self {
            Job::Build { .. } => true,
            Job::Run { deps, args, .. } => args.len() == deps.len(),
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
