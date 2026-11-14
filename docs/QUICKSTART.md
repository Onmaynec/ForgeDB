# Первый запуск ForgeDB CLI v1.0.0 (Stable)

CLI нужен для работы с локальной key-value базой через консоль. Установка Rust и сборка проекта не обязательны: для x86-64 Windows и Linux выпускаются готовые архивы в [GitHub Releases](https://github.com/Onmaynec/ForgeDB/releases/tag/v1.0.0). Архивы содержат одну программу `forgedb` / `forgedb.exe` и отдельный SHA-256 checksum.

## Windows (PowerShell)

Скачайте `forgedb-v1.0.0-windows-x86_64.zip` и одноимённый `.zip.sha256` из Assets релиза. Откройте PowerShell в каталоге загрузок.

```powershell
$archive = "forgedb-v1.0.0-windows-x86_64.zip"
$expected = (Get-Content "$archive.sha256").Split(" ")[0]
$actual = (Get-FileHash $archive -Algorithm SHA256).Hash
if ($actual -ne $expected) { throw "SHA256 checksum mismatch" }

Expand-Archive $archive -DestinationPath "./forgedb-cli" -Force
Set-Location ./forgedb-cli
./forgedb.exe put ./data hello world
./forgedb.exe get ./data hello
./forgedb.exe scan ./data
./forgedb.exe verify ./data --deep
```

CLI создаёт каталог `data`, сохраняет ключ `hello` со значением `world` и проверяет структуру базы. На Windows исполняемый файл может запускаться с `./` или `.\\`.

## Linux (x86-64)

Скачайте `forgedb-v1.0.0-linux-x86_64.tar.gz` и `.tar.gz.sha256` из Assets релиза.

```bash
sha256sum -c forgedb-v1.0.0-linux-x86_64.tar.gz.sha256
tar -xzf forgedb-v1.0.0-linux-x86_64.tar.gz
./forgedb put ./data hello world
./forgedb get ./data hello
./forgedb scan ./data
./forgedb verify ./data --deep
```

## Проверка состояния и перенос базы (v1.0.0)

```bash
forgedb version
forgedb doctor ./data
forgedb doctor ./data --deep
forgedb export ./data ./my-backup.fdbk
forgedb restore ./my-backup.fdbk ./restored-data
forgedb doctor ./restored-data --deep
forgedb cf list ./restored-data
```

Если CLI находится в текущем каталоге Linux, используйте `./forgedb`; в Windows PowerShell — `.\\forgedb.exe`. `GET`, `SCAN` и `doctor` открывают только существующую базу в read-only режиме: ошибка в пути не создаёт новый каталог. `doctor` никогда не выполняет cleanup или repair.

Архив `.fdbk` хранит бинарные ключи и значения из **всех действующих** Column Families, каждую на собственном MVCC snapshot. Он содержит CRC32C, но не шифрование и не криптографическую подпись. `restore` принимает **только новый** каталог и не перезаписывает работающую базу. Числовые family ID могут измениться; после восстановления определите их по имени через `cf list`. Данные разных families не имеют общей атомарной точки (Issue #35), поэтому при необходимости согласованного multi-family checkpoint остановите запись приложения до экспорта.

Формат архива и риски: [LOGICAL_ARCHIVE_V1.md](formats/LOGICAL_ARCHIVE_V1.md), [GA_READINESS.md](GA_READINESS.md).

## Подключение в Rust-проекте

Пакет с именем `forgedb` в crates.io принадлежит **другому проекту**; данный движок доступен как Git-зависимость по конкретному стабильному tag:

```toml
[dependencies]
forgedb = { git = "https://github.com/Onmaynec/ForgeDB.git", tag = "v1.0.0" }
```

Пример:

```rust
use forgedb::{Database, Options, Result, WriteOptions};

fn main() -> Result<()> {
    let db = Database::open("./data", Options::default())?;
    db.put(b"hello", b"world", WriteOptions::sync())?;
    assert_eq!(db.get(b"hello")?, Some(b"world".to_vec()));
    db.close()
}
```

Минимальный toolchain — Rust 1.89. Устанавливать Rust для готового CLI не нужно.

## Именованные Column Families (v0.7.0)

```bash
./forgedb cf create ./data users
./forgedb cf list ./data
./forgedb cf put ./data 1 user42 Alice --sync
./forgedb cf get ./data 1 user42
./forgedb cf scan ./data 1
```

`cf create` выводит ID, например `1`; используйте ID из `cf list` для `get/put/delete/scan/drop`. Удаление family командой `cf drop` исключает её из реестра, но не удаляет физические данные; повторное создание такого же имени получает новый ID. Для multi-operation atomic batch внутри family и транзакций используйте библиотечный `ColumnFamilies` API.

## FQL V1 — запросы и интерактивный shell (v0.8.0)

```powershell
.\forgedb.exe fql .\data "PUT key 'Hello FQL'"
.\forgedb.exe fql .\data "GET key" --read-only
.\forgedb.exe shell .\data
.\forgedb.exe shell .\data --file .\commands.fql
```

Файл `commands.fql`:

```text
CREATE FAMILY users
USE users
BEGIN
PUT account 'Alice'
COMMIT
SCAN FROM a TO z LIMIT 20
EXIT
```

В FQL одна команда на строку; `BEGIN/COMMIT` обеспечивает атомарную запись **только внутри одной family**. Скрипт целиком не является транзакцией. Ошибка файла останавливает исполнение, незавершённый BEGIN откатывается. Полное описание синтаксиса, бинарных литералов и ограничений: [docs/FQL.md](FQL.md).

## Дополнительные команды

```text
forgedb delete <каталог> <ключ>
forgedb transaction <каталог> --put <ключ> <значение> --sync
forgedb compact <каталог>
forgedb inventory <каталог>
forgedb inspect recovery-plan <каталог>
```

Простые CLI `put/get/scan` принимают UTF-8 аргументы; для произвольных бинарных данных используйте Rust API, FQL V1 binary literals либо LOGICAL_ARCHIVE V1. Движок предназначен для локального встраивания и не предоставляет сетевой SQL-сервер.

Прежние WAL V1, SSTable V1, MANIFEST V1/V2 и CF_REGISTRY V1 остаются совместимыми. Стабильный релиз ориентирован на локальное встраивание и доверенную файловую систему, но не включает шифрование, HA, репликацию или process-global FD limit. Для физической копии каталога закрывайте процесс; для переносимого **логического** экспорта используйте `export`.

Если в Assets релиза нет архива для вашей платформы, используйте сборку из исходников:

```bash
cargo build --locked --release -p forgedb-cli
```
