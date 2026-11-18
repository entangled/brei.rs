// ~/~ begin <<docs/src/brei.md#brei::src/tasks/db.rs>>[init]
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Mutex;
use super::rule::{Rule, RuleError, Runner, default_runners};
use super::target::{Target, TargetId, Targets};

pub(super) struct Entry {
    pub(super) rule: Rule,
    pub(super) creates: Vec<TargetId>,
    pub(super) requires: Vec<TargetId>,
    pub(super) stdin: Option<TargetId>,
}

pub struct TaskDb {
    pub root: PathBuf,
    pub targets: Targets,
    pub runners: HashMap<String, Runner>,
    pub force_run: bool,
    rules: Vec<Entry>,
    index: HashMap<TargetId, usize>,
    pub(super) history: Mutex<BTreeMap<PathBuf, String>>,
}

impl TaskDb {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        TaskDb {
            root: root.into(),
            targets: Targets::new(),
            runners: default_runners(),
            force_run: false,
            rules: Vec::new(),
            index: HashMap::new(),
            history: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn add(&mut self, rule: Rule) -> Result<(), RuleError> {
        let rule = rule.complete()?;
        let intern = |ts: &[Target]| ts.iter().map(|t| self.targets.intern(t)).collect::<Result<Vec<_>, _>>();
        let creates = intern(&rule.creates)?;
        let requires = intern(&rule.requires)?;
        if let Some((t, _)) = rule.creates.iter().zip(&creates).find(|(_, id)| self.index.contains_key(id)) {
            return Err(RuleError::Duplicate(t.clone()));
        }
        let stdin = match &rule.stdin {
            Some(t @ Target::Variable(..)) => self.targets.lookup(t),
            _ => None,
        };

        let n = self.rules.len();
        for id in &creates {
            self.index.insert(*id, n);
        }
        self.rules.push(Entry { rule, creates, requires, stdin });
        Ok(())
    }

    pub(super) fn rule_for(&self, target: TargetId) -> Option<usize> {
        self.index.get(&target).copied()
    }

    pub(super) fn entry(&self, n: usize) -> &Entry {
        &self.rules[n]
    }
}
// ~/~ end
