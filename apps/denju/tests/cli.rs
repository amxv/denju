use std::process::{Command, Output};

use serde_json::Value;
use tempfile::tempdir;

#[test]
fn plain_root_is_compact_guidance_with_one_next_action() {
    let output = denju(&[]);
    assert!(output.status.success());
    assert_eq!(
        stdout(&output),
        "Denju is ready to set up.\nNext: denju setup\n"
    );
    assert!(stderr(&output).is_empty());
    assert_eq!(stdout(&output).matches("Next:").count(), 1);
    assert!(!stdout(&output).contains("scaffold"));
}

#[test]
fn version_uses_public_binary_name() {
    for flag in ["--version", "-V"] {
        let output = denju(&[flag]);
        assert!(output.status.success());
        assert!(stdout(&output).starts_with("denju "));
        assert!(stderr(&output).is_empty());
    }
}

#[test]
fn json_root_and_version_emit_one_machine_result() {
    let root = denju(&["--json"]);
    assert!(root.status.success());
    assert!(stderr(&root).is_empty());
    assert_eq!(stdout(&root).lines().count(), 1);
    let root_json: Value = serde_json::from_str(stdout(&root).trim()).expect("valid JSON");
    assert_eq!(root_json["version"], 1);
    assert_eq!(root_json["ok"], true);
    assert_eq!(root_json["result"]["kind"], "guidance");
    assert_eq!(root_json["result"]["state"], "setup_required");
    assert_eq!(root_json["result"]["next_command"], "denju setup");

    let version = denju(&["--json", "--version"]);
    assert!(version.status.success());
    assert!(stderr(&version).is_empty());
    assert_eq!(stdout(&version).lines().count(), 1);
    let version_json: Value = serde_json::from_str(stdout(&version).trim()).expect("valid JSON");
    assert_eq!(version_json["version"], 1);
    assert_eq!(version_json["result"]["kind"], "version");
    assert!(version_json["result"]["version"].as_str().is_some());
}

#[test]
fn invalid_arguments_are_stable_and_do_not_corrupt_json_stdout() {
    let text = denju(&["not-a-command"]);
    assert_eq!(text.status.code(), Some(2));
    assert!(stdout(&text).is_empty());
    assert!(stderr(&text).starts_with("error: "));
    assert!(stderr(&text).contains("Next: denju --help"));

    let json = denju(&["--json", "not-a-command"]);
    assert_eq!(json.status.code(), Some(2));
    assert!(stderr(&json).is_empty());
    assert_eq!(stdout(&json).lines().count(), 1);
    let value: Value = serde_json::from_str(stdout(&json).trim()).expect("valid JSON error");
    assert_eq!(value["version"], 1);
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "invalid_arguments");
    assert_eq!(value["error"]["recovery"], "denju --help");
}

#[test]
fn help_is_available_in_text_and_json_modes() {
    let text = denju(&["--help"]);
    assert!(text.status.success());
    assert!(stdout(&text).contains("Usage: denju [OPTIONS]"));
    assert!(stdout(&text).contains("--json"));
    assert!(stderr(&text).is_empty());

    let json = denju(&["--json", "--help"]);
    assert!(json.status.success());
    assert!(stderr(&json).is_empty());
    let value: Value = serde_json::from_str(stdout(&json).trim()).expect("valid JSON help");
    assert_eq!(value["result"]["kind"], "help");
    assert!(
        value["result"]["text"]
            .as_str()
            .expect("help text")
            .contains("Usage: denju [OPTIONS]")
    );
}

#[test]
fn json_identity_commands_never_prompt_for_human_secrets() {
    for args in [
        vec!["--json", "claim", "@alice"],
        vec!["--json", "login", "@alice"],
        vec!["--json", "identity", "backup"],
        vec!["--json", "identity", "recover", "@alice"],
        vec!["--json", "identity", "delete", "--yes"],
    ] {
        let output = denju(&args);
        assert_eq!(output.status.code(), Some(1));
        assert!(stderr(&output).is_empty());
        assert_eq!(stdout(&output).lines().count(), 1);
        let value: Value = serde_json::from_str(stdout(&output).trim()).expect("valid JSON error");
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["code"], "interactive_required");
    }
}

