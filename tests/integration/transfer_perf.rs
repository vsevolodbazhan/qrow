//! Report-only, optimized export measurements against disposable real servers.
use anyhow::{Context, Result, ensure};
use qrow::{
    connector::{
        Cancellation, Completion, ConnectionControl, Connector, DatabaseConnector, MetadataRequest,
        QueryState, Secret, Session,
    },
    export,
    logs::ExecutionId,
    model::{
        Batch, Column, DatabaseType, PostgresSslMode, Profile,
        transfer::{IncrementalCollect, Transfer, TransferPreset, TransferSettings},
    },
    tls::Trust,
    worker::{Event, Worker},
};
use std::{
    process::Command,
    sync::{Arc, atomic::AtomicBool},
    thread,
    time::{Duration, Instant},
};

const ROWS: usize = 50_000;
const TIMEOUT: Duration = Duration::from_secs(180);

// Emulate the phase 7 Kyuubi transfer policy on the same pipeline and data.
// This is a behavioral baseline, not a build of an older source revision.
struct Baseline(Box<dyn Session>);
impl Session for Baseline {
    fn execute(&mut self, sql: &str) -> Result<Arc<dyn Cancellation>> {
        self.0.execute(sql)
    }
    fn execute_export_controlled(
        &mut self,
        sql: &str,
        control: &ConnectionControl,
    ) -> Result<Arc<dyn Cancellation>> {
        self.0.execute_export_controlled(sql, control)
    }
    fn configure_export(&mut self, _: &Transfer) -> Result<()> {
        Ok(())
    }
    fn export_fetch_rows(&self) -> usize {
        1000
    }
    fn start_export_fetch(&mut self) -> Result<()> {
        self.0.start_export_fetch()
    }
    fn execute_metadata(&mut self, request: &MetadataRequest) -> Result<Arc<dyn Cancellation>> {
        self.0.execute_metadata(request)
    }
    fn poll(&mut self) -> Result<QueryState> {
        self.0.poll()
    }
    fn columns(&mut self) -> Result<Vec<Column>> {
        self.0.columns()
    }
    fn fetch(&mut self, count: usize) -> Result<Batch> {
        self.0.fetch(count)
    }
    fn finish_execution(&mut self) -> Result<Completion> {
        self.0.finish_execution()
    }
    fn transport_cancellation(&self) -> Option<Arc<dyn Cancellation>> {
        self.0.transport_cancellation()
    }
    fn export_context(&self) -> export::Context {
        self.0.export_context()
    }
    fn progress_percentage(&self) -> Option<f64> {
        self.0.progress_percentage()
    }
    fn result_limited(&self) -> bool {
        self.0.result_limited()
    }
    fn close_operation(&mut self) -> Result<()> {
        self.0.close_operation()
    }
    fn execute_keep_alive(&mut self, sql: &str) -> Result<Arc<dyn Cancellation>> {
        self.0.execute_keep_alive(sql)
    }
    fn close_keep_alive(&mut self) -> Result<()> {
        self.0.close_keep_alive()
    }
    fn close(&mut self) -> Result<()> {
        self.0.close()
    }
}
struct MeasuredConnector {
    inner: DatabaseConnector,
    baseline: bool,
}
impl Connector for MeasuredConnector {
    fn connect(&self, p: &Profile, secret: Secret) -> Result<Box<dyn Session>> {
        let session = self.inner.connect(p, secret)?;
        Ok(if self.baseline {
            Box::new(Baseline(session))
        } else {
            session
        })
    }
    fn connect_controlled(
        &self,
        p: &Profile,
        secret: Secret,
        control: &ConnectionControl,
    ) -> Result<Box<dyn Session>> {
        let session = self.inner.connect_controlled(p, secret, control)?;
        Ok(if self.baseline {
            Box::new(Baseline(session))
        } else {
            session
        })
    }
}

