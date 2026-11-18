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

    /// A cycle of tasks in this promise that wait for each other, if there is one. Each task in the cycle waits for the next, and the last waits for the first.
    pub fn cycle(&self) -> Option<Vec<T::Key>> {
        enum Mark { Active, Done }
        // Only tasks that aren't ready yet wait for anything.
        let waits_for = |key: &T::Key| -> Vec<T::Key> {
            match self.tasks.get(key) {
                Some(task) if !task.ready() => task.dependencies().filter(|d| self.tasks.contains_key(d)).collect(),
                _ => Vec::new(),
            }
        };

        // A single task can only wait for itself; this saves some work in loops of tail calls.
        if self.tasks.len() == 1 {
            let key = self.tasks.keys().next().unwrap();
            return waits_for(key).contains(key).then(|| vec![*key]);
        }

        let mut marks: HashMap<T::Key, Mark> = HashMap::new();
        for start in self.tasks.keys() {
            if marks.contains_key(start) {
                continue;
            }
            marks.insert(*start, Mark::Active);
            let mut path = vec![*start];
            let mut todo = vec![waits_for(start).into_iter()];
            while let Some(deps) = todo.last_mut() {
                match deps.next() {
                    Some(dep) => match marks.get(&dep) {
                        Some(Mark::Active) => {
                            let i = path.iter().position(|k| *k == dep).unwrap();
                            return Some(path.split_off(i));
                        }
                        Some(Mark::Done) => {}
                        None => {
                            marks.insert(dep, Mark::Active);
                            path.push(dep);
                            todo.push(waits_for(&dep).into_iter());
                        }
                    },
                    None => {
                        marks.insert(path.pop().unwrap(), Mark::Done);
                        todo.pop();
                    }
                }
            }
        }
        None
    }
}

pub trait Worker<T: Task> {
    fn submit(&self, frame_id: u64, task: T) -> Result<(), InternalError>;
    fn listen(&self) -> impl Iterator<Item=(u64, T::Key, Response<T>)>;
    fn cancel(&self);
}

<<cancel-token>>
```

Tasks are given a `CancelToken` when they are run. A task that takes a long time, like running an external program, should check it every now and then, and stop when it is cancelled. What a cancelled task returns doesn't matter, as the response will be thrown away.

```rust
//| id: cancel-token
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
```

The kernel keeps a stack of frames, although it is not really a stack but a tree: every promise that is being evaluated lives in its own frame. A frame remembers where its result should go: the frame and key of the task that gave the continuation, or nothing if it is the top-level frame.

A task is run only once, so as soon as it is ready the kernel takes it out of its frame and hands it to the worker.

When a task that is *not* the root of its frame gives a continuation, the new frame gets that task as parent. When the root of a frame gives a continuation, the new frame replaces the old one and inherits its parent. This is the equivalent of tail-call optimisation: a chain of tail calls doesn't build up frames.

### Memoization

Most keys only need to be unique within a frame: they name intermediate steps in a computation. Some tasks however have an identity that is shared by the entire graph. In Brei these are the targets: if two rules depend on the same file, that file should be built only once. A task says it is one of these by returning `true` from `memoize()`, and its key must then be unique among all memoized tasks.

The kernel keeps a memo of memoized keys. When a memoized task is ready, the kernel looks up its key:

- not present: the task is submitted, and the key is marked as pending, owned by this frame;
- pending: the frame is added to the list of waiters, and nothing is submitted;
- done (or failed): the outcome is delivered to the frame right away.

When the owner of a memoized key gets its final value (possibly after a continuation was resolved), the value is stored in the memo and delivered to all waiters. Because the owner needs to see that final value, a memoized root that gives a continuation doesn't get tail-call optimisation: its frame stays alive until the continuation is done.

Only memoized keys are stored, so a loop of local tail calls still runs in constant memory. The size of the memo is bounded by the number of distinct memoized tasks, which in Brei is the number of targets. Computed values survive between calls to `run`, so a second run can reuse earlier results. Everything else (pending and failed entries) is removed at the end of a run, so a failed task will be tried again next time.

### Failure

By default the kernel stops at the first error: the error is returned, and all tasks that are still running are cancelled. For a build system it is often more useful to do as much as possible, and only skip what depends on a failure, like `make -k`. A kernel created with `.keep_going()` does that:

- a task that gives an error is marked as failed, and the error is stored;
- a task that depends on a failed task fails as well, without being run. If that task is the root of its frame, the failure is passed on to the parent. The frame itself is kept, because other tasks in it may still be running and their results may be needed elsewhere, through the memo;
- a failed memoized key is stored in the memo as failed, so that all frames waiting for it fail too.

The run ends when the top-level frame has a value, or when no work is left. If any task failed, the result is `Error::Jobs` with all errors, in the order they happened.

### Cycles

A memoized task that, through its continuation, ends up waiting for itself would never finish. Every frame knows its *enclosing* memoized key: the closest memoized task on the way up its chain of parents, whose result the frame is helping to compute. If a frame owns or waits for key `K`, then its enclosing key waits for `K`. The kernel keeps these edges in a graph. Before adding a waiter, it checks whether `K` already (indirectly) waits for the enclosing key. If it does, adding the edge would close a loop, so the run stops with `Error::Cycle`. The cycle lists the keys in order: each waits for the next, and the last waits for the first.

Edges for a memoized key are removed once its value is known, so the graph only contains the computations that are still running.

Tasks within a single promise can also wait for each other in a loop. Those tasks would never be ready, so the kernel checks every promise it adds, with a depth-first search over the dependencies between its tasks. A loop is reported as `Error::Cycle` too. Tasks that are ready don't wait for anything, so they can't be part of a loop.

### Implementation

Delivering a value may cause other values to be delivered: the root of a frame finishing will deliver its value to the parent, and a memoized key will deliver to its waiters. Instead of recursing, the kernel keeps a queue of deliveries `(frame, key, outcome)` that is drained after every response from the worker.

```rust
//| file: src/noodles/kernel.rs
use std::boxed::Box;
use std::collections::{HashMap, HashSet, VecDeque};
use super::task::{Task, Promise, Worker, InternalError, Error, Response};

