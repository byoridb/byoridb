// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use super::*;

impl MetaService {
    pub async fn create_edge(
        &self,
        space_id: u32,
        name: String,
        fields: Vec<Field>,
    ) -> Result<u32> {
        info!("Creating edge {} in space {}", name, space_id);

        // Check if space exists
        self.get_space(space_id).await?;

        // Check if edge already exists (lock-free read)
        if self.edge_names.contains_key(&(space_id, name.clone())) {
            return Err(MetaError::EdgeAlreadyExists(name));
        }

        // Generate edge ID atomically
        let edge_id = self.next_edge_id.fetch_add(1, Ordering::SeqCst);

        let edge = EdgeSchema {
            id: edge_id,
            space_id,
            name: name.clone(),
            version: 1,
            fields,
        };

        // Store edge (fine-grained locking)
        self.edges.insert((space_id, edge_id), edge.clone());
        self.edge_names.insert((space_id, name.clone()), edge_id);

        // Persist to KV store
        let key = MetaKey::edge(space_id, edge_id);
        let value = serde_json::to_vec(&edge)?;
        self.kvstore.put(&key, &value).await?;

        debug!("Created edge {} with ID {}", name, edge_id);
        Ok(edge_id)
    }

    /// Get an edge schema by name (lock-free read)
    pub async fn get_edge(&self, space_id: u32, name: &str) -> Result<EdgeSchema> {
        let edge_id = self
            .edge_names
            .get(&(space_id, name.to_string()))
            .map(|r| *r.value())
            .ok_or_else(|| MetaError::EdgeNotFound(name.to_string()))?;

        self.get_edge_by_id(space_id, edge_id).await
    }

    /// Get an edge schema by ID (lock-free read)
    pub async fn get_edge_by_id(&self, space_id: u32, edge_id: u32) -> Result<EdgeSchema> {
        self.edges
            .get(&(space_id, edge_id))
            .map(|r| r.value().clone())
            .ok_or_else(|| MetaError::EdgeNotFound(format!("ID {}", edge_id)))
    }

    /// List all edges in a space (lock-free iteration)
    pub async fn list_edges(&self, space_id: u32) -> Result<Vec<EdgeSchema>> {
        Ok(self
            .edges
            .iter()
            .filter(|r| r.value().space_id == space_id)
            .map(|r| r.value().clone())
            .collect())
    }

    /// Drop an edge
    pub async fn drop_edge(&self, space_id: u32, name: &str) -> Result<()> {
        info!("Dropping edge {} in space {}", name, space_id);

        let (_, edge_id) = self
            .edge_names
            .remove(&(space_id, name.to_string()))
            .ok_or_else(|| MetaError::EdgeNotFound(name.to_string()))?;

        self.edges.remove(&(space_id, edge_id));

        // Remove from KV store
        let key = MetaKey::edge(space_id, edge_id);
        self.kvstore.delete(&key).await?;

        debug!("Dropped edge {} (ID {})", name, edge_id);
        Ok(())
    }

    /// Alter an edge schema (add/drop/modify columns)
    ///
    /// Returns the new version number after the ALTER operation.
    /// For ADD COLUMN, the new column must be nullable or have a default value.
    pub async fn alter_edge(
        &self,
        space_id: u32,
        name: &str,
        operations: Vec<AlterOperation>,
    ) -> Result<i32> {
        info!("Altering edge {} in space {}", name, space_id);

        // Get current edge
        let edge_id = self
            .edge_names
            .get(&(space_id, name.to_string()))
            .map(|r| *r.value())
            .ok_or_else(|| MetaError::EdgeNotFound(name.to_string()))?;

        let current_edge = self
            .edges
            .get(&(space_id, edge_id))
            .map(|r| r.value().clone())
            .ok_or_else(|| MetaError::EdgeNotFound(name.to_string()))?;

        // Clone fields and apply operations
        let mut new_fields = current_edge.fields.clone();

        for op in &operations {
            match op {
                AlterOperation::AddColumn(field) => {
                    // Validate: new column must be nullable or have a default
                    if !field.nullable && field.default.is_none() {
                        return Err(MetaError::InvalidAlterOperation(format!(
                            "Column '{}' must be nullable or have a default value for ADD COLUMN",
                            field.name
                        )));
                    }

                    // Check if field already exists
                    if new_fields.iter().any(|f| f.name == field.name) {
                        return Err(MetaError::FieldAlreadyExists(field.name.clone()));
                    }

                    new_fields.push(field.clone());
                }
                AlterOperation::DropColumn(col_name) => {
                    let before = new_fields.len();
                    new_fields.retain(|f| &f.name != col_name);
                    if new_fields.len() == before {
                        return Err(MetaError::InvalidAlterOperation(format!(
                            "Column '{}' does not exist",
                            col_name
                        )));
                    }
                }
                AlterOperation::ChangeColumn(field) => {
                    let entry = new_fields
                        .iter_mut()
                        .find(|f| f.name == field.name)
                        .ok_or_else(|| {
                            MetaError::InvalidAlterOperation(format!(
                                "Column '{}' does not exist",
                                field.name
                            ))
                        })?;
                    *entry = field.clone();
                }
            }
        }

        let new_version = current_edge.version + 1;

        // Store old version with version suffix for history
        let history_key = MetaKey::edge_version(space_id, edge_id, current_edge.version);
        let history_value = serde_json::to_vec(&current_edge)?;
        // Create new edge with updated fields and version
        let new_edge = EdgeSchema {
            id: edge_id,
            space_id,
            name: name.to_string(),
            version: new_version,
            fields: new_fields,
        };

        // Update in-memory
        self.edges.insert((space_id, edge_id), new_edge.clone());

        // Decode keys and values for batch
        let key = MetaKey::edge(space_id, edge_id);
        let value = serde_json::to_vec(&new_edge)?;

        // Persist history and new version atomically
        self.kvstore
            .batch_put(vec![(history_key, history_value), (key, value)])
            .await?;

        debug!(
            "Altered edge {}, new version {} with {} fields",
            name,
            new_version,
            new_edge.fields.len()
        );
        Ok(new_version)
    }

    /// Get all versions of an edge schema
    pub async fn get_edge_versions(&self, space_id: u32, name: &str) -> Result<Vec<EdgeSchema>> {
        let edge_id = self
            .edge_names
            .get(&(space_id, name.to_string()))
            .map(|r| *r.value())
            .ok_or_else(|| MetaError::EdgeNotFound(name.to_string()))?;

        let current_edge = self
            .edges
            .get(&(space_id, edge_id))
            .map(|r| r.value().clone())
            .ok_or_else(|| MetaError::EdgeNotFound(name.to_string()))?;

        let mut versions = Vec::new();

        // Load historical versions from KV store
        for version in 1..current_edge.version {
            let key = MetaKey::edge_version(space_id, edge_id, version);
            if let Some(value) = self.kvstore.get(&key).await? {
                match serde_json::from_slice::<EdgeSchema>(&value) {
                    Ok(edge) => versions.push(edge),
                    Err(e) => {
                        warn!(
                            "Failed to deserialize history version {} for edge {}: {}",
                            version, name, e
                        );
                        // Skip corrupted version
                    }
                }
            }
        }

        // Add current version
        versions.push(current_edge);

        Ok(versions)
    }
}
