use std::io::Write;
use std::path::PathBuf;

use forgedb::{Database, Error, Options, Result, WriteOptions};

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
                let mut stdout = std::io::stdout().lock();
                stdout
                    .write_all(&value)
                    .map_err(|source| Error::io("write_stdout", None, source))?;
                stdout
                    .write_all(b"\n")
                    .map_err(|source| Error::io("write_stdout", None, source))?;
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

fn usage() -> String {
    [
        "Использование:",
        "  forgedb put <каталог> <ключ> <значение> [--sync|--no-sync|--disable-wal]",
        "  forgedb get <каталог> <ключ>",
        "  forgedb delete <каталог> <ключ> [--sync|--no-sync|--disable-wal]",
        "",
        "CLI v0.1.0 принимает ключи и значения как UTF-8 аргументы командной строки.",
    ]
    .join("\n")
}