struct Frame<T: Task> {
    parent: Option<(u64, T::Key)>,
    enclosing: Option<T::Key>,
    promise: Box<Promise<T>>,
}

enum State<T: Task> {
    Pending { owner: u64, waiters: Vec<u64> },
    Done(T::Value),
    Failed,
}

enum Outcome<T: Task> {
    Value(T::Value),
    Failed,
}

impl<T: Task> Clone for Outcome<T> {
    fn clone(&self) -> Self {
        match self {
            Outcome::Value(v) => Outcome::Value(v.clone()),
            Outcome::Failed => Outcome::Failed,
        }
    }
}

pub struct Kernel<T: Task> {
    keep_going: bool,
    frame_no: u64,
    frames: HashMap<u64, Frame<T>>,
    memo: HashMap<T::Key, State<T>>,
    waits_for: HashMap<T::Key, Vec<T::Key>>,
    queue: VecDeque<(u64, T::Key, Outcome<T>)>,
    errors: Vec<T::Error>,
}

impl<T: Task> Default for Kernel<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Task> Kernel<T> {
    pub fn new() -> Self {
        Self {
            keep_going: false,
            frame_no: 1,
            frames: HashMap::new(),
            memo: HashMap::new(),
            waits_for: HashMap::new(),
            queue: VecDeque::new(),
            errors: Vec::new(),
        }
    }

    /// Don't stop at the first error, but run everything that doesn't depend on a failed task.
    pub fn keep_going(mut self) -> Self {
        self.keep_going = true;
        self
    }

    /// Number of entries in the memo.
    pub fn memo_count(&self) -> usize {
        self.memo.len()
    }

    /// Whether `frame_id` is responsible for computing the memoized `key`.
    fn owns(&self, frame_id: u64, key: T::Key) -> bool {
        matches!(self.memo.get(&key), Some(State::Pending { owner, .. }) if *owner == frame_id)
    }

    /// A path from `from` to `to` in the waits-for graph, if there is one.
    fn find_path(&self, from: T::Key, to: T::Key) -> Option<Vec<T::Key>> {
        let mut previous: HashMap<T::Key, T::Key> = HashMap::new();
        let mut seen = HashSet::from([from]);
        let mut todo = VecDeque::from([from]);
        while let Some(key) = todo.pop_front() {
            if key == to {
                let mut path = vec![to];
                while let Some(k) = previous.get(path.last().unwrap()) {
                    path.push(*k);
                }
                path.reverse();
                return Some(path);
            }
            for next in self.waits_for.get(&key).into_iter().flatten() {
                if seen.insert(*next) {
                    previous.insert(*next, key);
                    todo.push_back(*next);
                }
            }
        }
        None
    }

