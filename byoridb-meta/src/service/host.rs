// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use super::*;

impl MetaService {
    pub fn handle_heartbeat(
        &self,
        host: String,
        port: u32,
        role: &str,
        req_cluster_id: i64,
    ) -> Result<HeartbeatInfo> {
        // Cluster identity check: reject nodes that present a wrong cluster_id.
        // cluster_id == 0 is allowed for first-time registration (node doesn't
        // know the cluster_id yet and will learn it from the response).
        if req_cluster_id != 0 && req_cluster_id != self.cluster_id {
            warn!(
                "Heartbeat rejected from {}:{}: cluster_id mismatch (got {}, expected {})",
                host, port, req_cluster_id, self.cluster_id
            );
            return Err(MetaError::InvalidOperation(format!(
                "cluster_id mismatch: expected {}, got {}",
                self.cluster_id, req_cluster_id
            )));
        }

        if role != "storage" {
            // Only storage nodes feed the `storage_hosts` registry today, but
            // graph / meta / unknown roles still hit this handler. Log at
            // debug so the heartbeat is at least observable instead of a
            // silent Ok(..). Extending the registry to other roles is a
            // follow-up item (see MOCK_REMEDIATION_PLAN.md Item 15).
            debug!(
                role = role,
                "Heartbeat from non-storage node {}:{} acknowledged but not tracked", host, port
            );
            return Ok(HeartbeatInfo {
                cluster_id: self.cluster_id,
            });
        }

        let key = (host.clone(), port);
        let now = Instant::now();

        self.storage_hosts
            .entry(key.clone())
            .and_modify(|info| {
                info.last_heartbeat = now;
                info.status = HostStatus::Online;
            })
            .or_insert(StorageHostInfo {
                host: host.clone(),
                port,
                last_heartbeat: now,
                status: HostStatus::Online,
                partitions: HashSet::new(),
            });

        info!("Heartbeat received from storage node {}:{}", host, port);
        Ok(HeartbeatInfo {
            cluster_id: self.cluster_id,
        })
    }

    /// Get list of active storage hosts
    ///
    /// Returns hosts that have sent a heartbeat within the timeout period.
    pub fn get_active_storage_hosts(&self) -> Vec<(String, u32)> {
        let timeout = self.host_timeout;
        let now = Instant::now();

        self.storage_hosts
            .iter()
            .filter(|entry| now.duration_since(entry.value().last_heartbeat) < timeout)
            .map(|entry| (entry.value().host.clone(), entry.value().port))
            .collect()
    }

    /// Get all registered storage hosts (including stale ones)
    pub fn get_all_storage_hosts(&self) -> Vec<StorageHostInfo> {
        self.storage_hosts
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Remove stale hosts that haven't sent heartbeat within timeout
    ///
    /// Call this periodically to clean up dead nodes.
    pub fn cleanup_stale_hosts(&self) -> Vec<(String, u32)> {
        let timeout = self.host_timeout;
        let now = Instant::now();
        let mut removed = Vec::new();

        self.storage_hosts.retain(|key, info| {
            let is_stale = now.duration_since(info.last_heartbeat) >= timeout;
            if is_stale {
                warn!("Removing stale host {}:{}", key.0, key.1);
                removed.push(key.clone());
            }
            !is_stale
        });

        removed
    }

    /// Get the number of registered storage hosts
    pub fn storage_host_count(&self) -> usize {
        self.storage_hosts.len()
    }

    /// Summarise all registered storage hosts with live leader / partition counts.
    ///
    /// The freshness of each host is evaluated against `host_timeout`:
    /// - hosts whose last heartbeat is within the timeout are reported `Online`;
    /// - older hosts are reported `Offline` but still included so operators can
    ///   see nodes that recently dropped out.
    ///
    /// Leader count is computed by cross-referencing `part_allocations`: a host
    /// is counted as leader for every partition where it appears as the first
    /// host in the allocation list (matching the convention used in
    /// `execute_show_parts`).
    pub fn list_hosts_with_counts(&self) -> Vec<HostSummary> {
        let now = Instant::now();
        let timeout = self.host_timeout;

        // Build leader-count index: (host, port) -> count.
        let mut leader_counts: HashMap<(String, u32), u64> = HashMap::new();
        for entry in self.part_allocations.iter() {
            for hosts in entry.value().values() {
                if let Some((h, p)) = hosts.first() {
                    *leader_counts.entry((h.clone(), *p)).or_insert(0) += 1;
                }
            }
        }

        self.storage_hosts
            .iter()
            .map(|entry| {
                let info = entry.value();
                let online = now.duration_since(info.last_heartbeat) < timeout;
                let status = if online {
                    HostStatus::Online
                } else {
                    HostStatus::Offline
                };
                let leader_count = leader_counts
                    .get(&(info.host.clone(), info.port))
                    .copied()
                    .unwrap_or(0);
                HostSummary {
                    host: info.host.clone(),
                    port: info.port,
                    status,
                    leader_count,
                    part_count: info.partitions.len() as u64,
                }
            })
            .collect()
    }

    /// Get all partitions owned by a specific host
    pub fn get_host_partitions(&self, host: &str, port: u32) -> Vec<(u32, u32)> {
        let key = (host.to_string(), port);
        if let Some(info) = self.storage_hosts.get(&key) {
            info.partitions.iter().cloned().collect()
        } else {
            vec![]
        }
    }

    /// Add a partition to a host's partition set
    pub fn add_host_partition(&self, host: &str, port: u32, space_id: u32, part_id: u32) {
        let key = (host.to_string(), port);
        if let Some(mut info) = self.storage_hosts.get_mut(&key) {
            info.partitions.insert((space_id, part_id));
        }
    }

    /// Remove a partition from a host's partition set
    pub fn remove_host_partition(&self, host: &str, port: u32, space_id: u32, part_id: u32) {
        let key = (host.to_string(), port);
        if let Some(mut info) = self.storage_hosts.get_mut(&key) {
            info.partitions.remove(&(space_id, part_id));
        }
    }
}
