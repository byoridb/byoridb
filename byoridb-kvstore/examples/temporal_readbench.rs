//! T-트랙 #1 리스크 측정용 격리 프로토타입 (프로덕션 경로 미접촉, 미커밋).
//!
//! 질문: 현재뷰(`sp:vertex:{vid}`) 키가 그대로 유지되고, 이력(`sp:vh:...`)이
//! 같은 redb B-tree에 대량 공존할 때 **현재뷰 읽기가 느려지는가?**
//!
//! 실행: cargo run --release -p byoridb-kvstore --example temporal_readbench [N] [VERSIONS]
//!   기본 N=100_000 현재뷰 정점, VERSIONS=9 → 이력 900_000행 (~10x).

use byoridb_kvstore::{KVStore, KVStoreOptions, RedbKVStore};
use std::time::Instant;

fn xorshift(s: &mut u64) -> u64 {
    let mut x = *s;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *s = x;
    x
}

fn pct(sorted: &[u128], p: f64) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx]
}

fn cur_key(vid: u64) -> Vec<u8> {
    format!("sp:vertex:{:016x}", vid).into_bytes()
}

// 이력 키: valid_from / tx 를 내림차순 인코딩(MAX - x) → vid 프리픽스 내에서
// 최신 버전이 먼저 정렬 → AS-OF = 프리픽스 seek 후 첫 행.
fn hist_key(vid: u64, valid_from: u64, tx: u64) -> Vec<u8> {
    format!(
        "sp:vh:{:016x}:{:016x}:{:016x}",
        vid,
        u64::MAX - valid_from,
        u64::MAX - tx
    )
    .into_bytes()
}

async fn point_get_stats(
    store: &RedbKVStore,
    n: u64,
    samples: usize,
    seed: u64,
) -> (u128, u128, u128) {
    let mut s = seed;
    let mut lat: Vec<u128> = Vec::with_capacity(samples);
    let mut hits = 0usize;
    for _ in 0..samples {
        let vid = xorshift(&mut s) % n;
        let key = cur_key(vid);
        let t = Instant::now();
        let got = store.get(&key).await.unwrap();
        lat.push(t.elapsed().as_nanos());
        if got.is_some() {
            hits += 1;
        }
    }
    assert_eq!(hits, samples, "current-view get 미스 발생 — 키 인코딩 확인");
    lat.sort_unstable();
    let mean = lat.iter().sum::<u128>() / lat.len() as u128;
    (pct(&lat, 0.50), pct(&lat, 0.99), mean)
}

