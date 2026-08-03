# Политика безопасности

ForgeDB находится до v1.0 и пока не предназначена для критичных production-данных.

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

## Что проверяется в v0.1.0

- exclusive process-level lock;
- database identity в WAL;
- CRC32C заголовка и полных кадров;
- bounded varint и length fields;
- непротиворечивый sequence range;
- полное декодирование batch до применения;
- запрет resynchronization после mid-file corruption;
- безопасный trailing partial frame только в конце файла.

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
