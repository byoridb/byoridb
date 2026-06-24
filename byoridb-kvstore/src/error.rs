// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, KVStoreError>;

#[derive(Error, Debug)]
pub enum KVStoreError {
    #[error("redb database error: {0}")]
    RedbDatabase(#[from] redb::DatabaseError),

    #[error("redb transaction error: {0}")]
    RedbTransaction(#[from] redb::TransactionError),

    #[error("redb table error: {0}")]
    RedbTable(#[from] redb::TableError),

    #[error("redb storage error: {0}")]
    RedbStorage(#[from] redb::StorageError),

    #[error("redb commit error: {0}")]
    RedbCommit(#[from] redb::CommitError),

    #[error("redb set-durability error: {0}")]
    RedbSetDurability(#[from] redb::SetDurabilityError),

    #[error("Key not found: {0:?}")]
    KeyNotFound(Vec<u8>),

    #[error("Raft error: {0}")]
    Raft(String),

    #[error("WAL error: {0}")]
    Wal(String),

    #[error("Serialization error: {0}")]
    Serialization(#[source] Box<bincode::Error>),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Store closed")]
    StoreClosed,
}

impl From<bincode::Error> for KVStoreError {
    fn from(err: bincode::Error) -> Self {
        KVStoreError::Serialization(Box::new(err))
    }
}

impl From<tokio::task::JoinError> for KVStoreError {
    fn from(err: tokio::task::JoinError) -> Self {
        KVStoreError::Io(std::io::Error::other(err))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_not_found_display_includes_key_bytes() {
        let err = KVStoreError::KeyNotFound(b"abc".to_vec());
        let msg = format!("{}", err);
        // The key is rendered as a debug Vec<u8> — the bytes must show up
        assert!(msg.contains("97") && msg.contains("98") && msg.contains("99"));
    }

    #[test]
    fn test_store_closed_display() {
        let err = KVStoreError::StoreClosed;
        assert_eq!(format!("{}", err), "Store closed");
    }

    #[test]
    fn test_wal_error_carries_message() {
        let err = KVStoreError::Wal("checksum mismatch".into());
        assert!(format!("{}", err).contains("checksum mismatch"));
    }

    #[test]
    fn test_raft_error_carries_message() {
        let err = KVStoreError::Raft("leader election failed".into());
        assert!(format!("{}", err).contains("leader election failed"));
    }

    #[test]
    fn test_from_io_error_preserves_kind() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let kv_err: KVStoreError = io_err.into();
        match kv_err {
            KVStoreError::Io(inner) => {
                assert_eq!(inner.kind(), std::io::ErrorKind::PermissionDenied);
            }
            other => panic!("expected Io variant, got {:?}", other),
        }
    }

    #[test]
    fn test_from_bincode_error_boxes_into_serialization() {
        // Use a deliberately malformed buffer to obtain a real bincode::Error
        let bad: Vec<u8> = vec![0xFF];
        let bincode_err = bincode::deserialize::<String>(&bad).unwrap_err();
        let kv_err: KVStoreError = bincode_err.into();
        assert!(matches!(kv_err, KVStoreError::Serialization(_)));
    }

    #[test]
    fn test_join_error_maps_to_io() {
        // Tokio JoinError can be obtained by aborting a spawned task.
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(async {
            let handle = tokio::spawn(async {
                std::future::pending::<()>().await;
            });
            handle.abort();
            let join_err = handle.await.unwrap_err();
            let kv_err: KVStoreError = join_err.into();
            assert!(matches!(kv_err, KVStoreError::Io(_)));
        });
    }

    #[test]
    fn test_result_alias_is_usable() {
        // Smoke check that the Result alias compiles in user code.
        fn returns_ok() -> Result<u8> {
            Ok(7)
        }
        assert_eq!(returns_ok().unwrap(), 7);
    }
}
