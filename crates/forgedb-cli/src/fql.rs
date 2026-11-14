//! FQL V1 — строгий, bounded язык запросов CLI поверх ColumnFamilies.
//! Не SQL: нет joins, общего WAL, range-reads внутри транзакций.

use std::io::Write;

use forgedb::{
    ColumnFamilies, ColumnFamilyId, Error, ReadOptions, Result, Transaction, WriteOptions,
};

pub(crate) const MAX_STATEMENT_BYTES: usize = 64 * 1024;
const MAX_TOKENS: usize = 512;
const DEFAULT_SCAN_LIMIT: usize = 100;
const MAX_SCAN_LIMIT: usize = 10_000;

#[derive(Clone, Debug, Eq, PartialEq)]
enum Token {
    Word(String),
    Literal(Vec<u8>),
    Semicolon,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Statement {
    ShowFamilies,
    CreateFamily(String),
    DropFamily(String),
    Use(String),
    Put(Vec<u8>, Vec<u8>),
    Get(Vec<u8>),
    Delete(Vec<u8>),
    Scan {
        from: Option<Vec<u8>>,
        to: Option<Vec<u8>>,
        limit: usize,
    },
    Begin,
    Commit,
    Rollback,
    Help,
    Exit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Flow {
    Continue,
    Exit,
}

/// Перекодирует произвольные binary bytes в безопасный, повторно читаемый FQL literal.
/// Все небуквенные ASCII и non-ASCII bytes представлены через \xHH.
fn display_bytes(bytes: &[u8]) -> String {
    let mut text = String::from("'");
    for &byte in bytes {
        match byte {
            b'\\' => text.push_str("\\\\"),
            b'\'' => text.push_str("\\'"),
            b'\n' => text.push_str("\\n"),
            b'\r' => text.push_str("\\r"),
            b'\t' => text.push_str("\\t"),
            0x20..=0x7e => text.push(char::from(byte)),
            _ => text.push_str(&format!("\\x{byte:02x}")),
        }
    }
    text.push('\'');
    text
}

fn syntax(reason: impl Into<String>) -> Error {
    Error::invalid_argument("fql", reason)
}

fn hex_nibble(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(syntax("неверная hexadecimal цифра в literal")),
    }
}

fn read_quoted(input: &[u8], position: &mut usize) -> Result<Vec<u8>> {
    // opening quote already located
    *position += 1;
    let mut decoded = Vec::new();
    while *position < input.len() {
        let next = input[*position];
        *position += 1;
        match next {
            b'\'' => return Ok(decoded),
            b'\\' => {
                let escape = *input
                    .get(*position)
                    .ok_or_else(|| syntax("незавершённый escape"))?;
                *position += 1;
                let decoded_byte = match escape {
                    b'\\' => b'\\',
                    b'\'' => b'\'',
                    b'n' => b'\n',
                    b'r' => b'\r',
                    b't' => b'\t',
                    b'x' => {
                        let high = *input
                            .get(*position)
                            .ok_or_else(|| syntax("незавершённый \\xHH escape"))?;
                        let low = *input
                            .get(*position + 1)
                            .ok_or_else(|| syntax("незавершённый \\xHH escape"))?;
                        *position += 2;
                        (hex_nibble(high)? << 4) | hex_nibble(low)?
                    }
                    _ => return Err(syntax("неизвестная escape последовательность")),
                };
                decoded.push(decoded_byte);
            }
            other => decoded.push(other),
        }
    }
    Err(syntax("незакрытая одинарная кавычка"))
}

fn read_hex(input: &[u8], position: &mut usize) -> Result<Vec<u8>> {
    // current position is x, next is quote
    *position += 2;
    let mut decoded = Vec::new();
    while *position < input.len() {
        if input[*position] == b'\'' {
            *position += 1;
            return Ok(decoded);
        }
        let high = input[*position];
        let low = *input
            .get(*position + 1)
            .ok_or_else(|| syntax("нечётная длина hexadecimal literal"))?;
        if low == b'\'' {
            return Err(syntax("нечётная длина hexadecimal literal"));
        }
        decoded.push((hex_nibble(high)? << 4) | hex_nibble(low)?);
        *position += 2;
    }
    Err(syntax("незакрытый hexadecimal literal"))
}

fn lex(input: &str) -> Result<Vec<Token>> {
    if input.len() > MAX_STATEMENT_BYTES {
        return Err(Error::ResourceLimit {
            resource: "fql_statement_bytes",
            limit: MAX_STATEMENT_BYTES as u64,
            observed: u64::try_from(input.len()).unwrap_or(u64::MAX),
        });
    }
    let source = input.as_bytes();
    let mut offset = 0;
    let mut tokens = Vec::new();
    while offset < source.len() {
        if source[offset].is_ascii_whitespace() {
            offset += 1;
            continue;
        }
        if tokens.len() >= MAX_TOKENS {
            return Err(Error::ResourceLimit {
                resource: "fql_tokens",
                limit: MAX_TOKENS as u64,
                observed: u64::try_from(tokens.len().saturating_add(1)).unwrap_or(u64::MAX),
            });
        }
        match source[offset] {
            b';' => {
                tokens.push(Token::Semicolon);
                offset += 1;
            }
            b'\'' => tokens.push(Token::Literal(read_quoted(source, &mut offset)?)),
            b'x' | b'X' if source.get(offset + 1) == Some(&b'\'') => {
                tokens.push(Token::Literal(read_hex(source, &mut offset)?));
            }
            _ => {
                let start = offset;
                while offset < source.len()
                    && !source[offset].is_ascii_whitespace()
                    && source[offset] != b';'
                    && source[offset] != b'\''
                {
                    offset += 1;
                }
                let raw = std::str::from_utf8(&source[start..offset])
                    .map_err(|_| syntax("некорректный UTF-8 в bare token"))?;
                tokens.push(Token::Word(raw.to_owned()));
            }
        }
    }
    Ok(tokens)
}

struct Cursor<'a> {
    tokens: &'a [Token],
    index: usize,
}