async fn insert_range(
    store: &RedbKVStore,
    keys: impl Iterator<Item = Vec<u8>>,
    val: &[u8],
    chunk: usize,
) -> usize {
    let mut batch: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(chunk);
    let mut total = 0usize;
    for k in keys {
        batch.push((k, val.to_vec()));
        if batch.len() >= chunk {
            total += batch.len();
            store.batch_put(std::mem::take(&mut batch)).await.unwrap();
        }
    }
    if !batch.is_empty() {
        total += batch.len();
        store.batch_put(batch).await.unwrap();
    }
    total
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let n: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(100_000);
    let versions: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(9);
    let val = vec![0xABu8; 200]; // ~200B 정점 blob 대용
    let samples = 20_000usize;

    let dir = "/private/tmp/claude-501/-Users-juikkim-opensource-byoridb/4cf25b71-01fd-4214-a410-61d741a4d893/scratchpad";
    let path = format!("{dir}/temporal_readbench.redb");
    let _ = std::fs::remove_file(&path);

    let store = RedbKVStore::open(&path, KVStoreOptions::default()).unwrap();

    println!("== T-트랙 현재뷰 읽기 회귀 벤치 ==");
    println!(
        "N(현재뷰 정점)={n}, VERSIONS(정점당 이력)={versions}, value={}B, samples={samples}\n",
        val.len()
    );

    // ---- Phase A: 현재뷰만 적재 ----
    let t = Instant::now();
    let a = insert_range(&store, (0..n).map(cur_key), &val, 20_000).await;
    println!("[A] 현재뷰 {a}행 적재: {:?}", t.elapsed());

    let (p50a, p99a, meana) = point_get_stats(&store, n, samples, 0x1234_5678).await;
    let t = Instant::now();
    let scan_a = store.scan_prefix(b"sp:vertex:").await.unwrap();
    let scan_a_dur = t.elapsed();
    let size_a = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    println!(
        "[A] point-get  p50={p50a}ns p99={p99a}ns mean={meana}ns | prefix-scan {}행 {:?} | db {}MB\n",
        scan_a.len(),
        scan_a_dur,
        size_a / 1_048_576
    );

    // ---- Phase B: 이력 대량 적재 (같은 DB) ----
    let t = Instant::now();
    let hist_iter =
        (0..n).flat_map(|vid| (0..versions).map(move |v| hist_key(vid, 1_000 + v, 1_000 + v)));
    let b = insert_range(&store, hist_iter, &val, 20_000).await;
    println!("[B] 이력 {b}행 적재(누적 {}행): {:?}", a + b, t.elapsed());

    let (p50b, p99b, meanb) = point_get_stats(&store, n, samples, 0x1234_5678).await;
    let t = Instant::now();
    let scan_b = store.scan_prefix(b"sp:vertex:").await.unwrap();
    let scan_b_dur = t.elapsed();
    let size_b = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    println!(
        "[B] point-get  p50={p50b}ns p99={p99b}ns mean={meanb}ns | prefix-scan {}행 {:?} | db {}MB\n",
        scan_b.len(),
        scan_b_dur,
        size_b / 1_048_576
    );

    // ---- AS-OF seek: 이력에서 한 정점의 최신 버전 조회 ----
    let mut s = 0xDEAD_BEEFu64;
    let mut asof: Vec<u128> = Vec::with_capacity(5_000);
    for _ in 0..5_000 {
        let vid = xorshift(&mut s) % n;
        let prefix = format!("sp:vh:{:016x}:", vid).into_bytes();
        let t = Instant::now();
        let rows = store.scan_prefix_limited(&prefix, Some(1)).await.unwrap();
        asof.push(t.elapsed().as_nanos());
        assert!(!rows.is_empty());
    }
    asof.sort_unstable();
    println!(
        "[AS-OF] 이력 seek(정점당 최신 1행) p50={}ns p99={}ns",
        pct(&asof, 0.50),
        pct(&asof, 0.99)
    );

    // ---- 쓰기 비용: 단일 put vs 단일+이력 append ----
    let mut w1: Vec<u128> = Vec::with_capacity(2_000);
    let mut w2: Vec<u128> = Vec::with_capacity(2_000);
    for i in 0..2_000u64 {
        let vid = n + i;
        let t = Instant::now();
        store.put(&cur_key(vid), &val).await.unwrap();
        w1.push(t.elapsed().as_nanos());

        let vid2 = n + 2_000 + i;
        let t = Instant::now();
        store
            .batch_put(vec![
                (cur_key(vid2), val.clone()),
                (hist_key(vid2, 2_000, 2_000), val.clone()),
            ])
            .await
            .unwrap();
        w2.push(t.elapsed().as_nanos());
    }
    w1.sort_unstable();
    w2.sort_unstable();
    println!(
        "[WRITE] 현재뷰만 put        p50={}ns p99={}ns",
        pct(&w1, 0.50),
        pct(&w1, 0.99)
    );
    println!(
        "[WRITE] 현재뷰+이력 append   p50={}ns p99={}ns",
        pct(&w2, 0.50),
        pct(&w2, 0.99)
    );

    // ---- 판정 ----
    println!("\n== 판정 ==");
    let pt_delta = (p50b as f64 - p50a as f64) / p50a as f64 * 100.0;
    let scan_delta =
        (scan_b_dur.as_secs_f64() - scan_a_dur.as_secs_f64()) / scan_a_dur.as_secs_f64() * 100.0;
    println!("현재뷰 point-get p50 변화: {pt_delta:+.1}% (이력 {b}행 공존 후)");
    println!("현재뷰 prefix-scan 변화:   {scan_delta:+.1}%");
    let _ = std::fs::remove_file(&path);
}
