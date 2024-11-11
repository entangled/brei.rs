use std::collections::{HashMap, HashSet};
use std::iter::Iterator;
use std::hash::Hash;


pub trait Task: Sized {
    type Value: Clone;
    type Key: Copy + Hash + Eq;
    type Error: Clone;

    fn key(&self) -> Self::Key;
    fn run(&self) -> Trampoline<Self>;
    fn assoc(&mut self, key: Self::Key, value: Self::Value);
    fn ready(&self) -> bool;
    // fn dependencies(&self) -> impl Iterator<Item=Self::Key>;
}

pub enum Trampoline<T: Task> {
    Value(T::Value),
    Continuation(Box<Promise<T>>),
    Error(T::Error)
}

pub enum InternalError {
    SendError(String),
    NoResult
}

pub enum Error<T: Task> {
    Job(T::Error),
    Internal(InternalError)
}

pub struct Promise<T: Task> {
    pub root: T::Key,
    pub tasks: HashMap<T::Key, T>,
    pub inverse_deps: HashMap<T::Key, HashSet<T::Key>>
}

pub trait Worker<T: Task> {
    fn submit(&self, frame_id: u64, task: &T) -> Result<(), InternalError>;
    fn listen(&self) -> impl Iterator<Item=(u64, T::Key, Trampoline<T>)>;
}
