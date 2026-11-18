// ~/~ begin <<docs/src/brei.md#brei::src/tasks/template.rs>>[init]
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use serde::Deserialize;
use crate::template_strings::{dollar, Env, Error, Format, Item};
use super::db::{Kind, Recipe, TaskDb};
use super::rule::{Failure, Rule, RuleError};
use super::target::{wrapped, Target, VarType};

/// A task, or a template, as written in a program. All strings are template strings.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RuleTemplate {
    /// The targets the task creates.
    pub creates: Vec<String>,
    /// The targets the task needs; `splice(name)` adds the elements of a list variable.
    pub requires: Vec<String>,
    /// Makes the task available as the phony target `#name`.
    pub name: Option<String>,
    /// The runner of the script; without one, every line of the script is a command.
    pub runner: Option<String>,
    /// A script file to run instead of `script`; needs a runner.
    pub path: Option<String>,
    /// The script to run.
    pub script: Option<String>,
    /// Standard input: a file, or a scalar variable `var(name)`.
    pub stdin: Option<String>,
    /// Standard output: a file, `var(name)` or `listvar(name)`.
    pub stdout: Option<String>,
    /// Shown in the log when the task runs.
    pub description: Option<String>,
    /// Always run the task.
    pub force: bool,
}

/// The values of list variables, by name.
pub(super) type Lists = HashMap<String, Vec<String>>;

/// Names of the variables in a template string.
pub(super) fn variables(s: &str) -> impl Iterator<Item=&str> {
    dollar(s).filter_map(|item| match item {
        Ok(Item::Variable(name)) => Some(name),
        _ => None,
    })
}

pub(super) fn substitute(s: &str, env: &Env) -> Result<String, Failure> {
    dollar(s).format(env).map_err(|e| Failure::Template(match e {
        Error::MissingVariable(name) => format!("variable `{name}` has no value"),
        Error::InvalidTemplate(msg) => msg,
    }))
}

/// Substitute the variables in `env`, and leave the others. The values in `env` are template strings themselves, and so is the result.
pub(super) fn fill(s: &str, env: &Env) -> String {
    dollar(s).flatten().map(|item| match item {
        Item::Variable(name) => env.get(name).cloned().unwrap_or_else(|| format!("${{{name}}}")),
        Item::Literal(text) => escape(text),
    }).collect()
}

/// A template string for the literal text `s`.
pub(super) fn escape(s: &str) -> String {
    s.replace('$', "$$")
}

pub(super) fn splice_name(s: &str) -> Option<&str> {
    wrapped(s, "splice")
}

fn parse(s: String) -> Result<Target, Failure> {
    s.parse().map_err(|_| Failure::Template("empty target".into()))
}

impl RuleTemplate {
    /// The targets: `creates`, `stdout` and the phony target `#name`.
    pub(super) fn targets(&self) -> Vec<String> {
        self.creates.iter().cloned()
            .chain(self.stdout.clone())
            .chain(self.name.as_ref().map(|n| format!("#{n}")))
            .collect()
    }

    fn strings(&self) -> impl Iterator<Item=&String> {
        let fields = [&self.name, &self.runner, &self.path, &self.script, &self.stdin, &self.stdout, &self.description];
        self.creates.iter().chain(&self.requires).chain(fields.into_iter().flatten())
    }

    /// Names of the variables used in the template.
    pub fn variables(&self) -> BTreeSet<&str> {
        self.strings().flat_map(|s| variables(s)).collect()
    }

    /// A short description of the rule, for error messages.
    pub fn label(&self) -> String {
        let targets = self.targets();
        if let Some(name) = &self.name {
            format!("rule `#{name}`")
        } else if !targets.is_empty() {
            let targets: Vec<String> = targets.iter().map(|t| format!("`{t}`")).collect();
            format!("rule for {}", targets.join(", "))
        } else if let Some(description) = &self.description {
            format!("rule \"{description}\"")
        } else {
            "rule without targets".to_string()
        }
    }

    /// Names of the variables used in the targets.
    pub fn target_variables(&self) -> BTreeSet<&str> {
        let fields = [&self.name, &self.stdout];
        self.creates.iter().chain(fields.into_iter().flatten()).flat_map(|s| variables(s)).collect()
    }

    fn map(&self, target: impl Fn(&str) -> String, other: impl Fn(&str) -> String) -> RuleTemplate {
        RuleTemplate {
            creates: self.creates.iter().map(|s| target(s)).collect(),
            requires: self.requires.iter().map(|s| other(s)).collect(),
            name: self.name.as_deref().map(&target),
            runner: self.runner.as_deref().map(&other),
            path: self.path.as_deref().map(&other),
            script: self.script.as_deref().map(&other),
            stdin: self.stdin.as_deref().map(&other),
            stdout: self.stdout.as_deref().map(&target),
            description: self.description.as_deref().map(&other),
            force: self.force,
        }
    }

