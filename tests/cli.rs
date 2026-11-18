// ~/~ begin <<docs/src/main_module.md#tests/cli.rs>>[init]
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn brei(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_brei")).args(args).current_dir(dir).output().unwrap()
}

fn setup(program: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("brei.toml"), program).unwrap();
    dir
}

fn read(dir: &TempDir, path: &str) -> String {
    fs::read_to_string(dir.path().join(path)).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

// ~/~ begin <<docs/src/main_module.md#cli-tests>>[init]
const HELLO: &str = r#"
[[task]]
name = "hello"
creates = ["hello.txt"]
runner = "bash"
script = "echo run >> log.txt; echo hello > hello.txt"
"#;

#[test]
fn build() {
    let dir = setup(HELLO);
    assert!(brei(dir.path(), &["hello"]).status.success());
    assert_eq!(read(&dir, "hello.txt"), "hello\n");
    assert!(dir.path().join(".brei_history").exists());

    assert!(brei(dir.path(), &["hello.txt"]).status.success());
    assert_eq!(read(&dir, "log.txt"), "run\n");

    assert!(brei(dir.path(), &["-B", "hello"]).status.success());
    assert_eq!(read(&dir, "log.txt"), "run\nrun\n");
}
// ~/~ end
// ~/~ begin <<docs/src/main_module.md#cli-tests>>[1]
const FAILING: &str = r#"
[[task]]
name = "fail"
script = "false"

[[task]]
creates = ["ok.txt"]
script = "touch ok.txt"
"#;

#[test]
fn failure() {
    let dir = setup(FAILING);
    let output = brei(dir.path(), &["-k", "-j", "1", "fail", "ok.txt"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("`false` failed with exit code 1"), "{}", stderr(&output));
    assert!(dir.path().join("ok.txt").exists());

    let output = brei(dir.path(), &["nothere"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("missing dependency: `nothere`"), "{}", stderr(&output));
}
// ~/~ end
// ~/~ begin <<docs/src/main_module.md#cli-tests>>[2]
#[test]
fn input_file() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("workflow.toml"), format!("[brei]\n{}", HELLO.replace("[[task]]", "[[brei.task]]"))).unwrap();
    assert!(brei(dir.path(), &["-i", "workflow.toml[brei]", "hello"]).status.success());
    assert_eq!(read(&dir, "hello.txt"), "hello\n");

    let output = brei(dir.path(), &["hello"]);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("no input file given"), "{}", stderr(&output));
}

#[test]
fn info() {
    let dir = TempDir::new().unwrap();
    let output = brei(dir.path(), &["--list-runners"]);
    assert!(String::from_utf8_lossy(&output.stdout).contains("bash"));
    let output = brei(dir.path(), &["-v"]);
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("Brei "));
}
// ~/~ end
// ~/~ end
