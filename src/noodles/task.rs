// ~/~ begin <<docs/src/engine.md#src/noodles/task.rs>>[init]
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::iter::Iterator;
use std::hash::Hash;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};


pub trait Task: Sized {
    type Value: Clone;
    type Key: Copy + Hash + Eq;
    type Error: Clone;

    fn key(&self) -> Self::Key;
    fn dependencies(&self) -> impl Iterator<Item=Self::Key>;
    fn memoize(&self) -> bool;
    fn run(self, cancel: &CancelToken) -> Response<Self>;
    fn assoc(&mut self, key: Self::Key, value: Self::Value);
    fn ready(&self) -> bool;
}

pub enum Response<T: Task> {
    Value(T::Value),
    Continuation(Box<Promise<T>>),
    Error(T::Error)
}

#[derive(Debug)]
pub enum InternalError {
    SendError(String),
    MissingFrame(u64),
    NoResult
}

pub enum Error<T: Task> {
    Jobs(Vec<T::Error>),
    Cycle(Vec<T::Key>),
    Internal(InternalError)
}

impl<T: Task> From<InternalError> for Error<T> {
    fn from(e: InternalError) -> Self {
        Error::Internal(e)
    }
}

impl<T: Task> fmt::Debug for Error<T> where T::Error: fmt::Debug, T::Key: fmt::Debug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Jobs(e) => f.debug_tuple("Jobs").field(e).finish(),
            Error::Cycle(c) => f.debug_tuple("Cycle").field(c).finish(),
            Error::Internal(e) => f.debug_tuple("Internal").field(e).finish(),
        }
    }
}

pub struct Promise<T: Task> {
    pub root: T::Key,
    pub tasks: HashMap<T::Key, T>,
    pub inverse_deps: HashMap<T::Key, HashSet<T::Key>>
}

impl<T: Task> Promise<T> {
    pub fn new(root: T::Key, tasks: impl IntoIterator<Item=T>) -> Self {
        let mut inverse_deps: HashMap<T::Key, HashSet<T::Key>> = HashMap::new();
        let tasks: HashMap<T::Key, T> = tasks.into_iter().map(|t| (t.key(), t)).collect();
        for (key, task) in &tasks {
            for dep in task.dependencies() {
                inverse_deps.entry(dep).or_default().insert(*key);
            }
        }
        Promise { root, tasks, inverse_deps }
    }
}

pub trait Worker<T: Task> {
    fn submit(&self, frame_id: u64, task: T) -> Result<(), InternalError>;
    fn listen(&self) -> impl Iterator<Item=(u64, T::Key, Response<T>)>;
    fn cancel(&self);
}

// ~/~ begin <<docs/src/engine.md#cancel-token>>[init]
#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}
// ~/~ end
// ~/~ end