fn fixture(engine: &str, case: &str) -> Result<(Profile, DatabaseConnector, String)> {
    let mut profile = Profile {
        name: "Transfer measurement".into(),
        host: "127.0.0.1".into(),
        username: "qrow".into(),
        ..Profile::default()
    };
    profile.transfer = Transfer {
        preset: match case {
            "baseline" | "legacy_policy" => TransferPreset::Balanced,
            "conservative" => TransferPreset::Conservative,
            "balanced" => TransferPreset::Balanced,
            "fast" => TransferPreset::Fast,
            "custom" => TransferPreset::Custom,
            _ => anyhow::bail!("Unknown transfer case"),
        },
        custom: TransferSettings {
            incremental_collect: IncrementalCollect::On,
            request_mib: 8,
            speed_limit_mb: 0,
            concurrent_exports: 2,
        },
    };
    let (trust, sql) = match engine {
        "kyuubi" => {
            ensure!(
                std::env::var("QROW_E2E_PROJECT")?.starts_with("qrow-e2e-"),
                "Not a disposable fixture"
            );
            profile.port = std::env::var("QROW_E2E_PORT")?.parse()?;
            profile.database = "default".into();
            profile.parameters.extend([
                ("kyuubi.engine.share.level".into(), "CONNECTION".into()),
                ("spark.sql.catalogImplementation".into(), "in-memory".into()),
                ("kyuubi.session.engine.idle.timeout".into(), "PT1S".into()),
                (
                    "kyuubi.engine.spark.operation.incremental.collect".into(),
                    "false".into(),
                ),
            ]);
            (
                Trust::default(),
                format!(
                    "SELECT id, concat(lpad(CAST(id AS STRING),10,'0'),repeat('x',1014)) AS payload FROM range(0,{ROWS},1,4)"
                ),
            )
        }
        "postgres" => {
            ensure!(
                std::env::var("QROW_POSTGRES_FIXTURE")?.starts_with("qrow-e2e-postgres-"),
                "Not a disposable fixture"
            );
            profile.database_type = DatabaseType::Postgres;
            profile.port = std::env::var("QROW_POSTGRES_PORT")?.parse()?;
            profile.database = "qrow".into();
            (
                Trust::default(),
                format!(
                    "SELECT id::bigint, lpad(id::text,10,'0') || repeat('x',1014) AS payload FROM generate_series(0,{}) AS t(id)",
                    ROWS - 1
                ),
            )
        }
        "trino" => {
            ensure!(
                std::env::var("QROW_TRINO_FIXTURE")?.starts_with("qrow-e2e-trino-"),
                "Not a disposable fixture"
            );
            profile.database_type = DatabaseType::Trino;
            profile.host = "localhost".into();
            profile.port = std::env::var("QROW_TRINO_PORT")?.parse()?;
            profile.tls = true;
            profile.database = "tpch".into();
            profile.trino_schema = "tiny".into();
            (
                Trust::from_pem(&std::fs::read(std::env::var("QROW_TRINO_CA")?)?)?,
                format!(
                    "SELECT orderkey AS id, lpad(CAST(orderkey AS varchar),10,'0') || rpad('',1014,'x') AS payload FROM tpch.sf1.orders WHERE orderkey <= 200000 ORDER BY orderkey LIMIT {ROWS}"
                ),
            )
        }
        _ => anyhow::bail!("Unknown engine"),
    };
    Ok((profile, DatabaseConnector::new(trust), sql))
}

