// ~/~ begin <<docs/src/brei.md#brei::src/tasks/target.rs>>[init]
use std::collections::HashMap;
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use std::sync::RwLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VarType {
    Scalar,
    List,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    Path(PathBuf),
    Phony(String),
    Variable(String, VarType),
}

// ~/~ begin <<docs/src/brei.md#brei::target-parse>>[init]
pub(super) fn wrapped<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let name = s.strip_prefix(prefix)?.strip_prefix('(')?.strip_suffix(')')?;
    let valid = !name.is_empty() && !name.contains(|c: char| c.is_whitespace() || c == '(' || c == ')');
    valid.then_some(name)
}

fn normalize(path: &Path) -> PathBuf {
    let normal: PathBuf = path.components().filter(|c| *c != Component::CurDir).collect();
    if normal.as_os_str().is_empty() { PathBuf::from(".") } else { normal }
}

impl Target {
    /// A path target, normalized.
    pub fn path(path: impl AsRef<Path>) -> Self {
        Target::Path(normalize(path.as_ref()))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct EmptyTarget;

impl FromStr for Target {
    type Err = EmptyTarget;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err(EmptyTarget);
        }
        Ok(if let Some(name) = s.strip_prefix('#') {
            Target::Phony(name.to_string())
        } else if let Some(name) = wrapped(s, "var") {
            Target::Variable(name.to_string(), VarType::Scalar)
        } else if let Some(name) = wrapped(s, "listvar") {
            Target::Variable(name.to_string(), VarType::List)
        } else {
            Target::path(s)
        })
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Target::Path(p) => write!(f, "{}", p.display()),
            Target::Phony(name) => write!(f, "#{}", name),
            Target::Variable(name, VarType::Scalar) => write!(f, "var({})", name),
            Target::Variable(name, VarType::List) => write!(f, "listvar({})", name),
        }
    }
}
// ~/~ end
// ~/~ begin <<docs/src/brei.md#brei::target-intern>>[init]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TargetId(u32);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Name {
    Path(PathBuf),
    Phony(String),
    Variable(String),
}

impl From<&Target> for Name {
    fn from(target: &Target) -> Self {
        match target {
            Target::Path(p) => Name::Path(p.clone()),
            Target::Phony(n) => Name::Phony(n.clone()),
            Target::Variable(n, _) => Name::Variable(n.clone()),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct TypeMismatch {
    pub name: String,
    pub known: VarType,
    pub found: VarType,
}

impl fmt::Display for TypeMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.known {
            VarType::List => write!(f, "`{}` is a list variable, it can only be used as `splice({})`", self.name, self.name),
            VarType::Scalar => write!(f, "`{}` is not a list variable, use `${{{}}}` instead", self.name, self.name),
        }
    }
}

#[derive(Default)]
struct Table {
    targets: Vec<Target>,
    index: HashMap<Name, TargetId>,
}

impl Table {
    /// Check that `target` agrees with the target that is known under `id`.
    fn check(&self, id: TargetId, target: &Target) -> Result<TargetId, TypeMismatch> {
        match (&self.targets[id.0 as usize], target) {
            (Target::Variable(name, known), Target::Variable(_, found)) if known != found =>
                Err(TypeMismatch { name: name.clone(), known: *known, found: *found }),
            _ => Ok(id),
        }
    }
}

#[derive(Default)]
pub struct Targets {
    table: RwLock<Table>,
}

impl Targets {
    pub fn new() -> Self {
        Self::default()
    }

    /// The id of `target`, adding it to the table if it is new.
    pub fn intern(&self, target: &Target) -> Result<TargetId, TypeMismatch> {
        let name = Name::from(target);
        {
            let table = self.table.read().unwrap();
            if let Some(id) = table.index.get(&name) {
                return table.check(*id, target);
            }
        }
        let mut table = self.table.write().unwrap();
        // Another thread may have added it in the mean time.
        if let Some(id) = table.index.get(&name) {
            return table.check(*id, target);
        }
        let id = TargetId(table.targets.len().try_into().expect("too many targets"));
        table.targets.push(target.clone());
        table.index.insert(name, id);
        Ok(id)
    }

    /// The id of `target`, if it is known (with the same type).
    pub fn lookup(&self, target: &Target) -> Option<TargetId> {
        let table = self.table.read().unwrap();
        let id = *table.index.get(&Name::from(target))?;
        table.check(id, target).ok()
    }

