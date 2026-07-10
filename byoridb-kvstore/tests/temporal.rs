//! T-1 저장 레이어(bitemporal version history) 유닛 테스트.
//! 동일 스위트를 두 백엔드(Memory, Redb)에 적용해 계약을 검증한다.

use byoridb_kvstore::{
    KVStore, KVStoreOptions, MemoryKVStore, RedbKVStore, VersionRecord, VALID_OPEN,
};

async fn run_suite(store: &dyn KVStore) {
    let e = b"sp:vertex:1".as_slice();

    // 3개 버전 append:
    //   [100,200)@tx10 = "A"
    //   [200,∞)@tx20   = "B"
    //   [100,200)@tx30 = "A2"  (tx30에 기록된 [100,200) 사실의 정정)
    store.put_version(e, 100, 200, 10, b"A").await.unwrap();
    store
        .put_version(e, 200, VALID_OPEN, 20, b"B")
        .await
        .unwrap();
    store.put_version(e, 100, 200, 30, b"A2").await.unwrap();

    // scan_history: 3개, newest-first (valid_from desc → tx desc)
    let hist = store.scan_history(e).await.unwrap();
    assert_eq!(hist.len(), 3);
    assert_eq!((hist[0].valid_from, hist[0].tx), (200, 20));
    assert_eq!((hist[1].valid_from, hist[1].tx), (100, 30));
    assert_eq!((hist[2].valid_from, hist[2].tx), (100, 10));
    assert_eq!(hist[0].value, b"B".to_vec());
    assert_eq!(hist[0].valid_to, VALID_OPEN);
    assert!(hist.contains(&VersionRecord {
        valid_from: 100,
        valid_to: 200,
        tx: 10,
        value: b"A".to_vec(),
    }));

    // get_as_of 이중 시간 해석
    assert_eq!(
        store.get_as_of(e, 150, 25).await.unwrap().as_deref(),
        Some(b"A".as_slice()),
        "tx25 기준: tx30 정정은 아직 모름 → A"
    );
    assert_eq!(
        store.get_as_of(e, 150, 35).await.unwrap().as_deref(),
        Some(b"A2".as_slice()),
        "tx35 기준: 정정 반영 → A2"
    );
    assert_eq!(
        store.get_as_of(e, 250, 25).await.unwrap().as_deref(),
        Some(b"B".as_slice()),
        "open interval [200,∞) 커버"
    );
    assert_eq!(
        store.get_as_of(e, 250, 15).await.unwrap(),
        None,
        "tx15 기준: B(tx20)는 아직 기록 안 됨"
    );
    assert_eq!(
        store.get_as_of(e, 50, 100).await.unwrap(),
        None,
        "valid 50을 커버하는 버전 없음"
    );

    // 물리 격리: 이력이 현재뷰 keyspace를 오염하지 않음
    store.put(e, b"CURRENT").await.unwrap();
    assert_eq!(
        store.get(e).await.unwrap().as_deref(),
        Some(b"CURRENT".as_slice())
    );
    let cur = store.scan_prefix(b"sp:vertex:").await.unwrap();
    assert_eq!(
        cur,
        vec![(e.to_vec(), b"CURRENT".to_vec())],
        "현재뷰 prefix-scan에 이력 행이 새어나오면 안 됨"
    );
    assert_eq!(
        store.scan_history(e).await.unwrap().len(),
        3,
        "이력은 그대로 유지"
    );

    // 없는 엔티티
    assert!(store
        .scan_history(b"sp:vertex:999")
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        store.get_as_of(b"sp:vertex:999", 100, 100).await.unwrap(),
        None
    );
}

#[tokio::test]
async fn temporal_memory_backend() {
    let store = MemoryKVStore::new();
    run_suite(&store).await;
}

#[tokio::test]
async fn temporal_redb_backend() {
    let dir = tempfile::tempdir().unwrap();
    let store = RedbKVStore::open(dir.path(), KVStoreOptions::default()).unwrap();
    run_suite(&store).await;
}
