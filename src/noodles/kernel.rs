// ~/~ begin <<docs/src/engine.md#src/noodles/kernel.rs>>[init]
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
// ~/~ end