    fn add<W: Worker<T>>(&mut self, worker: &W, parent: Option<(u64, T::Key)>, mut promise: Box<Promise<T>>) -> Result<(), Error<T>> {
        let frame_id = self.frame_no;
        self.frame_no += 1;

        if let Some(cycle) = promise.cycle() {
            return Err(Error::Cycle(cycle));
        }

        let enclosing = match parent {
            Some((p, k)) if self.owns(p, k) => Some(k),
            Some((p, _)) => self.frames.get(&p).and_then(|f| f.enclosing),
            None => None,
        };

        let ready: Vec<T::Key> = promise.tasks.iter()
            .filter(|(_, task)| task.ready())
            .map(|(key, _)| *key)
            .collect();
        let ready: Vec<T> = ready.iter().map(|key| promise.tasks.remove(key).unwrap()).collect();

        self.frames.insert(frame_id, Frame { parent, enclosing, promise });
        for task in ready {
            self.schedule(worker, frame_id, task)?;
        }
        Ok(())
    }

    /// Submit a ready task, unless its result is (or will be) available from the memo.
    fn schedule<W: Worker<T>>(&mut self, worker: &W, frame_id: u64, task: T) -> Result<(), Error<T>> {
        if !task.memoize() {
            return Ok(worker.submit(frame_id, task)?);
        }

        let key = task.key();
        let enclosing = self.frames.get(&frame_id).and_then(|f| f.enclosing);
        match self.memo.get_mut(&key) {
            Some(State::Done(v)) => {
                let v = v.clone();
                self.queue.push_back((frame_id, key, Outcome::Value(v)));
            }
            Some(State::Failed) => {
                self.queue.push_back((frame_id, key, Outcome::Failed));
            }
            Some(State::Pending { waiters, .. }) => {
                waiters.push(frame_id);
                if let Some(outer) = enclosing {
                    if let Some(cycle) = self.find_path(key, outer) {
                        return Err(Error::Cycle(cycle));
                    }
                    self.waits_for.entry(outer).or_default().push(key);
                }
            }
            None => {
                self.memo.insert(key, State::Pending { owner: frame_id, waiters: Vec::new() });
                if let Some(outer) = enclosing {
                    self.waits_for.entry(outer).or_default().push(key);
                }
                worker.submit(frame_id, task)?;
            }
        }
        Ok(())
    }

    /// Handle a response from the worker.
    fn respond<W: Worker<T>>(&mut self, worker: &W, frame_id: u64, key: T::Key, response: Response<T>) -> Result<(), Error<T>> {
        // Results for frames that were already discarded are ignored, unless the memo still needs them.
        if !self.frames.contains_key(&frame_id) && !self.owns(frame_id, key) {
            return Ok(());
        }

        match response {
            Response::Value(v) => {
                self.queue.push_back((frame_id, key, Outcome::Value(v)));
            }
            Response::Continuation(c) => {
                let tail_call = !self.owns(frame_id, key)
                    && self.frames.get(&frame_id).is_some_and(|f| f.promise.root == key);
                if tail_call {
                    let parent = self.frames.remove(&frame_id).unwrap().parent;
                    self.add(worker, parent, c)?;
                } else {
                    self.add(worker, Some((frame_id, key)), c)?;
                }
            }
            Response::Error(e) => {
                if !self.keep_going {
                    return Err(Error::Jobs(vec![e]));
                }
                self.errors.push(e);
                self.queue.push_back((frame_id, key, Outcome::Failed));
            }
        }
        Ok(())
    }

