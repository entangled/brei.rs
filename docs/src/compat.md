Compatibility with Python Brei
==============================

This chapter ports the tests of the Python version (`test/` in its repository), and the examples from its documentation, to check that we behave the same. The tests run the `brei` binary on a workflow in a temporary directory, so they test everything from reading the program to running commands. Many workflows use the `python` runner, so these tests need Python, and also `git` and `lua` for some of the examples.

Tests of the Python internals, like the lazy evaluation in `test_lazy.py`, have their counterparts in the [engine](./engine.md) and [Brei](./brei.md) chapters instead. The template string tests are in [Template Strings](./template_strings.md).

Differences
-----------

These differences from the Python version are deliberate:

- **Exit code:** when something fails, `brei` exits with code 1.
- **Standard input:** a command without `stdin` reads from `/dev/null`, not from the terminal.
- **Dollar escapes:** `$$` stays a literal dollar sign through calls of templates. In Python, every substitution turns `$$` into `$`, so `$$` in a template that is called ends up as a variable.
- **Error messages** are worded differently. For example, splicing a scalar variable says that it `is not a list variable`, where Python says that `splice` `needs a list variable`.
- **Empty variables:** a scalar variable with empty output is `""`, where Python gives `None`.
- **Variable rules:** a rule that writes to a variable always runs, also when the files it creates are up to date. Python skips it, and the variable has no value.
- **History:** the history file is always used. The Python library can also run without one.
- **YAML:** workflows can also be written in YAML (`.yaml` or `.yml`). Python Brei only reads TOML and JSON.

Setup
-----

A `Workflow` is a temporary directory with a `brei.toml`. Some examples create a temporary directory with `mktemp -d`; we set `TMPDIR` so that it ends up in the test directory too.

```rust
//| file: tests/python.rs
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant, SystemTime};
use tempfile::TempDir;

struct Workflow {
    dir: TempDir,
}

impl Workflow {
    fn new(program: &str) -> Self {
        let workflow = Workflow { dir: TempDir::new().unwrap() };
        workflow.write("brei.toml", program);
        workflow
    }

    fn path(&self, path: &str) -> std::path::PathBuf {
        self.dir.path().join(path)
    }

    fn write(&self, path: &str, text: &str) {
        let path = self.path(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn read(&self, path: &str) -> String {
        fs::read_to_string(self.path(path)).unwrap()
    }

    fn exists(&self, path: &str) -> bool {
        self.path(path).exists()
    }

    fn modified(&self, path: &str) -> SystemTime {
        fs::metadata(self.path(path)).unwrap().modified().unwrap()
    }

    fn brei(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_brei"))
            .args(args).current_dir(self.dir.path()).env("TMPDIR", self.dir.path()).env("NO_COLOR", "1")
            .output().unwrap()
    }

    /// Run `brei`, which should succeed; gives the log.
    fn run(&self, args: &[&str]) -> String {
        let output = self.brei(args);
        let log = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(output.status.success(), "brei {args:?} failed:\n{log}");
        log
    }

    /// Run `brei`, which should fail; gives the log.
    fn fail(&self, args: &[&str]) -> String {
        let output = self.brei(args);
        let log = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(!output.status.success(), "brei {args:?} should fail:\n{log}");
        log
    }

    /// Check the contents of files, ignoring white space around them.
    fn check(&self, files: &[(&str, &str)]) {
        for (path, content) in files {
            assert!(self.exists(path), "`{path}` doesn't exist");
            assert_eq!(self.read(path).trim(), content.trim(), "contents of `{path}`");
        }
    }
}

/// Give file modification times a chance to differ.
fn tick() {
    thread::sleep(Duration::from_millis(20));
}

<<python-tests>>
```

Workflows
---------

From `test_program.py`: workflows that build `#all`, with the files they should create.