fn driver_pids() -> Result<Vec<(String, String)>> {
    let project = std::env::var("QROW_E2E_PROJECT")?;
    ensure!(project.starts_with("qrow-e2e-"), "Not a disposable fixture");
    let output = Command::new("docker").args(["exec", &format!("{project}-kyuubi-1"), "sh", "-c",
        "for dir in /proc/[0-9]*; do [ \"$(cat \"$dir/comm\" 2>/dev/null)\" = java ] || continue; case \"$(tr '\\000' ' ' < \"$dir/cmdline\")\" in *org.apache.kyuubi.engine.spark.SparkSQLEngine*) cat \"$dir/stat\";; esac; done"]).output()?;
    ensure!(
        output.status.success(),
        "Cannot identify the Spark driver: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)?
        .lines()
        .map(|line| {
            let (before, after) = line.rsplit_once(')').context("Invalid driver stat")?;
            Ok((
                before.split_whitespace().next().unwrap().into(),
                after
                    .split_whitespace()
                    .nth(19)
                    .context("Missing process start time")?
                    .into(),
            ))
        })
        .collect()
}
fn driver_peak((pid, start): &(String, String)) -> Result<f64> {
    ensure!(
        pid.bytes().all(|b| b.is_ascii_digit()),
        "Invalid driver PID"
    );
    let project = std::env::var("QROW_E2E_PROJECT")?;
    let output = Command::new("docker")
        .args([
            "exec",
            &format!("{project}-kyuubi-1"),
            "cat",
            &format!("/proc/{pid}/stat"),
            &format!("/proc/{pid}/status"),
        ])
        .output()?;
    ensure!(output.status.success(), "Cannot read Spark driver peak RSS");
    let status = String::from_utf8(output.stdout)?;
    let current = status
        .lines()
        .next()
        .unwrap()
        .rsplit_once(')')
        .context("Invalid driver stat")?
        .1
        .split_whitespace()
        .nth(19)
        .context("Missing process start time")?;
    ensure!(current == start, "Driver PID was reused");
    let kb: f64 = status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))
        .context("Driver has no VmHWM")?
        .split_whitespace()
        .next()
        .unwrap()
        .parse()?;
    Ok(kb / 1024.)
}

