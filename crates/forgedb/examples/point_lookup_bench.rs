//! Повторяемый point-lookup benchmark без внешних зависимостей.
//!
//! cargo run --locked --release -p forgedb --example point_lookup_bench -- 8192 50000
//! Аргументы: количество ключей и число get за проход.

use std::fs;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use forgedb::{Database, Error, Options, Result, WriteBatch, WriteOptions};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let keys = parse_positive(args.next(), 8192, "keys")?;
    let reads = parse_positive(args.next(), 50_000, "reads")?;
    if args.next().is_some() {
        return Err(Error::invalid_argument(
            "arguments",
            "ожидаются только [keys] [reads]",
        ));
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "forgedb-lookup-bench-{}-{stamp}",
        std::process::id()
    ));

    let database = Database::open(
        &directory,
        Options {
            data_block_size: 1024,
            l0_compaction_trigger: 64,
            l0_slowdown_trigger: 65,
            l0_stop_trigger: 66,
            ..Options::default()
        },
    )?;

    let setup_start = Instant::now();
    for start in (0..keys).step_by(512) {
        let mut batch = WriteBatch::new();
        for number in start..(start + 512).min(keys) {
            batch.put(
                format!("key-{number:08}").into_bytes(),
                vec![u8::try_from(number % 251).unwrap_or_default(); 256],
            )?;
        }
        database.write(batch, WriteOptions::no_sync())?;
        database.flush()?;
    }
    let setup = setup_start.elapsed();

    let cold = run_reads(&database, keys, reads)?;
    let after_cold = database.cache_stats()?;
    let warm = run_reads(&database, keys, reads)?;
    let after_warm = database.cache_stats()?;

    println!("keys={keys} reads_per_pass={reads}");
    println!(
        "setup={setup:?} cold={:?} warm={:?}",
        cold.elapsed, warm.elapsed
    );
    print_latency("cold", reads, &cold);
    print_latency("warm", reads, &warm);
    println!(
        "cache: hits={} misses={} evictions={} bypasses={} resident_blocks={} resident_bytes={} capacity_bytes={}",
        after_warm.hits,
        after_warm.misses,
        after_warm.evictions,
        after_warm.bypasses,
        after_warm.resident_blocks,
        after_warm.resident_bytes,
        after_warm.capacity_bytes
    );
    println!(
        "cold_pass: hits={} misses={}",
        after_cold.hits, after_cold.misses
    );
    println!("Измерения локальные; сравнивайте одинаковые конфигурации и среды.");

    database.close()?;
    drop(database);
    fs::remove_dir_all(&directory)
        .map_err(|source| Error::io("remove_benchmark_directory", Some(directory), source))?;
    Ok(())
}

fn parse_positive(raw: Option<String>, default: usize, field: &'static str) -> Result<usize> {
    let number = match raw {
        Some(text) => text
            .parse::<usize>()
            .map_err(|_| Error::invalid_argument(field, "ожидается целое число"))?,
        None => default,
    };
    if number == 0 {
        return Err(Error::invalid_argument(
            field,
            "значение должно быть больше нуля",
        ));
    }
    Ok(number)
}

/// Latency percentiles are empirical observations of this run, not SLA numbers.
struct ReadMetrics {
    elapsed: std::time::Duration,
    p50_ns: u128,
    p95_ns: u128,
    p99_ns: u128,
}

fn print_latency(pass: &str, reads: usize, metrics: &ReadMetrics) {
    let throughput = (reads as f64) / metrics.elapsed.as_secs_f64().max(f64::EPSILON);
    println!(
        "{pass}: throughput_ops_sec={throughput:.2} p50_ns={} p95_ns={} p99_ns={}",
        metrics.p50_ns, metrics.p95_ns, metrics.p99_ns
    );
}

fn percentile(sorted: &[u128], percent: usize) -> u128 {
    let index = sorted.len().saturating_sub(1).saturating_mul(percent) / 100;
    sorted.get(index).copied().unwrap_or(0)
}

fn run_reads(database: &Database, keys: usize, reads: usize) -> Result<ReadMetrics> {
    let start = Instant::now();
    let mut latencies = Vec::with_capacity(reads);
    for number in 0..reads {
        let index = number.wrapping_mul(7919) % keys;
        let key = format!("key-{index:08}");
        let began = Instant::now();
        let value = database.get(key.as_bytes())?;
        latencies.push(began.elapsed().as_nanos());
        if value.is_none() {
            return Err(Error::InternalInvariant {
                invariant: "I-BENCHMARK-READBACK",
                reason: format!("ключ {key} отсутствует после flush"),
            });
        }
    }
    let elapsed = start.elapsed();
    latencies.sort_unstable();
    Ok(ReadMetrics {
        elapsed,
        p50_ns: percentile(&latencies, 50),
        p95_ns: percentile(&latencies, 95),
        p99_ns: percentile(&latencies, 99),
    })
}
