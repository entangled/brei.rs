Engine
======

The engine sends out tasks to get responses. A response is one of: a result, a continuation, or an error. The continuation should contain a promise, that is a graph of dependent tasks the root of which gives a new response, closing the recursive loop.

Every time a task is complete, its key is looked up in an inverse dependency map, and the result value maybe associated with a dependent task. If that task has all dependencies fulfilled, it may report ready.

```rust
//| file: src/noodles/mod.rs
pub mod task;
pub mod kernel;
pub mod single_worker;
pub mod worker_pool;

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
    fn memoize(&self) -> bool;
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

The kernel keeps a stack of frames, although it is not really a stack but a tree: every promise that is being evaluated lives in its own frame. A frame remembers where its result should go: the frame and key of the task that gave the continuation, or nothing if it is the top-level frame.

A task is run only once, so as soon as it is ready the kernel takes it out of its frame and hands it to the worker.

When a task that is *not* the root of its frame gives a continuation, the new frame gets that task as parent. When the root of a frame gives a continuation, the new frame replaces the old one and inherits its parent. This is the equivalent of tail-call optimisation: a chain of tail calls doesn't build up frames.

### Memoization

Most keys only need to be unique within a frame: they name intermediate steps in a computation. Some tasks however have an identity that is shared by the entire graph. In Brei these are the targets: if two rules depend on the same file, that file should be built only once. A task says it is one of these by returning `true` from `memoize()`, and its key must then be unique among all memoized tasks.

The kernel keeps a memo of memoized keys. When a memoized task is ready, the kernel looks up its key:

- not present: the task is submitted, and the key is marked as pending, owned by this frame;
- pending: the frame is added to the list of waiters, and nothing is submitted;
- done: the value is delivered to the frame right away.

When the owner of a memoized key gets its final value (possibly after a continuation was resolved), the value is stored in the memo and delivered to all waiters. Because the owner needs to see that final value, a memoized root that gives a continuation doesn't get tail-call optimisation: its frame stays alive until the continuation is done.

Only memoized keys are stored, so a loop of local tail calls still runs in constant memory. The size of the memo is bounded by the number of distinct memoized tasks, which in Brei is the number of targets. The memo survives between calls to `run`, so a second run can reuse earlier results. When a run fails, pending entries are removed.

A memoized task that, through its continuation, ends up waiting for itself will never finish. With the single worker this ends in `NoResult`.

### Implementation

Delivering a value may cause other values to be delivered: the root of a frame finishing will deliver its value to the parent, and a memoized key will deliver to its waiters. Instead of recursing, the kernel keeps a queue of deliveries `(frame, key, value)` that is drained after every response from the worker.

```rust
//| file: src/noodles/kernel.rs
use std::boxed::Box;
use std::collections::{HashMap, VecDeque};
use super::task::{Task, Promise, Worker, InternalError, Error, Response};

struct Frame<T: Task> {
    parent: Option<(u64, T::Key)>,
    promise: Box<Promise<T>>,
}

enum State<T: Task> {
    Pending { owner: u64, waiters: Vec<u64> },
    Done(T::Value),
}

pub struct Kernel<T: Task> {
    frame_no: u64,
    frames: HashMap<u64, Frame<T>>,
    memo: HashMap<T::Key, State<T>>,
    queue: VecDeque<(u64, T::Key, T::Value)>,
}

impl<T: Task> Kernel<T> {
    pub fn new() -> Self {
        Self { frame_no: 1, frames: HashMap::new(), memo: HashMap::new(), queue: VecDeque::new() }
    }

    /// Number of frames that are still alive.
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    /// Number of entries in the memo.
    pub fn memo_count(&self) -> usize {
        self.memo.len()
    }

    /// Whether `frame_id` is responsible for computing the memoized `key`.
    fn owns(&self, frame_id: u64, key: T::Key) -> bool {
        matches!(self.memo.get(&key), Some(State::Pending { owner, .. }) if *owner == frame_id)
    }

