// ~/~ begin <<docs/src/brei.md#brei::src/tasks/mod.rs>>[init]
//! Brei tasks, running on the engine in `noodles`.

mod target;
mod rule;
mod db;
mod template;
mod history;
mod program;
mod exec;
mod group;
mod job;

#[cfg(test)]
mod tests;

pub use target::{Target, TargetId, Key};
pub use rule::{Runner, default_runners};
pub use db::TaskDb;
pub use template::RuleTemplate;
pub use history::HISTORY_FILE;
pub use group::kill_all;
pub use program::{Program, Call, Argument, Join, resolve};
pub use job::{Job, build_all};

#[cfg(test)]
pub(crate) use {rule::{Rule, RuleError, Value, Failure}, job::build};
// ~/~ end
