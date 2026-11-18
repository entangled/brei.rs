// ~/~ begin <<docs/src/engine.md#src/noodles/kernel.rs>>[init]
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
// ~/~ end
