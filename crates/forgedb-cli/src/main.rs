use std::io::Write;
use std::path::PathBuf;

use forgedb::{
    Database, Error, FileInventory, Options, ReadOptions, RecoveryPlan, Result, VerifyOptions,
    WriteOptions,
};

fn main() {
    let exit_code = match run() {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("Ошибка ForgeDB: {error}");
            1
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
        "  forgedb delete <каталог> <ключ> [--sync|--no-sync|--disable-wal]",
        "  forgedb scan <каталог> [start] [end-exclusive]",
        "  forgedb compact <каталог> [start] [end-exclusive]",
        "  forgedb inventory <каталог>",
        "  forgedb inspect recovery-plan <каталог>",
        "  forgedb verify <каталог> [--deep]",
        "",
        "CLI v0.4.0 принимает границы scan/compact как UTF-8; library API остаётся binary-safe.",
    ]
    .join("\n")
}
