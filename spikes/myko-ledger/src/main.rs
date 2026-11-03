//! Throwaway proof for RAL-39: can a hash-chained ledger use myko 6.9.0 and Postgres as its store?
//!
//! `serve` is a real myko server process. The client subcommands are separate processes that use an
//! ordinary `MykoClient`. `check` recomputes the chain here, outside myko, from what a query returns.
//! `embedded-*` run the same store and append path in-process, without ever calling `run()`.
mod commands;
mod record;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use commands::{
    AppendRecord, AppendResult, BarrierResult, CHAIN_START, DurableBarrier, HEALTH, chain_hash,
    wait_for_drain,
};
use myko::{
    client::MykoClient,
    command::{CommandContext, CommandHandler as _},
    event::{MEvent, MEventType},
    hyphae::Gettable as _,
    common::to_value::ToValue as _,
    request::RequestContext,
    server::{BlackholePersister, Persister},
};
use myko_server::{MykoServer, MykoServerBuilder, postgres::PostgresConfig};
use record::{GetAllLedgerRecords, LedgerRecord, LedgerRecordId};

type Res<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn peak_rss_mib() -> f64 {
    let mut usage = unsafe { std::mem::zeroed::<libc::rusage>() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    // ru_maxrss is in bytes on macOS.
    usage.ru_maxrss as f64 / (1024.0 * 1024.0)
}

fn builder(pg: PostgresConfig) -> MykoServerBuilder {
    let blackhole: Arc<dyn Persister> = Arc::new(BlackholePersister);
    // As Muster does: connection bookkeeping stays out of the durable table.
    MykoServer::builder()
        .with_postgres(pg)
        .with_persister_override("Client", blackhole.clone())
        .with_persister_override("Server", blackhole)
}

fn pg() -> Res<PostgresConfig> {
    PostgresConfig::from_env().ok_or_else(|| "MYKO_POSTGRES_URL is not set".into())
}

const ENTITY: &str = "LedgerRecord";

fn store_count(server: &MykoServer) -> usize {
    server.registry.get_or_create(ENTITY).items_snapshot().len()
}

async fn serve(bind: SocketAddr, host_id: Option<uuid::Uuid>) -> Res {
    let started = Instant::now();
    let mut b = builder(pg()?).with_bind_addr(bind).after_init(move |server| {
        println!(
            "replayed: records_in_store={} elapsed_ms={} peak_rss_mib={:.1} host_id={}",
            store_count(server),
            started.elapsed().as_millis(),
            peak_rss_mib(),
            server.host_id
        );
    });
    if let Some(id) = host_id {
        b = b.with_host_id(id);
    }
    let server = b.build();
    let handle = server.postgres_producer.clone().ok_or("no producer")?;
    let _ = HEALTH.set(myko::server::Persister::health(&handle));
    server.run().await
}

/// The store with no listener: `run()` is never called, so nothing binds.
fn embedded_server() -> Res<MykoServer> {
    let started = Instant::now();
    let server = builder(pg()?).build();
    let handle = server.postgres_producer.clone().ok_or("no producer")?;
    let _ = HEALTH.set(myko::server::Persister::health(&handle));
    server.init_postgres_and_wait(Duration::from_secs(60))?;
    println!(
        "embedded: caught up records_in_store={} elapsed_ms={} host_id={}",
        store_count(&server),
        started.elapsed().as_millis(),
        server.host_id
    );
    Ok(server)
}

fn listening_note() {
    // The driver looks at lsof while we hold; this gives it a window.
    if let Ok(ms) = std::env::var("SPIKE_HOLD_MS") {
        println!("holding for lsof, pid={}", std::process::id());
        std::thread::sleep(Duration::from_millis(ms.parse().unwrap_or(0)));
    }
}

fn embedded_append(texts: &[String]) -> Res {
    let server = embedded_server()?;
    let ctx = Arc::new(server.ctx());
    for text in texts {
        let req = Arc::new(RequestContext::internal(Arc::from("embedded"), server.host_id, "spike"));
        let cmd = CommandContext::new(Arc::from("AppendRecord"), req, ctx.clone());
        let done = AppendRecord { text: text.clone() }.execute(cmd).map_err(|e| e.message)?;
        println!("{}:{}", done.seq, done.hash);
    }
    println!("{:?}", wait_for_drain(Duration::from_secs(10)));
    listening_note();
    Ok(())
}

fn embedded_check(expect: Option<&str>) -> Res {
    let server = embedded_server()?;
    let records: Vec<(String, String, String)> = server
        .registry
        .get_or_create(ENTITY)
        .items_snapshot()
        .iter()
        .map(|item| {
            let v = item.to_value();
            (
                v["id"].as_str().unwrap_or("").to_owned(),
                v["text"].as_str().unwrap_or("").to_owned(),
                v["hash"].as_str().unwrap_or("").to_owned(),
            )
        })
        .collect();
    report_chain(records, expect);
    Ok(())
}

/// The check that doesn't trust myko: it only reads what came back, and recomputes.
fn report_chain(mut records: Vec<(String, String, String)>, expect: Option<&str>) -> bool {
    records.sort_by_key(|(id, _, _)| id.parse::<u64>().unwrap_or(u64::MAX));
    let mut previous = CHAIN_START.to_owned();
    let mut problems = Vec::new();
    let mut last_seq = 0_u64;
    for (want, (id, text, claimed)) in (1_u64..).zip(&records) {
        let Ok(seq) = id.parse::<u64>() else {
            problems.push(format!("non-numeric id {id:?}"));
            continue;
        };
        if seq != want && !problems.iter().any(|p: &String| p.starts_with("gap")) {
            problems.push(format!("gap: expected seq {want}, found {seq}"));
        }
        let hash = chain_hash(&previous, seq, text);
        if &hash != claimed {
            problems.push(format!(
                "seq {seq}: claimed hash {} != recomputed {}",
                &claimed[..claimed.len().min(12)],
                &hash[..12]
            ));
        }
        previous.clone_from(claimed);
        last_seq = seq;
    }
    let head = format!("{last_seq}:{previous}");
    println!("records={} head={head}", records.len());
    if records.len() <= 5 {
        for (id, text, hash) in &records {
            println!("  id={id} hash={}.. text={text:?}", &hash[..hash.len().min(12)]);
        }
    }
    if let Some(expect) = expect {
        if expect == head {
            println!("expected head: MATCH");
        } else {
            problems.push(format!("head {head} != expected {expect}"));
        }
    }
    for p in problems.iter().take(6) {
        println!("PROBLEM: {p}");
    }
    if problems.len() > 6 {
        println!("PROBLEM: ... and {} more", problems.len() - 6);
    }
    println!("{}", if problems.is_empty() { "CHAIN OK" } else { "CHAIN BROKEN" });
    problems.is_empty()
}

async fn connect(addr: &str) -> Res<MykoClient> {
    let client = MykoClient::new();
    client.set_address(Some(format!("ws://{addr}/myko")));
    let started = Instant::now();
    while !matches!(client.connection_status_sync(), myko::client::ConnectionStatus::Connected(_)) {
        if started.elapsed() > Duration::from_secs(10) {
            return Err(format!("no connection to {addr} in 10s").into());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Ok(client)
}

async fn await_command<R>(
    cell: myko::hyphae::Cell<Option<Result<R, String>>, myko::hyphae::CellImmutable>,
    secs: u64,
) -> Res<R>
where
    R: Clone + std::fmt::Debug + PartialEq + Send + Sync + 'static,
{
    let started = Instant::now();
    loop {
        if let Some(result) = cell.get() {
            return result.map_err(Into::into);
        }
        if started.elapsed() > Duration::from_secs(secs) {
            return Err("no command response".into());
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

async fn append(addr: &str, texts: Vec<String>) -> Res {
    let client = connect(addr).await?;
    for text in texts {
        let cell = client.send_command::<AppendRecord, AppendResult>(&AppendRecord { text });
        let done = await_command(cell, 30).await?;
        println!("{}:{}", done.seq, done.hash);
    }
    Ok(())
}

async fn bulk(addr: &str, count: u64, size: usize) -> Res {
    let client = connect(addr).await?;
    let started = Instant::now();
    let mut head = None;
    for i in 0..count {
        let text = format!("{{\"n\":{i},\"pad\":\"{}\"}}", "x".repeat(size));
        let cell = client.send_command::<AppendRecord, AppendResult>(&AppendRecord { text });
        head = Some(await_command(cell, 30).await?);
    }
    eprintln!("bulk: {count} appends elapsed_ms={}", started.elapsed().as_millis());
    if let Some(h) = head {
        println!("{}:{}", h.seq, h.hash);
    }
    Ok(())
}

async fn barrier(addr: &str, timeout_ms: u64) -> Res {
    let client = connect(addr).await?;
    let cell = client.send_command::<DurableBarrier, BarrierResult>(&DurableBarrier { timeout_ms });
    println!("{:?}", await_command(cell, timeout_ms / 1000 + 10).await?);
    Ok(())
}

async fn check(addr: &str, expect: Option<&str>) -> Res {
    let client = connect(addr).await?;
    let items = client.watch_query::<GetAllLedgerRecords>(GetAllLedgerRecords {});
    // An empty result can't be told from one that hasn't arrived: wait for the count to settle.
    let mut last = usize::MAX;
    let mut stable = Instant::now();
    while stable.elapsed() < Duration::from_millis(1500) {
        let n = items.get().len();
        if n != last {
            last = n;
            stable = Instant::now();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let records = items
        .get()
        .iter()
        .map(|r| (r.id.0.to_string(), r.text.clone(), r.hash.clone()))
        .collect();
    report_chain(records, expect);
    Ok(())
}

/// An ordinary client sending a raw event, with no command and no append path.
async fn raw(
    addr: &str,
    kind: MEventType,
    id: &str,
    text: &str,
    hash: &str,
    created_at: Option<&str>,
) -> Res {
    let client = connect(addr).await?;
    let item = LedgerRecord {
        id: LedgerRecordId(Arc::from(id)),
        text: text.to_owned(),
        hash: hash.to_owned(),
    };
    let mut event = MEvent::from_item(&item, kind, "spike-raw-client");
    if let Some(at) = created_at {
        event.created_at = Arc::from(at);
    }
    client.send_event(event)?;
    tokio::time::sleep(Duration::from_millis(700)).await;
    println!("sent; send_event returned Ok (it has no server response)");
    Ok(())
}

/// The delete command `#[myko_item]` generates for every item, which no one wrote.
async fn gen_del(addr: &str, id: &str) -> Res {
    let client = connect(addr).await?;
    let cmd = record::DeleteLedgerRecord { id: LedgerRecordId(Arc::from(id)) };
    let cell = client.send_command::<_, record::DeleteLedgerRecordResult>(&cmd);
    println!("{:?}", await_command(cell, 10).await?);
    Ok(())
}

#[tokio::main]
async fn main() -> Res {
    if let Ok(filter) = std::env::var("SPIKE_LOG") {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
            .with_writer(std::io::stderr)
            .init();
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize| args.get(i).map(String::as_str).ok_or("missing argument");
    let opt = |i: usize| args.get(i).map(String::as_str);
    let result = match arg(0)? {
        "serve" => serve(arg(1)?.parse()?, opt(2).map(str::parse).transpose()?).await,
        "append" => append(arg(1)?, args[2..].to_vec()).await,
        "bulk" => bulk(arg(1)?, arg(2)?.parse()?, arg(3)?.parse()?).await,
        "barrier" => barrier(arg(1)?, arg(2)?.parse()?).await,
        "check" => check(arg(1)?, opt(2)).await,
        "raw-set" => raw(arg(1)?, MEventType::SET, arg(2)?, arg(3)?, arg(4)?, opt(5)).await,
        "raw-del" => raw(arg(1)?, MEventType::DEL, arg(2)?, "", "", None).await,
        "gen-del" => gen_del(arg(1)?, arg(2)?).await,
        "hash" => {
            println!("{}", chain_hash(arg(1)?, arg(2)?.parse()?, arg(3)?));
            Ok(())
        }
        "embedded-append" => embedded_append(&args[1..]),
        "embedded-check" => embedded_check(opt(1)),
        other => Err(format!("unknown subcommand {other}").into()),
    };
    // MykoServer's threads aren't built for a clean shutdown; a proof needn't care.
    if let Err(error) = &result {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
    std::process::exit(0);
}