impl<'a> Cursor<'a> {
    fn new(tokens: &'a [Token]) -> Self {
        Self { tokens, index: 0 }
    }

    fn next(&mut self) -> Result<&'a Token> {
        let token = self
            .tokens
            .get(self.index)
            .ok_or_else(|| syntax("недостаточно аргументов"))?;
        self.index += 1;
        Ok(token)
    }

    fn word(&mut self) -> Result<String> {
        match self.next()? {
            Token::Word(word) => Ok(word.clone()),
            _ => Err(syntax("ожидается имя/числовой ID без кавычек")),
        }
    }

    fn bytes(&mut self) -> Result<Vec<u8>> {
        match self.next()? {
            Token::Word(word) => Ok(word.as_bytes().to_vec()),
            Token::Literal(bytes) => Ok(bytes.clone()),
            Token::Semicolon => Err(syntax("ожидается key/value literal")),
        }
    }

    fn eat(&mut self, keyword: &str) -> bool {
        if let Some(Token::Word(word)) = self.tokens.get(self.index)
            && word.eq_ignore_ascii_case(keyword)
        {
            self.index += 1;
            return true;
        }
        false
    }

    fn expect(&mut self, keyword: &str) -> Result<()> {
        if self.eat(keyword) {
            Ok(())
        } else {
            Err(syntax(format!("ожидается keyword {keyword}")))
        }
    }

    fn finished(&self) -> Result<()> {
        if self.index == self.tokens.len() {
            Ok(())
        } else {
            Err(syntax("лишние аргументы или несколько statements в одной строке"))
        }
    }
}