    /// Deliver the outcome of `key` in frame `frame_id`. Returns the value of the top-level frame, once known.
    fn deliver<W: Worker<T>>(&mut self, worker: &W, frame_id: u64, key: T::Key, outcome: Outcome<T>) -> Result<Option<T::Value>, Error<T>> {
        if self.owns(frame_id, key) {
            let state = match &outcome {
                Outcome::Value(v) => State::Done(v.clone()),
                Outcome::Failed => State::Failed,
            };
            if let Some(State::Pending { waiters, .. }) = self.memo.insert(key, state) {
                for w in waiters {
                    self.queue.push_back((w, key, outcome.clone()));
                }
            }
            self.waits_for.remove(&key);
        }

        let Some(frame) = self.frames.get_mut(&frame_id) else { return Ok(None) };

        // The root of a frame reports to the parent; other tasks report to their own frame.
        if key == frame.promise.root {
            let parent = frame.parent;
            match outcome {
                Outcome::Value(v) => {
                    self.frames.remove(&frame_id);
                    match parent {
                        None => return Ok(Some(v)),
                        Some((p, pk)) => self.queue.push_back((p, pk, Outcome::Value(v))),
                    }
                }
                // The frame is kept, since other tasks in it may still be running.
                Outcome::Failed => {
                    if let Some((p, pk)) = parent {
                        self.queue.push_back((p, pk, Outcome::Failed));
                    }
                }
            }
            return Ok(None);
        }

        let mut ready = Vec::new();
        if let Some(dependents) = frame.promise.inverse_deps.get(&key) {
            for dep in dependents {
                match &outcome {
                    Outcome::Value(v) => {
                        let Some(task) = frame.promise.tasks.get_mut(dep) else { continue };
                        task.assoc(key, v.clone());
                        if task.ready() {
                            ready.push(frame.promise.tasks.remove(dep).unwrap());
                        }
                    }
                    // A task that depends on a failed task fails as well, without being run.
                    Outcome::Failed => {
                        if frame.promise.tasks.remove(dep).is_some() {
                            self.queue.push_back((frame_id, *dep, Outcome::Failed));
                        }
                    }
                }
            }
        }
        for task in ready {
            self.schedule(worker, frame_id, task)?;
        }
        Ok(None)
    }

    fn drain<W: Worker<T>>(&mut self, worker: &W) -> Result<Option<T::Value>, Error<T>> {
        while let Some((frame_id, key, outcome)) = self.queue.pop_front() {
            if let Some(result) = self.deliver(worker, frame_id, key, outcome)? {
                return Ok(Some(result));
            }
        }
        Ok(None)
    }

    fn run_frames<W: Worker<T>>(&mut self, worker: &W, promise: Box<Promise<T>>) -> Result<T::Value, Error<T>> {
        self.add(worker, None, promise)?;
        if let Some(result) = self.drain(worker)? {
            return Ok(result);
        }

        for (frame_id, key, response) in worker.listen() {
            self.respond(worker, frame_id, key, response)?;
            if let Some(result) = self.drain(worker)? {
                return Ok(result);
            }
        }

        Err(Error::Internal(InternalError::NoResult))
    }

    pub fn run<W: Worker<T>>(&mut self, worker: &W, promise: Box<Promise<T>>) -> Result<T::Value, Error<T>> {
        let result = self.run_frames(worker, promise);
        let errors = std::mem::take(&mut self.errors);
        let result = match result {
            Ok(_) | Err(Error::Internal(InternalError::NoResult)) if !errors.is_empty() => Err(Error::Jobs(errors)),
            result => result,
        };

        // Whatever is still running is no longer needed.
        worker.cancel();
        self.frames.clear();
        self.queue.clear();
        self.waits_for.clear();
        self.memo.retain(|_, state| matches!(state, State::Done(_)));
        result
    }
}
```

Single Worker
-------------

The single worker has a single sender-receiver pair. When tasks are submitted they are immediately executed and the result is push on the channel. Since all work happens inside `submit`, `listen` doesn't need to block: if the channel is empty, nothing is left to do and the kernel can report that no result was found. Tasks are never running while the kernel is busy, so there is nothing to cancel: cancelling only throws away results that weren't read yet.

```rust
//| file: src/noodles/single_worker.rs
use super::task::{Task, Response, Worker, InternalError, CancelToken};
use crossbeam_channel::{Sender, Receiver, unbounded};

pub struct SingleWorker<T: Task> {
    sender: Sender<(u64, T::Key, Response<T>)>,
    receiver: Receiver<(u64, T::Key, Response<T>)>,
    token: CancelToken,
}

impl<T: Task> SingleWorker<T> {
    pub fn new() -> Self {
        let (sender, receiver) = unbounded();
        SingleWorker{ sender, receiver, token: CancelToken::new() }
    }
}

