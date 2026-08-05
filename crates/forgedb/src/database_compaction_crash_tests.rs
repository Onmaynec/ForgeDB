const COMPACTION_CRASH_FAILPOINT_ENV: &str = "FORGEDB_COMPACTION_CRASH_FAILPOINT";
const COMPACTION_CRASH_WORKER_ENV: &str = "FORGEDB_COMPACTION_CRASH_WORKER";
const COMPACTION_CRASH_DATABASE_ENV: &str = "FORGEDB_COMPACTION_CRASH_DATABASE";
const COMPACTION_CRASH_SENTINEL_ENV: &str = "FORGEDB_COMPACTION_CRASH_SENTINEL";

fn compaction_crash_failpoint(name: &'static str) {
    if std::env::var(COMPACTION_CRASH_FAILPOINT_ENV)
        .ok()
        .as_deref()
        == Some(name)
    {
        std::process::abort();
    }
}

mod compaction_process_crash_tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    const CRASH_WORKER_FILTER: &str = "compaction_crash_worker";

    #[derive(Clone, Copy)]
    struct CrashCase {
        failpoint: &'static str,
        expected_live_sst: usize,
        expected_orphan_sst: usize,
    }

    fn crash_options() -> Options {
        Options {
            target_sst_size: 1024,
            data_block_size: 1024,
            l0_compaction_trigger: 2,
            l0_slowdown_trigger: 4,
            l0_stop_trigger: 6,
            max_compaction_bytes: 64 * 1024 * 1024,
            ..Options::default()
        }
    }

    fn temporary_directory(name: &str) -> Result<PathBuf> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "forgedb-compaction-crash-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path)
            .map_err(|source| Error::io("create_crash_test_directory", Some(path.clone()), source))?;
        Ok(path)
    }

    fn prepare_two_l0_files(directory: &Path) -> Result<()> {
        let database = StreamingDatabase::open(directory, crash_options())?;
        database.put(b"alpha", b"old", WriteOptions::sync())?;
        database.flush()?;
        database.put(b"alpha", b"new", WriteOptions::sync())?;
        database.put(b"beta", b"value", WriteOptions::sync())?;
        database.flush()?;
        let inventory = database.file_inventory()?;
        if inventory.live_sst.len() != 2 {
            return Err(Error::InternalInvariant {
                invariant: "I-COMPACTION-CRASH-SETUP-L0",
                reason: format!(
                    "crash setup ожидал 2 live SST, получил {}",
                    inventory.live_sst.len()
                ),
            });
        }
        database.close()
    }

    fn run_case(case: CrashCase) -> Result<()> {
        let directory = temporary_directory(case.failpoint)?;
        prepare_two_l0_files(&directory)?;
        let sentinel = directory.with_extension(format!("{}.returned", case.failpoint));
        let executable = std::env::current_exe()
            .map_err(|source| Error::io("resolve_crash_test_executable", None, source))?;
        let output = Command::new(&executable)
            .arg(CRASH_WORKER_FILTER)
            .arg("--nocapture")
            .env(COMPACTION_CRASH_WORKER_ENV, "1")
            .env(COMPACTION_CRASH_DATABASE_ENV, &directory)
            .env(COMPACTION_CRASH_FAILPOINT_ENV, case.failpoint)
            .env(COMPACTION_CRASH_SENTINEL_ENV, &sentinel)
            .output()
            .map_err(|source| {
                Error::io(
                    "run_compaction_crash_worker",
                    Some(executable.clone()),
                    source,
                )
            })?;
        if output.status.success() || sentinel.exists() {
            return Err(Error::InternalInvariant {
                invariant: "I-COMPACTION-CRASH-FAILPOINT-REACHED",
                reason: format!(
                    "worker не завершился в failpoint {}: status={:?}, stdout={}, stderr={}",
                    case.failpoint,
                    output.status,
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }

        let reopened = StreamingDatabase::open(&directory, crash_options())?;
        assert_eq!(reopened.get(b"alpha")?, Some(b"new".to_vec()));
        assert_eq!(reopened.get(b"beta")?, Some(b"value".to_vec()));
        let inventory = reopened.file_inventory()?;
        assert_eq!(inventory.live_sst.len(), case.expected_live_sst);
        assert_eq!(inventory.orphan_sst.len(), case.expected_orphan_sst);
        reopened.close()?;
        fs::remove_dir_all(&directory).map_err(|source| {
            Error::io("remove_crash_test_directory", Some(directory.clone()), source)
        })?;
        match fs::remove_file(&sentinel) {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(Error::io(
                "remove_crash_test_sentinel",
                Some(sentinel),
                source,
            )),
        }
    }

    #[test]
    fn compaction_process_crash_matrix_preserves_a_valid_durable_state() -> Result<()> {
        for case in [
            CrashCase {
                failpoint: "after_compaction_output_sync",
                expected_live_sst: 2,
                expected_orphan_sst: 1,
            },
            CrashCase {
                failpoint: "after_compaction_manifest_sync",
                expected_live_sst: 1,
                expected_orphan_sst: 2,
            },
            CrashCase {
                failpoint: "before_obsolete_sst_delete",
                expected_live_sst: 1,
                expected_orphan_sst: 2,
            },
        ] {
            run_case(case)?;
        }
        Ok(())
    }

    #[test]
    fn compaction_crash_worker() -> Result<()> {
        if std::env::var_os(COMPACTION_CRASH_WORKER_ENV).is_none() {
            return Ok(());
        }
        let directory = std::env::var_os(COMPACTION_CRASH_DATABASE_ENV)
            .map(PathBuf::from)
            .ok_or(Error::InternalInvariant {
                invariant: "I-COMPACTION-CRASH-WORKER-DATABASE",
                reason: "worker не получил database path".to_owned(),
            })?;
        let sentinel = std::env::var_os(COMPACTION_CRASH_SENTINEL_ENV)
            .map(PathBuf::from)
            .ok_or(Error::InternalInvariant {
                invariant: "I-COMPACTION-CRASH-WORKER-SENTINEL",
                reason: "worker не получил sentinel path".to_owned(),
            })?;
        let database = StreamingDatabase::open(&directory, crash_options())?;
        let result = database.compact_range(None, None);
        fs::write(&sentinel, b"compaction returned before failpoint").map_err(|source| {
            Error::io("write_crash_test_sentinel", Some(sentinel.clone()), source)
        })?;
        match result {
            Ok(()) => Err(Error::InternalInvariant {
                invariant: "I-COMPACTION-CRASH-FAILPOINT-MISSED",
                reason: "compaction завершилась без process crash".to_owned(),
            }),
            Err(error) => Err(error),
        }
    }
}
