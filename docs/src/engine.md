Engine
======

The engine sends out tasks to get responses. A response is one of: a result, a continuation, or an error. The continuation should contain a promise, that is a graph of dependent tasks the root of which gives a new response, closing the recursive loop.

Every time a task is complete, its key is looked up in an inverse dependency map, and the result value maybe associated with a dependent task. If that task has all dependencies fulfilled, it may report ready.

```rust
//| file: src/noodles/mod.rs
pub mod task;
pub mod kernel;
pub mod single_worker;

#[cfg(test)]
mod mock;
```

```rust
//| file: src/noodles/task.rs
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::iter::Iterator;
use std::hash::Hash;


pub trait Task: Sized {
    type Value: Clone;
    type Key: Copy + Hash + Eq;
    type Error: Clone;

    fn key(&self) -> Self::Key;
    fn dependencies(&self) -> impl Iterator<Item=Self::Key>;
    fn run(self) -> Response<Self>;
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
    Job(T::Error),
    Internal(InternalError)
}

impl<T: Task> fmt::Debug for Error<T> where T::Error: fmt::Debug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Job(e) => f.debug_tuple("Job").field(e).finish(),
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
}
```

The kernel keeps a stack of frames, although it is not really a stack but a tree: every promise that is being evaluated lives in its own frame. Keys only need to be unique within a frame. A frame remembers where its result should go: the frame and key of the task that gave the continuation, or nothing if it is the top-level frame.

A task is run only once, so as soon as it is ready the kernel takes it out of its frame and hands it to the worker.

When a task that is *not* the root of its frame gives a continuation, the new frame gets that task as parent. When the root of a frame gives a continuation, the new frame replaces the old one and inherits its parent. This is the equivalent of tail-call optimisation: a chain of tail calls doesn't build up frames.

```rust
//| file: src/noodles/kernel.rs
use std::boxed::Box;
use std::collections::HashMap;
use super::task::{Task, Promise, Worker, InternalError, Error, Response};

struct Frame<T: Task> {
    parent: Option<(u64, T::Key)>,
    promise: Box<Promise<T>>,
}

pub struct Kernel<T: Task> {
    frame_no: u64,
    frames: HashMap<u64, Frame<T>>,
}

impl<T: Task> Kernel<T> {
    pub fn new() -> Self {
        Self { frame_no: 1, frames: HashMap::new() }
    }

    /// Number of frames that are still alive.
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    fn add<W: Worker<T>>(&mut self, worker: &W, parent: Option<(u64, T::Key)>, mut promise: Box<Promise<T>>) -> Result<(), InternalError> {
        let frame_id = self.frame_no;
        self.frame_no += 1;

        let ready: Vec<T::Key> = promise.tasks.iter()
            .filter(|(_, task)| task.ready())
            .map(|(key, _)| *key)
            .collect();
        for key in ready {
            worker.submit(frame_id, promise.tasks.remove(&key).unwrap())?;
        }

        self.frames.insert(frame_id, Frame { parent, promise });
        Ok(())
    }

    fn handle_result<W: Worker<T>>(&mut self, worker: &W, frame_id: u64, key: T::Key, result: T::Value) -> Result<(), InternalError> {
        let frame = self.frames.get_mut(&frame_id).ok_or(InternalError::MissingFrame(frame_id))?;
        let Some(dependents) = frame.promise.inverse_deps.get(&key) else {
            return Ok(());
        };
        for dep in dependents {
            let Some(task) = frame.promise.tasks.get_mut(dep) else { continue };
            task.assoc(key, result.clone());
            if task.ready() {
                worker.submit(frame_id, frame.promise.tasks.remove(dep).unwrap())?;
            }
        }
        Ok(())
    }

    pub fn run<W: Worker<T>>(&mut self, worker: &W, promise: Box<Promise<T>>) -> Result<T::Value, Error<T>> {
        self.add(worker, None, promise).map_err(Error::Internal)?;

        for (frame_id, key, response) in worker.listen() {
            // Results for frames that were already discarded are ignored.
            let Some(frame) = self.frames.get(&frame_id) else { continue };
            let is_root = key == frame.promise.root;
            // The root of a frame reports to the parent; other tasks report to their own frame.
            let target = if is_root { frame.parent } else { Some((frame_id, key)) };
            if is_root {
                self.frames.remove(&frame_id);
            }

            match response {
                Response::Value(v) => match target {
                    None => return Ok(v),
                    Some((target_frame, target_key)) =>
                        self.handle_result(worker, target_frame, target_key, v).map_err(Error::Internal)?,
                },
                Response::Continuation(c) => {
                    self.add(worker, target, c).map_err(Error::Internal)?;
                },
                Response::Error(e) => {
                    self.frames.clear();
                    return Err(Error::Job(e));
                }
            }
        }

        self.frames.clear();
        Err(Error::Internal(InternalError::NoResult))
    }
}
```