impl<T: Task> Default for SingleWorker<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Task> Worker<T> for SingleWorker<T> {
    fn submit(&self, frame_id: u64, task: T) -> Result<(), InternalError> {
        let key = task.key();
        self.sender.send((frame_id, key, task.run(&self.token))).map_err(|e| InternalError::SendError(e.to_string()))
    }

    fn listen(&self) -> impl Iterator<Item=(u64, T::Key, Response<T>)> {
        self.receiver.try_iter()
    }

    fn cancel(&self) {
        while self.receiver.try_recv().is_ok() {}
    }
}
```

Worker Pool
-----------

The worker pool runs tasks on a fixed number of threads. Submitted tasks go into a shared channel, from which each thread takes the next task to run; responses come back through a second channel. Tasks, keys, values and errors now need to be `Send`, since they cross between threads.

The kernel needs to know when nothing is left to do, otherwise a graph that can't complete would block forever. The pool counts the number of tasks in flight: `submit` increments it, and receiving a response decrements it. When the count is zero the iterator returned by `listen` ends, just like the single worker.

If a task panics, the thread catches the panic and sends it back with the response. The panic is then resumed on the thread that called `listen`, so a panicking task has the same effect as with the single worker.

Every task is sent along with the cancel token that was current when it was submitted. To cancel, the pool triggers that token and replaces it with a fresh one for later tasks. Tasks that are still waiting in the channel are skipped when a thread picks them up; tasks that are running are told to stop through the token. Cancelling waits until every task in flight has reported back, and throws the responses away (including panics), so the next run starts clean. A task that never looks at its token will hold this up until it finishes.

When the pool is dropped, all tasks are cancelled, and the threads stop.

```rust
//| file: src/noodles/worker_pool.rs
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::panic::{self, AssertUnwindSafe};
use std::thread::{self, JoinHandle};
use super::task::{Task, Response, Worker, InternalError, CancelToken};
use crossbeam_channel::{Sender, Receiver, unbounded};

enum Outcome<T: Task> {
    Response(Response<T>),
    Panic(Box<dyn Any + Send>),
    Skipped,
}

type Message<T> = (u64, <T as Task>::Key, Outcome<T>);

pub struct WorkerPool<T: Task> {
    tasks: Option<Sender<(u64, T, CancelToken)>>,
    results: Receiver<Message<T>>,
    in_flight: Cell<usize>,
    token: RefCell<CancelToken>,
    threads: Vec<JoinHandle<()>>,
}

impl<T> WorkerPool<T>
where T: Task + Send + 'static, T::Key: Send, T::Value: Send, T::Error: Send
{
    pub fn new(n_threads: usize) -> Self {
        let (task_sender, task_receiver) = unbounded::<(u64, T, CancelToken)>();
        let (result_sender, result_receiver) = unbounded::<Message<T>>();

        let threads = (0..n_threads.max(1)).map(|_| {
            let tasks = task_receiver.clone();
            let results = result_sender.clone();
            thread::spawn(move || {
                for (frame_id, task, token) in tasks.iter() {
                    let key = task.key();
                    let outcome = if token.is_cancelled() {
                        Outcome::Skipped
                    } else {
                        match panic::catch_unwind(AssertUnwindSafe(|| task.run(&token))) {
                            Ok(response) => Outcome::Response(response),
                            Err(payload) => Outcome::Panic(payload),
                        }
                    };
                    if results.send((frame_id, key, outcome)).is_err() {
                        break;
                    }
                }
            })
        }).collect();

        WorkerPool {
            tasks: Some(task_sender),
            results: result_receiver,
            in_flight: Cell::new(0),
            token: RefCell::new(CancelToken::new()),
            threads,
        }
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
        let token = self.token.borrow().clone();
        sender.send((frame_id, task, token)).map_err(|e| InternalError::SendError(e.to_string()))?;
        self.in_flight.set(self.in_flight.get() + 1);
        Ok(())
    }

    fn listen(&self) -> impl Iterator<Item=(u64, T::Key, Response<T>)> {
        std::iter::from_fn(move || loop {
            if self.in_flight.get() == 0 {
                return None;
            }
            let (frame_id, key, outcome) = self.results.recv().ok()?;
            self.in_flight.set(self.in_flight.get() - 1);
            match outcome {
                Outcome::Response(response) => return Some((frame_id, key, response)),
                Outcome::Panic(payload) => panic::resume_unwind(payload),
                Outcome::Skipped => continue,
            }
        })
    }

    fn cancel(&self) {
        self.token.replace(CancelToken::new()).cancel();
        while self.in_flight.get() > 0 && self.results.recv().is_ok() {
            self.in_flight.set(self.in_flight.get() - 1);
        }
    }
}

