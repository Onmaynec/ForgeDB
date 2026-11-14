# Первый запуск ForgeDB CLI v0.6.2

CLI нужен для работы с локальной key-value базой через консоль. Установка Rust и сборка проекта не обязательны: для x86-64 Windows и Linux выпускаются готовые архивы в [GitHub Releases](https://github.com/Onmaynec/ForgeDB/releases/tag/v0.6.2). Архивы содержат одну программу `forgedb` / `forgedb.exe` и отдельный SHA-256 checksum.

## Windows (PowerShell)

Скачайте `forgedb-v0.6.2-windows-x86_64.zip` и одноимённый `.zip.sha256` из Assets релиза. Откройте PowerShell в каталоге загрузок.

```powershell
$archive = "forgedb-v0.6.2-windows-x86_64.zip"
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

Скачайте `forgedb-v0.6.2-linux-x86_64.tar.gz` и `.tar.gz.sha256` из Assets релиза.

```bash
sha256sum -c forgedb-v0.6.2-linux-x86_64.tar.gz.sha256
tar -xzf forgedb-v0.6.2-linux-x86_64.tar.gz
./forgedb put ./data hello world
./forgedb get ./data hello
./forgedb scan ./data
./forgedb verify ./data --deep
```

## Дополнительные команды

```text
forgedb delete <каталог> <ключ>
forgedb transaction <каталог> --put <ключ> <значение> --sync
forgedb compact <каталог>
forgedb inventory <каталог>
forgedb inspect recovery-plan <каталог>
```

CLI принимает текстовые ключи и значения в UTF-8; для произвольных бинарных данных используйте библиотечный Rust API. Движок предназначен для локального встраивания и не предоставляет сетевой SQL-сервер.

Прежние форматы WAL V1, SSTable V1 и MANIFEST V1/V2 совместимы с v0.6.2. Но база до v1.0 не рекомендуется для критичных production-данных. Перед экспериментами с повреждёнными файлами сделайте копию каталога базы **при закрытом процессе**.

Если в Assets релиза нет архива для вашей платформы, используйте сборку из исходников:

```bash
cargo build --locked --release -p forgedb-cli
```
