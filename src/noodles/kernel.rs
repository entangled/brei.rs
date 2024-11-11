use std::boxed::Box;
use std::collections::{HashMap, HashSet};
use super::task::{Task, Promise, Worker, InternalError, Error, Trampoline};

pub struct Kernel<T: Task> {
    frame_no: u64,
    frames: HashMap<u64, (u64, T::Key, Box<Promise<T>>)>,
}

impl<T: Task> Kernel<T> {
    pub fn new() -> Self {
        Self { frame_no: 1, frames: HashMap::new() }
    }

    fn add<W: Worker<T>>(&mut self, worker: &W, parent_frame_id: u64, parent_key: T::Key, p: Box<Promise<T>>) -> Result<(), InternalError> {
        for (_, task) in (*p).tasks.iter() {
            if task.ready() {
                worker.submit(self.frame_no, task)?;
            }
        }

        self.frames.insert(self.frame_no, (parent_frame_id, parent_key, p));
        self.frame_no += 1;

        Ok(())
    }

    fn handle_result<W: Worker<T>>(&mut self, worker: &W, frame_id: u64, key: T::Key, result: T::Value) -> Result<(), InternalError> {
        let (_, _, frame) = &mut self.frames.get_mut(&frame_id).unwrap();
        for dep in &frame.inverse_deps[&key] {
            let task = &mut frame.tasks.get_mut(&dep).unwrap();
            task.assoc(key, result.clone());
            if task.ready() {
                worker.submit(frame_id, task)?;
            }
        }
        Ok(())
    }

    pub fn run<W: Worker<T>>(&mut self, worker: &W, root: T::Key, p: Box<Promise<T>>) -> Result<T::Value, Error<T>> {
        self.add(worker, 0, root, p).map_err(Error::Internal)?;

        for (frame_id, key, result) in worker.listen() {
            let (parent_frame, parent_task, frame) = &self.frames[&frame_id];
            match result {
                Trampoline::Value(v) => {
                    if key == frame.root && *parent_task == root {
                        return Ok(v);
                    }
                    if key == frame.root {
                        self.handle_result(worker, *parent_frame, *parent_task, v).map_err(Error::Internal)?;
                    } else {
                        self.handle_result(worker, frame_id, key, v).map_err(Error::Internal)?;
                    }
                },
                Trampoline::Continuation(c) => {
                    if key == frame.root {
                        self.add(worker, *parent_frame, *parent_task, c).map_err(Error::Internal)?;
                    } else {
                        self.add(worker, frame_id, key, c).map_err(Error::Internal)?;
                    }
                },
                Trampoline::Error(e) => {
                    return Err(Error::Job(e));
                }
            }
        }

        Err(Error::Internal(InternalError::NoResult))
    }
}