fn clear_fixture_driver() -> Result<()> {
    let project = std::env::var("QROW_E2E_PROJECT")?;
    ensure!(project.starts_with("qrow-e2e-"), "Not a disposable fixture");
    if !driver_pids()?.is_empty() {
        let output = Command::new("docker")
            .args([
                "exec",
                &format!("{project}-kyuubi-1"),
                "pkill",
                "-9",
                "-f",
                "[o]rg.apache.kyuubi.engine.spark.SparkSQLEngine",
            ])
            .output()?;
        ensure!(
            matches!(output.status.code(), Some(0 | 1)),
            "Cannot stop the disposable fixture driver"
        );
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while !driver_pids()?.is_empty() {
        ensure!(Instant::now() < deadline, "Fixture driver did not stop");
        thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

fn export_sample(engine: &str, case: &str) -> Result<()> {
    let (profile, inner, sql) = fixture(engine, case)?;
    let driver_before =
        if engine == "kyuubi" && std::env::var("QROW_E2E_RUNTIME").as_deref() == Ok("docker") {
            Some(driver_pids()?)
        } else {
            None
        };
    let worker = Worker::with_connector(
        Arc::new(|| {}),
        Arc::new(MeasuredConnector {
            inner,
            baseline: matches!(case, "baseline" | "legacy_policy"),
        }),
        Arc::new(|_| Ok(Secret::password("qrow-test-password"))),
    );
    let deadline = Instant::now() + TIMEOUT;
    worker.run(profile.clone(), "SELECT 42".into());
    loop {
        match worker
            .events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))?
        {
            Event::Ready { .. } => break,
            Event::Error { message, .. } => anyhow::bail!("{message}"),
            _ => {}
        }
    }
    let driver = if let Some(before) = driver_before {
        let fresh: Vec<_> = driver_pids()?
            .into_iter()
            .filter(|pid| !before.contains(pid))
            .collect();
        ensure!(
            fresh.len() == 1,
            "Expected exactly one new CONNECTION driver, found {fresh:?}"
        );
        Some(fresh[0].clone())
    } else {
        None
    };
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("transfer.csv");
    let output = path.clone();
    let jobs = export::Jobs::default();
    let cancel = Arc::new(AtomicBool::new(false));
    let started = Instant::now();
    let download = worker.run_and_export(
        profile.clone(),
        sql,
        ExecutionId(9001),
        worker.session_generation(&profile),
        &jobs,
        cancel.clone(),
    )?;
    let guard = jobs.register_for_profile(
        cancel.clone(),
        None,
        profile.id,
        profile.transfer.settings().concurrent_exports,
    )?;
    let writer = thread::spawn(move || -> std::io::Result<(usize, u64)> {
        let _guard = guard;
        let spool = download.wait_spool()?;
        let rows = export::save(&output, &cancel, |out| {
            export::stream::write(out, &spool, &export::Settings::default(), &cancel)
        })?;
        Ok((rows, spool.bytes()))
    });
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match worker
            .events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))?
        {
            Event::Downloaded { .. } => break,
            Event::DownloadFailed { message, .. } | Event::Error { message, .. } => {
                anyhow::bail!("{message}")
            }
            _ => {}
        }
    }
    let (rows, spool_bytes) = writer.join().unwrap()?;
    ensure!(rows == ROWS, "Incomplete export");
    ensure!(
        jobs.cancel_and_wait(Duration::from_secs(5)),
        "Export cleanup did not finish"
    );
    let elapsed = started.elapsed().as_secs_f64();
    let bytes = std::fs::metadata(&path)?.len();
    let spark = driver.map(|pid| driver_peak(&pid)).transpose()?;
    let mut count = 0;
    let mut ids = std::collections::HashSet::new();
    for record in csv::Reader::from_path(path)?.records() {
        let record = record?;
        ensure!(record.len() == 2, "Unexpected export column count");
        let id = record[0].parse::<i64>()?;
        ensure!(ids.insert(id), "Duplicate exported ID");
        if engine != "trino" {
            ensure!((0..ROWS as i64).contains(&id), "Unexpected exported ID");
        }
        ensure!(
            record.len() == 2
                && record[1].len() == 1024
                && record[1].starts_with(&format!("{id:010}"))
                && record[1][10..].bytes().all(|byte| byte == b'x'),
            "Invalid exported row"
        );
        count += 1;
    }
    ensure!(
        count == ROWS && jobs.active_count() == 0,
        "Incomplete output or leaked admission"
    );
    worker.shutdown();
    worker.wait_for_shutdown(Duration::from_secs(5));
    ensure!(jobs.active_count() == 0, "Export cleanup did not finish");
    println!(
        "QROW_TRANSFER_SAMPLE {}",
        serde_json::json!({"seconds": elapsed, "bytes": bytes, "spool_bytes":spool_bytes, "spark_mib": spark})
    );
    Ok(())
}

