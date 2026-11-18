// ~/~ begin <<docs/src/program_loader.md#src/loader.rs>>[init]
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use serde::de::DeserializeOwned;
use crate::tasks::Program;

// ~/~ begin <<docs/src/program_loader.md#loader-error>>[init]
/// Why a program can't be read.
#[derive(Debug)]
pub enum LoadError {
    /// The file can't be read.
    Io(PathBuf, String),
    /// The extension isn't `.toml`, `.yaml`, `.yml` or `.json`.
    UnknownFormat(PathBuf),
    /// The file isn't a valid program.
    Invalid(PathBuf, String),
    /// The file doesn't have the given section.
    MissingSection(PathBuf, String),
    /// No input file was given, and none was found.
    NoInput,
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Io(path, msg) => write!(f, "can't read `{}`: {msg}", path.display()),
            LoadError::UnknownFormat(path) => write!(f, "unknown file format: `{}`, use `.toml`, `.yaml` or `.json`", path.display()),
            LoadError::Invalid(path, msg) => write!(f, "in `{}`: {msg}", path.display()),
            LoadError::MissingSection(path, section) => write!(f, "`{}` should contain a section `{section}`", path.display()),
            LoadError::NoInput => write!(f,
                "no input file given, and no `brei.toml`, `brei.yaml`, `brei.yml`, or `pyproject.toml` with a `[tool.brei]` section, was found"),
        }
    }
}
// ~/~ end
// ~/~ begin <<docs/src/program_loader.md#loader-read>>[init]
/// Read a TOML, YAML or JSON file, or a section of it (`a.b` for nested sections).
pub fn read_from_file<T: DeserializeOwned>(path: &Path, section: Option<&str>) -> Result<T, LoadError> {
    let text = fs::read_to_string(path).map_err(|e| LoadError::Io(path.into(), e.to_string()))?;
    let invalid = |msg: String| LoadError::Invalid(path.into(), msg);
    let missing = || LoadError::MissingSection(path.into(), section.unwrap_or("").into());
    let keys = || section.into_iter().flat_map(|s| s.split('.'));
    let json = |mut value: serde_json::Value| {
        for key in keys() {
            value = value.get_mut(key).ok_or_else(missing)?.take();
        }
        serde_json::from_value(value).map_err(|e| invalid(e.to_string()))
    };

    match path.extension().and_then(|e| e.to_str()) {
        Some("toml") if section.is_none() => toml::from_str(&text).map_err(|e| invalid(e.to_string())),
        Some("toml") => {
            let mut value: toml::Value = toml::from_str(&text).map_err(|e| invalid(e.to_string()))?;
            for key in keys() {
                value = value.get_mut(key).ok_or_else(missing)?.clone();
            }
            value.try_into().map_err(|e: toml::de::Error| invalid(e.to_string()))
        }
        Some("json") => json(serde_json::from_str(&text).map_err(|e| invalid(e.to_string()))?),
        Some("yaml" | "yml") if section.is_none() => serde_saphyr::from_str(&text).map_err(|e| invalid(e.to_string())),
        Some("yaml" | "yml") => json(serde_saphyr::from_str(&text).map_err(|e| invalid(e.to_string()))?),
        _ => Err(LoadError::UnknownFormat(path.into())),
    }
}
// ~/~ end
// ~/~ begin <<docs/src/program_loader.md#loader-program>>[init]
/// Split `file[section]` into the file and the section.
fn split_input(input: &str) -> (&str, Option<&str>) {
    match input.strip_suffix(']').and_then(|s| s.split_once('[')) {
        Some((file, section)) if !file.is_empty() && !section.is_empty() => (file, Some(section)),
        _ => (input, None),
    }
}

/// Read the program given on the command line, or find one in `dir`.
pub fn read_program(dir: &Path, input: Option<&str>) -> Result<Program, LoadError> {
    if let Some(input) = input {
        let (file, section) = split_input(input);
        return read_from_file(&dir.join(file), section);
    }
    for name in ["brei.toml", "brei.yaml", "brei.yml"] {
        let brei = dir.join(name);
        if brei.exists() {
            return read_from_file(&brei, None);
        }
    }
    let pyproject = dir.join("pyproject.toml");
    if pyproject.exists() {
        return match read_from_file(&pyproject, Some("tool.brei")) {
            Err(LoadError::MissingSection(..)) => Err(LoadError::NoInput),
            result => result,
        };
    }
    Err(LoadError::NoInput)
}

