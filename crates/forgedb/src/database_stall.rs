use std::time::Duration;

const MAX_WRITE_SLOWDOWN_MILLIS: usize = 50;
const WRITE_SLOWDOWN_STEP_MILLIS: usize = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WriteStallDecision {
    None,
    Slowdown(Duration),
    Stop,
}

impl Database {
    /// Применяет bounded L0 backpressure до входа в serialized write path.
    ///
    /// Sleep никогда не выполняется под database state lock. После задержки stop
    /// threshold проверяется повторно, чтобы write не прошёл через выросший L0 debt.
    fn apply_write_stall(&self) -> Result<()> {
        let decision = {
            let state = self.lock_state()?;
            ensure_writer_healthy(&state)?;
            write_stall_decision(
                l0_file_count(state.version.as_ref())?,
                self.options.l0_slowdown_trigger,
                self.options.l0_stop_trigger,
            )?
        };

        match decision {
            WriteStallDecision::None => Ok(()),
            WriteStallDecision::Stop => Err(l0_stop_error(self.options.l0_stop_trigger)),
            WriteStallDecision::Slowdown(delay) => {
                std::thread::sleep(delay);
                let state = self.lock_state()?;
                ensure_writer_healthy(&state)?;
                let l0_files = l0_file_count(state.version.as_ref())?;
                if l0_files >= self.options.l0_stop_trigger {
                    return Err(l0_stop_error(self.options.l0_stop_trigger));
                }
                Ok(())
            }
        }
    }
}

fn l0_file_count(version: &VersionSet) -> Result<usize> {
    version
        .metadata_levels()
        .first()
        .map(Vec::len)
        .ok_or(Error::InternalInvariant {
            invariant: "I-WRITE-STALL-L0-PRESENT",
            reason: "VersionSet не содержит L0".to_owned(),
        })
}

fn write_stall_decision(
    l0_files: usize,
    slowdown_trigger: usize,
    stop_trigger: usize,
) -> Result<WriteStallDecision> {
    if slowdown_trigger == 0 || stop_trigger < slowdown_trigger {
        return Err(Error::InternalInvariant {
            invariant: "I-WRITE-STALL-THRESHOLDS",
            reason: "validated slowdown/stop thresholds стали некорректными".to_owned(),
        });
    }
    if l0_files >= stop_trigger {
        return Ok(WriteStallDecision::Stop);
    }
    if l0_files < slowdown_trigger {
        return Ok(WriteStallDecision::None);
    }

    let pressure = l0_files
        .checked_sub(slowdown_trigger)
        .and_then(|value| value.checked_add(1))
        .ok_or(Error::ResourceLimit {
            resource: "write_stall_pressure",
            limit: u64::MAX,
            observed: u64::MAX,
        })?;
    let millis = pressure
        .saturating_mul(WRITE_SLOWDOWN_STEP_MILLIS)
        .min(MAX_WRITE_SLOWDOWN_MILLIS);
    let millis = u64::try_from(millis).map_err(|_| Error::ResourceLimit {
        resource: "write_stall_delay_millis",
        limit: u64::MAX,
        observed: u64::MAX,
    })?;
    Ok(WriteStallDecision::Slowdown(Duration::from_millis(
        millis,
    )))
}

fn l0_stop_error(stop_trigger: usize) -> Error {
    Error::Busy {
        reason: format!(
            "write остановлен: L0 достиг stop threshold {stop_trigger}; выполните compaction"
        ),
    }
}

#[cfg(test)]
mod write_stall_tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temporary_directory(name: &str) -> Result<PathBuf> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "forgedb-write-stall-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path)
            .map_err(|source| Error::io("create_test_directory", Some(path.clone()), source))?;
        Ok(path)
    }

    #[test]
    fn decision_boundaries_and_delay_cap_are_deterministic() -> Result<()> {
        assert_eq!(write_stall_decision(3, 4, 8)?, WriteStallDecision::None);
        assert_eq!(
            write_stall_decision(4, 4, 8)?,
            WriteStallDecision::Slowdown(Duration::from_millis(2))
        );
        assert_eq!(
            write_stall_decision(7, 4, 8)?,
            WriteStallDecision::Slowdown(Duration::from_millis(8))
        );
        assert_eq!(write_stall_decision(8, 4, 8)?, WriteStallDecision::Stop);
        assert_eq!(
            write_stall_decision(100, 1, usize::MAX)?,
            WriteStallDecision::Slowdown(Duration::from_millis(50))
        );
        Ok(())
    }

    #[test]
    fn stop_rejects_writes_but_manual_compaction_restores_progress() -> Result<()> {
        let directory = temporary_directory("stop-and-compact")?;
        let options = Options {
            l0_compaction_trigger: 1,
            l0_slowdown_trigger: 1,
            l0_stop_trigger: 2,
            target_sst_size: 1024,
            data_block_size: 1024,
            ..Options::default()
        };
        let database = StreamingDatabase::open(&directory, options)?;
        database.put(b"alpha", b"one", WriteOptions::sync())?;
        database.flush()?;
        database.put(b"beta", b"two", WriteOptions::sync())?;
        database.flush()?;

        assert!(matches!(
            database.put(b"blocked", b"value", WriteOptions::sync()),
            Err(Error::Busy { .. })
        ));
        assert_eq!(database.get(b"alpha")?, Some(b"one".to_vec()));

        database.compact_range(None, None)?;
        database.put(b"resumed", b"value", WriteOptions::sync())?;
        assert_eq!(database.get(b"resumed")?, Some(b"value".to_vec()));
        database.close()?;
        fs::remove_dir_all(&directory)
            .map_err(|source| Error::io("remove_test_directory", Some(directory), source))?;
        Ok(())
    }
}
