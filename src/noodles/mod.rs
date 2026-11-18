// ~/~ begin <<docs/src/engine.md#src/noodles/mod.rs>>[init]
pub mod task;
pub mod kernel;
pub mod worker_pool;

#[cfg(test)]
pub mod single_worker;
#[cfg(test)]
mod mock;
// ~/~ end
