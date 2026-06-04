use crate::codec::Codec;
use crate::error::Result;
use crate::key::KeyUtils;
use byoridb_common::Value;
use byoridb_kvstore::KVStore;
use std::sync::Arc;

pub struct StorageProcessor {
    store: Arc<dyn KVStore>,
}

impl StorageProcessor {
    pub fn new(store: Arc<dyn KVStore>) -> Self {
        Self { store }
    }

    pub async fn put(
        &self,
        _space_id: u32,
        part_id: u32,
        vid: i64,
        tag_id: u32,
        props: Value,
    ) -> Result<()> {
        let key = KeyUtils::vertex_key(part_id, vid, tag_id);
        let value = Codec::encode(&props)?;

        self.store
            .put(&key, &value)
            .await
            .map_err(|e| crate::error::StorageError::StoreError(e.to_string()))
    }

    pub async fn get(
        &self,
        _space_id: u32,
        part_id: u32,
        vid: i64,
        tag_id: u32,
    ) -> Result<Option<Value>> {
        let key = KeyUtils::vertex_key(part_id, vid, tag_id);

        match self
            .store
            .get(&key)
            .await
            .map_err(|e| crate::error::StorageError::StoreError(e.to_string()))?
        {
            Some(bytes) => {
                let val = Codec::decode(&bytes)?;
                Ok(Some(val))
            }
            None => Ok(None),
        }
    }
}
