// ~/~ begin <<docs/src/engine.md#src/noodles/worker_pool.rs>>[init]
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
// ~/~ end