Single Worker
-------------

The single worker has a single sender-receiver pair. When tasks are submitted they are immediately executed and the result is push on the channel. Since all work happens inside `submit`, `listen` doesn't need to block: if the channel is empty, nothing is left to do and the kernel can report that no result was found.

```rust
//| file: src/noodles/single_worker.rs
use super::task::{Task, Response, Worker, InternalError};
use crossbeam_channel::{Sender, Receiver, unbounded};

pub struct SingleWorker<T: Task> {
    sender: Sender<(u64, T::Key, Response<T>)>,
    receiver: Receiver<(u64, T::Key, Response<T>)>
}

impl<T: Task> SingleWorker<T> {
    pub fn new() -> Self {
        let (sender, receiver) = unbounded();
        return SingleWorker{ sender, receiver }
    }
}

impl<T: Task> Worker<T> for SingleWorker<T> {
    fn submit(&self, frame_id: u64, task: T) -> Result<(), InternalError> {
        let key = task.key();
        self.sender.send((frame_id, key, task.run())).map_err(|e| InternalError::SendError(e.to_string()))
    }

    fn listen(&self) -> impl Iterator<Item=(u64, T::Key, Response<T>)> {
        self.receiver.try_iter()
    }
}
```

Mock tasks
----------

To test the engine, we have a mock task that computes on integers. Each task has a `u64` key, a list of dependencies and a function that takes the values of its dependencies (in order) and gives a `Response`. Since the function may return a continuation, we can have recursion.

```rust
//| file: src/noodles/mock.rs
use std::collections::HashMap;
use std::sync::Arc;
use super::task::{Task, Promise, Response, Error, InternalError};
use super::kernel::Kernel;
use super::single_worker::SingleWorker;

type Func = Arc<dyn Fn(&[i64]) -> Response<Mock> + Send + Sync>;

struct Mock {
    key: u64,
    deps: Vec<u64>,
    args: HashMap<u64, i64>,
    func: Func,
}

impl Mock {
    fn new(key: u64, deps: &[u64], func: impl Fn(&[i64]) -> Response<Mock> + Send + Sync + 'static) -> Self {
        Mock { key, deps: deps.to_vec(), args: HashMap::new(), func: Arc::new(func) }
    }
}

impl Task for Mock {
    type Value = i64;
    type Key = u64;
    type Error = String;

    fn key(&self) -> u64 { self.key }

    fn dependencies(&self) -> impl Iterator<Item=u64> {
        self.deps.iter().copied()
    }

    fn run(self) -> Response<Self> {
        let args: Vec<i64> = self.deps.iter().map(|k| self.args[k]).collect();
        (self.func)(&args)
    }

    fn assoc(&mut self, key: u64, value: i64) {
        self.args.insert(key, value);
    }

    fn ready(&self) -> bool {
        self.deps.iter().all(|k| self.args.contains_key(k))
    }
}

<<mock-helpers>>
<<mock-recursion>>

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(p: Box<Promise<Mock>>) -> Result<i64, Error<Mock>> {
        let worker = SingleWorker::new();
        let mut kernel = Kernel::new();
        let result = kernel.run(&worker, p);
        assert_eq!(kernel.frame_count(), 0);
        result
    }

    <<mock-tests>>
}
```

Some shorthands for building promises and tasks.

```rust
//| id: mock-helpers
fn promise(root: u64, tasks: Vec<Mock>) -> Box<Promise<Mock>> {
    Box::new(Promise::new(root, tasks))
}

fn constant(key: u64, value: i64) -> Mock {
    Mock::new(key, &[], move |_| Response::Value(value))
}

fn binop(key: u64, a: u64, b: u64, f: fn(i64, i64) -> i64) -> Mock {
    Mock::new(key, &[a, b], move |x| Response::Value(f(x[0], x[1])))
}
```