fn parse_tokens(tokens: &[Token]) -> Result<Statement> {
    let mut cursor = Cursor::new(tokens);
    let command = cursor.word()?.to_ascii_uppercase();
    let statement = match command.as_str() {
        "SHOW" => {
            cursor.expect("FAMILIES")?;
            Statement::ShowFamilies
        }
        "CREATE" => {
            cursor.expect("FAMILY")?;
            Statement::CreateFamily(cursor.word()?)
        }
        "DROP" => {
            cursor.expect("FAMILY")?;
            Statement::DropFamily(cursor.word()?)
        }
        "USE" => Statement::Use(cursor.word()?),
        "PUT" => Statement::Put(cursor.bytes()?, cursor.bytes()?),
        "GET" => Statement::Get(cursor.bytes()?),
        "DELETE" => Statement::Delete(cursor.bytes()?),
        "SCAN" => {
            let mut from = None;
            let mut to = None;
            let mut limit = DEFAULT_SCAN_LIMIT;
            let mut seen_limit = false;
            while cursor.index < cursor.tokens.len() {
                if cursor.eat("FROM") {
                    if from.is_some() {
                        return Err(syntax("повторный FROM"));
                    }
                    from = Some(cursor.bytes()?);
                } else if cursor.eat("TO") {
                    if to.is_some() {
                        return Err(syntax("повторный TO"));
                    }
                    to = Some(cursor.bytes()?);
                } else if cursor.eat("LIMIT") {
                    if seen_limit {
                        return Err(syntax("повторный LIMIT"));
                    }
                    let number = cursor.word()?;
                    limit = number
                        .parse::<usize>()
                        .map_err(|_| syntax("LIMIT должен быть целым неотрицательным числом"))?;
                    if limit > MAX_SCAN_LIMIT {
                        return Err(Error::ResourceLimit {
                            resource: "fql_scan_limit",
                            limit: MAX_SCAN_LIMIT as u64,
                            observed: u64::try_from(limit).unwrap_or(u64::MAX),
                        });
                    }
                    seen_limit = true;
                } else {
                    return Err(syntax("SCAN принимает только FROM, TO и LIMIT"));
                }
            }
            if from.as_ref().zip(to.as_ref()).is_some_and(|(a, b)| a > b) {
                return Err(syntax("SCAN FROM не может быть больше TO"));
            }
            Statement::Scan { from, to, limit }
        }
        "BEGIN" => Statement::Begin,
        "COMMIT" => Statement::Commit,
        "ROLLBACK" => Statement::Rollback,
        "HELP" => Statement::Help,
        "EXIT" | "QUIT" => Statement::Exit,
        _ => return Err(syntax(format!("неизвестная FQL команда: {command}"))),
    };
    cursor.finished()?;
    Ok(statement)
}

/// FQL V1: ровно одно statement на строку; точка с запятой допускается в конце.
/// Пустые строки и полноcтрочные комментарии не меняют состояние.
pub(crate) fn parse(input: &str) -> Result<Option<Statement>> {
    if input.trim().is_empty() {
        return Ok(None);
    }
    let trimmed = input.trim_start();
    if trimmed.starts_with('#') || trimmed.starts_with("--") {
        return Ok(None);
    }
    let mut tokens = lex(input)?;
    if tokens.last() == Some(&Token::Semicolon) {
        tokens.pop();
    }
    if tokens.is_empty() {
        return Err(syntax("пустой statement перед ';'"));
    }
    if tokens.contains(&Token::Semicolon) {
        return Err(syntax("в одной строке разрешён только один statement"));
    }
    parse_tokens(&tokens).map(Some)
}

pub(crate) const HELP: &str = "\
FQL V1 — одна команда в строке; финальная ';' необязательна
  SHOW FAMILIES
  CREATE FAMILY <name>           DROP FAMILY <name|id>
  USE <name|id>
  PUT <key> <value>              GET <key>                DELETE <key>
  SCAN [FROM <key>] [TO <key>] [LIMIT <0..10000>]
  BEGIN                          COMMIT                   ROLLBACK
  HELP                           EXIT
  Literals: 'text with spaces', '\\x00\\xFF', X'00ff'
  Ограничения: транзакции только внутри одной family; SCAN запрещён до COMMIT.
  Script: команды выполняются последовательно; файл целиком не атомарен.";

pub(crate) struct Session {
    database: ColumnFamilies,
    active: ColumnFamilyId,
    transaction: Option<Transaction>,
    readonly: bool,
}

impl Session {
    pub(crate) fn new(database: ColumnFamilies, readonly: bool) -> Self {
        Self {
            database,
            active: ColumnFamilyId::DEFAULT,
            transaction: None,
            readonly,
        }
    }

    pub(crate) fn active_name(&self) -> Result<String> {
        self.database
            .list()?
            .into_iter()
            .find(|info| info.id == self.active)
            .map(|info| info.name)
            .ok_or_else(|| syntax("активная Column Family удалена"))
    }

