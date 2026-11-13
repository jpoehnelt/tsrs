use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod process;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const USAGE: &str = "tsrs_thread_sanitizer <INSTRUMENTED_BINARY> [--projects xstate-main,webpack|all] [--work-dir DIR]\n\
    [--out-dir DIR] [--timeout SECONDS]\n\
Projects must already be prepared with node bench/bench.mjs prepare.\n\
Each invocation keeps a new run directory under target/thread-sanitizer-results (or --out-dir).";

#[derive(Deserialize, Serialize)]
struct Project {
    name: String,
    project: PathBuf,
    suite_dir: Option<PathBuf>,
}

#[derive(Serialize)]
struct Summary {
    binary: PathBuf,
    binary_sha256: String,
    repository_commit: String,
    platform: String,
    work_dir: PathBuf,
    timeout_seconds: u64,
    expected_cases: usize,
    complete: bool,
    error: Option<String>,
    cases: Vec<process::Outcome>,
}

struct Gate {
    root: PathBuf,
    out: PathBuf,
    summary: Summary,
}

pub fn main(args: &[String]) -> Result<()> {
    if args.first().is_some_and(|arg| matches!(arg.as_str(), "--help" | "-h")) {
        println!("{USAGE}");
        return Ok(());
    }
    if !cfg!(unix) {
        return Err("the ThreadSanitizer task requires a Unix host".into());
    }
    let Some((binary, args)) = args.split_first().filter(|(arg, _)| !arg.starts_with('-')) else {
        return Err(USAGE.into());
    };
    let root = std::env::current_dir()?
        .ancestors()
        .find(|dir| dir.join("tasks/thread_sanitizer/Cargo.toml").is_file())
        .ok_or("run from the tsrs repository or one of its subdirectories")?
        .canonicalize()?;
    let options = options(args)?;
    let projects =
        projects(&root, options.get("--projects").map_or("xstate-main,webpack", String::as_str))?;
    let seconds = options.get("--timeout").map_or(Ok(900), |value| value.parse::<u64>())?;
    if seconds == 0 || std::time::Instant::now().checked_add(Duration::from_secs(seconds)).is_none()
    {
        return Err("--timeout must be a positive, representable duration".into());
    }
    let binary = fs::canonicalize(binary)?;
    let work = root.join(options.get("--work-dir").map_or("bench/.work", String::as_str));
    let base = root
        .join(options.get("--out-dir").map_or("target/thread-sanitizer-results", String::as_str));
    fs::create_dir_all(&base)?;
    let out = base.join(format!(
        "run-{}-{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
        std::process::id()
    ));
    fs::create_dir(&out)?;
    let commit = Command::new("git").current_dir(&root).args(["rev-parse", "HEAD"]).output()?;
    if !commit.status.success() {
        return Err("cannot identify the repository commit".into());
    }
    let mut gate = Gate {
        root,
        out,
        summary: Summary {
            binary_sha256: binary_sha256(&binary)?,
            binary,
            repository_commit: String::from_utf8(commit.stdout)?.trim().into(),
            platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
            work_dir: work,
            timeout_seconds: seconds,
            expected_cases: 2 * projects.len() + 9,
            complete: false,
            error: None,
            cases: Vec::new(),
        },
    };
    println!("thread-sanitizer: results in {}", gate.out.display());
    gate.save()?;
    let result = gate.workloads(&projects).and_then(|()| {
        if gate.summary.cases.len() == gate.summary.expected_cases {
            Ok(())
        } else {
            Err("incomplete workload matrix".into())
        }
    });
    gate.summary.complete = result.is_ok();
    gate.summary.error = result.err().map(|error| error.to_string());
    gate.save()?;
    gate.summarize()?;
    let failed = gate.summary.cases.iter().filter(|case| !case.failures.is_empty()).count();
    println!(
        "thread-sanitizer: {} cases, {failed} failed; {}",
        gate.summary.cases.len(),
        gate.out.display()
    );
    if let Some(error) = &gate.summary.error {
        return Err(format!("{error}; results in {}", gate.out.display()).into());
    }
    if failed > 0 {
        return Err(format!("{failed} cases failed; results in {}", gate.out.display()).into());
    }
    Ok(())
}

