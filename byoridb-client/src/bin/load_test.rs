use clap::Parser;
use colored::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Barrier;

#[derive(Parser, Debug)]
#[command(author, version, about = "ByoriDB Load Testing Tool", long_about = None)]
struct Args {
    #[arg(short, long, default_value = "http://127.0.0.1:9669")]
    address: String,

    /// Username — required (env: BYORIDB_USER). No default; built-in `root`
    /// removed to prevent unauthenticated load tests from masking auth
    /// issues in CI.
    #[arg(short, long, env = "BYORIDB_USER")]
    user: String,

    /// Password — required (env: BYORIDB_PASSWORD).
    #[arg(short, long, env = "BYORIDB_PASSWORD")]
    password: String,

    /// Number of concurrent clients
    #[arg(short, long, default_value_t = 50)]
    concurrency: usize,

    /// Duration of the test in seconds
    #[arg(short, long, default_value_t = 10)]
    duration: u64,

    /// Query to execute
    #[arg(short, long, default_value = "SHOW SPACES")]
    query: String,

    /// Setup query executed once per client after connecting (e.g. "USE my_space")
    #[arg(long, default_value = "")]
    setup: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    println!("{}", "🚀 Starting ByoriDB Load Test".green().bold());
    println!("Target: {}", args.address);
    println!("Concurrency: {}", args.concurrency);
    println!("Duration: {}s", args.duration);
    if !args.setup.is_empty() {
        println!("Setup query: configured ({} bytes)", args.setup.len());
    }
    println!("Query length: {} bytes", args.query.len());
    println!("---------------------------------------------------");

    // Shared metrics
    let total_requests = Arc::new(AtomicU64::new(0));
    let total_errors = Arc::new(AtomicU64::new(0));
    let start_signal = Arc::new(Barrier::new(args.concurrency + 1));
    let mut handles = Vec::new();
    let running = Arc::new(std::sync::atomic::AtomicBool::new(true));

    for i in 0..args.concurrency {
        let addr = args.address.clone();
        let user = args.user.clone();
        let password = args.password.clone();
        let query = args.query.clone();
        let setup = args.setup.clone();
        let start = start_signal.clone();
        let running = running.clone();
        let req_counter = total_requests.clone();
        let err_counter = total_errors.clone();

        handles.push(tokio::spawn(async move {
            let mut client = match byoridb_client::Client::connect(addr, user, password).await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Client {} failed to connect: {}", i, e);
                    return;
                }
            };

            if !setup.is_empty() {
                if let Err(e) = client.execute(&setup).await {
                    eprintln!("Client {} setup failed: {}", i, e);
                    return;
                }
            }

            start.wait().await;

            while running.load(Ordering::Relaxed) {
                if client.execute(&query).await.is_err() {
                    err_counter.fetch_add(1, Ordering::Relaxed);
                } else {
                    req_counter.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }

    println!("Initializing {} clients...", args.concurrency);
    // Wait for everyone to be ready
    start_signal.wait().await;
    let start_time = Instant::now();
    println!("Test started! Running for {} seconds...", args.duration);

    // Monitoring loop
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    let mut elapsed = 0;
    let mut last_reqs = 0;

    while elapsed < args.duration {
        interval.tick().await;
        elapsed += 1;

        let current_reqs = total_requests.load(Ordering::Relaxed);
        let current_errs = total_errors.load(Ordering::Relaxed);
        let qps = current_reqs - last_reqs;
        last_reqs = current_reqs;

        println!("[{}s] QPS: {}, Errors: {}", elapsed, qps, current_errs);
    }

    // Stop
    running.store(false, Ordering::Relaxed);
    let total_time = start_time.elapsed();

    // Wait for tasks (give them a bit of time to notice the flag)
    tokio::time::sleep(Duration::from_millis(100)).await;

    println!("---------------------------------------------------");
    println!("{}", "🎉 Test Completed!".green().bold());
    let total_reqs = total_requests.load(Ordering::Relaxed);
    let total_errs = total_errors.load(Ordering::Relaxed);

    println!("Total Requests: {}", total_reqs);
    println!("Total Errors:   {}", total_errs);
    println!("Total Time:     {:.2?}", total_time);
    println!(
        "Average QPS:    {:.2}",
        total_reqs as f64 / total_time.as_secs_f64()
    );

    Ok(())
}
