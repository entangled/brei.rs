// ~/~ begin <<docs/src/brei.md#brei::src/tasks/tests.rs>>[init]
use std::fs;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tempfile::TempDir;
use crate::noodles::kernel::Kernel;
use crate::noodles::single_worker::SingleWorker;
use crate::noodles::worker_pool::WorkerPool;
use crate::noodles::task::Error;
use super::*;

fn target(s: &str) -> Target {
    s.parse().unwrap()
}

fn targets(ts: &[&str]) -> Vec<Target> {
    ts.iter().map(|t| target(t)).collect()
}

fn rule(creates: &[&str], requires: &[&str], script: &str) -> Rule {
    Rule { creates: targets(creates), requires: targets(requires), script: Some(script.into()), ..Default::default() }
}

fn bash(creates: &[&str], requires: &[&str], script: &str) -> Rule {
    Rule { runner: Some("bash".into()), ..rule(creates, requires, script) }
}

fn run(db: &Arc<TaskDb>, t: &str) -> Result<Value, Error<Job>> {
    let id = db.targets.intern(&target(t)).unwrap();
    build(db, &mut Kernel::new(), &SingleWorker::new(), id)
}

fn read(dir: &TempDir, path: &str) -> String {
    fs::read_to_string(dir.path().join(path)).unwrap()
}

fn failures(result: Result<Value, Error<Job>>) -> Vec<Failure> {
    match result {
        Err(Error::Jobs(failures)) => failures,
        Err(e) => panic!("expected failures, got {e:?}"),
        Ok(v) => panic!("expected failures, got {v:?}"),
    }
}

// ~/~ begin <<docs/src/brei.md#brei::task-tests>>[init]
#[test]
fn hello() {
    let dir = TempDir::new().unwrap();
    let mut db = TaskDb::new(dir.path());
    db.add(Rule { stdout: Some(target("hello.txt")), ..rule(&[], &[], "echo hello") }).unwrap();
    assert_eq!(run(&Arc::new(db), "hello.txt").unwrap(), Value::Nothing);
    assert_eq!(read(&dir, "hello.txt"), "hello\n");
}

