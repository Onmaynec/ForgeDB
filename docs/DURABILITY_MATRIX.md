# Матрица durability

Эта таблица дополняет нормативный документ [`DURABILITY.md`](DURABILITY.md).

| Операция | Linux | Windows | Process crash | OS crash / power loss |
|---|---|---|---|---|
| Жизненный цикл `LOCK` | advisory exclusive lock, release при close handle/process | exclusive lock, release при close handle/process | освобождается ОС | не относится к сохранности данных |
| Установка нового `IDENTITY` | file `sync_all`, затем directory sync | file `sync_all`, затем directory handle sync | завершённый identity должен сохраниться | зависит от ФС и оборудования |
| Создание заголовка WAL | write header, file `sync_all`, close, directory sync | то же с Windows directory handle | полный header должен сохраниться | зависит от ФС и оборудования |
| `Sync` WriteBatch | `write_all` + `sync_all` до memtable visibility | то же | должен replay после успешного возврата | зависит от честности sync stack |
| `NoSync` WriteBatch | append без обязательного sync | append без обязательного sync | не гарантируется | не гарантируется |
| `DisableWal` WriteBatch | только memtable | только memtable | теряется | теряется |
| Последний partial WAL frame | игнорируется; read-write open выполняет truncate + sync | то же | предыдущие полные frames replay | применимо при следующем open |
| Полный frame с неверной CRC | open/replay завершается ошибкой | open/replay завершается ошибкой | более поздние frames не применяются | обнаруживается при чтении |
| `Database::close` | sync WAL, close writer, release lock | то же | после возврата ресурсы освобождены | предыдущие Sync гарантии не усиливаются |
| `rename_atomic` abstraction | `std::fs::rename` | `std::fs::rename` | пока не участвует в persisted-протоколе v0.1.0 | platform-dependent |

## Термины

- **Process crash** — аварийное завершение процесса при продолжающей работать ОС.
- **OS crash** — kernel panic, reset или остановка ОС без штатного shutdown.
- **Power loss** — потеря питания, включая поведение write cache накопителя и контроллера.

## Важная граница

Успешный системный `sync_all` — граница ответственности ForgeDB v0.1.0. Проект не может доказать выполнение обещаний
накопителем или контроллером. Для критичных данных необходимо проверять mount options, write-cache policy, power-loss
protection и реальные crash-тесты на целевой инфраструктуре.