    fn add<W: Worker<T>>(&mut self, worker: &W, parent: Option<(u64, T::Key)>, mut promise: Box<Promise<T>>) -> Result<(), InternalError> {
        let frame_id = self.frame_no;
        self.frame_no += 1;

        let ready: Vec<T::Key> = promise.tasks.iter()
            .filter(|(_, task)| task.ready())
            .map(|(key, _)| *key)
            .collect();
        let ready: Vec<T> = ready.iter().map(|key| promise.tasks.remove(key).unwrap()).collect();

        self.frames.insert(frame_id, Frame { parent, promise });
        for task in ready {
            self.schedule(worker, frame_id, task)?;
        }
        Ok(())
    }

    /// Submit a ready task, unless its result is (or will be) available from the memo.
    fn schedule<W: Worker<T>>(&mut self, worker: &W, frame_id: u64, task: T) -> Result<(), InternalError> {
        if task.memoize() {
            let key = task.key();
            match self.memo.get_mut(&key) {
                Some(State::Done(v)) => {
                    self.queue.push_back((frame_id, key, v.clone()));
                    return Ok(());
                }
                Some(State::Pending { waiters, .. }) => {
                    waiters.push(frame_id);
                    return Ok(());
                }
                None => {
                    self.memo.insert(key, State::Pending { owner: frame_id, waiters: Vec::new() });
                }
            }
        }
        worker.submit(frame_id, task)
    }

    /// Handle a response from the worker.
    fn respond<W: Worker<T>>(&mut self, worker: &W, frame_id: u64, key: T::Key, response: Response<T>) -> Result<(), Error<T>> {
        // Results for frames that were already discarded are ignored, unless the memo still needs them.
        if !self.frames.contains_key(&frame_id) && !self.owns(frame_id, key) {
            return Ok(());
        }

        match response {
            Response::Value(v) => {
                self.queue.push_back((frame_id, key, v));
                Ok(())
            }
            Response::Continuation(c) => {
                let tail_call = !self.owns(frame_id, key)
                    && self.frames.get(&frame_id).is_some_and(|f| f.promise.root == key);
                if tail_call {
                    let parent = self.frames.remove(&frame_id).unwrap().parent;
                    self.add(worker, parent, c).map_err(Error::Internal)
                } else {
                    self.add(worker, Some((frame_id, key)), c).map_err(Error::Internal)
                }
            }
            Response::Error(e) => Err(Error::Job(e)),
        }
    }

    /// Deliver the final value of `key` in frame `frame_id`. Returns the result of the top-level frame, once known.
    fn deliver<W: Worker<T>>(&mut self, worker: &W, frame_id: u64, key: T::Key, value: T::Value) -> Result<Option<T::Value>, InternalError> {
        if self.owns(frame_id, key) {
            if let Some(State::Pending { waiters, .. }) = self.memo.insert(key, State::Done(value.clone())) {
                for w in waiters {
                    self.queue.push_back((w, key, value.clone()));
                }
            }
        }

        let Some(frame) = self.frames.get_mut(&frame_id) else { return Ok(None) };

        // The root of a frame reports to the parent; other tasks report to their own frame.
        if key == frame.promise.root {
            let parent = frame.parent;
            self.frames.remove(&frame_id);
            return Ok(match parent {
                None => Some(value),
                Some((p, pk)) => {
                    self.queue.push_back((p, pk, value));
                    None
                }
            });
        }

        let mut ready = Vec::new();
        if let Some(dependents) = frame.promise.inverse_deps.get(&key) {
            for dep in dependents {
                let Some(task) = frame.promise.tasks.get_mut(dep) else { continue };
                task.assoc(key, value.clone());
                if task.ready() {
                    ready.push(*dep);
                }
            }
        }
        let ready: Vec<T> = ready.iter().map(|dep| frame.promise.tasks.remove(dep).unwrap()).collect();
        for task in ready {
            self.schedule(worker, frame_id, task)?;
        }
        Ok(None)
    }

    fn drain<W: Worker<T>>(&mut self, worker: &W) -> Result<Option<T::Value>, InternalError> {
        while let Some((frame_id, key, value)) = self.queue.pop_front() {
            if let Some(result) = self.deliver(worker, frame_id, key, value)? {
                return Ok(Some(result));
            }
        }
        Ok(None)
    }