```rust
//| id: python-tests
const HELLO_WORLD: &str = r##"
[[task]]
name = "all"
requires = ["hello.txt"]

[[task]]
stdout = "hello.txt"
script = "echo 'Hello, World'"
"##;

const INCLUDE: &str = r##"
include = [
    "generated_wf.toml"
]

[[task]]
stdout = "generated_wf.toml"
runner = "python"
script = '''
print("""
[[task]]
stdout = "hello.txt"
script = "echo 'Hello, World'"
""")
'''

[[task]]
name = "all"
requires = ["hello.txt"]
"##;

const TEMPLATE: &str = r##"
[template.echo]
stdout = "${stdout}"
runner = "python"
script = '''
print("${text}")
'''

[[task]]
name = "all"
requires = ["hello.txt"]

[[call]]
template = "echo"
args = { stdout = "hello.txt", text = "Hello, World" }
"##;

const ROT_13: &str = r##"
[[task]]
stdout = "secret.txt"
runner = "python"
script = """
print("Uryyb, Jbeyq!")
"""

[template.rot13]
stdout = "${stdout}"
stdin = "${stdin}"
script = "tr a-zA-Z n-za-mN-ZA-M"

[[call]]
template = "rot13"
  [call.args]
  stdin = "secret.txt"
  stdout = "hello.txt"

[[task]]
name = "all"
requires = ["hello.txt"]
"##;

const TEMPLATED_TASK: &str = r##"
[environment]
msg = "Hello, World!"

[[task]]
stdout = "hello.txt"
script = "echo '${msg}'"

[[task]]
name = "all"
requires = ["hello.txt"]
"##;

const VARIABLE_STDOUT: &str = r##"
[template.echo]
script = "echo '${text}'"
stdout = "${stdout}"

[[call]]
template = "echo"
  [call.args]
  text = "Hello, World!"
  stdout = "var(msg)"

[[task]]
script = "cat"
stdin = "var(msg)"
stdout = "hello.txt"

[[call]]
template = "echo"
  [call.args]
  text = "goodbye.txt"
  stdout = "var(file_name)"

[[task]]
script = "cat"
stdin = "var(msg)"
stdout = "${file_name}"

[[task]]
name = "all"
requires = ["hello.txt", "${file_name}"]
"##;

const ARRAY_CALL: &str = r##"
[template.echo]
script = "echo '${a}${b}'"
stdout = "${pre}-${a}-${b}"

[[call]]
template = "echo"
collect = "inner"
  [call.args]
  pre = "zip"
  a = ["1", "2", "3"]
  b = ["a", "b", "c"]

[[call]]
template = "echo"
join = "outer"
collect = "outer"
  [call.args]
  pre = "prod"
  a = ["1", "2"]
  b = ["a", "b"]

[[task]]
name = "all"
requires = ["#inner", "#outer"]
"##;

#[cfg_attr(windows, ignore = "uses Unix tools")]
#[test]
fn workflows() {
    let hello: &[(&str, &str)] = &[("hello.txt", "Hello, World")];
    let cases: &[(&str, &[(&str, &str)])] = &[
        (HELLO_WORLD, hello),
        (INCLUDE, hello),
        (TEMPLATE, hello),
        (ROT_13, &[("hello.txt", "Hello, World!")]),
        (TEMPLATED_TASK, &[("hello.txt", "Hello, World!")]),
        (VARIABLE_STDOUT, &[("hello.txt", "Hello, World!"), ("goodbye.txt", "Hello, World!")]),
        (ARRAY_CALL, &[("zip-1-a", "1a"), ("zip-2-b", "2b"), ("zip-3-c", "3c"),
                       ("prod-1-a", "1a"), ("prod-1-b", "1b"), ("prod-2-a", "2a"), ("prod-2-b", "2b")]),
    ];
    for (program, files) in cases {
        let workflow = Workflow::new(program);
        workflow.run(&["all"]);
        workflow.check(files);
    }
}
```