fn copy_sample(case: &str) -> Result<()> {
    use futures_util::{StreamExt, pin_mut};
    use std::io::Write;
    let (mut profile, connector, _) = fixture("postgres", "baseline")?;
    let sql = format!(
        "SELECT id::bigint, lpad(id::text,10,'0') || repeat('x',1014), CASE WHEN id % 7 = 0 THEN NULL ELSE E'comma,\\n\"unicode😀\"' END FROM generate_series(0,{}) AS t(id) ORDER BY id",
        ROWS - 1
    );
    // Both protocol measurements use unencrypted localhost transport.
    profile.postgres_ssl_mode = Some(PostgresSslMode::Disable);
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("protocol.csv");
    let cancel = AtomicBool::new(false);
    let seconds = if case == "protocol" {
        let mut session = connector.connect(&profile, Secret::password("qrow-test-password"))?;
        let started = Instant::now();
        session.execute_export(&sql)?;
        qrow::connector::wait_for_result(&mut *session, Some(Instant::now() + TIMEOUT))?;
        session.columns()?;
        export::save(&path, &cancel, |out| {
            let mut writer = csv::Writer::from_writer(out);
            let mut rows = 0;
            loop {
                let batch = session.fetch(1000).map_err(std::io::Error::other)?;
                for row in &batch.rows {
                    writer.write_record(row.iter().map(|value| value.as_deref().unwrap_or("")))?;
                }
                rows += batch.rows.len();
                if batch.rows.is_empty() {
                    break;
                }
            }
            writer.flush()?;
            Ok(rows)
        })?;
        session.finish_execution()?;
        let elapsed = started.elapsed().as_secs_f64();
        session.close()?;
        elapsed
    } else {
        ensure!(case == "copy_csv", "Unknown COPY case");
        let runtime = tokio::runtime::Runtime::new()?;
        let (client, connection) = runtime.block_on(async {
            let mut config = tokio_postgres::Config::new();
            config
                .host(&profile.host)
                .port(profile.port)
                .user(&profile.username)
                .password("qrow-test-password")
                .dbname(&profile.database);
            let (client, connection) = config.connect(tokio_postgres::NoTls).await?;
            Ok::<_, anyhow::Error>((client, tokio::spawn(connection)))
        })?;
        let started = Instant::now();
        // COPY generates CSV on the server; the normal arm formats typed rows locally.
        export::save(&path, &cancel, |out| {
            runtime.block_on(async {
                let stream = client
                    .copy_out(&format!(
                        "COPY ({sql}) TO STDOUT WITH (FORMAT CSV, HEADER false)"
                    ))
                    .await
                    .map_err(std::io::Error::other)?;
                pin_mut!(stream);
                while let Some(batch) = stream.next().await {
                    out.write_all(&batch.map_err(std::io::Error::other)?)?;
                }
                Ok::<_, std::io::Error>(ROWS)
            })
        })?;
        let elapsed = started.elapsed().as_secs_f64();
        drop(client);
        runtime.block_on(connection)??;
        elapsed
    };
    let mut rows = 0;
    for record in csv::ReaderBuilder::new()
        .has_headers(false)
        .from_path(&path)?
        .records()
    {
        let record = record?;
        ensure!(record.len() == 3, "Unexpected column count");
        ensure!(record[0] == rows.to_string(), "Unexpected row order");
        ensure!(
            record[1] == format!("{rows:010}{}", "x".repeat(1014)),
            "Unexpected payload"
        );
        ensure!(
            &record[2]
                == if rows % 7 == 0 {
                    ""
                } else {
                    "comma,\n\"unicode😀\""
                },
            "Unexpected NULL or escaped value"
        );
        rows += 1;
    }
    ensure!(rows == ROWS, "Unexpected protocol result: {rows} rows");
    let bytes = std::fs::metadata(path)?.len();
    println!(
        "QROW_TRANSFER_SAMPLE {}",
        serde_json::json!({"seconds":seconds,"bytes":bytes,"spark_mib":null})
    );
    Ok(())
}

#[test]
#[ignore = "internal fresh-process performance sample"]
fn sample() -> Result<()> {
    let engine = std::env::var("QROW_TRANSFER_ENGINE")?;
    let case = std::env::var("QROW_TRANSFER_CASE")?;
    if case == "protocol" || case == "copy_csv" {
        copy_sample(&case)
    } else {
        export_sample(&engine, &case)
    }
}
fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}
fn report(probe: &str, value: f64, unit: &str) {
    println!(
        "QROW_PERF {}",
        serde_json::json!({"probe":probe,"value":value,"unit":unit,"budget":100000.})
    );
}
#[derive(Default)]
struct Measurements {
    latency: Vec<f64>,
    memory: Vec<f64>,
    spark: Vec<f64>,
}