    fn resolve(&self, selector: &str) -> Result<ColumnFamilyId> {
        let families = self.database.list()?;
        if let Ok(value) = selector.parse::<u32>() {
            let id = ColumnFamilyId::new(value);
            return families
                .iter()
                .find(|info| info.id == id)
                .map(|info| info.id)
                .ok_or_else(|| syntax(format!("Column Family ID {value} не существует")));
        }
        families
            .iter()
            .find(|info| info.name == selector || (selector.eq_ignore_ascii_case("default") && info.id == ColumnFamilyId::DEFAULT))
            .map(|info| info.id)
            .ok_or_else(|| syntax(format!("Column Family '{selector}' не существует")))
    }

    fn deny_active_transaction(&self) -> Result<()> {
        if self.transaction.is_some() {
            Err(syntax("сначала COMMIT или ROLLBACK активной транзакции"))
        } else {
            Ok(())
        }
    }

    fn writable(&self) -> Result<()> {
        if self.readonly {
            Err(Error::ReadOnly)
        } else {
            Ok(())
        }
    }

    /// Исполняет полностью разобранный statement, после проверки всех аргументов.
    pub(crate) fn execute(&mut self, statement: Statement, output: &mut impl Write) -> Result<Flow> {
        match statement {
            Statement::ShowFamilies => {
                for family in self.database.list()? {
                    writeln!(output, "{}\t{}", family.id.get(), family.name)
                        .map_err(|source| Error::io("write_fql_output", None, source))?;
                }
            }
            Statement::CreateFamily(name) => {
                self.deny_active_transaction()?;
                self.writable()?;
                let id = self.database.create(&name)?;
                writeln!(output, "{}", id.get())
                    .map_err(|source| Error::io("write_fql_output", None, source))?;
            }
            Statement::DropFamily(name) => {
                self.deny_active_transaction()?;
                self.writable()?;
                let id = self.resolve(&name)?;
                self.database.drop_family(id)?;
                if self.active == id {
                    self.active = ColumnFamilyId::DEFAULT;
                }
                writeln!(output, "OK")
                    .map_err(|source| Error::io("write_fql_output", None, source))?;
            }
            Statement::Use(name) => {
                self.deny_active_transaction()?;
                let id = self.resolve(&name)?;
                self.active = id;
                writeln!(output, "USING\t{}\t{}", id.get(), name)
                    .map_err(|source| Error::io("write_fql_output", None, source))?;
            }
            Statement::Put(key, value) => {
                self.writable()?;
                if let Some(transaction) = &mut self.transaction {
                    transaction.put(&key, &value)?;
                    writeln!(output, "QUEUED")
                        .map_err(|source| Error::io("write_fql_output", None, source))?;
                } else {
                    self.database.put(self.active, key, value, WriteOptions::sync())?;
                    writeln!(output, "OK")
                        .map_err(|source| Error::io("write_fql_output", None, source))?;
                }
            }
            Statement::Get(key) => {
                let value = if let Some(transaction) = &mut self.transaction {
                    transaction.get(&key)?
                } else {
                    self.database.get(self.active, &key)?
                };
                match value {
                    Some(value) => writeln!(output, "{}", display_bytes(&value)),
                    None => writeln!(output, "NULL"),
                }
                .map_err(|source| Error::io("write_fql_output", None, source))?;
            }
            Statement::Delete(key) => {
                self.writable()?;
                if let Some(transaction) = &mut self.transaction {
                    transaction.delete(&key)?;
                    writeln!(output, "QUEUED")
                        .map_err(|source| Error::io("write_fql_output", None, source))?;
                } else {
                    self.database.delete(self.active, key, WriteOptions::sync())?;
                    writeln!(output, "OK")
                        .map_err(|source| Error::io("write_fql_output", None, source))?;
                }
            }
            Statement::Scan { from, to, limit } => {
                self.deny_active_transaction()?;
                let options = ReadOptions::default().with_limit(limit);
                let iterator = match (from, to) {
                    (None, None) => self.database.scan(self.active, .., options)?,
                    (Some(start), None) => self.database.scan(self.active, start.., options)?,
                    (None, Some(end)) => self.database.scan(self.active, ..end, options)?,
                    (Some(start), Some(end)) => {
                        self.database.scan(self.active, start..end, options)?
                    }
                };
                for entry in iterator {
                    let (key, value) = entry?;
                    writeln!(output, "{}\t{}", display_bytes(&key), display_bytes(&value))
                        .map_err(|source| Error::io("write_fql_output", None, source))?;
                }
            }
            Statement::Begin => {
                self.deny_active_transaction()?;
                self.writable()?;
                self.transaction = Some(self.database.begin_transaction(self.active)?);
                writeln!(output, "BEGIN")
                    .map_err(|source| Error::io("write_fql_output", None, source))?;
            }
            Statement::Commit => {
                let mut transaction = self.transaction.take()
                    .ok_or_else(|| syntax("COMMIT без активной транзакции"))?;
                transaction.commit(WriteOptions::sync())?;
                writeln!(output, "COMMIT")
                    .map_err(|source| Error::io("write_fql_output", None, source))?;
            }
            Statement::Rollback => {
                let mut transaction = self.transaction.take()
                    .ok_or_else(|| syntax("ROLLBACK без активной транзакции"))?;
                transaction.rollback()?;
                writeln!(output, "ROLLBACK")
                    .map_err(|source| Error::io("write_fql_output", None, source))?;
            }
            Statement::Help => {
                writeln!(output, "{HELP}")
                    .map_err(|source| Error::io("write_fql_output", None, source))?;
            }
            Statement::Exit => return Ok(Flow::Exit),
        }
        Ok(Flow::Continue)
    }