A rule with `force` always runs, also when its targets are up to date.

```rust
//| id: python-tests
const FORCE_RUN: &str = r##"
[[task]]
name = "forced"
force = true
creates = ["forced.txt"]
script = "touch forced.txt"

[[task]]
name = "unforced"
force = false
creates = ["unforced.txt"]
script = "touch unforced.txt"

[[task]]
name = "all"
requires = ["#forced", "#unforced"]
"##;

#[cfg_attr(windows, ignore = "uses Unix tools")]
#[test]
fn forcing() {
    let workflow = Workflow::new(FORCE_RUN);
    workflow.run(&["all"]);
    let (forced, unforced) = (workflow.modified("forced.txt"), workflow.modified("unforced.txt"));
    tick();
    workflow.run(&["all"]);
    assert!(workflow.modified("forced.txt") > forced);
    assert_eq!(workflow.modified("unforced.txt"), unforced);
}
```

A failing command fails, also when it wrote some output. Its target may be written partially, so it shouldn't be in the history.

```rust
//| id: python-tests
const FAILING: &str = r##"
[[task]]
stdout = "runner.txt"
runner = "python"
script = '''
import sys
print("partial output")
sys.exit(3)
'''

[[task]]
stdout = "oneliner.txt"
script = "python -c 'import sys; sys.exit(4)'"
"##;

#[test]
fn failing_command() {
    for (target, code) in [("runner.txt", 3), ("oneliner.txt", 4)] {
        let workflow = Workflow::new(FAILING);
        let log = workflow.fail(&[target]);
        assert!(log.contains(&format!("failed with exit code {code}")), "{log}");
        assert!(!workflow.read(".brei_history").contains(target));
    }
}
```

List variables
--------------

A call can splice a list variable into its arguments. Empty lines are not part of the list.

```rust
//| id: python-tests
const LIST_NAMES: &str = r##"
[[task]]
stdout = "listvar(names)"
script = '''python -c 'print("a"); print(""); print("b")' '''
"##;

const SPLICE_CALL: &str = r##"
[template.write]
stdout = "out-${name}.txt"
script = '''python -c 'print("${name}")' '''

[[call]]
template = "write"
collect = "outs"
args = { name = ["splice(names)", "c"] }

[[task]]
name = "all"
requires = ["#outs"]
"##;

#[test]
fn splice_call() {
    let workflow = Workflow::new(&format!("{LIST_NAMES}{SPLICE_CALL}"));
    workflow.run(&["all"]);
    workflow.check(&[("out-a.txt", "a"), ("out-b.txt", "b"), ("out-c.txt", "c")]);
    assert!(!workflow.exists("out-.txt"));
}
```

A rule can splice a list variable into its dependencies. When one of them changes, the rule runs again.

```rust
//| id: python-tests
const SPLICE_REQUIRES: &str = r##"
[[task]]
stdout = "listvar(inputs)"
script = '''python -c 'print("in-1.txt"); print("in-2.txt")' '''

[[task]]
stdout = "summary.txt"
requires = ["splice(inputs)"]
script = '''python -c 'print(open("in-1.txt").read() + open("in-2.txt").read())' '''
"##;

#[test]
fn splice_requires() {
    let workflow = Workflow::new(SPLICE_REQUIRES);
    workflow.write("in-1.txt", "1");
    workflow.write("in-2.txt", "2");
    workflow.run(&["summary.txt"]);
    workflow.check(&[("summary.txt", "12")]);

    tick();
    workflow.write("in-2.txt", "3");
    workflow.run(&["summary.txt"]);
    workflow.check(&[("summary.txt", "13")]);
}

#[test]
fn splice_empty_list() {
    let workflow = Workflow::new(r##"
[[task]]
stdout = "listvar(nothing)"
script = "python -c 'pass'"

[template.write]
stdout = "out-${name}.txt"
script = "python -c 'pass'"

[[call]]
template = "write"
collect = "outs"
args = { name = ["splice(nothing)"] }
"##);
    workflow.run(&["outs"]);
    assert!(!fs::read_dir(workflow.dir.path()).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().starts_with("out-")));
}
```

