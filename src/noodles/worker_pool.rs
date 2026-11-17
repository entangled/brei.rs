// ~/~ begin <<docs/src/engine.md#src/noodles/worker_pool.rs>>[init]
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
// ~/~ end
