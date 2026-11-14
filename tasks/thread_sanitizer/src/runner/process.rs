use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

use super::Result;

#[derive(Serialize)]
pub(super) struct Outcome {
    label: String,
    command: Vec<String>,
    cwd: PathBuf,
    environment: BTreeMap<String, String>,
    expected_exit_codes: Vec<i32>,
    pub(super) exit_code: Option<i32>,
    elapsed_seconds: f64,
    pub(super) reports: usize,
    pub(super) report_files: Vec<PathBuf>,
    pub(super) failures: Vec<String>,
}

pub(super) fn invoke(
    root: &Path,
    directory: &Path,
    label: &str,
    command: &mut Command,
    expected: &[i32],
    timeout: Duration,
    probe: bool,
) -> Result<Outcome> {
    fs::create_dir_all(directory)?;
    let suppressions = root.join("tasks/thread_sanitizer/suppressions.txt");
    let log = directory.join("tsan");
    command.env(
        "TSAN_OPTIONS",
        format!(
            "suppressions={} halt_on_error=0 exitcode=66 history_size=4 log_path={}{}",
            sanitizer_path(&suppressions)?,
            sanitizer_path(&log)?,
            if probe { " verbosity=1" } else { "" },
        ),
    );
    command.env("LOG", directory.join("watch.log"));
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(fs::File::create(directory.join("stdout.log"))?))
        .stderr(Stdio::from(fs::File::create(directory.join("stderr.log"))?));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut environment: BTreeMap<_, _> = std::env::vars()
        .filter(|(name, _)| {
            name.starts_with("TSRS_")
                || name.starts_with("TS_TEST_")
                || name.starts_with("RAYON_")
                || ["LANG", "LC_ALL", "TZ"].contains(&name.as_str())
        })
        .collect();
    for (name, value) in command.get_envs() {
        if let Some(value) = value {
            environment
                .insert(name.to_string_lossy().into_owned(), value.to_string_lossy().into_owned());
        } else {
            environment.remove(name.to_string_lossy().as_ref());
        }
    }
    let mut outcome = Outcome {
        label: label.into(),
        command: std::iter::once(command.get_program())
            .chain(command.get_args())
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect(),
        cwd: command.get_current_dir().unwrap_or(root).to_path_buf(),
        environment,
        expected_exit_codes: expected.to_vec(),
        exit_code: None,
        elapsed_seconds: 0.0,
        reports: 0,
        report_files: Vec::new(),
        failures: Vec::new(),
    };
    // Write the replay command before launch, so an interrupted task leaves useful evidence too.
    fs::write(directory.join("command.json"), serde_json::to_vec_pretty(&outcome)?)?;
    let started = Instant::now();
    match command.spawn() {
        Err(error) => outcome.failures.push(format!("launch failed: {error}")),
        Ok(mut child) => {
            let deadline = started + timeout;
            loop {
                if let Some(status) = child.try_wait()? {
                    outcome.exit_code = status.code();
                    if !status.code().is_some_and(|code| expected.contains(&code)) {
                        outcome
                            .failures
                            .push(format!("unexpected exit {status}; expected {expected:?}"));
                    }
                    break;
                }
                if Instant::now() >= deadline {
                    outcome.exit_code = terminate(&mut child)?.code();
                    outcome.failures.push(format!("timeout after {} seconds", timeout.as_secs()));
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
    outcome.elapsed_seconds = started.elapsed().as_secs_f64();
    let mut instrumented = false;
    let mut files = fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    files.sort();
    for path in files {
        if !path.is_file() || path.extension().is_some_and(|ext| ext == "json") {
            continue;
        }
        let text = fs::read_to_string(&path)?;
        instrumented |= text.contains("Running under ThreadSanitizer");
        let reports = text.matches("WARNING: ThreadSanitizer:").count();
        let fatal = text.contains("FATAL: ThreadSanitizer");
        if reports > 0 || fatal {
            outcome.reports += reports;
            outcome.report_files.push(path);
            if fatal {
                outcome.failures.push("ThreadSanitizer runtime failure".into());
            }
        }
    }
    if outcome.reports > 0 {
        outcome.failures.push(format!("{} ThreadSanitizer reports", outcome.reports));
    }
    if probe && !instrumented {
        outcome.failures.push("missing ThreadSanitizer startup banner".into());
    }
    fs::write(directory.join("result.json"), serde_json::to_vec_pretty(&outcome)?)?;
    Ok(outcome)
}

fn sanitizer_path(path: &Path) -> Result<String> {
    let path = path.to_str().ok_or("ThreadSanitizer paths must be UTF-8")?;
    if path.contains(['"', '\n', '\r']) {
        return Err("ThreadSanitizer paths cannot contain double quotes or line breaks".into());
    }
    Ok(format!("\"{path}\""))
}

fn terminate(child: &mut Child) -> Result<ExitStatus> {
    #[cfg(unix)]
    {
        // The helper and compiler share this dedicated group. Give watch helpers time to restore edited files.
        let group = format!("-{}", child.id());
        Command::new("kill")
            .args(["-TERM", "--", &group])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break Some(status);
            }
            if Instant::now() >= deadline {
                break None;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        Command::new("kill")
            .args(["-KILL", "--", &group])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if let Some(status) = status {
            return Ok(status);
        }
    }
    if child.try_wait()?.is_none() {
        child.kill()?;
    }
    Ok(child.wait()?)
}
