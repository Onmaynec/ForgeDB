use std::fs::File;
use std::io::{self, BufRead, BufReader, IsTerminal, Write};
use std::path::PathBuf;

mod fql;

use forgedb::{
    ColumnFamilies, ColumnFamilyId, Database, Error, FileInventory, Options, ReadOptions,
    RecoveryPlan, Result, VerifyOptions, WriteOptions,
};

fn main() {
    let exit_code = match run() {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("Ошибка ForgeDB: {error}");
            exit_code_for_error(&error)
        }
    };
    std::process::exit(exit_code);
}

fn run() -> Result<()> {
    let mut arguments = std::env::args().skip(1);
    let command = arguments
        .next()
        .ok_or_else(|| Error::invalid_argument("command", usage()))?;
    match command.as_str() {
        "put" => {
            let path = required(&mut arguments, "path")?;
            let key = required(&mut arguments, "key")?;
            let value = required(&mut arguments, "value")?;
            let write_options = parse_write_options(arguments.next().as_deref())?;
            ensure_no_extra(arguments)?;
            let database = Database::open(PathBuf::from(path), Options::default())?;
            database.put(key.as_bytes(), value.as_bytes(), write_options)?;
            database.close()
        }
        "get" => {
            let path = required(&mut arguments, "path")?;
            let key = required(&mut arguments, "key")?;
            ensure_no_extra(arguments)?;
            let database = Database::open(PathBuf::from(path), Options::default())?;
            if let Some(value) = database.get(key.as_bytes())? {
                write_bytes_line(&value)?;
            }
            database.close()
        }
        "delete" => {
            let path = required(&mut arguments, "path")?;
            let key = required(&mut arguments, "key")?;
            let write_options = parse_write_options(arguments.next().as_deref())?;
            ensure_no_extra(arguments)?;
            let database = Database::open(PathBuf::from(path), Options::default())?;
            database.delete(key.as_bytes(), write_options)?;
            database.close()
        }
        "transaction" | "tx" => run_transaction_command(arguments),
        "cf" => run_column_family_command(arguments),
        "fql" => run_fql_once(arguments),
        "shell" => run_fql_shell(arguments),
        "scan" => {
            let path = required(&mut arguments, "path")?;
            let start = arguments.next().map(String::into_bytes);
            let end = arguments.next().map(String::into_bytes);
            ensure_no_extra(arguments)?;
            let database = Database::open(PathBuf::from(path), Options::default())?;
            let iterator = match (start, end) {
                (None, None) => database.scan(.., ReadOptions::default())?,
                (Some(start), None) => database.scan(start.., ReadOptions::default())?,
                (None, Some(end)) => database.scan(..end, ReadOptions::default())?,
                (Some(start), Some(end)) => database.scan(start..end, ReadOptions::default())?,
            };
            let mut stdout = std::io::stdout().lock();
            for row in iterator {
                let (key, value) = row?;
                stdout
                    .write_all(&key)
                    .and_then(|_| stdout.write_all(b"\t"))
                    .and_then(|_| stdout.write_all(&value))
                    .and_then(|_| stdout.write_all(b"\n"))
                    .map_err(|source| Error::io("write_stdout", None, source))?;
            }
            drop(stdout);
            database.close()
        }
        "compact" => {
            let path = required(&mut arguments, "path")?;
            let start = arguments.next().map(String::into_bytes);
            let end = arguments.next().map(String::into_bytes);
            ensure_no_extra(arguments)?;
            let database = Database::open(PathBuf::from(path), Options::default())?;
            database.compact_range(start.as_deref(), end.as_deref())?;
            let cleanup = database.cleanup_obsolete_sstables()?;
            for name in cleanup.removed {
                println!("removed obsolete SST: {name}");
            }
            for name in cleanup.deferred {
                eprintln!("deferred obsolete SST: {name}");
            }
            for name in cleanup.failed {
                eprintln!("obsolete SST cleanup failed: {name}");
            }
            database.close()
        }
        "inventory" => {
            let path = required(&mut arguments, "path")?;
            ensure_no_extra(arguments)?;
            let options = Options {
                read_only: true,
                ..Options::default()
            };
            let database = Database::open(PathBuf::from(path), options)?;
            print_inventory(&database.file_inventory()?);
            database.close()
        }
        "inspect" => {
            let subcommand = required(&mut arguments, "inspect_command")?;
            if subcommand != "recovery-plan" {
                return Err(Error::invalid_argument("inspect_command", usage()));
            }
            let path = required(&mut arguments, "path")?;
            ensure_no_extra(arguments)?;
            print_recovery_plan(&RecoveryPlan::inspect(PathBuf::from(path))?);
            Ok(())
        }
        "verify" => {
            let path = required(&mut arguments, "path")?;
            let verify_options = match arguments.next().as_deref() {
                None => VerifyOptions::quick(),
                Some("--deep") => VerifyOptions::deep(),
                Some(_) => return Err(Error::invalid_argument("verify_mode", usage())),
            };
            ensure_no_extra(arguments)?;
            let options = Options {
                read_only: true,
                ..Options::default()
            };
            let database = Database::open(PathBuf::from(path), options)?;
            let report = database.verify(verify_options)?;
            println!("deep: {}", report.deep);
            println!("levels: {}", report.level_count);
            println!("live SSTables: {}", report.live_sstables);
            println!("entries checked: {}", report.entries_checked);
            println!(
                "oldest active snapshot: {:?}",
                report.oldest_active_snapshot
            );
            for warning in report.warnings {
                println!("warning: {warning}");
            }
            print_inventory(&report.inventory);
            database.close()
        }
        "help" | "--help" | "-h" => {
            println!("{}", usage());
            Ok(())
        }
        _ => Err(Error::invalid_argument("command", usage())),
    }
}

