use super::task::{Task, Trampoline, Worker, InternalError};
use crossbeam_channel::{Sender, Receiver, unbounded};

// pub trait Worker<T: Task> {
//     fn submit(&self, frame_id: u64, task: &T);
//     fn listen(&self) -> impl Iterator<Item=(u64, T::Key, Trampoline<T>)>;
// }

pub struct SingleWorker<T: Task> {
    sender: Sender<(u64, T::Key, Trampoline<T>)>,
    receiver: Receiver<(u64, T::Key, Trampoline<T>)>
}

impl<T: Task> SingleWorker<T> {
    fn new() -> Self {
        let (sender, receiver) = unbounded();
        return SingleWorker{ sender, receiver }
    }
}

impl<T: Task> Worker<T> for SingleWorker<T> {
    fn submit(&self, frame_id: u64, task: &T) -> Result<(), InternalError> {
        self.sender.send((frame_id, task.key(), task.run())).map_err(|e| InternalError::SendError(e.to_string()))
    }

    fn listen(&self) -> impl Iterator<Item=(u64, T::Key, Trampoline<T>)> {
        self.receiver.iter()
    }
}
