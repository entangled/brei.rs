// ~/~ begin <<docs/src/brei.md#brei::src/tasks/rule.rs>>[init]
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;
use serde::Deserialize;
use super::target::{Target, VarType, TypeMismatch};

// ~/~ begin <<docs/src/brei.md#brei::rule-value>>[init]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Nothing,
    Scalar(String),
    List(Vec<String>),
}
// ~/~ end
// ~/~ begin <<docs/src/brei.md#brei::rule-failure>>[init]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    Missing(Target),
    UnknownRunner(String),
    InvalidScript(String),
    Template(String),
    Io(String),
    Command { command: String, code: Option<i32> },
    NotAchieved(Vec<Target>),
    Cancelled,
    Rule(String, Box<Failure>),
}

impl From<std::io::Error> for Failure {
    fn from(e: std::io::Error) -> Self {
        Failure::Io(e.to_string())
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::Missing(t) => write!(f, "missing dependency: `{t}`"),
            Failure::UnknownRunner(r) => write!(f, "unknown runner: `{r}`"),
            Failure::InvalidScript(line) => write!(f, "can't parse command: `{line}`"),
            Failure::Template(msg) => write!(f, "error in template: {msg}"),
            Failure::Io(msg) => write!(f, "{msg}"),
            Failure::Command { command, code: Some(code) } => write!(f, "`{command}` failed with exit code {code}"),
            Failure::Command { command, code: None } => write!(f, "`{command}` was killed"),
            Failure::NotAchieved(targets) => {
                let targets: Vec<String> = targets.iter().map(|t| format!("`{t}`")).collect();
                write!(f, "rule didn't create {}", targets.join(", "))
            }
            Failure::Cancelled => write!(f, "cancelled"),
            Failure::Rule(note, failure) => write!(f, "{note}: {failure}"),
        }
    }
}
// ~/~ end
// ~/~ begin <<docs/src/brei.md#brei::rule-runner>>[init]
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Runner {
    pub command: String,
    pub args: Vec<String>,
}

pub fn default_runners() -> HashMap<String, Runner> {
    let runner = |command: &str| Runner { command: command.into(), args: vec!["${script}".into()] };
    HashMap::from([
        ("python".into(), runner("python")),
        ("bash".into(), runner("bash")),
    ])
}
// ~/~ end
// ~/~ begin <<docs/src/brei.md#brei::rule-rule>>[init]
#[derive(Clone, Debug, Default)]
pub struct Rule {
    pub creates: Vec<Target>,
    pub requires: Vec<Target>,
    pub name: Option<String>,
    pub runner: Option<String>,
    pub path: Option<PathBuf>,
    pub script: Option<String>,
    pub stdin: Option<Target>,
    pub stdout: Option<Target>,
    pub description: Option<String>,
    pub force: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RuleError {
    TypeMismatch(TypeMismatch),
    Duplicate(Target),
    Invalid(String),
}

impl From<TypeMismatch> for RuleError {
    fn from(e: TypeMismatch) -> Self {
        RuleError::TypeMismatch(e)
    }
}

impl fmt::Display for RuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RuleError::TypeMismatch(e) => write!(f, "{e}"),
            RuleError::Duplicate(t) => write!(f, "`{t}` is created by more than one rule"),
            RuleError::Invalid(msg) => write!(f, "{msg}"),
        }
    }
}

fn dedup(targets: &mut Vec<Target>) {
    let mut seen = HashSet::new();
    targets.retain(|t| seen.insert(t.clone()));
}

impl Rule {
    pub fn target_paths(&self) -> impl Iterator<Item=&PathBuf> {
        self.creates.iter().filter_map(|t| match t { Target::Path(p) => Some(p), _ => None })
    }

    pub fn dependency_paths(&self) -> impl Iterator<Item=&PathBuf> {
        self.requires.iter().filter_map(|t| match t { Target::Path(p) => Some(p), _ => None })
    }

    pub fn digest(&self) -> Option<String> {
        self.script.as_ref().map(|s| format!("{:x}", md5::compute(s)))
    }

    /// A short note on the rule, for the log: its description, its name, or its targets.
    pub fn note(&self) -> String {
        if let Some(description) = &self.description {
            return description.clone();
        }
        if let Some(name) = &self.name {
            return format!("#{name}");
        }
        let targets: Vec<String> = self.creates.iter().map(|t| format!("`{t}`")).collect();
        format!("creating {}", targets.join(" "))
    }

    pub fn always_run(&self) -> bool {
        self.force || self.target_paths().next().is_none() || matches!(self.stdout, Some(Target::Variable(..)))
    }

    /// Add implied targets and dependencies, and check that the rule makes sense.
    pub(super) fn complete(mut self) -> Result<Rule, RuleError> {
        if let Some(name) = &self.name {
            self.creates.push(Target::Phony(name.clone()));
        }
        if let Some(stdin) = &self.stdin {
            self.requires.push(stdin.clone());
        }
        if let Some(path) = &self.path {
            self.requires.push(Target::path(path));
        }
        if let Some(stdout) = &self.stdout {
            self.creates.push(stdout.clone());
        }
        dedup(&mut self.creates);
        dedup(&mut self.requires);

        let invalid = |msg: String| Err(RuleError::Invalid(msg));
        match &self.stdin {
            None | Some(Target::Path(_)) | Some(Target::Variable(_, VarType::Scalar)) => {}
            Some(t) => return invalid(format!("`stdin` can't be `{t}`")),
        }
        if let Some(t @ Target::Phony(_)) = &self.stdout {
            return invalid(format!("`stdout` can't be `{t}`"));
        }
        match (&self.runner, &self.script, &self.path) {
            (_, Some(_), Some(_)) => invalid("a rule can't have both `script` and `path`".into()),
            (None, None, Some(_)) => invalid("a rule with a `path` needs a `runner`".into()),
            (Some(_), None, None) => invalid("a rule with a `runner` needs a `script` or `path`".into()),
            (None, Some(script), None) if script.lines().count() > 1 && self.stdin.is_some() =>
                invalid("a script of more than one line can't have `stdin`, use a runner".into()),
            _ => Ok(self),
        }
    }
}
// ~/~ end
// ~/~ end
