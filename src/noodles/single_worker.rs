// ~/~ begin <<docs/src/engine.md#src/noodles/single_worker.rs>>[init]
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
// ~/~ end