List variables used in the wrong way are errors.

```rust
//| id: python-tests
#[test]
fn list_variable_errors() {
    let cases = [
        (format!("{LIST_NAMES}{}", r##"
[[task]]
stdout = "x.txt"
script = "python -c 'print(\"${names}\")'"
"##), "list variable"),
        (format!("{LIST_NAMES}{}", r##"
[[task]]
stdout = "x.txt"
requires = ["listvar(names)"]
script = "python -c 'pass'"
"##), "can only be used as `stdout`"),
        (r##"
[[task]]
stdout = "var(name)"
script = "python -c 'print(1)'"

[template.write]
stdout = "out-${name}.txt"
script = "python -c 'pass'"

[[call]]
template = "write"
args = { name = ["splice(name)"] }
"##.to_string(), "is not a list variable"),
        (format!("{LIST_NAMES}{}", r##"
[template.write]
stdout = "out-${a}-${b}.txt"
script = "python -c 'pass'"

[[call]]
template = "write"
args = { a = ["splice(names)"], b = ["1", "2", "3"] }
"##), "same length"),
    ];
    for (program, message) in cases {
        let log = Workflow::new(&program).fail(&["x.txt"]);
        assert!(log.contains(message), "expected `{message}` in:\n{log}");
    }
}
```

History
-------

From `test_history.py`. With the history, a rule that is up to date doesn't run, until its script changes. Without the history (here: when we remove it), it always runs, because we can't know which script created the target.

```rust
//| id: python-tests
const WF1: &str = r##"{"task": [{"stdout": "x", "script": "echo 'hello'"}, {"name": "all", "requires": ["x"]}]}"##;
const WF2: &str = r##"{"task": [{"stdout": "x", "script": "echo 'goodbye'"}, {"name": "all", "requires": ["x"]}]}"##;

#[cfg_attr(windows, ignore = "uses Unix tools")]
#[test]
fn history() {
    let workflow = Workflow::new("");
    workflow.write("wf1.json", WF1);
    workflow.write("wf2.json", WF2);

    workflow.run(&["-i", "wf1.json", "all"]);
    let s1 = workflow.modified("x");
    tick();
    workflow.run(&["-i", "wf1.json", "all"]);
    let s2 = workflow.modified("x");
    tick();
    workflow.run(&["-i", "wf2.json", "all"]);
    let s3 = workflow.modified("x");
    assert_eq!(workflow.read("x").trim(), "goodbye");
    assert_eq!(s1, s2);
    assert!(s3 > s2);

    fs::remove_file(workflow.path(".brei_history")).unwrap();
    tick();
    workflow.run(&["-i", "wf2.json", "all"]);
    assert!(workflow.modified("x") > s3);
}
```

Rules and programs
------------------

From `test_lazy.py`: rules run in parallel, and a change in an input file rebuilds what depends on it.

