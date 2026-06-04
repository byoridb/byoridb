// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use super::*;

impl MetaService {
    pub async fn get_parts_alloc(&self, space_id: u32) -> Result<Vec<PartAllocation>> {
        if let Some(space_parts) = self.part_allocations.get(&space_id) {
            let result: Vec<PartAllocation> = space_parts
                .value()
                .iter()
                .map(|(&part_id, hosts)| PartAllocation {
                    space_id,
                    part_id,
                    hosts: hosts.clone(),
                })
                .collect();
            Ok(result)
        } else {
            // Space exists but no partitions yet
            Ok(Vec::new())
        }
    }

    /// Get partition ID for a given VID using default Hash strategy
    /// Uses consistent hashing: hash(vid) % partition_num + 1
    ///
    /// Note: This delegates to the centralized hash function in byoridb_common::hash
    #[inline]
    pub fn get_part_id(vid: i64, partition_num: u32) -> u32 {
        byoridb_common::hash::compute_partition(vid, partition_num)
    }

    /// Get partition ID for a given VID in a specific space using the space's partition strategy
    ///
    /// This method takes into account the partition strategy configured for the space.
    pub async fn get_part_id_for_space(&self, space_id: u32, vid: i64) -> Result<u32> {
        let space = self.get_space(space_id).await?;
        Ok(space
            .partition_strategy
            .compute_partition(vid, space.partition_num))
    }

    /// Get partition ID for a VID given a partition strategy
    #[inline]
    pub fn compute_part_id(
        vid: i64,
        partition_num: u32,
        strategy: &byoridb_common::PartitionStrategy,
    ) -> u32 {
        strategy.compute_partition(vid, partition_num)
    }

    /// Get hosts for a specific partition (lock-free read)
    pub async fn get_part_hosts(&self, space_id: u32, part_id: u32) -> Result<Vec<(String, u32)>> {
        self.part_allocations
            .get(&space_id)
            .and_then(|parts| parts.value().get(&part_id).cloned())
            .ok_or(MetaError::PartitionNotFound(space_id, part_id))
    }

    /// Update partition allocation for a specific partition
    ///
    /// This method updates the hosts assigned to a partition. Used during rebalancing
    /// to migrate partitions between nodes.
    pub async fn update_part_allocation(
        &self,
        space_id: u32,
        part_id: u32,
        hosts: Vec<(String, u32)>,
    ) -> Result<()> {
        self.part_allocations
            .entry(space_id)
            .or_default()
            .insert(part_id, hosts);
        Ok(())
    }

    // ===== Host Management =====

    /// Handle heartbeat from a storage node
    ///
    /// Registers or updates the storage node in the active hosts list.
    /// This allows the Meta service to track available storage nodes for
    /// partition allocation.
    pub async fn persist_ring(&self, space_id: u32) -> Result<()> {
        // Serialize while holding the lock, then release before await
        let serialized = if let Some(ring_lock) = self.hash_rings.get(&space_id) {
            let ring = ring_lock.read();
            Some(serde_json::to_vec(&*ring)?)
        } else {
            None
        };

        if let Some(value) = serialized {
            let key = MetaKey::ring(space_id);
            self.kvstore.put(&key, &value).await?;
            debug!("Persisted hash ring for space {}", space_id);
        }
        Ok(())
    }

    /// Load a ring from KV store
    #[allow(dead_code)]
    pub async fn load_ring(&self, space_id: u32) -> Result<Option<ConsistentHashRing>> {
        let key = MetaKey::ring(space_id);
        if let Some(value) = self.kvstore.get(&key).await? {
            let ring: ConsistentHashRing = serde_json::from_slice(&value)?;
            Ok(Some(ring))
        } else {
            Ok(None)
        }
    }

    /// Add a storage node to all active hash rings
    ///
    /// Called when a new storage node joins the cluster.
    /// Returns the list of migration tasks needed.
    pub async fn add_node_to_rings(
        &self,
        host: String,
        port: u32,
    ) -> Result<Vec<byoridb_common::hash::MigrationTask>> {
        let new_node = RingNode::new(host, port);
        let mut all_migrations = Vec::new();
        let mut space_ids_to_persist = Vec::new();

        // First pass: update rings and compute migrations (sync)
        for entry in self.hash_rings.iter() {
            let space_id = *entry.key();
            let mut ring = entry.value().write();

            // Skip if node already in ring
            if ring.nodes().contains(&new_node) {
                continue;
            }

            // Clone old ring for migration calculation
            let mut old_ring = ring.clone();

            // Add node to ring
            ring.add_node(new_node.clone());

            // Compute migrations
            let migrations = ConsistentHashRing::compute_migrations(&mut old_ring, &mut ring);
            all_migrations.extend(migrations);

            // Update partition allocations
            let allocations = ring.get_allocations();
            self.part_allocations.insert(space_id, allocations);

            space_ids_to_persist.push(space_id);
        }

        // Second pass: persist updated rings (async)
        for space_id in space_ids_to_persist {
            self.persist_ring(space_id).await?;
        }

        info!(
            "Added node {}:{} to {} rings, {} migrations needed",
            new_node.host,
            new_node.port,
            self.hash_rings.len(),
            all_migrations.len()
        );

        Ok(all_migrations)
    }

    /// Remove a storage node from all hash rings
    ///
    /// Called when a storage node leaves the cluster.
    /// Returns the list of migration tasks needed.
    pub async fn remove_node_from_rings(
        &self,
        host: &str,
        port: u32,
    ) -> Result<Vec<byoridb_common::hash::MigrationTask>> {
        let node = RingNode::new(host.to_string(), port);
        let mut all_migrations = Vec::new();
        let mut space_ids_to_persist = Vec::new();

        // First pass: update rings and compute migrations (sync)
        for entry in self.hash_rings.iter() {
            let space_id = *entry.key();
            let mut ring = entry.value().write();

            // Skip if node not in ring
            if !ring.nodes().contains(&node) {
                continue;
            }

            // Clone old ring for migration calculation
            let mut old_ring = ring.clone();

            // Remove node from ring
            ring.remove_node(&node);

            // Compute migrations
            let migrations = ConsistentHashRing::compute_migrations(&mut old_ring, &mut ring);
            all_migrations.extend(migrations);

            // Update partition allocations
            let allocations = ring.get_allocations();
            self.part_allocations.insert(space_id, allocations);

            space_ids_to_persist.push(space_id);
        }

        // Second pass: persist updated rings (async)
        for space_id in space_ids_to_persist {
            self.persist_ring(space_id).await?;
        }

        info!(
            "Removed node {}:{} from rings, {} migrations needed",
            host,
            port,
            all_migrations.len()
        );

        Ok(all_migrations)
    }

    /// Get the hash ring for a space
    pub fn get_ring(
        &self,
        space_id: u32,
    ) -> Option<dashmap::mapref::one::Ref<'_, u32, RwLock<ConsistentHashRing>>> {
        self.hash_rings.get(&space_id)
    }

    /// Check if consistent hashing is enabled for a space
    pub fn has_ring(&self, space_id: u32) -> bool {
        self.hash_rings.contains_key(&space_id)
    }
}
