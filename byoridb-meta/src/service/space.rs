// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use super::*;

impl MetaService {
    pub async fn create_space(
        &self,
        name: String,
        partition_num: u32,
        replica_factor: u32,
        vid_type: VidType,
        partition_strategy: byoridb_common::PartitionStrategy,
    ) -> Result<u32> {
        info!("Creating space: {}", name);

        // Check if space already exists (lock-free read)
        if self.space_names.contains_key(&name) {
            return Err(MetaError::SpaceAlreadyExists(name));
        }

        // Validate partition strategy
        if let Err(e) = partition_strategy.validate(partition_num) {
            return Err(MetaError::InvalidPartitionStrategy(e));
        }

        // Generate new space ID atomically
        let space_id = self.next_space_id.fetch_add(1, Ordering::SeqCst);

        let space = Space {
            id: space_id,
            name: name.clone(),
            partition_num,
            replica_factor,
            vid_type,
            partition_strategy,
        };

        // Store space (fine-grained locking per shard)
        self.spaces.insert(space_id, space.clone());
        self.space_names.insert(name, space_id);

        // Persist to KV store
        let key = MetaKey::space(space_id);
        let value = serde_json::to_vec(&space)?;
        self.kvstore.put(&key, &value).await?;

        // Get available hosts for partition allocation
        let available_hosts = self.get_active_storage_hosts();

        // Create consistent hash ring for this space
        let mut ring =
            ConsistentHashRing::new(partition_num, replica_factor, RingConfig::default());

        // Add available hosts to the ring
        for (host, port) in &available_hosts {
            ring.add_node(RingNode::new(host.clone(), *port));
        }

        // Get partition allocations from the ring
        let allocations = ring.get_allocations();

        // Store partition allocations
        self.part_allocations.insert(space_id, allocations.clone());

        // Store the ring for future topology changes
        self.hash_rings.insert(space_id, RwLock::new(ring));

        // Update host partition tracking
        for (part_id, hosts) in &allocations {
            for (host, port) in hosts {
                if let Some(mut host_info) = self.storage_hosts.get_mut(&(host.clone(), *port)) {
                    host_info.partitions.insert((space_id, *part_id));
                }
            }
        }

        // Persist the ring to KV store
        self.persist_ring(space_id).await?;

        let host_count = if available_hosts.is_empty() {
            1
        } else {
            available_hosts.len()
        };
        debug!("Created space {} with ID {} and {} partitions across {} hosts (using consistent hashing)",
               space.name, space_id, partition_num, host_count);
        Ok(space_id)
    }

    /// Get a space by ID (lock-free read)
    pub async fn get_space(&self, space_id: u32) -> Result<Space> {
        self.spaces
            .get(&space_id)
            .map(|r| r.value().clone())
            .ok_or_else(|| MetaError::SpaceNotFound(format!("ID {}", space_id)))
    }

    /// Get a space by name (lock-free read)
    pub async fn get_space_by_name(&self, name: &str) -> Result<Space> {
        let space_id = self
            .space_names
            .get(name)
            .map(|r| *r.value())
            .ok_or_else(|| MetaError::SpaceNotFound(name.to_string()))?;

        self.get_space(space_id).await
    }

    /// List all spaces (lock-free iteration)
    pub async fn list_spaces(&self) -> Result<Vec<Space>> {
        Ok(self.spaces.iter().map(|r| r.value().clone()).collect())
    }

    /// Drop a space
    pub async fn drop_space(&self, name: &str) -> Result<()> {
        info!("Dropping space: {}", name);

        let (_, space_id) = self
            .space_names
            .remove(name)
            .ok_or_else(|| MetaError::SpaceNotFound(name.to_string()))?;

        self.spaces.remove(&space_id);
        self.part_allocations.remove(&space_id);
        self.hash_rings.remove(&space_id);

        // Remove from KV store
        let key = MetaKey::space(space_id);
        self.kvstore.delete(&key).await?;

        // Also remove the ring from KV store
        let ring_key = MetaKey::ring(space_id);
        self.kvstore.delete(&ring_key).await?;

        debug!("Dropped space {} (ID {})", name, space_id);
        Ok(())
    }
}