    /// Substitute the variables in `env` in all fields, leaving the others (see `fill`).
    pub(super) fn fill(&self, env: &Env) -> RuleTemplate {
        self.map(|s| fill(s, env), |s| fill(s, env))
    }

    /// Substitute the variables in `env` in the targets only.
    pub(super) fn fill_targets(&self, env: &Env) -> RuleTemplate {
        self.map(|s| fill(s, env), str::to_string)
    }

    /// Names of the list variables spliced into `requires`.
    pub fn splices(&self) -> BTreeSet<&str> {
        self.requires.iter().filter_map(|s| splice_name(s)).collect()
    }

    fn check(&self) -> Result<(), RuleError> {
        let invalid = |msg: String| Err(RuleError::Invalid(msg));
        let targets = self.targets();
        if let Some(t) = targets.iter().find(|t| variables(t).next().is_some()) {
            return invalid(format!("the targets of a rule can't contain variables, found `{t}`"));
        }
        let inputs = || self.stdin.iter().chain(&self.path);
        if let Some(s) = self.creates.iter().chain(&self.requires).chain(inputs()).find(|s| wrapped(s, "listvar").is_some()) {
            return invalid(format!("`{s}` can only be used as `stdout` of a rule"));
        }
        if let Some(s) = targets.iter().chain(inputs()).find(|s| splice_name(s).is_some()) {
            return invalid(format!("`{s}` can only be used in `requires`"));
        }
        Ok(())
    }

    /// Substitute variables and splice lists, giving a rule.
    pub(super) fn expand(&self, env: &Env, lists: &Lists) -> Result<Rule, Failure> {
        let sub = |s: &Option<String>| s.as_deref().map(|s| substitute(s, env)).transpose();
        let target = |s: &str| parse(substitute(s, env)?);

        let mut requires = Vec::new();
        for s in &self.requires {
            match splice_name(s) {
                Some(name) => {
                    let items = lists.get(name)
                        .ok_or_else(|| Failure::Template(format!("list variable `{name}` has no value")))?;
                    for item in items {
                        requires.push(parse(item.clone())?);
                    }
                }
                None => requires.push(target(s)?),
            }
        }

        Ok(Rule {
            creates: self.creates.iter().map(|s| target(s)).collect::<Result<_, _>>()?,
            requires,
            name: sub(&self.name)?,
            runner: sub(&self.runner)?,
            path: sub(&self.path)?.map(PathBuf::from),
            script: sub(&self.script)?,
            stdin: sub(&self.stdin)?.map(parse).transpose()?,
            stdout: sub(&self.stdout)?.map(parse).transpose()?,
            description: sub(&self.description)?,
            force: self.force,
        })
    }
}

fn scalar(name: &str) -> Target {
    Target::Variable(name.to_string(), VarType::Scalar)
}

impl TaskDb {
    /// Add a rule that may use variables.
    pub fn add_template(&self, template: RuleTemplate) -> Result<(), RuleError> {
        template.check()?;
        let invalid = |f: Failure| RuleError::Invalid(f.to_string());
        let (vars, lists) = (template.variables(), template.splices());
        if vars.is_empty() && lists.is_empty() {
            let rule = template.expand(&Env::new(), &Lists::new()).map_err(invalid)?;
            return self.add(rule);
        }

        let requires = vars.iter().map(|v| scalar(v))
            .chain(lists.iter().map(|v| Target::Variable(v.to_string(), VarType::List)))
            .map(|t| self.targets.intern(&t))
            .collect::<Result<Vec<_>, _>>()?;
        let mut creates = Vec::new();
        for t in template.targets() {
            let t = parse(substitute(&t, &Env::new()).map_err(invalid)?).map_err(invalid)?;
            creates.push(self.targets.intern(&t)?);
        }
        self.insert(Recipe { creates, requires, kind: Kind::Template(template) })
    }

    /// Add a variable whose value is a template string.
    pub fn add_variable(&self, name: &str, template: &str) -> Result<(), RuleError> {
        let creates = vec![self.targets.intern(&scalar(name))?];
        let requires = variables(template).map(|v| self.targets.intern(&scalar(v))).collect::<Result<Vec<_>, _>>()?;
        self.insert(Recipe { creates, requires, kind: Kind::Variable(template.to_string()) })
    }
}
// ~/~ end