#[test]
fn up_to_date() {
    let dir = TempDir::new().unwrap();
    let mut db = TaskDb::new(dir.path());
    db.add(bash(&["a.txt"], &[], "echo a >> log.txt; echo a > a.txt")).unwrap();
    db.add(bash(&["b.txt"], &["a.txt"], "echo b >> log.txt; cat a.txt > b.txt")).unwrap();
    let db = Arc::new(db);

    run(&db, "b.txt").unwrap();
    assert_eq!(read(&dir, "log.txt"), "a\nb\n");
    assert_eq!(read(&dir, "b.txt"), "a\n");

    run(&db, "b.txt").unwrap();
    assert_eq!(read(&dir, "log.txt"), "a\nb\n");

    // make `b.txt` older than `a.txt`
    let earlier = SystemTime::now() - Duration::from_secs(10);
    fs::File::options().write(true).open(dir.path().join("b.txt")).unwrap().set_modified(earlier).unwrap();
    run(&db, "b.txt").unwrap();
    assert_eq!(read(&dir, "log.txt"), "a\nb\nb\n");
}
// ~/~ end
// ~/~ begin <<docs/src/brei.md#brei::task-tests>>[1]
#[test]
fn variables() {
    let dir = TempDir::new().unwrap();
    let mut db = TaskDb::new(dir.path());
    db.add(Rule { stdout: Some(target("var(greeting)")), ..rule(&[], &[], "echo hello world") }).unwrap();
    db.add(Rule {
        stdin: Some(target("var(greeting)")),
        stdout: Some(target("shout.txt")),
        ..rule(&[], &[], "tr a-z A-Z")
    }).unwrap();
    db.add(Rule { stdout: Some(target("listvar(files)")), ..rule(&[], &[], r"printf 'a\nb\n\nc\n'") }).unwrap();
    let db = Arc::new(db);

    assert_eq!(run(&db, "var(greeting)").unwrap(), Value::Scalar("hello world".into()));
    run(&db, "shout.txt").unwrap();
    assert_eq!(read(&dir, "shout.txt"), "HELLO WORLD");
    assert_eq!(run(&db, "listvar(files)").unwrap(), Value::List(vec!["a".into(), "b".into(), "c".into()]));
}
// ~/~ end
// ~/~ begin <<docs/src/brei.md#brei::task-tests>>[2]
#[test]
fn several_targets() {
    let dir = TempDir::new().unwrap();
    let mut db = TaskDb::new(dir.path());
    db.add(bash(&["a.txt", "b.txt", "c.txt"], &[], "echo run >> log.txt; touch a.txt b.txt c.txt")).unwrap();
    db.add(Rule { name: Some("all".into()), requires: targets(&["a.txt", "b.txt", "c.txt"]), ..Default::default() }).unwrap();
    db.add(Rule { name: Some("answer".into()), stdout: Some(target("var(answer)")), ..rule(&[], &[], "echo 42") }).unwrap();
    let db = Arc::new(db);

    let id = db.targets.lookup(&target("#all")).unwrap();
    build(&db, &mut Kernel::new(), &WorkerPool::new(4), id).unwrap();
    assert_eq!(read(&dir, "log.txt"), "run\n");

    // `#answer` is the main target, `var(answer)` is an alias.
    assert_eq!(run(&db, "var(answer)").unwrap(), Value::Scalar("42".into()));
}
// ~/~ end
// ~/~ begin <<docs/src/brei.md#brei::task-tests>>[3]
#[test]
fn missing_dependency() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("input.txt"), "data").unwrap();
    let mut db = TaskDb::new(dir.path());
    db.add(rule(&["copy.txt"], &["input.txt"], "cp input.txt copy.txt")).unwrap();
    db.add(rule(&["other.txt"], &["nothere.txt"], "touch other.txt")).unwrap();
    let db = Arc::new(db);

    run(&db, "copy.txt").unwrap();
    assert_eq!(read(&dir, "copy.txt"), "data");
    assert_eq!(failures(run(&db, "other.txt")), [Failure::Missing(target("nothere.txt"))]);
    assert_eq!(failures(run(&db, "#nothing")), [Failure::Missing(target("#nothing"))]);
}
// ~/~ end
// ~/~ begin <<docs/src/brei.md#brei::task-tests>>[4]
#[test]
fn failing_rules() {
    let dir = TempDir::new().unwrap();
    let mut db = TaskDb::new(dir.path());
    db.add(rule(&["x.txt"], &[], "false")).unwrap();
    db.add(rule(&["y.txt"], &[], "true")).unwrap();
    db.add(rule(&["z.txt"], &[], "no-such-command")).unwrap();
    let db = Arc::new(db);

    assert_eq!(failures(run(&db, "x.txt")), [Failure::Command { command: "false".into(), code: Some(1) }]);
    assert_eq!(failures(run(&db, "y.txt")), [Failure::NotAchieved(targets(&["y.txt"]))]);
    assert!(matches!(&failures(run(&db, "z.txt"))[..], [Failure::Io(msg)] if msg.contains("no-such-command")));
}
// ~/~ end
// ~/~ begin <<docs/src/brei.md#brei::task-tests>>[5]
#[test]
fn cancel() {
    let dir = TempDir::new().unwrap();
    let mut db = TaskDb::new(dir.path());
    db.add(rule(&["slow.txt"], &[], "sleep 10")).unwrap();
    db.add(rule(&["fail.txt"], &[], "false")).unwrap();
    db.add(Rule { name: Some("all".into()), requires: targets(&["slow.txt", "fail.txt"]), ..Default::default() }).unwrap();
    let db = Arc::new(db);

    let start = Instant::now();
    let id = db.targets.lookup(&target("#all")).unwrap();
    assert!(build(&db, &mut Kernel::new(), &WorkerPool::new(2), id).is_err());
    assert!(start.elapsed() < Duration::from_secs(5));
}
// ~/~ end
// ~/~ begin <<docs/src/brei.md#brei::task-tests>>[6]
#[test]
fn invalid_rules() {
    let mut db = TaskDb::new(".");
    db.add(rule(&["a.txt"], &[], "true")).unwrap();
    assert_eq!(db.add(rule(&["a.txt"], &[], "true")), Err(RuleError::Duplicate(target("a.txt"))));

    db.add(Rule { stdout: Some(target("listvar(x)")), ..rule(&[], &[], "true") }).unwrap();
    assert!(matches!(db.add(Rule { stdin: Some(target("var(x)")), ..rule(&["b.txt"], &[], "cat") }),
        Err(RuleError::TypeMismatch(_))));

    let invalid = [
        Rule { stdin: Some(target("#a")), ..rule(&["c.txt"], &[], "cat") },
        Rule { stdout: Some(target("#a")), ..rule(&[], &[], "true") },
        Rule { path: Some("script.py".into()), ..Rule::default() },
        Rule { runner: Some("bash".into()), ..Rule::default() },
        Rule { stdin: Some(target("in.txt")), ..rule(&["d.txt"], &[], "cat\ncat") },
    ];
    for r in invalid {
        assert!(matches!(db.add(r), Err(RuleError::Invalid(_))));
    }
}
// ~/~ end
// ~/~ end