fn options(args: &[String]) -> Result<BTreeMap<String, String>> {
    let mut options = BTreeMap::new();
    let mut args = args.iter();
    while let Some(name) = args.next() {
        if !["--projects", "--work-dir", "--out-dir", "--timeout"].contains(&name.as_str()) {
            return Err(format!("unknown option {name}; {USAGE}").into());
        }
        let value = args
            .next()
            .filter(|value| !value.starts_with("--"))
            .ok_or_else(|| format!("missing value for {name}"))?;
        if options.insert(name.clone(), value.clone()).is_some() {
            return Err(format!("duplicate option {name}").into());
        }
    }
    Ok(options)
}

fn projects(root: &Path, selection: &str) -> Result<Vec<Project>> {
    #[derive(Deserialize)]
    struct Catalog {
        projects: Vec<Project>,
    }
    let mut catalog: Catalog =
        serde_json::from_slice(&fs::read(root.join("bench/projects.json"))?)?;
    let selected: BTreeSet<_> = selection.split(',').collect();
    if selection != "all"
        && (selected.len() != selection.split(',').count()
            || selected.iter().any(|name| !catalog.projects.iter().any(|p| p.name == *name)))
    {
        return Err("select unique benchmark project names or all".into());
    }
    catalog
        .projects
        .retain(|project| selection == "all" || selected.contains(project.name.as_str()));
    if catalog.projects.is_empty() {
        return Err("no projects selected".into());
    }
    Ok(catalog.projects)
}

fn binary_sha256(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 8192];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    let mut hex = String::with_capacity(64);
    for byte in digest.finalize() {
        write!(hex, "{byte:02x}")?;
    }
    Ok(hex)
}

impl Gate {
    fn save(&self) -> Result<()> {
        let temporary = self.out.join("summary.json.tmp");
        let mut file = fs::File::create(&temporary)?;
        serde_json::to_writer_pretty(&mut file, &self.summary)?;
        writeln!(file)?;
        fs::rename(temporary, self.out.join("summary.json"))?;
        Ok(())
    }

    fn compiler(&self) -> Command {
        Command::new(&self.summary.binary)
    }

    fn helper(&self, mode: &str) -> Command {
        let mut command = Command::new("node");
        command.arg(self.root.join("tasks/thread_sanitizer/workloads.mjs")).arg(mode);
        command
    }

    fn project_dir(&self, project: &Project) -> PathBuf {
        self.summary.work_dir.join(project.suite_dir.as_ref().map_or_else(
            || PathBuf::from("solutions").join(&project.name),
            |dir| PathBuf::from("suite").join(dir),
        ))
    }

    fn run(
        &mut self,
        label: &str,
        command: &mut Command,
        expected: &[i32],
        instrumented: bool,
    ) -> Result<bool> {
        command.current_dir(&self.root);
        let outcome = process::invoke(
            &self.root,
            &self.out.join(label),
            label,
            command,
            expected,
            Duration::from_secs(self.summary.timeout_seconds),
            instrumented,
        )?;
        let passed = outcome.failures.is_empty();
        println!(
            "{} {label}: exit {:?}, {} reports{}",
            if passed { "pass" } else { "FAIL" },
            outcome.exit_code,
            outcome.reports,
            if passed { String::new() } else { format!("; {}", outcome.failures.join("; ")) }
        );
        self.summary.cases.push(outcome);
        self.save()?;
        Ok(passed)
    }

