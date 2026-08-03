use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

use forgedb_core::Result;

/// Контракт последовательного чтения файла.
pub trait SequentialReader: Read + Send {}

impl<T> SequentialReader for T where T: Read + Send {}

/// Контракт произвольного чтения, который позднее будет использовать SSTable reader.
pub trait RandomAccessReader: Read + Seek + Send + Sync {}

impl<T> RandomAccessReader for T where T: Read + Seek + Send + Sync {}

/// Контракт записываемого и синхронизируемого файла.
pub trait WritableFile: Write + Seek + Send {
    /// Синхронизирует данные и метаданные файла, необходимые платформе.
    fn sync_all(&self) -> std::io::Result<()>;

    /// Синхронизирует данные файла, когда платформа различает данные и метаданные.
    fn sync_data(&self) -> std::io::Result<()>;
}

impl WritableFile for std::fs::File {
    fn sync_all(&self) -> std::io::Result<()> {
        std::fs::File::sync_all(self)
    }

    fn sync_data(&self) -> std::io::Result<()> {
        std::fs::File::sync_data(self)
    }
}

/// Операции файловой системы, необходимые persisted-протоколам ForgeDB.
pub trait FileSystem: Send + Sync + 'static {
    /// Тип последовательного reader.
    type SequentialReader: SequentialReader;
    /// Тип random-access reader.
    type RandomAccessReader: RandomAccessReader;
    /// Тип записываемого файла.
    type WritableFile: WritableFile;
    /// Тип guard эксклюзивной блокировки базы.
    type LockGuard: Send + Sync;

    /// Создаёт каталог и отсутствующих родителей.
    fn create_dir_all(&self, path: &Path) -> Result<()>;
    /// Открывает файл для последовательного чтения.
    fn open_sequential(&self, path: &Path) -> Result<Self::SequentialReader>;
    /// Открывает файл для произвольного чтения.
    fn open_random_access(&self, path: &Path) -> Result<Self::RandomAccessReader>;
    /// Создаёт или обнуляет записываемый файл.
    fn create_writable(&self, path: &Path) -> Result<Self::WritableFile>;
    /// Создаёт файл только тогда, когда его ещё нет.
    fn create_new_writable(&self, path: &Path) -> Result<Self::WritableFile>;
    /// Открывает или создаёт файл для append.
    fn open_appendable(&self, path: &Path) -> Result<Self::WritableFile>;
    /// Выполняет native rename одного пути в другой.
    fn rename_atomic(&self, from: &Path, to: &Path) -> Result<()>;
    /// Удаляет один обычный файл.
    fn remove_file(&self, path: &Path) -> Result<()>;
    /// Возвращает прямых потомков каталога.
    fn list_dir(&self, path: &Path) -> Result<Vec<PathBuf>>;
    /// Возвращает текущий размер файла.
    fn file_size(&self, path: &Path) -> Result<u64>;
    /// Усекает файл до проверенной границы и синхронизирует новый размер.
    fn truncate_file(&self, path: &Path, length: u64) -> Result<()>;
    /// Синхронизирует метаданные каталога, когда платформа это поддерживает.
    fn sync_directory(&self, path: &Path) -> Result<()>;
    /// Получает неблокирующую эксклюзивную process-level блокировку.
    fn lock_exclusive(&self, path: &Path) -> Result<Self::LockGuard>;
}
