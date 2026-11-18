// ~/~ begin <<docs/src/brei.md#brei::src/tasks/exec.rs>>[init]
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime};
use tempfile::{NamedTempFile, TempPath};
use crate::noodles::task::CancelToken;
use crate::template_strings::{dollar, Env, Format};
use super::db::TaskDb;
use super::rule::{Rule, Value, Failure};
use super::target::{Target, VarType};

impl TaskDb {
    fn modified(&self, path: &Path) -> Result<SystemTime, Failure> {
        Ok(fs::metadata(self.root.join(path))?.modified()?)
    }

    pub fn needs_run(&self, rule: &Rule) -> Result<bool, Failure> {
        if rule.target_paths().any(|p| !self.root.join(p).exists()) {
            return Ok(true);
        }
        let targets = rule.target_paths().map(|p| self.modified(p)).collect::<Result<Vec<_>, _>>()?;
        let deps = rule.dependency_paths().map(|p| self.modified(p)).collect::<Result<Vec<_>, _>>()?;
        if targets.iter().any(|t| deps.iter().any(|d| t < d)) {
            return Ok(true);
        }
        let digest = rule.digest();
        let history = self.history.lock().unwrap();
        Ok(rule.target_paths().any(|p| history.get(p) != digest.as_ref()))
    }

    fn remember(&self, rule: &Rule) {
        let mut history = self.history.lock().unwrap();
        for p in rule.target_paths() {
            match rule.digest() {
                Some(digest) => history.insert(p.clone(), digest),
                None => history.remove(p),
            };
        }
    }

    /// Targets of a rule that failed may be written partially, so we forget them, and they are built again next time.
    fn forget(&self, rule: &Rule) {
        let mut history = self.history.lock().unwrap();
        for p in rule.target_paths() {
            history.remove(p);
        }
    }

    // ~/~ begin <<docs/src/brei.md#brei::exec-commands>>[init]
    fn commands(&self, rule: &Rule, script_file: &mut Option<TempPath>) -> Result<Vec<Vec<String>>, Failure> {
        let Some(name) = &rule.runner else {
            let script = rule.script.as_deref().unwrap_or("");
            return script.lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| shlex::split(line).ok_or_else(|| Failure::InvalidScript(line.to_string())))
                .collect();
        };

        let runner = self.runner(name).ok_or_else(|| Failure::UnknownRunner(name.clone()))?;
        let path: PathBuf = match (&rule.path, &rule.script) {
            (Some(path), _) => self.root.join(path),
            (None, Some(script)) => {
                let mut file = NamedTempFile::new()?;
                file.write_all(script.as_bytes())?;
                // Close the file, so that it can be opened by the runner on any platform.
                let temp = file.into_temp_path();
                let path = temp.to_path_buf();
                *script_file = Some(temp);
                path
            }
            (None, None) => return Err(Failure::InvalidScript(String::new())),
        };

        let mut env = Env::new();
        env.insert("script".into(), path.to_string_lossy().into_owned());
        let args = runner.args.iter()
            .map(|arg| dollar(arg.as_str()).format(&env).map_err(|e| Failure::Template(format!("{e:?}"))))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(vec![std::iter::once(runner.command.clone()).chain(args).collect()])
    }
    // ~/~ end
    // ~/~ begin <<docs/src/brei.md#brei::exec-execute>>[init]
    pub(super) fn execute(&self, rule: &Rule, input: Option<&str>, cancel: &CancelToken) -> Result<Value, Failure> {
        if !(self.force_run || rule.always_run()) && !self.needs_run(rule)? {
            return Ok(Value::Nothing);
        }
        if rule.runner.is_none() && rule.script.is_none() {
            return Ok(Value::Nothing);
        }

        let mut script_file = None;
        let commands = self.commands(rule, &mut script_file)?;
        let stdin = match &rule.stdin {
            Some(Target::Path(p)) => Some(File::open(self.root.join(p))?),
            _ => None,
        };
        let stdout = match &rule.stdout {
            Some(Target::Path(p)) => Some(File::create(self.root.join(p))?),
            _ => None,
        };
        let capture = matches!(rule.stdout, Some(Target::Variable(..)));

        let mut output = Vec::new();
        for command in &commands {
            let mut cmd = Command::new(&command[0]);
            cmd.args(&command[1..]).current_dir(&self.root).stderr(Stdio::inherit());
            cmd.stdin(match &stdin {
                Some(file) => Stdio::from(file.try_clone()?),
                None if input.is_some() => Stdio::piped(),
                None => Stdio::null(),
            });
            cmd.stdout(match &stdout {
                Some(file) => Stdio::from(file.try_clone()?),
                None if capture => Stdio::piped(),
                None => Stdio::inherit(),
            });
            match run_command(cmd, &command.join(" "), input, cancel) {
                Ok(out) => output.extend(out),
                Err(failure) => {
                    self.forget(rule);
                    return Err(failure);
                }
            }
        }
        drop(script_file);

        self.remember(rule);
        if self.needs_run(rule)? {
            return Err(Failure::NotAchieved(rule.creates.clone()));
        }

        let text = String::from_utf8_lossy(&output);
        Ok(match &rule.stdout {
            Some(Target::Variable(_, VarType::List)) =>
                Value::List(text.lines().filter(|l| !l.is_empty()).map(String::from).collect()),
            Some(Target::Variable(_, VarType::Scalar)) => Value::Scalar(text.trim().to_string()),
            _ => Value::Nothing,
        })
    }
    // ~/~ end
}

// ~/~ begin <<docs/src/brei.md#brei::exec-run-command>>[init]
fn run_command(mut cmd: Command, display: &str, input: Option<&str>, cancel: &CancelToken) -> Result<Vec<u8>, Failure> {
    let mut child = cmd.spawn().map_err(|e| Failure::Io(format!("can't run `{display}`: {e}")))?;

    let writer = match (child.stdin.take(), input) {
        (Some(mut stdin), Some(data)) => {
            let data = data.as_bytes().to_vec();
            // The command may exit without reading all of its input; that's fine.
            Some(thread::spawn(move || { let _ = stdin.write_all(&data); }))
        }
        _ => None,
    };
    let reader = child.stdout.take().map(|mut stdout| thread::spawn(move || {
        let mut buffer = Vec::new();
        stdout.read_to_end(&mut buffer).map(|_| buffer)
    }));

    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Failure::Cancelled);
        }
        thread::sleep(Duration::from_millis(10));
    };

    if let Some(writer) = writer {
        let _ = writer.join();
    }
    let output = match reader {
        Some(reader) => reader.join().expect("reader thread panicked")?,
        None => Vec::new(),
    };
    if !status.success() {
        return Err(Failure::Command { command: display.to_string(), code: status.code() });
    }
    Ok(output)
}
// ~/~ end
// ~/~ end