/// Read an included program.
pub fn load_include(path: &Path) -> Result<Program, String> {
    read_from_file(path, None).map_err(|e| e.to_string())
}
// ~/~ end

#[cfg(test)]
mod tests {
    // ~/~ begin <<docs/src/program_loader.md#loader-tests>>[init]
    use std::fs;
    use std::sync::Arc;
    use tempfile::TempDir;
    use crate::noodles::kernel::Kernel;
    use crate::noodles::single_worker::SingleWorker;
    use crate::tasks::{TaskDb, Argument, Join, Value, resolve, build};
    use super::*;

    const SAMPLE: &str = r#"
        [environment]
        dir = "out"

        [[task]]
        description = "Creating output directory"
        creates = ["out/.keep"]
        script = "mkdir -p out\ntouch out/.keep"
        runner = "bash"

        [template.echo]
        stdout = "${stdout}"
        requires = ["out/.keep"]
        script = "echo ${text}"

        [template.rot13]
        stdout = "${stdout}"
        stdin = "${stdin}"
        script = "tr a-zA-Z n-za-mN-ZA-M"

        [[call]]
        template = "echo"
            [call.args]
            stdout = "${dir}/secret.txt"
            text = "Uryyb, Jbeyq!"

        [[call]]
        template = "rot13"
        join = "outer"
            [call.args]
            stdin = "${dir}/secret.txt"
            stdout = "var(message)"

        [runner.sh]
        command = "sh"
        args = ["${script}"]
    "#;