impl<T: Task> Drop for WorkerPool<T> {
    fn drop(&mut self) {
        self.token.borrow().cancel();
        self.tasks.take();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}
```

Mock tasks
----------

To test the engine, we have a mock task that computes on integers. Each task has a `u64` key, a list of dependencies and a function that takes the values of its dependencies (in order) and gives a `Response`. Since the function may return a continuation, we can have recursion. Tasks are not memoized, unless built with `.memoized()`. Tasks that want to look at the cancel token are built with `Mock::cancellable`.

```rust
//| file: src/noodles/mock.rs
use std::collections::HashMap;
use std::sync::{Arc, Barrier};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use super::task::{Task, Promise, Response, Error, InternalError, CancelToken};
use super::kernel::Kernel;
use super::single_worker::SingleWorker;
use super::worker_pool::WorkerPool;

type Func = Arc<dyn Fn(&[i64], &CancelToken) -> Response<Mock> + Send + Sync>;

struct Mock {
    key: u64,
    deps: Vec<u64>,
    args: HashMap<u64, i64>,
    memoize: bool,
    func: Func,
}

impl Mock {
    fn new(key: u64, deps: &[u64], func: impl Fn(&[i64]) -> Response<Mock> + Send + Sync + 'static) -> Self {
        Self::cancellable(key, deps, move |x, _| func(x))
    }