    /// Неподтверждённые операции откатываются при EOF/EXIT, после чего все DB закрываются.
    pub(crate) fn close(mut self) -> Result<()> {
        if let Some(mut transaction) = self.transaction.take() {
            transaction.rollback()?;
        }
        self.database.close()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(input: &str) -> Result<Statement> {
        parse(input)?.ok_or_else(|| syntax("ожидался statement"))
    }

    #[test]
    fn binary_literals_and_escaped_text_roundtrip() -> Result<()> {
        assert_eq!(
            parsed("PUT 'k\\x00' X'00ff42';")?,
            Statement::Put(vec![b'k', 0], vec![0, 255, b'B'])
        );
        assert_eq!(
            parsed("PUT 'spaces and \\'quotes\\'' 'line\\nnext'")?,
            Statement::Put(b"spaces and 'quotes'".to_vec(), b"line\nnext".to_vec())
        );
        for value in [b"abc".as_slice(), b"a\x00\xffb".as_slice(), b"'\n\\\t".as_slice()] {
            let literal = display_bytes(value);
            assert_eq!(parsed(&format!("GET {literal}"))?, Statement::Get(value.to_vec()));
        }
        Ok(())
    }

    #[test]
    fn invalid_queries_are_rejected_before_execution() {
        for query in [
            "PUT key",
            "PUT key value unexpected",
            "PUT key value; DELETE key",
            "GET key; ;",
            "SCAN LIMIT 10001",
            "SCAN LIMIT -1",
            "SCAN LIMIT nan",
            "SCAN FROM z TO a",
            "SCAN FROM a FROM b",
            "SCAN TO z TO y",
            "SCAN LIMIT 2 LIMIT 3",
            "BEGIN unexpected",
            "SHOW TABLES",
            "CREATE FAMILY",
            "GET 'unfinished",
            "PUT a '\\x0'",
            "PUT a '\\y'",
            "PUT a X'0'",
            "PUT a X'zz'",
            "UNKNOWN",
        ] {
            assert!(parse(query).is_err(), "accepted invalid query: {query}");
        }
    }

    #[test]
    fn comments_are_noops_and_keywords_are_case_insensitive() -> Result<()> {
        assert_eq!(parse("  # comment")?, None);
        assert_eq!(parse("-- comment")?, None);
        assert_eq!(parse("    ")?, None);
        assert_eq!(parsed("sCaN FrOm a tO z LiMit 2;")?,
            Statement::Scan { from: Some(b"a".to_vec()), to: Some(b"z".to_vec()), limit: 2 });
        assert_eq!(parsed("USE 5")?, Statement::Use("5".to_owned()));
        Ok(())
    }

    #[test]
    fn statements_and_token_limits_are_bounded() {
        let long = "x".repeat(MAX_STATEMENT_BYTES + 1);
        assert!(matches!(parse(&long), Err(Error::ResourceLimit { resource: "fql_statement_bytes", .. })));
        let many = format!("GET {}", "a ".repeat(MAX_TOKENS + 1));
        assert!(matches!(parse(&many), Err(Error::ResourceLimit { resource: "fql_tokens", .. })));
    }
}