#[test]
fn subscribe_release_version_does_not_conflict_with_binary_version_flag() {
    let output = denju(&["--json", "subscribe", "@alice/review", "--version", "1"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).is_empty());
    let value: Value = serde_json::from_str(stdout(&output).trim()).expect("valid JSON error");
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "setup_required");
}

#[test]
fn lifecycle_cli_shapes_parse_without_ambiguity() {
    for args in [
        vec!["--json", "history", "prune", "@alice/review", "--yes"],
        vec!["--json", "subscribe", "@alice/review", "--retain-on-delete"],
        vec!["--json", "rename", "@alice/review", "renamed"],
        vec!["--json", "unpublish", "@alice/review"],
        vec!["--json", "delete", "@alice/review", "--yes"],
        vec![
            "--json",
            "deprecate",
            "@alice/review",
            "--replacement",
            "@alice/new-review",
        ],
    ] {
        let output = denju(&args);
        assert_eq!(output.status.code(), Some(1), "args: {args:?}");
        assert!(stderr(&output).is_empty(), "args: {args:?}");
        let value: Value = serde_json::from_str(stdout(&output).trim()).expect("valid JSON error");
        assert_eq!(value["ok"], false, "args: {args:?}");
        assert_eq!(value["error"]["code"], "setup_required", "args: {args:?}");
    }
}

#[test]
fn fork_and_sharing_cli_shapes_parse_without_ambiguity() {
    for args in [
        vec!["--json", "share", "@alice/review", "@bob"],
        vec!["--json", "unshare", "@alice/review", "@bob"],
        vec!["--json", "fork", "@alice/review"],
        vec!["--json", "fork", "sync", "@bob/review"],
        vec![
            "--json",
            "fork",
            "resolve",
            "@alice/review",
            "--as",
            "review-local",
        ],
        vec![
            "--json",
            "fork",
            "resolve",
            "@alice/review",
            "--merge-into",
            "@bob/review-local",
        ],
        vec!["--json", "fork", "resolve", "@alice/review", "--discard"],
    ] {
        let output = denju(&args);
        assert_eq!(output.status.code(), Some(1), "args: {args:?}");
        assert!(stderr(&output).is_empty(), "args: {args:?}");
        let value: Value = serde_json::from_str(stdout(&output).trim()).expect("valid JSON error");
        assert_eq!(value["ok"], false, "args: {args:?}");
        assert_eq!(value["error"]["code"], "setup_required", "args: {args:?}");
    }
}

#[test]
fn proposal_cli_shapes_parse_without_ambiguity() {
    for args in [
        vec!["--json", "propose", "@alice/review"],
        vec![
            "--json",
            "propose",
            "@alice/review",
            "--message",
            "please review",
        ],
        vec!["--json", "proposals"],
        vec![
            "--json",
            "proposal",
            "show",
            "01890f47-6a1d-7ad0-8f43-9a4d8c29f002",
        ],
        vec![
            "--json",
            "proposal",
            "accept",
            "01890f47-6a1d-7ad0-8f43-9a4d8c29f002",
        ],
        vec![
            "--json",
            "proposal",
            "reject",
            "01890f47-6a1d-7ad0-8f43-9a4d8c29f002",
        ],
        vec![
            "--json",
            "proposal",
            "withdraw",
            "01890f47-6a1d-7ad0-8f43-9a4d8c29f002",
        ],
    ] {
        let output = denju(&args);
        assert_eq!(output.status.code(), Some(1), "args: {args:?}");
        assert!(stderr(&output).is_empty(), "args: {args:?}");
        let value: Value = serde_json::from_str(stdout(&output).trim()).expect("valid JSON error");
        assert_eq!(value["ok"], false, "args: {args:?}");
        assert_eq!(value["error"]["code"], "setup_required", "args: {args:?}");
    }
}

