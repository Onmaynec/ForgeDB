use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn temporary_directory(name: &str) -> std::io::Result<PathBuf> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "forgedb-cli-v080-{name}-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn fql(path: &Path, statement: &str, read_only: bool) -> std::io::Result<Output> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_forgedb"));
    command.arg("fql").arg(path).arg(statement);
    if read_only {
        command.arg("--read-only");
    }
    command.output()
}

fn file_script(path: &Path, script: &str) -> std::io::Result<Output> {
    let file = path.join("test.fql");
    fs::write(&file, script)?;
    Command::new(env!("CARGO_BIN_EXE_forgedb"))
        .arg("shell")
        .arg(path)
        .arg("--file")
        .arg(file)
        .output()
}

fn piped_shell(path: &Path, script: &str) -> std::io::Result<Output> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_forgedb"))
        .arg("shell")
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let Some(mut stdin) = child.stdin.take() else {
        return Err(std::io::Error::other("stdin subprocess pipe unavailable"));
    };
    stdin.write_all(script.as_bytes())?;
    drop(stdin);
    child.wait_with_output()
}

#[test]
fn fql_one_shot_checks_syntax_before_creating_database() -> Result<(), Box<dyn std::error::Error>> {
    let path = temporary_directory("parse-first")?;
    let invalid = fql(&path, "PUT a b; DELETE c", false)?;
    assert_eq!(invalid.status.code(), Some(2));
    assert!(!path.join("IDENTITY").exists());

    let created = fql(&path, "PUT key 'hello world'", false)?;
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    assert_eq!(created.stdout, b"OK\n");

    let got = fql(&path, "GET key;", true)?;
    assert!(got.status.success());
    assert_eq!(got.stdout, b"'hello world'\n");

    let raw = fql(&path, "PUT X'00ff' X'ff00'", false)?;
    assert!(raw.status.success());
    let bytes = fql(&path, "GET X'00ff'", true)?;
    assert_eq!(bytes.stdout, b"'\\xff\\x00'\n");

    fs::remove_dir_all(path)?;
    Ok(())
}

#[test]
fn script_supports_cf_switch_transactions_and_reopen() -> Result<(), Box<dyn std::error::Error>> {
    let path = temporary_directory("script")?;
    let script = "\
# FQL V1
CREATE FAMILY users;
SHOW FAMILIES
USE users
PUT account 'Alice Smith'
BEGIN
PUT account 'Bob'
GET account
ROLLBACK
GET account
BEGIN
PUT account 'Carol'
COMMIT
GET account
USE default
GET account
EXIT
";
    let first = file_script(&path, script)?;
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let stdout = String::from_utf8(first.stdout)?;
    assert!(stdout.contains("0\tdefault\n1\tusers\n"));
    assert!(stdout.contains("'Bob'\nROLLBACK\n'Alice Smith'\n"));
    assert!(stdout.contains("COMMIT\n'Carol'\n"));
    assert!(stdout.ends_with("USING\t0\tdefault\nNULL\n"));

    let next = file_script(&path, "USE users\nGET account\n")?;
    assert!(
        next.status.success(),
        "{}",
        String::from_utf8_lossy(&next.stderr)
    );
    assert!(String::from_utf8(next.stdout)?.ends_with("'Carol'\n"));

    let old = fql(&path, "GET account", true)?;
    assert_eq!(old.stdout, b"NULL\n");
    fs::remove_dir_all(path)?;
    Ok(())
}

#[test]
fn failed_script_rolls_back_open_transaction_and_stops_before_commit()
-> Result<(), Box<dyn std::error::Error>> {
    let path = temporary_directory("rollback-error")?;
    let result = file_script(
        &path,
        "BEGIN\nPUT secret staged\nGET secret\nBAD COMMAND\nCOMMIT\n",
    )?;
    assert_eq!(result.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&result.stderr).contains("строка 4"));
    assert!(String::from_utf8_lossy(&result.stdout).contains("'staged'\n"));

    let after = fql(&path, "GET secret", true)?;
    assert!(after.status.success());
    assert_eq!(after.stdout, b"NULL\n");

    let eof = file_script(&path, "BEGIN\nPUT secret uncommitted\n")?;
    assert!(eof.status.success());
    assert_eq!(fql(&path, "GET secret", true)?.stdout, b"NULL\n");
    fs::remove_dir_all(path)?;
    Ok(())
}

#[test]
fn repl_continues_after_invalid_statement_but_does_not_apply_it()
-> Result<(), Box<dyn std::error::Error>> {
    let path = temporary_directory("repl-recovery")?;
    let output = piped_shell(&path, "PUT a good\nPUT incomplete\nGET a\nEXIT\n")?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("FQL:"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("'good'\n"));
    assert_eq!(fql(&path, "GET incomplete", true)?.stdout, b"NULL\n");

    let output = piped_shell(&path, "BEGIN\nPUT staged value\nEXIT\n")?;
    assert!(output.status.success());
    assert_eq!(fql(&path, "GET staged", true)?.stdout, b"NULL\n");
    fs::remove_dir_all(path)?;
    Ok(())
}

#[test]
fn scan_limit_is_respected_and_read_only_refuses_mutations()
-> Result<(), Box<dyn std::error::Error>> {
    let path = temporary_directory("scan-limits")?;
    assert!(
        file_script(&path, "PUT a one\nPUT b two\nPUT c three\n")?
            .status
            .success()
    );
    let limit = fql(&path, "SCAN FROM a TO c LIMIT 1", true)?;
    assert!(limit.status.success());
    assert_eq!(limit.stdout, b"'a'\t'one'\n");
    assert_eq!(fql(&path, "SCAN LIMIT 0", true)?.stdout, b"");

    assert_eq!(fql(&path, "PUT a changed", true)?.status.code(), Some(2));
    assert_eq!(
        fql(&path, "CREATE FAMILY new", true)?.status.code(),
        Some(2)
    );
    assert_eq!(fql(&path, "GET a", true)?.stdout, b"'one'\n");
    fs::remove_dir_all(path)?;
    Ok(())
}

#[test]
fn oversized_repl_line_is_discarded_without_desynchronizing_next_line(
) -> Result<(), Box<dyn std::error::Error>> {
    let path = temporary_directory("oversized-repl")?;
    let oversized = format!("PUT {} garbage\nPUT safe saved\nGET safe\nEXIT\n", "x".repeat(70_000));
    let output = piped_shell(&path, &oversized)?;
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(String::from_utf8_lossy(&output.stderr).contains("fql_statement_bytes"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("'saved'\n"));
    assert_eq!(fql(&path, "GET safe", true)?.stdout, b"'saved'\n");

    let invalid_script = file_script(&path, &format!("# {}\nPUT too_late value\n", "a".repeat(70_000)))?;
    assert_eq!(invalid_script.status.code(), Some(2));
    assert_eq!(fql(&path, "GET too_late", true)?.stdout, b"NULL\n");
    fs::remove_dir_all(path)?;
    Ok(())
}

#[test]
fn changing_families_during_transaction_is_rejected_and_not_applied()
-> Result<(), Box<dyn std::error::Error>> {
    let path = temporary_directory("transaction-scope")?;
    let result = file_script(
        &path,
        "CREATE FAMILY log\nUSE log\nBEGIN\nPUT key work\nUSE default\nCOMMIT\n",
    )?;
    assert_eq!(result.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&result.stderr).contains("строка 5"));
    let next = file_script(&path, "USE log\nGET key\n")?;
    assert!(next.status.success());
    assert!(String::from_utf8(next.stdout)?.ends_with("NULL\n"));
    fs::remove_dir_all(path)?;
    Ok(())
}