    fn cancellable(key: u64, deps: &[u64], func: impl Fn(&[i64], &CancelToken) -> Response<Mock> + Send + Sync + 'static) -> Self {
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

    fn run(self, cancel: &CancelToken) -> Response<Self> {
        let args: Vec<i64> = self.deps.iter().map(|k| self.args[k]).collect();
        (self.func)(&args, cancel)
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
        kernel.run(&worker, p)
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
    assert!(matches!(eval(p), Err(Error::Jobs(e)) if e == ["division by zero"]));
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
    assert_eq!(kernel.memo_count(), 0);
}
```

A memoized task that waits for itself is a cycle.

```rust
//| id: mock-tests
#[test]
fn cycle() {
    let looping = Mock::new(1, &[], |_| Response::Continuation(promise(2, vec![
        Mock::new(1, &[], |_| Response::Value(0)).memoized(),
        Mock::new(2, &[1], |x| Response::Value(x[0])),
    ]))).memoized();
    assert!(matches!(eval(promise(1, vec![looping])), Err(Error::Cycle(c)) if c == [1]));
}
```

A cycle can also go through several memoized tasks: here task `1` needs `2`, which needs `1` again.

```rust
//| id: mock-memo
fn needs(key: u64, dep: u64) -> Mock {
    Mock::new(key, &[], move |_| Response::Continuation(promise(SUM, vec![
        needs(dep, key),
        Mock::new(SUM, &[dep], |x| Response::Value(x[0])),
    ]))).memoized()
}
```

```rust
//| id: mock-tests
#[test]
fn indirect_cycle() {
    assert!(matches!(eval(promise(1, vec![needs(1, 2)])), Err(Error::Cycle(c)) if c == [1, 2]));
}
```

Tasks in one promise that wait for each other are a cycle too, whether they are memoized or not. The cycle is found when the promise is added, also when it comes from a continuation. Tasks outside the cycle that depend on it are not part of it.

```rust
//| id: mock-tests
#[test]
fn local_cycle() {
    let looping = || vec![
        Mock::new(1, &[2], |x| Response::Value(x[0])),
        Mock::new(2, &[3], |x| Response::Value(x[0])),
        Mock::new(3, &[2], |x| Response::Value(x[0])).memoized(),
        Mock::new(4, &[4], |x| Response::Value(x[0])),
    ];
    let is_cycle = |c: &[u64]| c == [2, 3] || c == [3, 2] || c == [4];
    assert!(matches!(eval(promise(1, looping())), Err(Error::Cycle(c)) if is_cycle(&c)));

    let continued = Mock::new(0, &[], move |_| Response::Continuation(promise(1, looping())));
    assert!(matches!(eval_pool(promise(0, vec![continued])), Err(Error::Cycle(c)) if is_cycle(&c)));

    let add = |key, a, b| binop(key, a, b, |x, y| x + y);
    let diamond = vec![constant(1, 1), add(2, 1, 1), add(3, 1, 2), add(4, 2, 3)];
    assert_eq!(eval(promise(4, diamond)).unwrap(), 5);
}
```

### Worker pool

The same computations give the same answers on a worker pool. Memoization still runs every Fibonacci task only once, even though tasks now run concurrently: the memo is managed by the kernel, which runs on a single thread.

```rust
//| id: mock-tests
fn eval_pool(p: Box<Promise<Mock>>) -> Result<i64, Error<Mock>> {
    let worker = WorkerPool::new(4);
    let mut kernel = Kernel::new();
    kernel.run(&worker, p)
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
    assert!(matches!(eval_pool(p), Err(Error::Jobs(e)) if e == ["failed"]));
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
```

### Failure and cancellation

When a task fails, the kernel stops by default. Here an error happens next to a memoized Fibonacci computation. When we keep going, the Fibonacci numbers are still computed and end up in the memo, even though the run as a whole fails. Without keeping going, the run stops before that.

```rust
//| id: mock-tests
fn failing_next_to_fibonacci() -> Box<Promise<Mock>> {
    promise(SUM, vec![
        Mock::new(100, &[], |_| Response::Error("failed".into())),
        fibonacci_memo(10, Arc::new(AtomicUsize::new(0))),
        binop(SUM, 100, 10, |a, b| a + b),
    ])
}

#[test]
fn keep_going() {
    let worker = SingleWorker::new();
    let mut kernel = Kernel::new().keep_going();
    assert!(matches!(kernel.run(&worker, failing_next_to_fibonacci()), Err(Error::Jobs(e)) if e == ["failed"]));
    assert_eq!(kernel.memo_count(), 11);

    let mut kernel = Kernel::new();
    assert!(matches!(kernel.run(&worker, failing_next_to_fibonacci()), Err(Error::Jobs(e)) if e == ["failed"]));
    assert!(kernel.memo_count() < 11);
}
```

All errors are collected, also when they happen deep down in a continuation.

```rust
//| id: mock-tests
#[test]
fn keep_going_collects_errors() {
    let deep_error = Mock::new(2, &[], |_| Response::Continuation(promise(1, vec![
        Mock::new(1, &[], |_| Response::Error("deep".into())),
    ])));
    let p = promise(3, vec![
        Mock::new(1, &[], |_| Response::Error("shallow".into())),
        deep_error,
        binop(3, 1, 2, |a, b| a + b),
    ]);
    let worker = SingleWorker::new();
    let mut kernel = Kernel::new().keep_going();
    let Err(Error::Jobs(mut errors)) = kernel.run(&worker, p) else { panic!("expected errors") };
    errors.sort();
    assert_eq!(errors, ["deep", "shallow"]);
}
```

On the worker pool, an error cancels the tasks that are still running. Task `2` would run for ten seconds, unless it notices it is cancelled. The barrier makes sure that task `2` is already running when task `1` fails.

```rust
//| id: mock-tests
#[test]
fn pool_cancel() {
    let barrier = Arc::new(Barrier::new(2));
    let cancelled = Arc::new(AtomicBool::new(false));
    let p = {
        let (b1, b2) = (barrier.clone(), barrier.clone());
        let cancelled = cancelled.clone();
        promise(3, vec![
            Mock::new(1, &[], move |_| {
                b1.wait();
                Response::Error("failed".into())
            }),
            Mock::cancellable(2, &[], move |_, token| {
                b2.wait();
                let start = Instant::now();
                while start.elapsed() < Duration::from_secs(10) {
                    if token.is_cancelled() {
                        cancelled.store(true, Ordering::Relaxed);
                        break;
                    }
                    thread::sleep(Duration::from_millis(1));
                }
                Response::Value(0)
            }),
            binop(3, 1, 2, |a, b| a + b),
        ])
    };

    let start = Instant::now();
    assert!(matches!(eval_pool(p), Err(Error::Jobs(e)) if e == ["failed"]));
    assert!(cancelled.load(Ordering::Relaxed));
    assert!(start.elapsed() < Duration::from_secs(5));
}
```