fn run_fql_once(mut arguments: impl Iterator<Item = String>) -> Result<()> {
    let path = required(&mut arguments, "path")?;
    let statement_text = required(&mut arguments, "statement")?;
    let readonly = match arguments.next().as_deref() {
        None => false,
        Some("--read-only") => true,
        Some(_) => return Err(Error::invalid_argument("fql_option", usage())),
    };
    ensure_no_extra(arguments)?;
    // Parse the entire statement before opening/creating the database.
    let statement = fql::parse(&statement_text)?
        .ok_or_else(|| Error::invalid_argument("fql", "пустой statement"))?;
    let database = ColumnFamilies::open(
        path,
        Options {
            create_if_missing: !readonly,
            read_only: readonly,
            ..Options::default()
        },
    )?;
    let mut session = fql::Session::new(database, readonly);
    let result = session.execute(statement, &mut io::stdout().lock());
    let closing = session.close();
    result.and(closing)
}

fn run_fql_shell(mut arguments: impl Iterator<Item = String>) -> Result<()> {
    let path = required(&mut arguments, "path")?;
    let mut readonly = false;
    let mut script: Option<PathBuf> = None;
    while let Some(option) = arguments.next() {
        match option.as_str() {
            "--read-only" if !readonly => readonly = true,
            "--file" if script.is_none() => {
                script = Some(PathBuf::from(required(&mut arguments, "script_file")?));
            }
            _ => return Err(Error::invalid_argument("shell_option", usage())),
        }
    }

    // Check script path before initializing an empty database.
    let file = if let Some(path) = script.as_ref() {
        Some(
            File::open(path)
                .map_err(|source| Error::io("open_fql_script", Some(path.clone()), source))?,
        )
    } else {
        None
    };

    let database = ColumnFamilies::open(
        path,
        Options {
            read_only: readonly,
            create_if_missing: !readonly,
            ..Options::default()
        },
    )?;
    let mut session = fql::Session::new(database, readonly);
    let result = if let Some(file) = file {
        execute_fql_script(BufReader::new(file), &mut session)
    } else {
        execute_fql_repl(&mut session)
    };
    // Close also performs rollback of a pending BEGIN on errors, EOF and EXIT.
    let closing = session.close();
    result.and(closing)
}