    pub fn get(&self, id: TargetId) -> Target {
        self.table.read().unwrap().targets[id.0 as usize].clone()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.table.read().unwrap().targets.len()
    }
}
// ~/~ end
// ~/~ begin <<docs/src/brei.md#brei::target-key>>[init]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Target(TargetId),
    Local(u32),
}

impl Key {
    /// Whether tasks with this key should be memoized by the engine.
    pub fn is_global(&self) -> bool {
        matches!(self, Key::Target(_))
    }
}

impl From<TargetId> for Key {
    fn from(id: TargetId) -> Self {
        Key::Target(id)
    }
}
// ~/~ end

#[cfg(test)]
mod tests {
    use super::*;

    // ~/~ begin <<docs/src/brei.md#brei::target-tests>>[init]
    fn target(s: &str) -> Target {
        s.parse().unwrap()
    }

    #[test]
    fn parse() {
        assert_eq!(target("data/output.csv"), Target::Path("data/output.csv".into()));
        assert_eq!(target("#all"), Target::Phony("all".into()));
        assert_eq!(target("var(name)"), Target::Variable("name".into(), VarType::Scalar));
        assert_eq!(target("listvar(files)"), Target::Variable("files".into(), VarType::List));
        assert_eq!("".parse::<Target>(), Err(EmptyTarget));
    }

    #[test]
    fn parse_edge_cases() {
        // not a valid variable name, so these are paths
        assert_eq!(target("var(a b)"), Target::Path("var(a b)".into()));
        assert_eq!(target("var()"), Target::Path("var()".into()));
        assert_eq!(target("variable(x)"), Target::Path("variable(x)".into()));
        // normalization
        assert_eq!(target("./data.csv"), target("data.csv"));
        assert_eq!(target("data//output.csv"), target("data/output.csv"));
        assert_eq!(target("data/./output.csv"), target("data/output.csv"));
        assert_ne!(target("a/../b"), target("b"));
        assert_eq!(target("./"), Target::Path(".".into()));
    }

    #[test]
    fn display_round_trip() {
        for s in ["data/output.csv", "#all", "var(name)", "listvar(files)"] {
            assert_eq!(target(s).to_string(), s);
        }
    }
    // ~/~ end
    // ~/~ begin <<docs/src/brei.md#brei::target-tests>>[1]
    #[test]
    fn interning() {
        let targets = Targets::new();
        let a = targets.intern(&target("a.txt")).unwrap();
        let b = targets.intern(&target("#b")).unwrap();
        assert_ne!(a, b);
        assert_eq!(targets.intern(&target("./a.txt")), Ok(a));
        assert_eq!(targets.lookup(&target("var(c)")), None);
        assert_eq!(targets.get(b), target("#b"));
        assert_eq!(targets.len(), 2);
    }

    #[test]
    fn interning_from_threads() {
        let targets = std::sync::Arc::new(Targets::new());
        let handles: Vec<_> = (0..8).map(|_| {
            let targets = targets.clone();
            std::thread::spawn(move || {
                (0..100).map(|i| targets.intern(&target(&format!("file{i}.txt"))).unwrap()).collect::<Vec<_>>()
            })
        }).collect();
        let ids: Vec<Vec<TargetId>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(ids.iter().all(|v| *v == ids[0]));
        assert_eq!(targets.len(), 100);
    }

    #[test]
    fn variable_types() {
        let targets = Targets::new();
        let files = targets.intern(&target("listvar(files)")).unwrap();
        assert_eq!(targets.intern(&target("listvar(files)")), Ok(files));
        assert_eq!(targets.lookup(&target("var(files)")), None);

        let err = targets.intern(&target("var(files)")).unwrap_err();
        assert_eq!(err, TypeMismatch { name: "files".into(), known: VarType::List, found: VarType::Scalar });
        assert_eq!(err.to_string(), "`files` is a list variable, it can only be used as `splice(files)`");

        targets.intern(&target("var(name)")).unwrap();
        let err = targets.intern(&target("listvar(name)")).unwrap_err();
        assert_eq!(err.to_string(), "`name` is not a list variable, use `${name}` instead");
        assert_eq!(targets.len(), 2);
    }
    // ~/~ end
    // ~/~ begin <<docs/src/brei.md#brei::target-tests>>[2]
    #[test]
    fn keys() {
        let targets = Targets::new();
        let key: Key = targets.intern(&target("a.txt")).unwrap().into();
        assert!(key.is_global());
        assert!(!Key::Local(0).is_global());
    }
    // ~/~ end
}
// ~/~ end