### Primitive computations

A diamond shaped graph: `(2 + 3) * (2 * 3)`.

```rust
//| id: mock-tests
#[test]
fn diamond() {
    let p = promise(5, vec![
        constant(1, 2),
        constant(2, 3),
        binop(3, 1, 2, |a, b| a + b),
        binop(4, 1, 2, |a, b| a * b),
        binop(5, 3, 4, |a, b| a * b),
    ]);
    assert_eq!(eval(p).unwrap(), 30);
}
```

Errors are propagated to the caller.

```rust
//| id: mock-tests
#[test]
fn error() {
    let p = promise(3, vec![
        constant(1, 1),
        constant(2, 0),
        Mock::new(3, &[1, 2], |x| if x[1] == 0 {
            Response::Error("division by zero".into())
        } else {
            Response::Value(x[0] / x[1])
        }),
    ]);
    assert!(matches!(eval(p), Err(Error::Job(e)) if e == "division by zero"));
}
```

If the graph can never complete, because the root depends on a task that doesn't exist, the single worker runs dry and we get `NoResult`.

```rust
//| id: mock-tests
#[test]
fn missing_dependency() {
    let p = promise(2, vec![binop(2, 1, 3, |a, b| a + b), constant(1, 1)]);
    assert!(matches!(eval(p), Err(Error::Internal(InternalError::NoResult))));
}
```

### Recursion

The factorial is computed by having `factorial(n)` return a continuation: a promise to compute `n * factorial(n - 1)`. The task for `factorial(n - 1)` is not the root of the new frame, so its result is fed into the multiplication. Note that every frame reuses the keys `1` and `2`.

```rust
//| id: mock-recursion
fn factorial(key: u64, n: i64) -> Mock {
    Mock::new(key, &[], move |_| {
        if n <= 1 {
            return Response::Value(1);
        }
        Response::Continuation(promise(2, vec![
            factorial(1, n - 1),
            Mock::new(2, &[1], move |x| Response::Value(n * x[0])),
        ]))
    })
}
```

The tail-recursive version passes an accumulator. Here the continuation consists of only a root task, so the kernel replaces frames instead of nesting them.

```rust
//| id: mock-recursion
fn factorial_acc(key: u64, n: i64, acc: i64) -> Mock {
    Mock::new(key, &[], move |_| {
        if n <= 1 {
            return Response::Value(acc);
        }
        Response::Continuation(promise(1, vec![factorial_acc(1, n - 1, acc * n)]))
    })
}
```

Fibonacci has two recursive calls in each continuation.

```rust
//| id: mock-recursion
fn fibonacci(key: u64, n: i64) -> Mock {
    Mock::new(key, &[], move |_| {
        if n < 2 {
            return Response::Value(n);
        }
        Response::Continuation(promise(3, vec![
            fibonacci(1, n - 1),
            fibonacci(2, n - 2),
            binop(3, 1, 2, |a, b| a + b),
        ]))
    })
}

fn sum_to(key: u64, n: i64) -> Mock {
    Mock::new(key, &[], move |_| {
        if n == 0 {
            return Response::Value(0);
        }
        Response::Continuation(promise(2, vec![
            sum_to(1, n - 1),
            Mock::new(2, &[1], move |x| Response::Value(n + x[0])),
        ]))
    })
}
```

```rust
//| id: mock-tests
#[test]
fn recursion() {
    for (n, f) in [(0, 1), (1, 1), (5, 120), (20, 2432902008176640000)] {
        assert_eq!(eval(promise(0, vec![factorial(0, n)])).unwrap(), f);
        assert_eq!(eval(promise(0, vec![factorial_acc(0, n, 1)])).unwrap(), f);
    }
    assert_eq!(eval(promise(0, vec![fibonacci(0, 20)])).unwrap(), 6765);
}
```

Recursion doesn't use the Rust stack, so we can go very deep without overflowing.

```rust
//| id: mock-tests
#[test]
fn deep_recursion() {
    let n = 100_000;
    assert_eq!(eval(promise(0, vec![sum_to(0, n)])).unwrap(), n * (n + 1) / 2);
}
```