```rust
//| id: python-tests
#[test]
fn runtime() {
    let mut program = String::new();
    for a in 0..4 {
        program += &format!("[[task]]\nname = \"sleep{a}\"\nrunner = \"python\"\nscript = \"import time; time.sleep(0.2)\"\n\n");
    }
    program += "[[task]]\nname = \"all\"\nrequires = [\"#sleep0\", \"#sleep1\", \"#sleep2\", \"#sleep3\"]\n";
    let workflow = Workflow::new(&program);
    let start = Instant::now();
    workflow.run(&["-j", "4", "all"]);
    let elapsed = start.elapsed();
    // One after another would take 0.8 seconds.
    assert!(elapsed > Duration::from_millis(200) && elapsed < Duration::from_millis(800), "{elapsed:?}");
}

const REBUILD: &str = r##"
[[task]]
stdout = "a"
requires = ["i1"]
runner = "python"
script = "print(int(open('i1','r').read()) + 1)"

[[task]]
stdout = "b"
requires = ["a", "i2"]
runner = "python"
script = "print(int(open('a','r').read()) * int(open('i2','r').read()))"

[[task]]
stdout = "c"
requires = ["a", "b"]
runner = "python"
script = "print(int(open('b','r').read()) * int(open('a','r').read()))"
"##;

#[test]
fn rebuild() {
    let workflow = Workflow::new(REBUILD);
    workflow.write("i1", "1\n");
    workflow.write("i2", "3\n");
    workflow.run(&["c"]);
    workflow.check(&[("a", "2"), ("b", "6"), ("c", "12")]);
    let a = workflow.modified("a");

    tick();
    workflow.write("i2", "4\n");
    workflow.run(&["c"]);
    assert_eq!(workflow.modified("a"), a);
    workflow.check(&[("a", "2"), ("b", "8"), ("c", "16")]);
}
```

From `test_construct.py`: programs with mistakes in them. Unknown fields are errors.

```rust
//| id: python-tests
#[test]
fn construct_errors() {
    let workflow = Workflow::new("");
    let cases = [
        (r##"{"task": [{"creates": "hello.txt"}]}"##, "invalid type"),
        (r##"{"call": [{"template": "t", "args": {}, "join": "sideways"}]}"##, "unknown variant"),
        (r##"{"tasks": []}"##, "unknown field"),
        (r##"{"task": [{"creates": ["x"], "requries": ["y"]}]}"##, "unknown field"),
        (r##"{"template": {"t": {"creates": ["${x}"], "bogus": 1}}}"##, "unknown field"),
        (r##"{"call": [{"template": "t", "args": {}, "jion": "outer"}]}"##, "unknown field"),
        (r##"{"runner": {"r": {"command": "r", "args": [], "env": {}}}}"##, "unknown field"),
    ];
    for (program, message) in cases {
        workflow.write("program.json", program);
        let log = workflow.fail(&["-i", "program.json"]);
        assert!(log.contains(message), "expected `{message}` in:\n{log}");
    }
}
```

Examples
--------

The examples from the documentation of the Python version.

```rust
//| id: python-tests
const ROT13_EXAMPLE: &str = r##"
[[task]]
description = "Creating temporary directory"
stdout = "var(dir)"
script = "mktemp -d"

[template.echo]
stdout = "${stdout}"
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
  [call.args]
  stdin = "${dir}/secret.txt"
  stdout = "${dir}/msg.txt"

[[task]]
name = "all"
requires = ["${dir}/msg.txt"]
script = "cat ${dir}/msg.txt"
"##;

const TASKS_EXAMPLE: &str = r##"
[[task]]
creates = ["hello.txt"]
runner = "bash"
script = "echo 'Hello, World!' > hello.txt"

[[task]]
name = "all"
requires = ["hello.txt"]

[[task]]
name = "clean"
script = "rm hello.txt"
"##;

#[cfg_attr(windows, ignore = "uses Unix tools")]
#[test]
fn example_tasks() {
    let workflow = Workflow::new(TASKS_EXAMPLE);
    workflow.run(&["all"]);
    workflow.check(&[("hello.txt", "Hello, World!")]);
    workflow.run(&["clean"]);
    assert!(!workflow.exists("hello.txt"));
}

#[cfg_attr(windows, ignore = "uses Unix tools")]
#[test]
fn example_rot13() {
    let log = Workflow::new(ROT13_EXAMPLE).run(&["all"]);
    assert!(log.contains("#all Hello, World!"), "{log}");
}
```

Templates can be called with lists, which are joined, or combined in every way. The targets of the template are in a temporary directory.

