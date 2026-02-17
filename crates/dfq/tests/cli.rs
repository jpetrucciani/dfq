use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_path(suffix: &str) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let mut path = std::env::temp_dir();
    path.push(format!(
        "dfq-{suffix}-{}-{timestamp}.Dockerfile",
        std::process::id()
    ));
    path
}

struct Fixture {
    path: PathBuf,
}

impl Fixture {
    fn new(contents: &str) -> Self {
        let path = unique_path("fixture");
        fs::write(&path, contents).expect("fixture write should succeed");
        Self { path }
    }

    fn path_str(&self) -> &str {
        self.path
            .to_str()
            .expect("fixture path should be valid utf-8")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

const README_DOCKERFILE: &str = "ARG VERSION=0.5.13\n\
                                 FROM alpine:${VERSION} AS builder\n\
                                 ARG VERSION=1.0\n\
                                 RUN apk add --no-cache curl\n\
                                 RUN echo \"build complete\"\n\
                                 FROM builder\n\
                                 RUN echo \"runtime\"\n";

fn readme_fixture() -> Fixture {
    Fixture::new(README_DOCKERFILE)
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_dfq"))
        .args(args)
        .output()
        .expect("command should run")
}

fn run_with_stdin(args: &[&str], input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dfq"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("command should spawn");

    {
        let mut stdin = child.stdin.take().expect("stdin should be piped");
        stdin
            .write_all(input.as_bytes())
            .expect("stdin write should succeed");
    }

    child
        .wait_with_output()
        .expect("command output should be available")
}

fn stdout_text(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout should be utf-8")
}

fn stderr_text(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("stderr should be utf-8")
}

#[test]
fn returns_global_arg_value() {
    let fixture = Fixture::new(
        "ARG VERSION=0.5.13\n\
         FROM alpine:${VERSION}\n",
    );

    let output = run(&["--file", fixture.path_str(), "ARG.VERSION"]);
    assert!(output.status.success());
    assert_eq!(stdout_text(&output), "0.5.13\n");
    assert_eq!(stderr_text(&output), "");
}

#[test]
fn stage_name_selector_returns_stage_arg_value() {
    let fixture = Fixture::new(
        "ARG VERSION=0.5.13\n\
         FROM alpine:${VERSION} AS builder\n\
         ARG VERSION=1.0\n",
    );

    let output = run(&[
        "--file",
        fixture.path_str(),
        "STAGE[\"builder\"].ARG.VERSION",
    ]);
    assert!(output.status.success());
    assert_eq!(stdout_text(&output), "1.0\n");
}

#[test]
fn structured_output_without_json_returns_usage_error() {
    let fixture = Fixture::new("FROM alpine\n");

    let output = run(&["--file", fixture.path_str(), "STAGE"]);
    assert_eq!(output.status.code(), Some(64));
    assert_eq!(stdout_text(&output), "");
    assert!(stderr_text(&output).contains("structured result requires --json"));
}

#[test]
fn json_wildcard_reports_missing_paths() {
    let fixture = Fixture::new(
        "FROM alpine AS build\n\
         FROM build\n",
    );

    let output = run(&["--file", fixture.path_str(), "--json", "STAGE[*].ARG.NOPE"]);
    assert!(output.status.success());

    let stdout = stdout_text(&output);
    assert!(stdout.contains("\"query\":\"STAGE[*].ARG.NOPE\""));
    assert!(stdout.contains("\"value\":[null,null]"));
    assert!(stdout.contains("\"missing_paths\":[\"STAGE[0].ARG.NOPE\",\"STAGE[1].ARG.NOPE\"]"));
}

#[test]
fn duplicate_stage_name_selector_returns_eval_error() {
    let fixture = Fixture::new(
        "FROM alpine AS dup\n\
         FROM alpine AS dup\n",
    );

    let output = run(&["--file", fixture.path_str(), "STAGE[\"dup\"].NAME"]);
    assert_eq!(output.status.code(), Some(5));
    assert!(stderr_text(&output).contains("stage selector \"dup\" is ambiguous"));
}

#[test]
fn strict_missing_var_returns_eval_error() {
    let fixture = Fixture::new("FROM alpine\n");

    let output = run(&[
        "--file",
        fixture.path_str(),
        "--strict",
        "RESOLVE(\"x:${NOPE}\")",
    ]);
    assert_eq!(output.status.code(), Some(5));
    assert!(stderr_text(&output).contains("missing variable 'NOPE'"));
}

#[test]
fn output_mode_conflict_returns_usage_error() {
    let fixture = Fixture::new("FROM alpine\n");

    let output = run(&[
        "--file",
        fixture.path_str(),
        "--json",
        "--raw",
        "ARG.VERSION",
    ]);
    assert_eq!(output.status.code(), Some(64));
    assert!(stderr_text(&output).contains("--json is mutually exclusive with --raw"));
}

#[test]
fn build_arg_override_is_literal() {
    let fixture = Fixture::new(
        "ARG VERSION=0.5.13\n\
         FROM alpine:${VERSION}\n",
    );

    let output = run(&[
        "--file",
        fixture.path_str(),
        "--build-arg",
        "VERSION=${NOPE}",
        "FROM[0].RESOLVED",
    ]);
    assert!(output.status.success());
    assert_eq!(stdout_text(&output), "alpine:${NOPE}\n");
}

#[test]
fn run_wildcard_streams_scalar_array_in_text_mode() {
    let fixture = Fixture::new(
        "FROM alpine\n\
         RUN apk add curl\n\
         RUN echo done\n",
    );

    let output = run(&["--file", fixture.path_str(), "RUN[*]"]);
    assert!(output.status.success());
    assert_eq!(stdout_text(&output), "RUN apk add curl\nRUN echo done\n");
}

#[test]
fn run_directives_filter_and_count() {
    let fixture = Fixture::new(
        "FROM alpine\n\
         RUN apk add curl\n\
         RUN echo done\n",
    );

    let grep_output = run(&["--file", fixture.path_str(), "RUN.GREP(\"apk\")"]);
    assert!(grep_output.status.success());
    assert_eq!(stdout_text(&grep_output), "RUN apk add curl\n");

    let count_output = run(&["--file", fixture.path_str(), "RUN.GREP(\"apk\").COUNT"]);
    assert!(count_output.status.success());
    assert_eq!(stdout_text(&count_output), "1\n");

    let wildcard_grep_output = run(&["--file", fixture.path_str(), "RUN[*].GREP(\"apk\")"]);
    assert!(wildcard_grep_output.status.success());
    assert_eq!(stdout_text(&wildcard_grep_output), "RUN apk add curl\n");
}

#[test]
fn readme_common_examples_are_valid() {
    let fixture = readme_fixture();

    let arg_output = run(&["--file", fixture.path_str(), "ARG.VERSION"]);
    assert!(arg_output.status.success());
    assert_eq!(stdout_text(&arg_output), "0.5.13\n");

    let from_output = run(&[
        "--file",
        fixture.path_str(),
        "--build-arg",
        "VERSION=1.2.3",
        "FROM[0].RESOLVED",
    ]);
    assert!(from_output.status.success());
    assert_eq!(stdout_text(&from_output), "alpine:1.2.3\n");

    let stage_arg_output = run(&[
        "--file",
        fixture.path_str(),
        "STAGE[\"builder\"].ARG.VERSION",
    ]);
    assert!(stage_arg_output.status.success());
    assert_eq!(stdout_text(&stage_arg_output), "1.0\n");

    let stage_dump_output = run(&["--file", fixture.path_str(), "--json", "STAGE"]);
    assert!(stage_dump_output.status.success());
    let stage_dump_stdout = stdout_text(&stage_dump_output);
    assert!(stage_dump_stdout.contains("\"query\":\"STAGE\""));
    assert!(stage_dump_stdout.contains("\"type\":\"array\""));
    assert!(stage_dump_stdout.contains("\"name\":\"builder\""));
    assert!(stage_dump_stdout.contains("\"resolved\":\"alpine:0.5.13\""));

    let resolve_output = run(&[
        "--file",
        fixture.path_str(),
        "RESOLVE(\"image:${VERSION}\")",
    ]);
    assert!(resolve_output.status.success());
    assert_eq!(stdout_text(&resolve_output), "image:0.5.13\n");

    let strict_output = run(&[
        "--file",
        fixture.path_str(),
        "--strict",
        "RESOLVE(\"x:${NOPE}\")",
    ]);
    assert_eq!(strict_output.status.code(), Some(5));
    assert!(stderr_text(&strict_output).contains("missing variable 'NOPE'"));
}

#[test]
fn readme_run_examples_are_valid() {
    let fixture = readme_fixture();

    let wildcard_output = run(&["--file", fixture.path_str(), "RUN[*]"]);
    assert!(wildcard_output.status.success());
    assert_eq!(
        stdout_text(&wildcard_output),
        "RUN apk add --no-cache curl\nRUN echo \"build complete\"\nRUN echo \"runtime\"\n"
    );

    let count_output = run(&["--file", fixture.path_str(), "RUN.COUNT"]);
    assert!(count_output.status.success());
    assert_eq!(stdout_text(&count_output), "3\n");

    let first_output = run(&["--file", fixture.path_str(), "RUN[0]"]);
    assert!(first_output.status.success());
    assert_eq!(stdout_text(&first_output), "RUN apk add --no-cache curl\n");

    let command_output = run(&["--file", fixture.path_str(), "RUN[0].COMMAND"]);
    assert!(command_output.status.success());
    assert_eq!(stdout_text(&command_output), "apk add --no-cache curl\n");

    let stage_output = run(&["--file", fixture.path_str(), "RUN[0].STAGE"]);
    assert!(stage_output.status.success());
    assert_eq!(stdout_text(&stage_output), "0\n");

    let span_output = run(&["--file", fixture.path_str(), "--json", "RUN[*].SPAN"]);
    assert!(span_output.status.success());
    let span_stdout = stdout_text(&span_output);
    assert!(span_stdout.contains("\"query\":\"RUN[*].SPAN\""));
    assert!(span_stdout.contains(
        "\"value\":[{\"end\":4,\"start\":4},{\"end\":5,\"start\":5},{\"end\":7,\"start\":7}]"
    ));

    let grep_output = run(&["--file", fixture.path_str(), "RUN.GREP(\"apk\")"]);
    assert!(grep_output.status.success());
    assert_eq!(stdout_text(&grep_output), "RUN apk add --no-cache curl\n");

    let wildcard_grep_output = run(&["--file", fixture.path_str(), "RUN[*].GREP(\"apk\")"]);
    assert!(wildcard_grep_output.status.success());
    assert_eq!(
        stdout_text(&wildcard_grep_output),
        "RUN apk add --no-cache curl\n"
    );

    let grep_count_output = run(&["--file", fixture.path_str(), "RUN.GREP(\"apk\").COUNT"]);
    assert!(grep_count_output.status.success());
    assert_eq!(stdout_text(&grep_count_output), "1\n");

    let contains_output = run(&["--file", fixture.path_str(), "RUN.CONTAINS(\"runtime\")"]);
    assert!(contains_output.status.success());
    assert_eq!(stdout_text(&contains_output), "true\n");
}

#[test]
fn readme_cli_flags_and_output_modes_are_valid() {
    let fixture = readme_fixture();

    let context_output = run(&[
        "--file",
        fixture.path_str(),
        "--context",
        ".",
        "ARG.VERSION",
    ]);
    assert!(context_output.status.success());
    assert_eq!(stdout_text(&context_output), "0.5.13\n");

    let show_missing_output = run(&[
        "--file",
        fixture.path_str(),
        "--show-missing",
        "--json",
        "RESOLVE(\"x:${NOPE}\")",
    ]);
    assert!(show_missing_output.status.success());
    let show_missing_stdout = stdout_text(&show_missing_output);
    assert!(show_missing_stdout.contains("\"missing_vars\":[\"NOPE\"]"));
    assert!(show_missing_stdout.contains("\"used_vars\":[\"NOPE\"]"));

    let raw_output = run(&["--file", fixture.path_str(), "--raw", "ARG.VERSION"]);
    assert!(raw_output.status.success());
    assert_eq!(raw_output.stdout, b"0.5.13");

    let nul_output = run(&["--file", fixture.path_str(), "--null", "ARG.VERSION"]);
    assert!(nul_output.status.success());
    assert_eq!(nul_output.stdout, b"0.5.13\0");

    let stdin_output = run_with_stdin(&["--stdin", "ARG.VERSION"], README_DOCKERFILE);
    assert!(stdin_output.status.success());
    assert_eq!(stdout_text(&stdin_output), "0.5.13\n");
}

#[test]
fn readme_interpolation_support_and_limits_are_valid() {
    let fixture = readme_fixture();

    let bare_output = run(&["--file", fixture.path_str(), "RESOLVE(\"$VERSION\")"]);
    assert!(bare_output.status.success());
    assert_eq!(stdout_text(&bare_output), "0.5.13\n");

    let braced_output = run(&["--file", fixture.path_str(), "RESOLVE(\"${VERSION}\")"]);
    assert!(braced_output.status.success());
    assert_eq!(stdout_text(&braced_output), "0.5.13\n");

    let unsupported_output = run(&["--file", fixture.path_str(), "RESOLVE(\"${VERSION:-x}\")"]);
    assert_eq!(unsupported_output.status.code(), Some(5));
    assert!(stderr_text(&unsupported_output).contains("unsupported interpolation form"));
}

#[test]
fn readme_exit_codes_are_observable() {
    let fixture = readme_fixture();

    let success_output = run(&["--file", fixture.path_str(), "ARG.VERSION"]);
    assert_eq!(success_output.status.code(), Some(0));

    let query_parse_output = run(&["--file", fixture.path_str(), "ARG["]);
    assert_eq!(query_parse_output.status.code(), Some(2));

    let bad_fixture = Fixture::new("ARG VERSION\nFROM\n");
    let docker_parse_output = run(&["--file", bad_fixture.path_str(), "ARG.VERSION"]);
    assert_eq!(docker_parse_output.status.code(), Some(3));

    let not_found_output = run(&["--file", fixture.path_str(), "ARG.NOPE"]);
    assert_eq!(not_found_output.status.code(), Some(4));

    let eval_output = run(&[
        "--file",
        fixture.path_str(),
        "--strict",
        "RESOLVE(\"x:${NOPE}\")",
    ]);
    assert_eq!(eval_output.status.code(), Some(5));

    let io_output = run(&["--file", "/tmp/dfq-file-does-not-exist", "ARG.VERSION"]);
    assert_eq!(io_output.status.code(), Some(6));

    let usage_output = run(&[
        "--file",
        fixture.path_str(),
        "--json",
        "--raw",
        "ARG.VERSION",
    ]);
    assert_eq!(usage_output.status.code(), Some(64));
}

#[test]
fn bash_completion_is_emitted() {
    let output = run(&["completion", "bash"]);
    assert!(output.status.success());

    let stdout = stdout_text(&output);
    assert!(stdout.contains("_dfq()"));
    assert!(stdout.contains("complete -F _dfq -o bashdefault -o default dfq"));
}

#[test]
fn zsh_completion_is_emitted() {
    let output = run(&["completion", "zsh"]);
    assert!(output.status.success());

    let stdout = stdout_text(&output);
    assert!(stdout.contains("#compdef dfq"));
    assert!(stdout.contains("_dfq \"$@\""));
}

#[test]
fn fish_completion_is_emitted() {
    let output = run(&["completion", "fish"]);
    assert!(output.status.success());

    let stdout = stdout_text(&output);
    assert!(stdout.contains("complete -c dfq"));
    assert!(stdout.contains("__fish_dfq_needs_command"));
}

#[test]
fn help_contains_detailed_flag_and_query_docs() {
    let output = run(&["--help"]);
    assert!(output.status.success());

    let stdout = stdout_text(&output);
    assert!(stdout.contains("Parse a Dockerfile and query resolved values"));
    assert!(stdout.contains("completion"));
    assert!(stdout.contains("Override ARG values"));
    assert!(stdout.contains("RUN[*] | grep apt-get"));
    assert!(stdout.contains("RUN[*].GREP(\"apt-get\")"));
    assert!(stdout.contains("Query expression to evaluate"));
}