    fn run_frames<W: Worker<T>>(&mut self, worker: &W, promise: Box<Promise<T>>) -> Result<T::Value, Error<T>> {
        self.add(worker, None, promise).map_err(Error::Internal)?;
        if let Some(result) = self.drain(worker).map_err(Error::Internal)? {
            return Ok(result);
        }

        for (frame_id, key, response) in worker.listen() {
            self.respond(worker, frame_id, key, response)?;
            if let Some(result) = self.drain(worker).map_err(Error::Internal)? {
                return Ok(result);
            }
        }

        Err(Error::Internal(InternalError::NoResult))
    }

    pub fn run<W: Worker<T>>(&mut self, worker: &W, promise: Box<Promise<T>>) -> Result<T::Value, Error<T>> {
        let result = self.run_frames(worker, promise);
        if result.is_err() {
            self.frames.clear();
            self.queue.clear();
            self.memo.retain(|_, state| matches!(state, State::Done(_)));
        }
        result
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

Worker Pool
-----------

The worker pool runs tasks on a fixed number of threads. Submitted tasks go into a shared channel, from which each thread takes the next task to run; responses come back through a second channel. Tasks, keys, values and errors now need to be `Send`, since they cross between threads.

The kernel needs to know when nothing is left to do, otherwise a graph that can't complete would block forever. The pool counts the number of tasks in flight: `submit` increments it, and receiving a response decrements it. When the count is zero the iterator returned by `listen` ends, just like the single worker.

If a task panics, the thread catches the panic and sends it back with the response. The panic is then resumed on the thread that called `listen`, so a panicking task has the same effect as with the single worker.

When the pool is dropped, the threads are told to stop: tasks that are still waiting in the channel are not run, but tasks that are running are allowed to finish.

```rust
//| file: src/noodles/worker_pool.rs
use std::cell::Cell;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use super::task::{Task, Response, Worker, InternalError};
use crossbeam_channel::{Sender, Receiver, unbounded};

type Outcome<T> = (u64, <T as Task>::Key, thread::Result<Response<T>>);

pub struct WorkerPool<T: Task> {
    tasks: Option<Sender<(u64, T)>>,
    results: Receiver<Outcome<T>>,
    in_flight: Cell<usize>,
    shutdown: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl<T> WorkerPool<T>
where T: Task + Send + 'static, T::Key: Send, T::Value: Send, T::Error: Send
{
    pub fn new(n_threads: usize) -> Self {
        let (task_sender, task_receiver) = unbounded::<(u64, T)>();
        let (result_sender, result_receiver) = unbounded::<Outcome<T>>();
        let shutdown = Arc::new(AtomicBool::new(false));

        let threads = (0..n_threads.max(1)).map(|_| {
            let tasks = task_receiver.clone();
            let results = result_sender.clone();
            let shutdown = shutdown.clone();
            thread::spawn(move || {
                for (frame_id, task) in tasks.iter() {
                    if shutdown.load(Ordering::Relaxed) {
                        break;
                    }
                    let key = task.key();
                    let response = panic::catch_unwind(AssertUnwindSafe(move || task.run()));
                    if results.send((frame_id, key, response)).is_err() {
                        break;
                    }
                }
            })
        }).collect();

        WorkerPool { tasks: Some(task_sender), results: result_receiver, in_flight: Cell::new(0), shutdown, threads }
    }

    /// A pool with as many threads as the machine can run in parallel.
    pub fn with_available_parallelism() -> Self {
        Self::new(thread::available_parallelism().map_or(1, |n| n.get()))
    }
}

impl<T> Worker<T> for WorkerPool<T>
where T: Task + Send + 'static, T::Key: Send, T::Value: Send, T::Error: Send
{
    fn submit(&self, frame_id: u64, task: T) -> Result<(), InternalError> {
        let sender = self.tasks.as_ref().ok_or(InternalError::SendError("worker pool is shut down".into()))?;
        sender.send((frame_id, task)).map_err(|e| InternalError::SendError(e.to_string()))?;
        self.in_flight.set(self.in_flight.get() + 1);
        Ok(())
    }

    fn listen(&self) -> impl Iterator<Item=(u64, T::Key, Response<T>)> {
        std::iter::from_fn(move || {
            if self.in_flight.get() == 0 {
                return None;
            }
            let (frame_id, key, response) = self.results.recv().ok()?;
            self.in_flight.set(self.in_flight.get() - 1);
            match response {
                Ok(response) => Some((frame_id, key, response)),
                Err(payload) => panic::resume_unwind(payload),
            }
        })
    }
}

impl<T: Task> Drop for WorkerPool<T> {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        self.tasks.take();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}
```

Mock tasks
----------

To test the engine, we have a mock task that computes on integers. Each task has a `u64` key, a list of dependencies and a function that takes the values of its dependencies (in order) and gives a `Response`. Since the function may return a continuation, we can have recursion. Tasks are not memoized, unless built with `.memoized()`.

```rust
//| file: src/noodles/mock.rs
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use super::task::{Task, Promise, Response, Error, InternalError};
use super::kernel::Kernel;
use super::single_worker::SingleWorker;
use super::worker_pool::WorkerPool;

type Func = Arc<dyn Fn(&[i64]) -> Response<Mock> + Send + Sync>;

struct Mock {
    key: u64,
    deps: Vec<u64>,
    args: HashMap<u64, i64>,
    memoize: bool,
    func: Func,
}

impl Mock {
    fn new(key: u64, deps: &[u64], func: impl Fn(&[i64]) -> Response<Mock> + Send + Sync + 'static) -> Self {
        Mock { key, deps: deps.to_vec(), args: HashMap::new(), memoize: false, func: Arc::new(func) }
    }

    fn memoized(mut self) -> Self {
        self.memoize = true;
        self
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

    fn memoize(&self) -> bool { self.memoize }

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
<<mock-memo>>

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

### Memoization

Fibonacci again, but now with memoized tasks: `fibonacci_memo(n)` has key `n`. The addition is a local task, so it gets a key that doesn't clash. A counter keeps track of how many times a Fibonacci task was actually run.

```rust
//| id: mock-memo
const SUM: u64 = u64::MAX;

fn fibonacci_memo(n: u64, runs: Arc<AtomicUsize>) -> Mock {
    Mock::new(n, &[], move |_| {
        runs.fetch_add(1, Ordering::Relaxed);
        if n < 2 {
            return Response::Value(n as i64);
        }
        Response::Continuation(promise(SUM, vec![
            fibonacci_memo(n - 1, runs.clone()),
            fibonacci_memo(n - 2, runs.clone()),
            binop(SUM, n - 1, n - 2, |a, b| a + b),
        ]))
    }).memoized()
}

fn sum_acc(key: u64, n: i64, acc: i64) -> Mock {
    Mock::new(key, &[], move |_| {
        if n == 0 {
            return Response::Value(acc);
        }
        Response::Continuation(promise(1, vec![sum_acc(1, n - 1, acc + n)]))
    })
}
```

Each Fibonacci number is computed exactly once. Running again with the same kernel takes the answer from the memo.

```rust
//| id: mock-tests
#[test]
fn memoization() {
    let runs = Arc::new(AtomicUsize::new(0));
    let worker = SingleWorker::new();
    let mut kernel = Kernel::new();
    let fib50 = 12_586_269_025;

    assert_eq!(kernel.run(&worker, promise(50, vec![fibonacci_memo(50, runs.clone())])).unwrap(), fib50);
    assert_eq!(runs.load(Ordering::Relaxed), 51);
    assert_eq!(kernel.memo_count(), 51);
    assert_eq!(kernel.frame_count(), 0);

    assert_eq!(kernel.run(&worker, promise(50, vec![fibonacci_memo(50, runs.clone())])).unwrap(), fib50);
    assert_eq!(runs.load(Ordering::Relaxed), 51);
}
```

A loop of local tail calls leaves nothing behind.

```rust
//| id: mock-tests
#[test]
fn local_loop() {
    let worker = SingleWorker::new();
    let mut kernel = Kernel::new();
    let n = 1_000_000;
    assert_eq!(kernel.run(&worker, promise(0, vec![sum_acc(0, n, 0)])).unwrap(), n * (n + 1) / 2);
    assert_eq!(kernel.frame_count(), 0);
    assert_eq!(kernel.memo_count(), 0);
}
```

A memoized task that waits for itself never finishes.

```rust
//| id: mock-tests
#[test]
fn cycle() {
    let looping = Mock::new(1, &[], |_| Response::Continuation(promise(2, vec![
        Mock::new(1, &[], |_| Response::Value(0)).memoized(),
        Mock::new(2, &[1], |x| Response::Value(x[0])),
    ]))).memoized();
    assert!(matches!(eval(promise(1, vec![looping])), Err(Error::Internal(InternalError::NoResult))));
}
```

### Worker pool

The same computations give the same answers on a worker pool. Memoization still runs every Fibonacci task only once, even though tasks now run concurrently: the memo is managed by the kernel, which runs on a single thread.

```rust
//| id: mock-tests
fn eval_pool(p: Box<Promise<Mock>>) -> Result<i64, Error<Mock>> {
    let worker = WorkerPool::new(4);
    let mut kernel = Kernel::new();
    let result = kernel.run(&worker, p);
    assert_eq!(kernel.frame_count(), 0);
    result
}

#[test]
fn pool_computations() {
    let p = promise(3, vec![constant(1, 2), constant(2, 3), binop(3, 1, 2, |a, b| a * b)]);
    assert_eq!(eval_pool(p).unwrap(), 6);
    assert_eq!(eval_pool(promise(0, vec![factorial(0, 20)])).unwrap(), 2432902008176640000);
    assert_eq!(eval_pool(promise(0, vec![factorial_acc(0, 20, 1)])).unwrap(), 2432902008176640000);
    assert_eq!(eval_pool(promise(0, vec![fibonacci(0, 20)])).unwrap(), 6765);
    assert_eq!(eval_pool(promise(0, vec![sum_to(0, 10_000)])).unwrap(), 50_005_000);
}

#[test]
fn pool_memoization() {
    let runs = Arc::new(AtomicUsize::new(0));
    assert_eq!(eval_pool(promise(50, vec![fibonacci_memo(50, runs.clone())])).unwrap(), 12_586_269_025);
    assert_eq!(runs.load(Ordering::Relaxed), 51);
}

#[test]
fn pool_error() {
    let p = promise(2, vec![
        Mock::new(1, &[], |_| Response::Error("failed".into())),
        Mock::new(2, &[1], |x| Response::Value(x[0])),
    ]);
    assert!(matches!(eval_pool(p), Err(Error::Job(e)) if e == "failed"));
    assert!(matches!(eval_pool(promise(2, vec![binop(2, 1, 3, |a, b| a + b)])), Err(Error::Internal(InternalError::NoResult))));
}
```

To show that tasks really run in parallel, four tasks wait on a barrier for four threads. This can only finish if all four run at the same time; on the single worker it would block forever.

```rust
//| id: mock-tests
#[test]
fn pool_parallel() {
    let barrier = Arc::new(std::sync::Barrier::new(4));
    let mut tasks: Vec<Mock> = (1..=4).map(|k| {
        let barrier = barrier.clone();
        Mock::new(k, &[], move |_| {
            barrier.wait();
            Response::Value(k as i64)
        })
    }).collect();
    tasks.push(Mock::new(5, &[1, 2, 3, 4], |x| Response::Value(x.iter().sum())));
    assert_eq!(eval_pool(promise(5, tasks)).unwrap(), 10);
}
```

A panic in a task is passed on to the caller.

```rust
//| id: mock-tests
#[test]
#[should_panic(expected = "boom")]
fn pool_panic() {
    let _ = eval_pool(promise(1, vec![Mock::new(1, &[], |_| panic!("boom"))]));
}
```
