use std::collections::BTreeSet;
use std::sync::Weak;

#[derive(Debug)]
struct ObsoleteVersion {
    version: Weak<VersionSet>,
    file_numbers: Vec<u64>,
}

impl ObsoleteVersion {
    fn new(version: &Arc<VersionSet>, file_numbers: Vec<u64>) -> Result<Self> {
        let mut unique = BTreeSet::new();
        for file_number in file_numbers {
            if file_number == 0 {
                return Err(Error::corruption(
                    "obsolete SST lifecycle",
                    "file number равен нулю",
                ));
            }
            if !unique.insert(file_number) {
                return Err(Error::corruption(
                    "obsolete SST lifecycle",
                    "duplicate obsolete file number",
                ));
            }
        }
        if unique.is_empty() {
            return Err(Error::invalid_argument(
                "obsolete_files",
                "obsolete Version должна содержать минимум один SSTable",
            ));
        }
        Ok(Self {
            version: Arc::downgrade(version),
            file_numbers: unique.into_iter().collect(),
        })
    }

    fn is_referenced(&self) -> bool {
        self.version.upgrade().is_some()
    }
}

impl Database {
    /// Удаляет только SSTables, чью старую immutable Version больше никто не удерживает.
    ///
    /// При ошибке удаления неуспешные file numbers остаются в очереди для retry.
    fn collect_obsolete_ssts_locked(&self, state: &mut DatabaseState) -> Result<()> {
        let pending = std::mem::take(&mut state.obsolete_versions);
        let mut retained = Vec::new();
        let mut first_error = None;
        let filesystem = NativeFileSystem;

        for obsolete in pending {
            if obsolete.is_referenced() {
                retained.push(obsolete);
                continue;
            }

            let mut failed_numbers = Vec::new();
            for file_number in obsolete.file_numbers {
                let path = self.directory.join(sst_file_name(file_number));
                if let Err(error) = remove_if_exists(&filesystem, &path, &self.directory) {
                    failed_numbers.push(file_number);
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
            if !failed_numbers.is_empty() {
                retained.push(ObsoleteVersion {
                    version: Weak::new(),
                    file_numbers: failed_numbers,
                });
            }
        }

        state.obsolete_versions = retained;
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}