fn execute_fql_script(reader: impl BufRead, session: &mut fql::Session) -> Result<()> {
    let mut stdout = io::stdout().lock();
    for (index, next) in reader.lines().enumerate() {
        let line = next.map_err(|source| Error::io("read_fql_script", None, source))?;
        let result = (|| {
            if let Some(statement) = fql::parse(&line)? {
                session.execute(statement, &mut stdout)
            } else {
                Ok(fql::Flow::Continue)
            }
        })();
        match result {
            Ok(fql::Flow::Continue) => {}
            Ok(fql::Flow::Exit) => return Ok(()),
            Err(error) => {
                eprintln!("FQL: строка {}: {error}", index + 1);
                return Err(error);
            }
        }
    }
    Ok(())
}

fn execute_fql_repl(session: &mut fql::Session) -> Result<()> {
    let interactive = io::stdin().is_terminal();
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut stdout = io::stdout().lock();
    if interactive {
        eprintln!("ForgeDB FQL v0.8.0. HELP — список команд, EXIT — выход.");
    }
    loop {
        if interactive {
            write!(io::stderr(), "forgedb[{}]> ", session.active_name()?)
                .and_then(|()| io::stderr().flush())
                .map_err(|source| Error::io("write_fql_prompt", None, source))?;
        }
        let mut line = String::new();
        let read = input
            .read_line(&mut line)
            .map_err(|source| Error::io("read_fql_stdin", None, source))?;
        if read == 0 {
            return Ok(());
        }
        let result = (|| {
            if let Some(statement) = fql::parse(&line)? {
                session.execute(statement, &mut stdout)
            } else {
                Ok(fql::Flow::Continue)
            }
        })();
        match result {
            Ok(fql::Flow::Continue) => {
                stdout
                    .flush()
                    .map_err(|source| Error::io("flush_fql_stdout", None, source))?;
            }
            Ok(fql::Flow::Exit) => return Ok(()),
            Err(error) => {
                eprintln!("FQL: {error}");
                // In interactive mode a failed statement is NOT retried or committed.
                // Existing BEGIN remains active until explicit ROLLBACK/COMMIT.
            }
        }
    }
}

fn run_column_family_command(mut arguments: impl Iterator<Item = String>) -> Result<()> {
    let subcommand = required(&mut arguments, "cf_command")?;
    let path = required(&mut arguments, "path")?;
    let readonly = matches!(subcommand.as_str(), "list" | "get" | "scan");
    let options = Options {
        create_if_missing: !readonly,
        read_only: readonly,
        ..Options::default()
    };
    let families = ColumnFamilies::open(PathBuf::from(path), options)?;

    match subcommand.as_str() {
        "list" => {
            ensure_no_extra(arguments)?;
            for family in families.list()? {
                println!("{}\t{}", family.id.get(), family.name);
            }
        }
        "create" => {
            let name = required(&mut arguments, "name")?;
            ensure_no_extra(arguments)?;
            let id = families.create(&name)?;
            println!("{}", id.get());
        }
        "drop" => {
            let id = parse_column_family_id(&mut arguments)?;
            ensure_no_extra(arguments)?;
            families.drop_family(id)?;
        }
        "put" => {
            let id = parse_column_family_id(&mut arguments)?;
            let key = required(&mut arguments, "key")?;
            let value = required(&mut arguments, "value")?;
            let write_options = parse_write_options(arguments.next().as_deref())?;
            ensure_no_extra(arguments)?;
            families.put(id, key.as_bytes(), value.as_bytes(), write_options)?;
        }
        "delete" => {
            let id = parse_column_family_id(&mut arguments)?;
            let key = required(&mut arguments, "key")?;
            let write_options = parse_write_options(arguments.next().as_deref())?;
            ensure_no_extra(arguments)?;
            families.delete(id, key.as_bytes(), write_options)?;
        }
        "get" => {
            let id = parse_column_family_id(&mut arguments)?;
            let key = required(&mut arguments, "key")?;
            ensure_no_extra(arguments)?;
            if let Some(value) = families.get(id, key.as_bytes())? {
                write_bytes_line(&value)?;
            }
        }
        "scan" => {
            let id = parse_column_family_id(&mut arguments)?;
            let start = arguments.next().map(String::into_bytes);
            let end = arguments.next().map(String::into_bytes);
            ensure_no_extra(arguments)?;
            let iterator = match (start, end) {
                (None, None) => families.scan(id, .., ReadOptions::default())?,
                (Some(start), None) => families.scan(id, start.., ReadOptions::default())?,
                (None, Some(end)) => families.scan(id, ..end, ReadOptions::default())?,
                (Some(start), Some(end)) => {
                    families.scan(id, start..end, ReadOptions::default())?
                }
            };
            let mut stdout = std::io::stdout().lock();
            for row in iterator {
                let (key, value) = row?;
                stdout
                    .write_all(&key)
                    .and_then(|_| stdout.write_all(b"\t"))
                    .and_then(|_| stdout.write_all(&value))
                    .and_then(|_| stdout.write_all(b"\n"))
                    .map_err(|source| Error::io("write_stdout", None, source))?;
            }
        }
        _ => return Err(Error::invalid_argument("cf_command", usage())),
    }
    families.close()
}