```rust
//| id: python-tests
const MULTIPLEXING_EXAMPLE: &str = r##"
[[task]]
description = "Creating temporary directory"
stdout = "var(dir)"
script = "mktemp -d"

[template.touch]
description = "${pre} ${a} ${b}"
creates = ["${dir}/${pre}-${a}-${b}"]
script = "touch '${dir}/${pre}-${a}-${b}'"

[[call]]
template = "touch"
collect = "inner"
  [call.args]
  pre = "inner"
  a = ["x", "y", "z"]
  b = ["1", "2", "3"]

[[call]]
template = "touch"
collect = "outer"
join = "outer"
  [call.args]
  pre = "outer"
  a = ["x", "y"]
  b = ["1", "2"]

[[task]]
name = "all"
requires = ["#inner", "#outer"]
"##;

#[cfg_attr(windows, ignore = "uses Unix tools")]
#[test]
fn example_template_multiplexing() {
    let workflow = Workflow::new(MULTIPLEXING_EXAMPLE);
    workflow.run(&["all"]);
    let tmp = fs::read_dir(workflow.dir.path()).unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .expect("temporary directory");
    let mut files: Vec<String> = fs::read_dir(tmp).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    files.sort();
    assert_eq!(files, ["inner-x-1", "inner-y-2", "inner-z-3", "outer-x-1", "outer-x-2", "outer-y-1", "outer-y-2"]);
}
```

Includes can be static files, or generated by a rule. Here, a Python script generates a workflow in JSON.

```rust
//| id: python-tests
const ECHO_EXAMPLE: &str = r##"
[template.echo]
stdout = "${stdout}"
script = "echo '${text}'"
"##;

const HELLO_INCLUDES_EXAMPLE: &str = r##"
include = [
    "./echo.toml"
]

[[call]]
template = "echo"
  [call.args]
  stdout = "hello.txt"
  text = "Hello, World!"

[[task]]
name = "all"
requires = ["hello.txt"]
"##;

const INCLUDE_GEN_EXAMPLE: &str = r##"
include = [
    "${dir}/gen.json"
]

[[task]]
description = "Creating temporary directory"
stdout = "var(dir)"
script = "mktemp -d"

[[task]]
description = "Generating workflow"
stdout = "${dir}/gen.json"
runner = "python"
script = """
import json
tasks = [
    {"stdout": f"${dir}/out{i}.dat",
     "script": f"echo '{i}'"} for i in range(10)
]
tasks.append({"name": "write-outs", "requires": [
    f"${dir}/out{i}.dat" for i in range(10)
]})
print(json.dumps({"task": tasks}))
"""

[[task]]
name = "all"
requires = ["#write-outs"]
"##;

#[cfg_attr(windows, ignore = "uses Unix tools")]
#[test]
fn example_includes() {
    let workflow = Workflow::new(HELLO_INCLUDES_EXAMPLE);
    workflow.write("echo.toml", ECHO_EXAMPLE);
    workflow.run(&["all"]);
    workflow.check(&[("hello.txt", "Hello, World!")]);

    let workflow = Workflow::new(INCLUDE_GEN_EXAMPLE);
    let log = workflow.run(&["all"]);
    let count = log.lines().filter(|l| l.starts_with("creating `") && l.ends_with(".dat`")).count();
    assert_eq!(count, 10, "{log}");
}
```

A list variable gives the files to copy.

