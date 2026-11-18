// ~/~ begin <<docs/src/brei.md#brei::src/tasks/mod.rs>>[init]
//! Brei tasks, running on the engine in `noodles`.

mod target;
mod rule;
mod db;
mod history;
mod exec;
mod job;

#[cfg(test)]
mod tests;

pub use target::{Target, VarType, TargetId, Targets, Key, EmptyTarget, TypeMismatch};
pub use rule::{Rule, RuleError, Runner, Value, Failure, default_runners};
pub use db::TaskDb;
pub use history::HISTORY_FILE;
pub use job::{Job, build, build_all};
// ~/~ end
