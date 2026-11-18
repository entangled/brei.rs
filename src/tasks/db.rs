// ~/~ begin <<docs/src/brei.md#brei::src/tasks/db.rs>>[init]
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use super::rule::{Rule, RuleError, Runner, default_runners};
use super::target::{Target, TargetId, Targets};
use super::template::{variables, RuleTemplate};

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

pub enum Kind {
    Rule(Arc<Entry>),
    Template(RuleTemplate),
    Variable(String),
}

pub struct Recipe {
    pub(super) creates: Vec<TargetId>,
    pub(super) requires: Vec<TargetId>,
    pub(super) kind: Kind,
}

pub struct TaskDb {
    pub root: PathBuf,
    pub targets: Targets,
    pub force_run: bool,
    runners: RwLock<HashMap<String, Runner>>,
    recipes: RwLock<HashMap<TargetId, Arc<Recipe>>>,
    pub(super) history: Mutex<BTreeMap<PathBuf, String>>,
}

impl TaskDb {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        TaskDb {
            root: root.into(),
            targets: Targets::new(),
            force_run: false,
            runners: RwLock::new(default_runners()),
            recipes: RwLock::new(HashMap::new()),
            history: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn add(&self, rule: Rule) -> Result<(), RuleError> {
        let entry = Entry::new(rule, &self.targets)?;
        self.insert(Recipe { creates: entry.creates.clone(), requires: entry.requires.clone(), kind: Kind::Rule(Arc::new(entry)) })
    }

    pub(super) fn insert(&self, recipe: Recipe) -> Result<(), RuleError> {
        let mut recipes = self.recipes.write().unwrap();
        if let Some(id) = recipe.creates.iter().find(|id| recipes.contains_key(id)) {
            return Err(RuleError::Duplicate(self.targets.get(*id)));
        }
        let recipe = Arc::new(recipe);
        for id in &recipe.creates {
            recipes.insert(*id, recipe.clone());
        }
        Ok(())
    }

    pub(super) fn recipe_for(&self, target: TargetId) -> Option<Arc<Recipe>> {
        self.recipes.read().unwrap().get(&target).cloned()
    }

    /// Whether some recipe creates `target`.
    pub fn creates(&self, target: &Target) -> bool {
        self.targets.lookup(target).and_then(|id| self.recipe_for(id)).is_some()
    }

    /// Add a runner. Its arguments can only use the variable `script`.
    pub fn add_runner(&self, name: &str, runner: Runner) -> Result<(), RuleError> {
        if let Some(var) = runner.args.iter().flat_map(|arg| variables(arg)).find(|v| *v != "script") {
            return Err(RuleError::Invalid(format!("its arguments use `${{{var}}}`, but only `${{script}}` can be used")));
        }
        self.runners.write().unwrap().insert(name.to_string(), runner);
        Ok(())
    }

    pub fn runner(&self, name: &str) -> Option<Runner> {
        self.runners.read().unwrap().get(name).cloned()
    }

    pub fn runner_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.runners.read().unwrap().keys().cloned().collect();
        names.sort();
        names
    }
}
// ~/~ end
