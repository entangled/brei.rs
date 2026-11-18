// ~/~ begin <<docs/src/brei.md#brei::src/tasks/db.rs>>[init]
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use super::rule::{Rule, RuleError, Runner, default_runners};
use super::target::{Target, TargetId, Targets};
use super::template::RuleTemplate;

pub struct Entry {
    pub(super) rule: Rule,
    pub(super) creates: Vec<TargetId>,
    pub(super) requires: Vec<TargetId>,
    pub(super) stdin: Option<TargetId>,
}

impl Entry {
    pub(super) fn new(rule: Rule, targets: &Targets) -> Result<Self, RuleError> {
        let rule = rule.complete()?;
        let intern = |ts: &[Target]| ts.iter().map(|t| targets.intern(t)).collect::<Result<Vec<_>, _>>();
        let creates = intern(&rule.creates)?;
        let requires = intern(&rule.requires)?;
        let stdin = match &rule.stdin {
            Some(t @ Target::Variable(..)) => targets.lookup(t),
            _ => None,
        };
        Ok(Entry { rule, creates, requires, stdin })
    }
}

pub(super) enum Kind {
    Rule(Arc<Entry>),
    Template(RuleTemplate),
    Variable(String),
}

pub(super) struct Recipe {
    pub(super) creates: Vec<TargetId>,
    pub(super) requires: Vec<TargetId>,
    pub(super) kind: Kind,
}

pub struct TaskDb {
    pub root: PathBuf,
    pub targets: Targets,
    pub runners: HashMap<String, Runner>,
    pub force_run: bool,
    recipes: Vec<Recipe>,
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
            recipes: Vec::new(),
            index: HashMap::new(),
            history: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn add(&mut self, rule: Rule) -> Result<(), RuleError> {
        let entry = Entry::new(rule, &self.targets)?;
        self.insert(Recipe { creates: entry.creates.clone(), requires: entry.requires.clone(), kind: Kind::Rule(Arc::new(entry)) })
    }

    pub(super) fn insert(&mut self, recipe: Recipe) -> Result<(), RuleError> {
        if let Some(id) = recipe.creates.iter().find(|id| self.index.contains_key(id)) {
            return Err(RuleError::Duplicate(self.targets.get(*id)));
        }
        let n = self.recipes.len();
        for id in &recipe.creates {
            self.index.insert(*id, n);
        }
        self.recipes.push(recipe);
        Ok(())
    }

    pub(super) fn recipe_for(&self, target: TargetId) -> Option<usize> {
        self.index.get(&target).copied()
    }

    pub(super) fn recipe(&self, n: usize) -> &Recipe {
        &self.recipes[n]
    }
}
// ~/~ end