    fn workloads(&mut self, projects: &[Project]) -> Result<()> {
        let mut probe = self.compiler();
        probe.arg("--version");
        if !self.run("instrumentation", &mut probe, &[0], true)? {
            return Err("ThreadSanitizer startup failed or the binary is not instrumented".into());
        }
        for project in projects {
            let config = self.project_dir(project).join(&project.project);
            if !config.exists() {
                return Err(format!(
                    "{} is missing; prepare {} with node bench/bench.mjs prepare --projects {}",
                    config.display(),
                    project.name,
                    project.name
                )
                .into());
            }
            for checkers in [2, 8] {
                let mut command = self.compiler();
                command.arg("-p").arg(&config).args([
                    "--noEmit",
                    "--incremental",
                    "false",
                    "--pretty",
                    "false",
                    "--checkers",
                    &checkers.to_string(),
                ]);
                self.run(
                    &format!("check-{}-{checkers}", project.name),
                    &mut command,
                    &[0, 2],
                    false,
                )?;
            }
        }
        let first = &projects[0];
        let first_dir = self.project_dir(first);
        let mut emit = self.compiler();
        emit.arg("-p")
            .arg(first_dir.join(&first.project))
            .args(["--noEmit", "false", "--declaration", "--sourceMap", "--outDir"])
            .arg(self.out.join("emit"))
            .args(["--incremental", "false", "--pretty", "false", "--checkers", "8"]);
        self.run(&format!("emit-{}", first.name), &mut emit, &[0, 2], false)?;

        let graph = self.out.join("graph");
        if !self.run("graph", self.helper("graph").arg(&graph), &[0], false)? {
            return Err("project graph generation failed".into());
        }
        self.run(
            "build-clean",
            self.compiler().arg("-b").arg(&graph).args(["--pretty", "false"]),
            &[0],
            false,
        )?;
        writeln!(
            fs::OpenOptions::new().append(true).open(graph.join("p0/src/f5.ts"))?,
            "\nexport const edited = 1;"
        )?;
        self.run(
            "build-incremental",
            self.compiler().arg("-b").arg(&graph).args(["--pretty", "false"]),
            &[0],
            false,
        )?;
        let mut watch = self.helper("watch");
        for file in ["p0/src/f3.ts", "p5/src/f0.ts", "p11/src/f7.ts"] {
            watch.arg(graph.join(file));
        }
        watch.arg("--").arg(&self.summary.binary).arg("-b").arg(&graph).args([
            "--watch",
            "--preserveWatchOutput",
            "--pretty",
            "false",
        ]);
        self.run("watch-build", &mut watch, &[0], false)?;

        if !self.run("select-file", self.helper("select-file").arg(&first_dir), &[0], false)? {
            return Err("no suitable TypeScript file for watch and LSP workloads".into());
        }
        let file: String =
            serde_json::from_slice(&fs::read(self.out.join("select-file/stdout.log"))?)?;
        let watch_file = first_dir.join(&file);
        let original = fs::read(&watch_file)?;
        let mut watch = self.helper("watch");
        watch
            .arg(&watch_file)
            .arg(&watch_file)
            .arg("--")
            .arg(&self.summary.binary)
            .arg("--watch")
            .arg("-p")
            .arg(first_dir.join(&first.project))
            .args(["--noEmit", "--preserveWatchOutput", "--pretty", "false"]);
        let result = self.run("watch", &mut watch, &[0], false);
        // Restore even when a helper is killed on timeout before its own finally block can run.
        fs::write(&watch_file, original)?;
        result?;

        let mut lsp = self.helper("lsp");
        lsp.arg(&self.summary.binary)
            .arg(&first_dir)
            .arg(file)
            .arg(self.out.join("lsp/transcript"));
        self.run("lsp", &mut lsp, &[0], false)?;

        Ok(())
    }

    fn summarize(&self) -> Result<()> {
        let reports: Vec<_> =
            self.summary.cases.iter().flat_map(|case| &case.report_files).collect();
        if !reports.is_empty() {
            let output = Command::new("node")
                .arg(self.root.join("tasks/thread_sanitizer/summary.mjs"))
                .args(reports)
                .output()?;
            fs::write(self.out.join("reports.txt"), &output.stdout)?;
            print!("{}", String::from_utf8_lossy(&output.stdout));
            if !output.stderr.is_empty() {
                eprint!("{}", String::from_utf8_lossy(&output.stderr));
            }
        }
        Ok(())
    }
}