fn parse_column_family_id(arguments: &mut impl Iterator<Item = String>) -> Result<ColumnFamilyId> {
    let raw = required(arguments, "column_family_id")?;
    let id = raw.parse::<u32>().map_err(|_| {
        Error::invalid_argument("column_family_id", "ожидается числовой ID из 'cf list'")
    })?;
    Ok(ColumnFamilyId::new(id))
}

fn run_transaction_command(mut arguments: impl Iterator<Item = String>) -> Result<()> {
    let path = required(&mut arguments, "path")?;
    let database = Database::open(PathBuf::from(path), Options::default())?;
    let mut transaction = database.begin_transaction()?;
    let mut write_options = WriteOptions::sync();

    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--sync" => write_options = WriteOptions::sync(),
            "--no-sync" => write_options = WriteOptions::no_sync(),
            "--put" => {
                let key = required(&mut arguments, "key")?;
                let value = required(&mut arguments, "value")?;
                transaction.put(key.as_bytes(), value.as_bytes())?;
            }
            "--delete" => {
                let key = required(&mut arguments, "key")?;
                transaction.delete(key.as_bytes())?;
            }
            "--get" => {
                let key = required(&mut arguments, "key")?;
                if let Some(value) = transaction.get(key.as_bytes())? {
                    write_bytes_line(&value)?;
                }
            }
            "--rollback" => {
                ensure_no_extra(arguments)?;
                transaction.rollback()?;
                return database.close();
            }
            "--disable-wal" => {
                return Err(Error::invalid_argument(
                    "durability",
                    "--disable-wal запрещён для transaction; используйте --sync или --no-sync",
                ));
            }
            _ => return Err(Error::invalid_argument("transaction_operation", usage())),
        }
    }

    transaction.commit(write_options)?;
    database.close()
}

fn required(arguments: &mut impl Iterator<Item = String>, field: &'static str) -> Result<String> {
    arguments
        .next()
        .ok_or_else(|| Error::invalid_argument(field, usage()))
}

fn parse_write_options(argument: Option<&str>) -> Result<WriteOptions> {
    match argument {
        None | Some("--sync") => Ok(WriteOptions::sync()),
        Some("--no-sync") => Ok(WriteOptions::no_sync()),
        Some("--disable-wal") => Ok(WriteOptions::disable_wal()),
        Some(_) => Err(Error::invalid_argument("durability", usage())),
    }
}

fn ensure_no_extra(mut arguments: impl Iterator<Item = String>) -> Result<()> {
    if arguments.next().is_some() {
        return Err(Error::invalid_argument("arguments", usage()));
    }
    Ok(())
}

