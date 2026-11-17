// ~/~ begin <<docs/src/engine.md#src/noodles/kernel.rs>>[init]
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
// ~/~ end