    fn write(dir: &TempDir, path: &str, text: &str) {
        fs::write(dir.path().join(path), text).unwrap();
    }
    // ~/~ end
    // ~/~ begin <<docs/src/program_loader.md#loader-tests>>[1]
    #[test]
    fn sample() {
        let dir = TempDir::new().unwrap();
        write(&dir, "brei.toml", SAMPLE);
        let program = read_program(dir.path(), None).unwrap();
        assert_eq!(program.task[0].description.as_deref(), Some("Creating output directory"));
        assert_eq!(program.call[1].join, Join::Outer);
        assert_eq!(program.call[0].args["text"], Argument::Single("Uryyb, Jbeyq!".into()));
        assert_eq!(program.runner["sh"].command, "sh");

        let db = Arc::new(TaskDb::new(dir.path()));
        let (mut kernel, worker) = (Kernel::new(), SingleWorker::new());
        resolve(&db, &mut kernel, &worker, program, &load_include).unwrap();
        let id = db.targets.lookup(&"var(message)".parse().unwrap()).unwrap();
        assert_eq!(build(&db, &mut kernel, &worker, id).unwrap(), Value::Scalar("Hello, World!".into()));
    }
    // ~/~ end
    // ~/~ begin <<docs/src/program_loader.md#loader-tests>>[2]
    #[test]
    fn yaml() {
        let dir = TempDir::new().unwrap();
        write(&dir, "workflow.yaml", r#"
    environment:
      name: World
    task:
      - stdout: var(greeting)
        script: echo Hello
    template:
      echo:
        stdout: ${out}
        script: echo ${greeting}, ${name}!
    call:
      - template: echo
        args: { out: "var(message)" }
    "#);
        let program = read_program(dir.path(), Some("workflow.yaml")).unwrap();
        let db = Arc::new(TaskDb::new(dir.path()));
        let (mut kernel, worker) = (Kernel::new(), SingleWorker::new());
        resolve(&db, &mut kernel, &worker, program, &load_include).unwrap();
        let id = db.targets.lookup(&"var(message)".parse().unwrap()).unwrap();
        assert_eq!(build(&db, &mut kernel, &worker, id).unwrap(), Value::Scalar("Hello, World!".into()));
    }
    // ~/~ end
    // ~/~ begin <<docs/src/program_loader.md#loader-tests>>[3]
    #[test]
    fn invalid() {
        let dir = TempDir::new().unwrap();
        write(&dir, "typo.toml", "[[task]]\nscirpt = \"echo\"\n");
        write(&dir, "join.toml", "[[call]]\ntemplate = \"t\"\nargs = {}\njoin = \"left\"\n");
        write(&dir, "list.json", r#"{"task": [{"creates": "a.txt"}]}"#);
        write(&dir, "typo.yaml", "task:\n  - scirpt: echo\n");
        write(&dir, "brei.ini", "");
        for file in ["typo.toml", "join.toml", "list.json", "typo.yaml"] {
            let err = read_program(dir.path(), Some(file)).unwrap_err();
            assert!(matches!(err, LoadError::Invalid(..)), "{file}: {err}");
            assert!(err.to_string().starts_with(&format!("in `{}", dir.path().join(file).display())));
        }
        assert!(matches!(read_program(dir.path(), Some("brei.ini")), Err(LoadError::UnknownFormat(_))));
        assert!(matches!(read_program(dir.path(), Some("nothere.toml")), Err(LoadError::Io(..))));
    }
    // ~/~ end
    // ~/~ begin <<docs/src/program_loader.md#loader-tests>>[4]
    #[test]
    fn sections() {
        let dir = TempDir::new().unwrap();
        assert!(matches!(read_program(dir.path(), None), Err(LoadError::NoInput)));
        write(&dir, "pyproject.toml", "[project]\nname = \"test\"\n");
        assert!(matches!(read_program(dir.path(), None), Err(LoadError::NoInput)));

        write(&dir, "pyproject.toml", "[tool.brei]\ninclude = [\"pyproject\"]\n");
        assert_eq!(read_program(dir.path(), None).unwrap().include, ["pyproject"]);
        write(&dir, "brei.yml", "include: [yml]\n");
        assert_eq!(read_program(dir.path(), None).unwrap().include, ["yml"]);
        write(&dir, "brei.yaml", "include: [yaml]\n");
        assert_eq!(read_program(dir.path(), None).unwrap().include, ["yaml"]);
        write(&dir, "brei.toml", "include = [\"brei\"]\n");
        assert_eq!(read_program(dir.path(), None).unwrap().include, ["brei"]);

        write(&dir, "workflow.json", r#"{"a": {"b": {"include": ["json"]}}}"#);
        assert_eq!(read_program(dir.path(), Some("workflow.json[a.b]")).unwrap().include, ["json"]);
        assert!(matches!(read_program(dir.path(), Some("workflow.json[a.c]")), Err(LoadError::MissingSection(..))));
        write(&dir, "workflow.yml", "a:\n  b:\n    include: [yaml]\n");
        assert_eq!(read_program(dir.path(), Some("workflow.yml[a.b]")).unwrap().include, ["yaml"]);
        assert!(matches!(read_program(dir.path(), Some("workflow.yml[a.c]")), Err(LoadError::MissingSection(..))));
        assert_eq!(read_program(dir.path(), Some("pyproject.toml[tool.brei]")).unwrap().include, ["pyproject"]);
        assert_eq!(split_input("a[b]c"), ("a[b]c", None));
    }
    // ~/~ end
    // ~/~ begin <<docs/src/program_loader.md#loader-tests>>[5]
    #[test]
    fn rule_errors() {
        let dir = TempDir::new().unwrap();
        write(&dir, "brei.toml", r#"
            [[task]]
            stdout = "listvar(x)"
            script = "true"

            [[task]]
            creates = ["b.txt"]
            script = "echo ${x}"
        "#);
        let program = read_program(dir.path(), None).unwrap();
        let db = Arc::new(TaskDb::new(dir.path()));
        let err = resolve(&db, &mut Kernel::new(), &SingleWorker::new(), program, &load_include).unwrap_err();
        assert_eq!(err.to_string(), "in rule for `b.txt`: `x` is a list variable, it can only be used as `splice(x)`");
    }
    // ~/~ end
}
// ~/~ end
