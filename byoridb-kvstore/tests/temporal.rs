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

    // 경계값: valid_at == valid_from (반열린 구간 [from, to) 는 from 을 포함)
    assert_eq!(
        store.get_as_of(e, 100, 10).await.unwrap().as_deref(),
        Some(b"A".as_slice()),
        "valid_at == valid_from 포함"
    );
    assert_eq!(
        store.get_as_of(e, 200, 20).await.unwrap().as_deref(),
        Some(b"B".as_slice()),
        "open interval 시작 경계"
    );
    assert_eq!(
        store.get_as_of(e, 199, 35).await.unwrap().as_deref(),
        Some(b"A2".as_slice()),
        "valid_to 직전(199)은 포함, [from, to) 반열린"
    );

    // v1.1 ①: batch_apply — 현재뷰 put/delete 와 이력 append 가 한 번에 적용.
    store.put(b"sp:vertex:7", b"OLD").await.unwrap();
    store
        .batch_apply(
            vec![(b"sp:vertex:8".to_vec(), b"NEW".to_vec())],
            vec![b"sp:vertex:7".to_vec()],
            vec![
                (
                    b"sp:vertex:8".to_vec(),
                    500,
                    VALID_OPEN,
                    500,
                    b"NEW".to_vec(),
                ),
                (b"sp:vertex:7".to_vec(), 500, VALID_OPEN, 500, Vec::new()),
            ],
        )
        .await
        .unwrap();
    assert_eq!(
        store.get(b"sp:vertex:8").await.unwrap().as_deref(),
        Some(b"NEW".as_slice())
    );
    assert_eq!(store.get(b"sp:vertex:7").await.unwrap(), None);
    assert_eq!(store.scan_history(b"sp:vertex:8").await.unwrap().len(), 1);
    let tomb = store.scan_history(b"sp:vertex:7").await.unwrap();
    assert_eq!(tomb.len(), 1);
    assert!(tomb[0].value.is_empty(), "tombstone");
    // 빈 인자는 no-op
    store
        .batch_apply(Vec::new(), Vec::new(), Vec::new())
        .await
        .unwrap();

    // seek 기반 resolution 스트레스: 버전 다수 + 미래 tx 스킵이 섞여도 정확.
    let m = b"sp:vertex:many".as_slice();
    for i in 0..50 {
        let vf = 1000 + i * 10;
        store
            .put_version(m, vf, vf + 10, vf, format!("v{i}").as_bytes())
            .await
            .unwrap();
    }
    // 정정: [1200,1210) 구간을 미래 tx(9999)로 다시 기록
    store
        .put_version(m, 1200, 1210, 9999, b"corrected")
        .await
        .unwrap();
    assert_eq!(
        store.get_as_of(m, 1205, 1205).await.unwrap().as_deref(),
        Some(b"v20".as_slice()),
        "tx 1205 기준: 정정(tx9999)은 아직 모름"
    );
    assert_eq!(
        store.get_as_of(m, 1205, 10000).await.unwrap().as_deref(),
        Some(b"corrected".as_slice()),
        "tx 10000 기준: 정정 반영"
    );
    assert_eq!(
        store.get_as_of(m, 1495, 2000).await.unwrap().as_deref(),
        Some(b"v49".as_slice()),
        "마지막 구간 [1490,1500)"
    );
    assert_eq!(
        store.get_as_of(m, 1500, 2000).await.unwrap(),
        None,
        "모든 구간이 닫혀 1500 은 미커버"
    );

    // v2-a: scan_history_entity_keys — prefix 아래 distinct 엔티티 열거
    // (버전 다수 → 1개로 dedupe, 삭제-전용(tombstone) 엔티티 포함, prefix 경계).
    store
        .put_version(b"sp:edge:1:rel:2:0", 10, VALID_OPEN, 10, b"e1")
        .await
        .unwrap();
    store
        .put_version(b"sp:edge:1:rel:2:0", 20, VALID_OPEN, 20, b"e1v2")
        .await
        .unwrap();
    store
        .put_version(
            b"sp:edge:1:rel:3:0",
            10,
            VALID_OPEN,
            10,
            Vec::new().as_slice(),
        )
        .await
        .unwrap();
    store
        .put_version(b"sp:edge:2:rel:9:0", 10, VALID_OPEN, 10, b"other-src")
        .await
        .unwrap();
    let ents = store.scan_history_entity_keys(b"sp:edge:1:").await.unwrap();
    assert_eq!(
        ents,
        vec![b"sp:edge:1:rel:2:0".to_vec(), b"sp:edge:1:rel:3:0".to_vec()],
        "dedupe + tombstone-전용 포함 + 다른 src 제외"
    );
    assert!(store
        .scan_history_entity_keys(b"sp:edge:9:")
        .await
        .unwrap()
        .is_empty());
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
