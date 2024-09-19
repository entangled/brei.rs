use std::path::PathBuf;
use std::sync::LazyLock;
use std::ops::Fn;
use std::collections::{HashSet, HashMap};

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub enum Target {
    File(PathBuf),
    Variable(String),
    Phony(String)
}

pub struct Output {
    return_code: i64,
    stdout: String,
    stderr: String
}

pub enum Error {
    CyclicDependency,
    MissingDependency(Target),
    FailedScript(Output),
    TargetsNotRealized(Output)
}

pub type Result<T> = core::result::Result<T, Error>;

pub enum Trampoline<T: Task> {
    Output(T::Output),
    Task(T)
}

trait Task: Sized {
    type Target;
    type Output;

    fn creates(&self) -> &Vec<Target>;
    fn requires(&self) -> &Vec<Target>;

    fn run(&self) -> Result<Trampoline<Self>>;
}

pub struct TaskDB<T: Task>{
    tasks: Vec<T>,
    index: HashMap<Target, usize>
}

impl<T: Task> TaskDB {
}
