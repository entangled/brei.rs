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

fn in_rule(note: &str, failure: Failure) -> Failure {
    Failure::Rule(note.into(), Box::new(failure))
}

fn failures(result: Result<Value, Error<Job>>) -> Vec<Failure> {
    match result {
        Err(Error::Jobs(failures)) => failures,
        Err(e) => panic!("expected failures, got {e:?}"),
        Ok(v) => panic!("expected failures, got {v:?}"),
    }
}

// ~/~ begin <<docs/src/brei.md#brei::task-tests>>[init]
#[cfg_attr(windows, ignore = "uses Unix tools")]
#[test]
fn hello() {
    let dir = TempDir::new().unwrap();
    let db = TaskDb::new(dir.path());
    db.add(Rule { stdout: Some(target("hello.txt")), ..rule(&[], &[], "echo hello") }).unwrap();
    assert_eq!(run(&Arc::new(db), "hello.txt").unwrap(), Value::Nothing);
    assert_eq!(read(&dir, "hello.txt"), "hello\n");
}

#[cfg_attr(windows, ignore = "uses Unix tools")]
#[test]
fn up_to_date() {
    let dir = TempDir::new().unwrap();
    let db = TaskDb::new(dir.path());
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
#[cfg_attr(windows, ignore = "uses Unix tools")]
#[test]
fn variables() {
    let dir = TempDir::new().unwrap();
    let db = TaskDb::new(dir.path());
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
#[cfg_attr(windows, ignore = "uses Unix tools")]
#[test]
fn variable_and_file() {
    let dir = TempDir::new().unwrap();
    let db = TaskDb::new(dir.path());
    db.add(Rule { stdout: Some(target("var(x)")), ..bash(&["x.txt"], &[], "touch x.txt; echo x") }).unwrap();
    let db = Arc::new(db);

    assert_eq!(run(&db, "var(x)").unwrap(), Value::Scalar("x".into()));
    assert_eq!(run(&db, "var(x)").unwrap(), Value::Scalar("x".into()));
}
// ~/~ end
// ~/~ begin <<docs/src/brei.md#brei::task-tests>>[3]
#[cfg_attr(windows, ignore = "uses Unix tools")]
#[test]
fn several_targets() {
    let dir = TempDir::new().unwrap();
    let db = TaskDb::new(dir.path());
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
// ~/~ begin <<docs/src/brei.md#brei::task-tests>>[4]
#[cfg_attr(windows, ignore = "uses Unix tools")]
#[test]
fn missing_dependency() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("input.txt"), "data").unwrap();
    let db = TaskDb::new(dir.path());
    db.add(rule(&["copy.txt"], &["input.txt"], "cp input.txt copy.txt")).unwrap();
    db.add(rule(&["other.txt"], &["nothere.txt"], "touch other.txt")).unwrap();
    let db = Arc::new(db);

    run(&db, "copy.txt").unwrap();
    assert_eq!(read(&dir, "copy.txt"), "data");
    assert_eq!(failures(run(&db, "other.txt")), [Failure::Missing(target("nothere.txt"))]);
    assert_eq!(failures(run(&db, "#nothing")), [Failure::Missing(target("#nothing"))]);
}
// ~/~ end
// ~/~ begin <<docs/src/brei.md#brei::task-tests>>[5]
#[cfg_attr(windows, ignore = "uses Unix tools")]
#[test]
fn failing_rules() {
    let dir = TempDir::new().unwrap();
    let db = TaskDb::new(dir.path());
    db.add(rule(&["x.txt"], &[], "false")).unwrap();
    db.add(rule(&["y.txt"], &[], "true")).unwrap();
    db.add(rule(&["z.txt"], &[], "no-such-command")).unwrap();
    let db = Arc::new(db);

    assert_eq!(failures(run(&db, "x.txt")), [in_rule("creating `x.txt`", Failure::Command { command: "false".into(), code: Some(1) })]);
    assert_eq!(failures(run(&db, "y.txt")), [in_rule("creating `y.txt`", Failure::NotAchieved(targets(&["y.txt"])))]);
    let z = failures(run(&db, "z.txt"));
    assert!(matches!(&z[..], [Failure::Rule(_, f)] if matches!(&**f, Failure::Io(msg) if msg.contains("no-such-command"))));
    assert_eq!(z[0].to_string(), "creating `z.txt`: can't run `no-such-command`: command not found");
}
// ~/~ end
// ~/~ begin <<docs/src/brei.md#brei::task-tests>>[6]
#[cfg_attr(windows, ignore = "uses Unix tools")]
#[test]
fn cancel() {
    let dir = TempDir::new().unwrap();
    let db = TaskDb::new(dir.path());
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
// ~/~ begin <<docs/src/brei.md#brei::task-tests>>[7]
#[cfg(target_os = "linux")]
#[test]
fn kill_group() {
    let dir = TempDir::new().unwrap();
    let db = TaskDb::new(dir.path());
    db.add(bash(&["slow.txt"], &[], "sleep 30 & echo $! > sleep.pid; wait")).unwrap();
    db.add(bash(&["fail.txt"], &[], "while [ ! -s sleep.pid ]; do sleep 0.01; done; false")).unwrap();
    db.add(Rule { name: Some("all".into()), requires: targets(&["slow.txt", "fail.txt"]), ..Default::default() }).unwrap();
    let db = Arc::new(db);

    let id = db.targets.lookup(&target("#all")).unwrap();
    assert!(build(&db, &mut Kernel::new(), &WorkerPool::new(2), id).is_err());
    let pid = read(&dir, "sleep.pid");
    let alive = || fs::read_to_string(format!("/proc/{}/stat", pid.trim()))
        .is_ok_and(|stat| stat.rsplit(") ").next().is_some_and(|rest| !rest.starts_with('Z')));
    let start = Instant::now();
    while alive() && start.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!alive(), "`sleep` is still running");
}
// ~/~ end
// ~/~ begin <<docs/src/brei.md#brei::task-tests>>[8]
#[test]
fn invalid_rules() {
    let db = TaskDb::new(".");
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
// ~/~ begin <<docs/src/brei.md#brei::task-tests>>[9]
#[test]
fn runner_arguments() {
    let db = TaskDb::new(".");
    let runner = |args: &[&str]| Runner { command: "lua".into(), args: args.iter().map(|a| a.to_string()).collect() };
    db.add_runner("lua", runner(&["-W", "${script}"])).unwrap();
    assert_eq!(db.add_runner("lua", runner(&["${script}", "${other}"])),
        Err(RuleError::Invalid("its arguments use `${other}`, but only `${script}` can be used".into())));
}
// ~/~ end
// ~/~ begin <<docs/src/brei.md#brei::task-tests>>[10]
#[cfg_attr(windows, ignore = "uses Unix tools")]
#[test]
fn persistent_history() {
    let dir = TempDir::new().unwrap();
    let rules = |script: &str| {
        let db = TaskDb::new(dir.path());
        db.add(bash(&["a.txt"], &[], &format!("echo {script} >> log.txt; echo {script} > a.txt"))).unwrap();
        db.add(bash(&["b.txt"], &[], "echo b >> log.txt; [ -e ok ] && touch b.txt")).unwrap();
        db.load_history(HISTORY_FILE).unwrap();
        Arc::new(db)
    };

    let db = rules("a");
    run(&db, "a.txt").unwrap();
    assert!(run(&db, "b.txt").is_err());
    db.save_history(HISTORY_FILE).unwrap();
    assert_eq!(read(&dir, "log.txt"), "a\nb\n");

    // `a.txt` is up to date, `b.txt` failed so it runs again.
    fs::write(dir.path().join("ok"), "").unwrap();
    let db = rules("a");
    run(&db, "a.txt").unwrap();
    run(&db, "b.txt").unwrap();
    db.save_history(HISTORY_FILE).unwrap();
    assert_eq!(read(&dir, "log.txt"), "a\nb\nb\n");

    // a different script for `a.txt`
    let db = rules("A");
    run(&db, "a.txt").unwrap();
    run(&db, "b.txt").unwrap();
    assert_eq!(read(&dir, "log.txt"), "a\nb\nb\nA\n");

    fs::write(dir.path().join(HISTORY_FILE), "not json").unwrap();
    assert!(TaskDb::new(dir.path()).load_history(HISTORY_FILE).is_err());
}
// ~/~ end
// ~/~ begin <<docs/src/brei.md#brei::task-tests>>[11]
#[cfg_attr(windows, ignore = "uses Unix tools")]
#[test]
fn build_several() {
    let dir = TempDir::new().unwrap();
    let db = TaskDb::new(dir.path());
    db.add(bash(&["a.txt"], &[], "echo a >> log.txt; touch a.txt")).unwrap();
    db.add(bash(&["b.txt"], &["a.txt"], "touch b.txt")).unwrap();
    db.add(bash(&["c.txt"], &["a.txt"], "touch c.txt")).unwrap();
    db.add(rule(&["x.txt"], &[], "false")).unwrap();
    let db = Arc::new(db);
    let ids = |ts: &[&str]| ts.iter().map(|t| db.targets.lookup(&target(t)).unwrap()).collect::<Vec<_>>();

    build_all(&db, &mut Kernel::new(), &WorkerPool::new(4), &ids(&["b.txt", "c.txt", "b.txt"])).unwrap();
    assert_eq!(read(&dir, "log.txt"), "a\n");
    assert!(dir.path().join("b.txt").exists() && dir.path().join("c.txt").exists());
    build_all(&db, &mut Kernel::new(), &SingleWorker::new(), &[]).unwrap();

    fs::remove_file(dir.path().join("b.txt")).unwrap();
    fs::remove_file(dir.path().join("c.txt")).unwrap();
    let result = build_all(&db, &mut Kernel::new().keep_going(), &SingleWorker::new(), &ids(&["x.txt", "b.txt", "c.txt"]));
    assert_eq!(failures(result.map(|_| Value::Nothing)), [in_rule("creating `x.txt`", Failure::Command { command: "false".into(), code: Some(1) })]);
    assert!(dir.path().join("b.txt").exists() && dir.path().join("c.txt").exists());
}
// ~/~ end
// ~/~ begin <<docs/src/brei.md#brei::task-tests>>[12]
fn template(creates: &[&str], requires: &[&str], script: &str) -> RuleTemplate {
    let strings = |ts: &[&str]| ts.iter().map(|t| t.to_string()).collect();
    RuleTemplate { creates: strings(creates), requires: strings(requires), script: Some(script.into()), ..Default::default() }
}

#[cfg_attr(windows, ignore = "uses Unix tools")]
#[test]
fn environment() {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("data")).unwrap();
    fs::write(dir.path().join("data/input.txt"), "hello").unwrap();
    let db = TaskDb::new(dir.path());
    db.add_variable("dir", "data").unwrap();
    db.add_variable("input", "${dir}/input.txt").unwrap();
    db.add(Rule { stdout: Some(target("var(output)")), ..rule(&[], &[], "echo output.txt") }).unwrap();
    db.add_template(template(&["output.txt"], &["${input}"], "cp ${input} ${output}")).unwrap();
    let db = Arc::new(db);

    assert_eq!(run(&db, "var(input)").unwrap(), Value::Scalar("data/input.txt".into()));
    run(&db, "output.txt").unwrap();
    assert_eq!(read(&dir, "output.txt"), "hello");
}
// ~/~ end
// ~/~ begin <<docs/src/brei.md#brei::task-tests>>[13]
#[cfg_attr(windows, ignore = "uses Unix tools")]
#[test]
fn template_variables() {
    let dir = TempDir::new().unwrap();
    let db = TaskDb::new(dir.path());
    db.add_variable("name", "greeting").unwrap();
    db.add(Rule { stdout: Some(target("var(greeting)")), ..rule(&[], &[], "echo hello") }).unwrap();
    db.add_template(RuleTemplate {
        stdin: Some("var(${name})".into()), stdout: Some("var(shout)".into()), ..template(&[], &[], "tr a-z A-Z")
    }).unwrap();
    db.add_template(RuleTemplate { stdout: Some("var(dollars)".into()), ..template(&[], &[], "echo $$1 $1 $(x)") }).unwrap();
    let db = Arc::new(db);

    assert_eq!(run(&db, "var(shout)").unwrap(), Value::Scalar("HELLO".into()));
    assert_eq!(run(&db, "var(dollars)").unwrap(), Value::Scalar("$1 $1 $(x)".into()));
}
// ~/~ end
// ~/~ begin <<docs/src/brei.md#brei::task-tests>>[14]
#[cfg_attr(windows, ignore = "uses Unix tools")]
#[test]
fn splice() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("c.txt"), "c\n").unwrap();
    let db = TaskDb::new(dir.path());
    db.add(Rule { stdout: Some(target("listvar(files)")), ..rule(&[], &[], r"printf 'a.txt\nb.txt\nc.txt\n'") }).unwrap();
    db.add(bash(&["a.txt"], &[], "echo a > a.txt")).unwrap();
    db.add(bash(&["b.txt"], &[], "echo b > b.txt")).unwrap();
    db.add_template(RuleTemplate {
        runner: Some("bash".into()),
        ..template(&["all.txt"], &["splice(files)"], "cat a.txt b.txt c.txt > all.txt")
    }).unwrap();
    db.add_template(template(&["d.txt"], &["splice(files)", "nothere.txt"], "touch d.txt")).unwrap();
    let db = Arc::new(db);

    run(&db, "all.txt").unwrap();
    assert_eq!(read(&dir, "all.txt"), "a\nb\nc\n");
    assert_eq!(failures(run(&db, "d.txt")), [Failure::Missing(target("nothere.txt"))]);
}
// ~/~ end
// ~/~ begin <<docs/src/brei.md#brei::task-tests>>[15]
#[test]
fn invalid_templates() {
    let dir = TempDir::new().unwrap();
    let db = TaskDb::new(dir.path());
    db.add_template(template(&["a.txt"], &[], "touch ${unknown}")).unwrap();
    db.add_variable("x", "${y}").unwrap();
    assert_eq!(db.add_variable("x", "1"), Err(RuleError::Duplicate(target("var(x)"))));
    db.add(Rule { stdout: Some(target("listvar(list)")), ..rule(&[], &[], "true") }).unwrap();
    assert!(matches!(db.add_template(template(&["b.txt"], &[], "echo ${list}")), Err(RuleError::TypeMismatch(_))));
    assert!(matches!(db.add_template(template(&["b.txt"], &["splice(x)"], "true")), Err(RuleError::TypeMismatch(_))));

    let invalid = [
        template(&["${x}.txt"], &[], "true"),
        RuleTemplate { name: Some("${x}".into()), ..template(&[], &[], "true") },
        template(&["c.txt"], &["listvar(list)"], "true"),
        template(&["listvar(list)"], &[], "true"),
        template(&["splice(list)"], &[], "true"),
        RuleTemplate { stdin: Some("splice(list)".into()), ..template(&["c.txt"], &[], "cat") },
    ];
    for t in invalid {
        assert!(matches!(db.add_template(t), Err(RuleError::Invalid(_))));
    }

    let db = Arc::new(db);
    assert_eq!(failures(run(&db, "a.txt")), [Failure::Missing(target("var(unknown)"))]);
    assert_eq!(failures(run(&db, "var(x)")), [Failure::Missing(target("var(y)"))]);
}
// ~/~ end
// ~/~ end