fn exit_code_for_error(error: &Error) -> i32 {
    match error {
        Error::InvalidArgument { .. } | Error::ResourceLimit { .. } | Error::ReadOnly => 2,
        Error::UnsupportedFormat { .. } => 4,
        Error::Busy { .. } => 5,
        Error::Io { .. } => 6,
        Error::Corruption { .. } => 7,
        Error::Conflict { .. } => 9,
        Error::BackgroundFailure { .. } | Error::InternalInvariant { .. } => 12,
        Error::Closed => 1,
    }
}

fn write_bytes_line(bytes: &[u8]) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(bytes)
        .and_then(|_| stdout.write_all(b"\n"))
        .map_err(|source| Error::io("write_stdout", None, source))
}

fn print_recovery_plan(plan: &RecoveryPlan) {
    println!("database: {}", plan.database_path.display());
    println!("selected MANIFEST: {}", plan.selected_manifest);
    for value in &plan.replay_wals {
        println!("replay WAL: {value}");
    }
    print_group("live SST", &plan.live_sstables);
    print_group("obsolete", &plan.obsolete_files);
    print_group("orphan SST", &plan.orphan_files);
    print_group("temporary", &plan.temporary_files);
    print_group("backup artifact", &plan.backup_artifacts);
    print_group("user file", &plan.user_files);
    print_group("unknown", &plan.unknown_files);
    print_group("planned safe cleanup", &plan.planned_safe_cleanup);
    print_group("warning", &plan.warnings);
    print_group("blocker", &plan.blockers);
    println!("normal open possible: {}", plan.can_open_normally());
}

fn print_inventory(inventory: &FileInventory) {
    print_group("live metadata", &inventory.live_metadata);
    print_group("live WAL", &inventory.live_wal);
    print_group("live SST", &inventory.live_sst);
    print_group("obsolete", &inventory.obsolete);
    print_group("orphan SST", &inventory.orphan_sst);
    print_group("temporary", &inventory.temporary);
    print_group("backup artifact", &inventory.backup_artifacts);
    print_group("user file", &inventory.user_files);
    print_group("unknown", &inventory.unknown);
    print_group("blockers", &inventory.blockers);
}

fn print_group(name: &str, values: &[String]) {
    for value in values {
        println!("{name}: {value}");
    }
}

fn usage() -> String {
    [
        "Использование:",
        "  forgedb put <каталог> <ключ> <значение> [--sync|--no-sync|--disable-wal]",
        "  forgedb get <каталог> <ключ>",
        "  forgedb fql <каталог> \"<одна FQL команда>\" [--read-only]",
        "  forgedb shell <каталог> [--read-only] [--file <script.fql>]",
        "  forgedb cf list <каталог>",
        "  forgedb cf create <каталог> <имя>",
        "  forgedb cf drop <каталог> <id>",
        "  forgedb cf put <каталог> <id> <ключ> <значение> [--sync|--no-sync|--disable-wal]",
        "  forgedb cf get <каталог> <id> <ключ>",
        "  forgedb cf delete <каталог> <id> <ключ> [--sync|--no-sync|--disable-wal]",
        "  forgedb cf scan <каталог> <id> [start] [end-exclusive]",
        "  forgedb delete <каталог> <ключ> [--sync|--no-sync|--disable-wal]",
        "  forgedb transaction <каталог> [--sync|--no-sync] [--put <ключ> <значение>] [--delete <ключ>] [--get <ключ>] [--rollback] ...",
        "  forgedb scan <каталог> [start] [end-exclusive]",
        "  forgedb compact <каталог> [start] [end-exclusive]",
        "  forgedb inventory <каталог>",
        "  forgedb inspect recovery-plan <каталог>",
        "  forgedb verify <каталог> [--deep]",
        "",
        "CLI v0.8.0: FQL поддерживает binary literals, транзакции и отдельные Column Families.",
        "Transaction commit запрещает --disable-wal и использует optimistic snapshot isolation.",
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_exit_code_is_stable() {
        let error = Error::Conflict {
            keys: vec![b"key".to_vec()],
        };
        assert_eq!(exit_code_for_error(&error), 9);
    }

    #[test]
    fn transaction_usage_is_documented() {
        assert!(usage().contains("forgedb transaction"));
        assert!(usage().contains("--disable-wal"));
    }
}
