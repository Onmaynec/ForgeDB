# Политика безопасности

ForgeDB v1.0.0 — стабильный локальный embedded KV engine для доверенной среды Windows/Linux. Он не рассчитан на недоверенную файловую систему, злоумышленного администратора или критичную инфраструктуру без независимого backup/monitoring и тестов восстановления приложения. Стабильный статус не означает наличие encryption, ACL или отказоустойчивого кластера.

## Сообщение об уязвимости

Используйте приватное сообщение об уязвимости GitHub для этого репозитория. Не публикуйте exploit details
в открытом issue до того, как maintainers смогут исследовать проблему и подготовить исправление.

В отчёте желательно указать:

- затронутую версию и commit;
- операционную систему и файловую систему;
- минимальный reproducer;
- влияние на confidentiality, integrity или availability;
- необходимость malformed persisted-файла или локального доступа;
- возможное влияние на recovery и durability.

## Текущая модель угроз

ForgeDB считает недоверенными:

- пути и конфигурацию;
- ключи, значения и batch contents;
- `IDENTITY`, internal keys и WAL bytes;
- длины, счётчики, sequence numbers и version fields;
- физически усечённые или намеренно повреждённые файлы.

Parsers используют checked arithmetic, точные границы layout и настроенные allocation limits. Неизвестные версии,
ненулевые reserved fields и checksum mismatch отклоняются.

## Контроль повреждений и жизненного цикла (включая v1.0.0)

- exclusive process-level lock;
- database identity в WAL;
- CRC32C заголовка и полных кадров;
- bounded varint и length fields;
- непротиворечивый sequence range;
- полное декодирование batch до применения;
- запрет resynchronization после mid-file corruption;
- безопасный trailing partial frame только в конце файла;
- контроль checksums, структуры и границ SSTable V1;
- MANIFEST V1/V2 и безопасная установка версии через CURRENT;
- snapshot-aware recovery после WAL rotation, flush и compaction;
- защита от частичного `WriteBatch` при recovery;
- синхронизация WAL/MANIFEST до завершения `close()` с возможностью retry при ошибке;
- reference-aware удаление только доказанно устаревших SSTable, без удаления непроверенных orphan-файлов.

## Что не предоставляется

ForgeDB пока не предоставляет:

- encryption at rest;
- authentication или authorization;
- tamper-proof storage;
- secure deletion;
- защиту от администратора или процесса с правом изменять файлы базы;
- криптографическую целостность WAL;
- защиту от rollback старой копии каталога;
- сетевой security boundary.

CRC32C предназначен для обнаружения случайного повреждения, а не для защиты от активного атакующего.

## Stable v1.0.0: экспорт, проверка и supply chain

Новый LOGICAL_ARCHIVE V1 использует CRC32C, strict version/length/EOF checks, проверку возрастающих ключей и staging restore в новый каталог. **CRC32C не является подписью или шифрованием**: архив нужно хранить и передавать через доверенные каналы и при необходимости шифровать внешними средствами. Данные нескольких named families экспортируются через независимые MVCC snapshots, без общей atomic time boundary.

Команда `doctor --deep` и CLI `get/scan` не должны изменять базу; на повреждения и незнакомые версии возвращаются ошибки. GitHub stable release v1.0.0 собирается из аннотированного tag на Windows/Linux, выполняет end-to-end smoke и проверяет SHA-256 вложений до публикации как Stable. SHA-256 помогает выявлять случайное изменение скачанного файла, но не заменяет доверенную цепочку получения релиза.

Список не реализованных границ: [GA_READINESS.md](docs/GA_READINESS.md), issues #31 и #35.