#[test]
fn pack_cli_shapes_parse_without_ambiguity() {
    for args in [
        vec!["--json", "pack", "create", "@alice/packs/core"],
        vec![
            "--json",
            "pack",
            "add",
            "@alice/packs/core",
            "@bob/review",
            "@carol/test@v3",
        ],
        vec![
            "--json",
            "pack",
            "remove",
            "@alice/packs/core",
            "@bob/review",
        ],
        vec!["--json", "show", "@alice/packs/core"],
        vec!["--json", "publish", "@alice/packs/core"],
        vec!["--json", "subscribe", "@alice/packs/core"],
        vec!["--json", "unsubscribe", "@alice/packs/core"],
    ] {
        let output = denju(&args);
        assert_eq!(output.status.code(), Some(1), "args: {args:?}");
        assert!(stderr(&output).is_empty(), "args: {args:?}");
        let value: Value = serde_json::from_str(stdout(&output).trim()).expect("valid JSON error");
        assert_eq!(value["ok"], false, "args: {args:?}");
        assert_eq!(value["error"]["code"], "setup_required", "args: {args:?}");
    }
}

#[test]
fn destructive_lifecycle_json_requires_prior_confirmation() {
    for args in [
        vec!["--json", "delete", "@alice/review"],
        vec!["--json", "delete", "@alice/packs/core"],
        vec!["--json", "history", "prune", "@alice/review"],
    ] {
        let output = denju(&args);
        assert_eq!(output.status.code(), Some(1), "args: {args:?}");
        assert!(stderr(&output).is_empty(), "args: {args:?}");
        let value: Value = serde_json::from_str(stdout(&output).trim()).expect("valid JSON error");
        assert_eq!(value["ok"], false, "args: {args:?}");
        assert_eq!(
            value["error"]["code"], "confirmation_required",
            "args: {args:?}"
        );
    }
}

#[test]
fn legacy_test_knobs_cannot_run_without_an_explicit_marked_test_home() {
    let home = tempdir().expect("temporary HOME");
    let output = Command::new(env!("CARGO_BIN_EXE_denju"))
        .args(["--json", "setup", "--registry", "http://127.0.0.1:9"])
        .env("HOME", home.path())
        .env("DENJU_TEST_FILE_CREDENTIALS", "1")
        .env_remove(denju_local::TEST_HOME_ENV)
        .output()
        .expect("run denju setup");
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).is_empty());
    let value: Value = serde_json::from_str(stdout(&output).trim()).expect("valid JSON error");
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "local_state");
    assert!(
        value["error"]["message"]
            .as_str()
            .expect("error message")
            .contains("DENJU_TEST_HOME")
    );
    assert!(!home.path().join(".denju").exists());
}

fn denju(args: &[&str]) -> Output {
    let home = tempdir().expect("isolated test home");
    std::fs::write(
        home.path().join(denju_local::TEST_HOME_MARKER),
        b"isolated\n",
    )
    .expect("mark isolated test home");
    Command::new(env!("CARGO_BIN_EXE_denju"))
        .args(args)
        .env(denju_local::TEST_HOME_ENV, home.path())
        // Deliberately poison inherited harness overrides. DENJU_TEST_HOME must make these
        // irrelevant so tests can never project into a developer's custom real roots.
        .env("CODEX_HOME", "/developer-home/.gg/codex")
        .env("CLAUDE_CONFIG_DIR", "/developer-home/.gg/claude")
        .env_remove("DENJU_TEST_FILE_CREDENTIALS")
        .env_remove("DENJU_TEST_SERVICE_INSTALL_ONLY")
        .env_remove("DENJU_DAEMON_ONCE")
        .output()
        .expect("run denju binary")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout is UTF-8")
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("stderr is UTF-8")
}
