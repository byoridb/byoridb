// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use super::*;

impl MetaService {
    pub async fn create_tag_index(
        &self,
        space_id: u32,
        index_name: String,
        tag_name: String,
        fields: Vec<String>,
    ) -> Result<u32> {
        info!("Creating tag index {} on {}", index_name, tag_name);

        // Check if tag exists
        let tag = self.get_tag(space_id, &tag_name).await?;

        // Validate fields exist in tag
        for field in &fields {
            if !tag.fields.iter().any(|f| &f.name == field) {
                return Err(MetaError::FieldNotFound(field.clone()));
            }
        }

        // Check if index already exists (lock-free read)
        if self
            .tag_index_names
            .contains_key(&(space_id, index_name.clone()))
        {
            return Err(MetaError::IndexAlreadyExists(index_name));
        }

        // Generate index ID atomically
        let index_id = self.next_tag_index_id.fetch_add(1, Ordering::SeqCst);

        let index = TagIndex {
            id: index_id,
            space_id,
            index_name: index_name.clone(),
            tag_id: tag.id,
            fields,
        };

        // Store index (fine-grained locking)
        self.tag_indexes.insert((space_id, index_id), index.clone());
        self.tag_index_names
            .insert((space_id, index_name.clone()), index_id);

        // Persist to KV store
        let key = MetaKey::tag_index(space_id, index_id);
        let value = serde_json::to_vec(&index)?;
        self.kvstore.put(&key, &value).await?;

        debug!("Created tag index {} with ID {}", index_name, index_id);
        Ok(index_id)
    }

    /// Create an edge index
    pub async fn create_edge_index(
        &self,
        space_id: u32,
        index_name: String,
        edge_name: String,
        fields: Vec<String>,
    ) -> Result<u32> {
        info!("Creating edge index {} on {}", index_name, edge_name);

        // Check if edge exists
        let edge = self.get_edge(space_id, &edge_name).await?;

        // Validate fields exist in edge
        for field in &fields {
            if !edge.fields.iter().any(|f| &f.name == field) {
                return Err(MetaError::FieldNotFound(field.clone()));
            }
        }

        // Check if index already exists (lock-free read)
        if self
            .edge_index_names
            .contains_key(&(space_id, index_name.clone()))
        {
            return Err(MetaError::IndexAlreadyExists(index_name));
        }

        // Generate index ID atomically
        let index_id = self.next_edge_index_id.fetch_add(1, Ordering::SeqCst);

        let index = EdgeIndex {
            id: index_id,
            space_id,
            index_name: index_name.clone(),
            edge_type: edge.id,
            fields,
        };

        // Store index (fine-grained locking)
        self.edge_indexes
            .insert((space_id, index_id), index.clone());
        self.edge_index_names
            .insert((space_id, index_name.clone()), index_id);

        // Persist to KV store
        let key = MetaKey::edge_index(space_id, index_id);
        let value = serde_json::to_vec(&index)?;
        self.kvstore.put(&key, &value).await?;

        debug!("Created edge index {} with ID {}", index_name, index_id);
        Ok(index_id)
    }

    /// Get a tag index by name (lock-free read)
    pub async fn get_tag_index(&self, space_id: u32, name: &str) -> Result<TagIndex> {
        let index_id = self
            .tag_index_names
            .get(&(space_id, name.to_string()))
            .map(|r| *r.value())
            .ok_or_else(|| MetaError::IndexNotFound(name.to_string()))?;

        self.tag_indexes
            .get(&(space_id, index_id))
            .map(|r| r.value().clone())
            .ok_or_else(|| MetaError::IndexNotFound(name.to_string()))
    }

    /// Get an edge index by name (lock-free read)
    pub async fn get_edge_index(&self, space_id: u32, name: &str) -> Result<EdgeIndex> {
        let index_id = self
            .edge_index_names
            .get(&(space_id, name.to_string()))
            .map(|r| *r.value())
            .ok_or_else(|| MetaError::IndexNotFound(name.to_string()))?;

        self.edge_indexes
            .get(&(space_id, index_id))
            .map(|r| r.value().clone())
            .ok_or_else(|| MetaError::IndexNotFound(name.to_string()))
    }

    /// List tag indexes (lock-free iteration)
    pub async fn list_tag_indexes(&self, space_id: u32) -> Result<Vec<TagIndex>> {
        Ok(self
            .tag_indexes
            .iter()
            .filter(|r| r.value().space_id == space_id)
            .map(|r| r.value().clone())
            .collect())
    }

    /// List edge indexes (lock-free iteration)
    pub async fn list_edge_indexes(&self, space_id: u32) -> Result<Vec<EdgeIndex>> {
        Ok(self
            .edge_indexes
            .iter()
            .filter(|r| r.value().space_id == space_id)
            .map(|r| r.value().clone())
            .collect())
    }

    /// Drop a tag index
    pub async fn drop_tag_index(&self, space_id: u32, name: &str) -> Result<()> {
        info!("Dropping tag index {}", name);

        let (_, index_id) = self
            .tag_index_names
            .remove(&(space_id, name.to_string()))
            .ok_or_else(|| MetaError::IndexNotFound(name.to_string()))?;

        self.tag_indexes.remove(&(space_id, index_id));

        // Remove from KV store
        let key = MetaKey::tag_index(space_id, index_id);
        self.kvstore.delete(&key).await?;

        debug!("Dropped tag index {} (ID {})", name, index_id);
        Ok(())
    }

    /// Drop an edge index
    pub async fn drop_edge_index(&self, space_id: u32, name: &str) -> Result<()> {
        info!("Dropping edge index {}", name);

        let (_, index_id) = self
            .edge_index_names
            .remove(&(space_id, name.to_string()))
            .ok_or_else(|| MetaError::IndexNotFound(name.to_string()))?;

        self.edge_indexes.remove(&(space_id, index_id));

        // Remove from KV store
        let key = MetaKey::edge_index(space_id, index_id);
        self.kvstore.delete(&key).await?;

        debug!("Dropped edge index {} (ID {})", name, index_id);
        Ok(())
    }

    /// Get tag indexes for a specific tag (lock-free iteration)
    pub async fn get_tag_indexes_for_tag(
        &self,
        space_id: u32,
        tag_id: u32,
    ) -> Result<Vec<TagIndex>> {
        Ok(self
            .tag_indexes
            .iter()
            .filter(|r| r.value().space_id == space_id && r.value().tag_id == tag_id)
            .map(|r| r.value().clone())
            .collect())
    }

    /// Get edge indexes for a specific edge type (lock-free iteration)
    pub async fn get_edge_indexes_for_edge(
        &self,
        space_id: u32,
        edge_type: u32,
    ) -> Result<Vec<EdgeIndex>> {
        Ok(self
            .edge_indexes
            .iter()
            .filter(|r| r.value().space_id == space_id && r.value().edge_type == edge_type)
            .map(|r| r.value().clone())
            .collect())
    }
}
