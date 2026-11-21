use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const CHILD: &str = "AQUA_APPKIT_CASE";
const CASE_LIMIT: Duration = Duration::from_secs(40);

pub struct Case {
    pub name: &'static str,
    pub run: fn(),
    pub args: &'static [&'static str],
}

pub const fn case(name: &'static str, run: fn()) -> Case {
    Case {
        name,
        run,
        args: &[],
    }
}

pub const fn case_with(name: &'static str, run: fn(), args: &'static [&'static str]) -> Case {
    Case { name, run, args }
}

pub fn main(cases: &[Case]) {
    match std::env::var(CHILD) {
        Ok(name) => {
            let case = cases
                .iter()
                .find(|case| case.name == name)
                .unwrap_or_else(|| panic!("no case named {name}"));
            (case.run)();
        }
        Err(_) => parent(cases),
    }
}

enum Outcome {
    Passed(String),
    Skipped(String),
    Failed(String),
}

fn collect(mut stream: impl Read + Send + 'static) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut text = String::new();
        stream
            .read_to_string(&mut text)
            .expect("the child writes UTF-8 output");
        text
    })
}

fn spawn_case(case: &Case) -> Outcome {
    let exe = std::env::current_exe().expect("the test binary knows its path");
    let mut child = Command::new(exe)
        .args(case.args)
        .env(CHILD, case.name)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the test binary can start itself");
    let stdout = collect(child.stdout.take().expect("stdout is piped"));
    let stderr = collect(child.stderr.take().expect("stderr is piped"));
    let pid = child.id();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let status = child.wait();
        let _ = sender.send(status);
    });
    let status = receiver.recv_timeout(CASE_LIMIT);
    if status.is_err() {
        let _ = Command::new("kill").arg("-9").arg(pid.to_string()).status();
    }
    let output = format!(
        "{}{}",
        stdout.join().expect("the stdout reader finishes"),
        stderr.join().expect("the stderr reader finishes")
    );
    match status {
        Ok(Ok(status)) if status.success() => {
            match output.lines().find(|line| line.starts_with("SKIP-")) {
                Some(line) => Outcome::Skipped(line.to_owned()),
                None => Outcome::Passed(output),
            }
        }
        Ok(Ok(status)) => Outcome::Failed(format!("exited with {status}\n{output}")),
        Ok(Err(error)) => Outcome::Failed(format!("could not wait: {error}\n{output}")),
        Err(_) => Outcome::Failed(format!("timed out after {CASE_LIMIT:?}\n{output}")),
    }
}

fn parent(cases: &[Case]) {
    let filters: Vec<String> = std::env::args()
        .skip(1)
        .filter(|arg| !arg.starts_with('-'))
        .collect();
    let selected: Vec<&Case> = cases
        .iter()
        .filter(|case| {
            filters.is_empty()
                || filters
                    .iter()
                    .any(|filter| case.name.contains(filter.as_str()))
        })
        .collect();
    println!("running {} tests", selected.len());
    let mut failures = Vec::new();
    let mut skipped = 0;
    for case in &selected {
        match spawn_case(case) {
            Outcome::Passed(output) => {
                println!("test {} ... ok", case.name);
                output
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .for_each(|line| println!("    {line}"));
            }
            Outcome::Skipped(reason) => {
                skipped += 1;
                println!("test {} ... skipped ({reason})", case.name);
            }
            Outcome::Failed(output) => {
                println!("test {} ... FAILED\n{output}", case.name);
                failures.push(case.name);
            }
        }
    }
    println!(
        "test result: {}. {} passed; {} failed; {skipped} skipped",
        if failures.is_empty() { "ok" } else { "FAILED" },
        selected.len() - failures.len() - skipped,
        failures.len()
    );
    if !failures.is_empty() {
        println!("failures: {failures:?}");
        std::process::exit(1);
    }
}
