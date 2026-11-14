//! Повторяемый сценарий холодных/прогретых point reads и streaming scan.
//!
//! cargo run --locked --release -p forgedb --example performance_bench -- 4096 16384
//! Аргументы: число ключей, число GET за проход. Стандартный вывод содержит
//! время и percentiles, но сравнение между запусками требует одной машины,
//! профиля, нагрузки, версии Rust и одинакового набора параметров.

use std::fs;
use std::hint::black_box;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use forgedb::{Database, Error, Options, ReadOptions, Result, WriteBatch, WriteOptions};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let keys = positive(args.next(), 4096, "keys")?;
    let reads = positive(args.next(), 16_384, "reads")?;
    if args.next().is_some() {
        return Err(Error::invalid_argument(
            "arguments",
            "ожидаются [keys] [reads]",
        ));
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "forgedb-performance-bench-{}-{stamp}",
        std::process::id()
    ));
    let options = Options {
        data_block_size: 1024,
        block_cache_size: 0, // измеряем физический SST path, не hits decoded-block cache
        table_cache_capacity: 8,
        l0_compaction_trigger: 128,
        l0_slowdown_trigger: 129,
        l0_stop_trigger: 130,
        ..Options::default()
    };
    let database = Database::open(&directory, options)?;
    let setup = Instant::now();
    for start in (0..keys).step_by(128) {
        let mut batch = WriteBatch::new();
        for i in start..(start + 128).min(keys) {
            batch.put(
                format!("key-{i:08}").into_bytes(),
                vec![u8::try_from(i % 251).unwrap_or(0); 256],
            )?;
        }
        database.write(batch, WriteOptions::no_sync())?;
        database.flush()?;
    }
    let setup_elapsed = setup.elapsed();
    let cold = measure_gets(&database, keys, reads)?;
    let warm = measure_gets(&database, keys, reads)?;
    let scan = Instant::now();
    let mut count = 0_usize;
    let mut bytes = 0_usize;
    for entry in database.scan(.., ReadOptions::default())? {
        let (key, value) = entry?;
        black_box(&key);
        bytes = bytes.saturating_add(value.len());
        count = count.saturating_add(1);
    }
    let scan_elapsed = scan.elapsed();
    if count != keys || bytes != keys.saturating_mul(256) {
        return Err(Error::InternalInvariant {
            invariant: "I-PERF-BENCH-RESULT",
            reason: "scan возвратил неполный или неверный объём данных".to_owned(),
        });
    }
    let handles = database.table_handle_stats()?;
    // Full workload обязателен к проверке реальных evictions, иначе
    // benchmark случайно обходил бы оптимизированный LRU путь.
    if keys.div_ceil(128) > handles.capacity && handles.evictions == 0 {
        return Err(Error::InternalInvariant {
            invariant: "I-PERF-BENCH-LRU",
            reason: "full workload не выполнил ни одного SST handle eviction".to_owned(),
        });
    }
    println!(
        "configuration: keys={keys} reads_pass={reads} value_bytes=256 block_cache_bytes=0 table_cache_capacity=8"
    );
    println!(
        "setup: elapsed_ms={:.3}",
        setup_elapsed.as_secs_f64() * 1000.0
    );
    print_get("get_cold", reads, &cold);
    print_get("get_warm", reads, &warm);
    println!(
        "scan: entries={count} bytes={bytes} elapsed_ms={:.3} entries_per_sec={:.2}",
        scan_elapsed.as_secs_f64() * 1000.0,
        throughput(count, scan_elapsed)
    );
    println!(
        "handle_pool: open={} capacity={} hits={} misses={} evictions={} reopens={}",
        handles.open_handles,
        handles.capacity,
        handles.hits,
        handles.misses,
        handles.evictions,
        handles.reopens
    );
    println!(
        "Используйте одинаковую машину/нагрузку/Rust для сравнения; это локальные измерения, не гарантия ускорения."
    );

    database.close()?;
    drop(database);
    fs::remove_dir_all(&directory)
        .map_err(|source| Error::io("remove_performance_bench", Some(directory), source))?;
    Ok(())
}

fn positive(arg: Option<String>, default: usize, name: &'static str) -> Result<usize> {
    let n = match arg {
        None => default,
        Some(v) => v
            .parse::<usize>()
            .map_err(|_| Error::invalid_argument(name, "нужно положительное число"))?,
    };
    if n == 0 {
        return Err(Error::invalid_argument(
            name,
            "число должно быть больше нуля",
        ));
    }
    Ok(n)
}

fn throughput(n: usize, time: Duration) -> f64 {
    n as f64 / time.as_secs_f64().max(f64::EPSILON)
}

struct Metrics {
    time: Duration,
    p50_ns: u128,
    p95_ns: u128,
    p99_ns: u128,
}

fn measure_gets(db: &Database, keys: usize, reads: usize) -> Result<Metrics> {
    let start = Instant::now();
    let mut samples = Vec::with_capacity(reads);
    for i in 0..reads {
        let key = format!("key-{:08}", i.wrapping_mul(7919) % keys);
        let began = Instant::now();
        let result = db.get(key.as_bytes())?;
        samples.push(began.elapsed().as_nanos());
        if result.is_none() {
            return Err(Error::InternalInvariant {
                invariant: "I-PERF-BENCH-LOOKUP",
                reason: format!("отсутствует key {key}"),
            });
        }
        black_box(result);
    }
    let time = start.elapsed();
    samples.sort_unstable();
    let percentile = |pct: usize| {
        let index = samples.len().saturating_sub(1).saturating_mul(pct) / 100;
        samples.get(index).copied().unwrap_or(0)
    };
    Ok(Metrics {
        time,
        p50_ns: percentile(50),
        p95_ns: percentile(95),
        p99_ns: percentile(99),
    })
}

fn print_get(name: &str, reads: usize, m: &Metrics) {
    println!(
        "{name}: elapsed_ms={:.3} ops_sec={:.2} p50_ns={} p95_ns={} p99_ns={}",
        m.time.as_secs_f64() * 1000.0,
        throughput(reads, m.time),
        m.p50_ns,
        m.p95_ns,
        m.p99_ns
    );
}