fn measure(engine: &str) -> Result<()> {
    let executable = std::env::current_exe()?;
    let baseline = if std::env::var_os("QROW_TRANSFER_BASELINE_BIN").is_some() {
        "baseline"
    } else {
        "legacy_policy"
    };
    let mut cases = vec![baseline, "conservative", "balanced", "fast", "custom"];
    if engine == "postgres" {
        cases.extend(["protocol", "copy_csv"]);
    }
    let mut measurements: std::collections::BTreeMap<&str, Measurements> = cases
        .iter()
        .map(|&case| (case, Measurements::default()))
        .collect();
    for round in 0..4 {
        let length = cases.len();
        cases.rotate_left(round % length);
        for &case in &cases {
            // The Docker worker has two cores, all held by readiness's USER
            // engine. Each sample must own a fresh CONNECTION engine.
            if engine == "kyuubi" && std::env::var("QROW_E2E_RUNTIME").as_deref() == Ok("docker") {
                clear_fixture_driver()?;
            }
            let mut command = Command::new("/usr/bin/time");
            command.arg(if cfg!(target_os = "macos") {
                "-l"
            } else {
                "-v"
            });
            if case == "baseline"
                && let Ok(baseline) = std::env::var("QROW_TRANSFER_BASELINE_BIN")
            {
                command.arg(baseline).args([
                    "--exact",
                    "transfer_perf::sample",
                    "--ignored",
                    "--nocapture",
                ]);
            } else {
                command.arg(&executable).args([
                    "--exact",
                    "transfer_perf::sample",
                    "--ignored",
                    "--nocapture",
                ]);
            }
            let output = command
                .env("LC_ALL", "C")
                .env("RUST_TEST_THREADS", "1")
                .env("QROW_TRANSFER_ENGINE", engine)
                .env("QROW_TRANSFER_CASE", case)
                .output()?;
            ensure!(
                output.status.success(),
                "{engine}/{case} sample failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let stdout = String::from_utf8(output.stdout)?;
            let sample: serde_json::Value = serde_json::from_str(
                stdout
                    .lines()
                    .find_map(|line| {
                        line.split_once("QROW_TRANSFER_SAMPLE ")
                            .map(|(_, json)| json)
                    })
                    .context("Missing sample result")?,
            )?;
            let stderr = String::from_utf8(output.stderr)?;
            let rss = stderr
                .lines()
                .find(|line| {
                    line.to_ascii_lowercase()
                        .contains("maximum resident set size")
                })
                .context("OS did not report client peak RSS")?;
            let rss: f64 = if cfg!(target_os = "macos") {
                rss.split_whitespace().next().unwrap().parse::<f64>()? / 1048576.
            } else {
                rss.rsplit(':').next().unwrap().trim().parse::<f64>()? / 1024.
            };
            println!(
                "QROW_TRANSFER_RAW {}",
                serde_json::json!({"engine":engine,"case":case,"round":round,"sample":sample,"client_peak_rss_mib":rss,"baseline_revision": std::env::var("QROW_TRANSFER_BASELINE_REVISION").ok()})
            );
            if round > 0 {
                let measurement = measurements.get_mut(case).unwrap();
                measurement.latency.push(
                    sample["seconds"].as_f64().unwrap() * 1000.
                        / (sample["bytes"].as_f64().unwrap() / 1048576.),
                );
                measurement.memory.push(rss);
                if let Some(value) = sample["spark_mib"].as_f64() {
                    measurement.spark.push(value);
                }
            }
        }
    }
    for (
        case,
        Measurements {
            mut latency,
            memory,
            spark,
        },
    ) in measurements
    {
        report(
            &format!("e2e.export.{engine}.{case}.ms_per_mib"),
            median(&mut latency),
            "ms/MiB",
        );
        report(
            &format!("e2e.export.{engine}.{case}.client_peak_rss"),
            memory.into_iter().fold(0., f64::max),
            "MiB",
        );
        if !spark.is_empty() {
            report(
                &format!("e2e.export.{engine}.{case}.spark_driver_peak_rss"),
                spark.into_iter().fold(0., f64::max),
                "MiB",
            );
        }
    }
    Ok(())
}
#[test]
#[ignore = "performance probe: ./qtest run perf-e2e"]
fn kyuubi() -> Result<()> {
    measure("kyuubi")
}
#[test]
#[ignore = "performance probe: ./qtest run perf-postgres"]
fn postgres() -> Result<()> {
    measure("postgres")
}
#[test]
#[ignore = "performance probe: ./qtest run perf-trino"]
fn trino() -> Result<()> {
    measure("trino")
}