```rust
//| id: python-tests
const LIST_VARIABLES_EXAMPLE: &str = r##"
[[task]]
description = "Listing figures"
stdout = "listvar(figures)"
runner = "python"
script = """
from pathlib import Path
for p in sorted(Path("docs/fig").glob("*.svg")):
    print(p.name)
"""

[template.copy]
description = "copy `${name}`"
requires = ["docs/fig/${name}"]
creates = ["docs/site/fig/${name}"]
runner = "python"
script = """
import shutil
shutil.copy("docs/fig/${name}", "docs/site/fig/${name}")
"""

[[call]]
template = "copy"
collect = "figures"
args = { name = ["splice(figures)"] }

[[task]]
name = "all"
requires = ["#figures"]
"##;

#[test]
fn example_list_variables() {
    let workflow = Workflow::new(LIST_VARIABLES_EXAMPLE);
    workflow.write("docs/fig/a.svg", "<svg>a</svg>");
    workflow.write("docs/fig/b.svg", "<svg>b</svg>");
    workflow.write("docs/site/fig/.keep", "");
    workflow.run(&["all"]);
    workflow.check(&[("docs/site/fig/a.svg", "<svg>a</svg>"), ("docs/site/fig/b.svg", "<svg>b</svg>")]);
}
```

The output directory depends on the current git commit. The script `scripts/run.py` is not part of the example, so we write one. Note that the link `output/latest` in this example is broken: `ln -sf ./output/<commit> output/latest` makes a relative link, which points to `output/output/<commit>`. So our script writes to the output directory directly.

```rust
//| id: python-tests
const VERSIONED_OUTPUT_EXAMPLE: &str = r##"
[environment]
data_dir = "./data"
output_dir = "./output/${commit}"

[[task]]
stdout = "var(commit)"
script = "git rev-parse HEAD"

[[task]]
creates = ["${output_dir}/data.h5"]
requires = ["${data_dir}/input.h5", "#prepare"]
runner = "python"
path = "scripts/run.py"

[[task]]
name = "prepare"
script = """
mkdir -p ${output_dir}
ln -sf ${output_dir} output/latest
"""

[[task]]
name = "all"
requires = ["${output_dir}/data.h5"]
"##;

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git").args(args).current_dir(dir)
        .env("GIT_AUTHOR_NAME", "test").env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test").env("GIT_COMMITTER_EMAIL", "test@example.com")
        .status().unwrap();
    assert!(status.success());
}

#[cfg_attr(windows, ignore = "uses Unix tools")]
#[test]
fn example_versioned_output() {
    let workflow = Workflow::new(VERSIONED_OUTPUT_EXAMPLE);
    workflow.write("data/input.h5", "data");
    workflow.write("scripts/run.py", concat!(
        "import shutil, subprocess\n",
        "commit = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()\n",
        "shutil.copy('data/input.h5', f'output/{commit}/data.h5')\n"));
    git(workflow.dir.path(), &["init", "-q"]);
    git(workflow.dir.path(), &["commit", "-q", "--allow-empty", "-m", "test"]);
    let commit = String::from_utf8(Command::new("git").args(["rev-parse", "HEAD"]).current_dir(workflow.dir.path())
        .output().unwrap().stdout).unwrap();

    workflow.run(&["all"]);
    workflow.check(&[(&format!("output/{}/data.h5", commit.trim()), "data")]);
}
```

A custom runner, for Lua. This test is skipped when Lua isn't installed.

```rust
//| id: python-tests
const CUSTOM_RUNNER_EXAMPLE: &str = r##"
[[task]]
description = "Creating temporary directory"
stdout = "var(dir)"
script = "mktemp -d"

[runner.lua]
command = "lua"
args = ["${script}"]

[[task]]
runner = "lua"
stdout = "${dir}/hello.txt"
script = """
function fact (n)
  if n == 0 then
    return 1
  else
    return n * fact(n-1)
  end
end

print("10! = ", fact(10))
"""

[[task]]
name = "all"
requires = ["${dir}/hello.txt"]
script = "cat ${dir}/hello.txt"
"##;

#[test]
fn example_custom_runner() {
    if Command::new("lua").arg("-v").output().is_err() {
        eprintln!("skipped: lua is not installed");
        return;
    }
    let log = Workflow::new(CUSTOM_RUNNER_EXAMPLE).run(&["all"]);
    assert!(log.contains("#all 10! = \t3628800"), "{log}");
}
```
