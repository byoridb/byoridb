// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use super::*;

impl MetaService {
    pub async fn create_tag(&self, space_id: u32, name: String, fields: Vec<Field>) -> Result<u32> {
        info!("Creating tag {} in space {}", name, space_id);

        // Check if space exists
        self.get_space(space_id).await?;

        // Check if tag already exists (lock-free read)
        if self.tag_names.contains_key(&(space_id, name.clone())) {
            return Err(MetaError::TagAlreadyExists(name));
        }

        // Generate tag ID atomically
        let tag_id = self.next_tag_id.fetch_add(1, Ordering::SeqCst);

        let tag = TagSchema {
            id: tag_id,
            space_id,
            name: name.clone(),
            version: 1,
            fields,
        };

        // Store tag (fine-grained locking)
        self.tags.insert((space_id, tag_id), tag.clone());
        self.tag_names.insert((space_id, name.clone()), tag_id);

        // Persist to KV store
        let key = MetaKey::tag(space_id, tag_id);
        let value = serde_json::to_vec(&tag)?;
        self.kvstore.put(&key, &value).await?;

        debug!("Created tag {} with ID {}", name, tag_id);
        Ok(tag_id)
    }

    /// Get a tag schema by name (lock-free read)
    pub async fn get_tag(&self, space_id: u32, name: &str) -> Result<TagSchema> {
        let tag_id = self
            .tag_names
            .get(&(space_id, name.to_string()))
            .map(|r| *r.value())
            .ok_or_else(|| MetaError::TagNotFound(name.to_string()))?;

        self.get_tag_by_id(space_id, tag_id).await
    }

    /// Get a tag schema by ID (lock-free read)
    pub async fn get_tag_by_id(&self, space_id: u32, tag_id: u32) -> Result<TagSchema> {
        self.tags
            .get(&(space_id, tag_id))
            .map(|r| r.value().clone())
            .ok_or_else(|| MetaError::TagNotFound(format!("ID {}", tag_id)))
    }

    /// List all tags in a space (lock-free iteration)
    pub async fn list_tags(&self, space_id: u32) -> Result<Vec<TagSchema>> {
        Ok(self
            .tags
            .iter()
            .filter(|r| r.value().space_id == space_id)
            .map(|r| r.value().clone())
            .collect())
    }

    /// Drop a tag
    pub async fn drop_tag(&self, space_id: u32, name: &str) -> Result<()> {
        info!("Dropping tag {} in space {}", name, space_id);

        let (_, tag_id) = self
            .tag_names
            .remove(&(space_id, name.to_string()))
            .ok_or_else(|| MetaError::TagNotFound(name.to_string()))?;

        self.tags.remove(&(space_id, tag_id));

        // Remove from KV store
        let key = MetaKey::tag(space_id, tag_id);
        self.kvstore.delete(&key).await?;

        debug!("Dropped tag {} (ID {})", name, tag_id);
        Ok(())
    }

    /// Alter a tag schema (add/drop/modify columns)
    ///
    /// Returns the new version number after the ALTER operation.
    /// For ADD COLUMN, the new column must be nullable or have a default value.
    pub async fn alter_tag(
        &self,
        space_id: u32,
        name: &str,
        operations: Vec<AlterOperation>,
    ) -> Result<i32> {
        info!("Altering tag {} in space {}", name, space_id);

        // Get current tag
        let tag_id = self
            .tag_names
            .get(&(space_id, name.to_string()))
            .map(|r| *r.value())
            .ok_or_else(|| MetaError::TagNotFound(name.to_string()))?;

        let current_tag = self
            .tags
            .get(&(space_id, tag_id))
            .map(|r| r.value().clone())
            .ok_or_else(|| MetaError::TagNotFound(name.to_string()))?;

        // Clone fields and apply operations
        let mut new_fields = current_tag.fields.clone();

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

        let new_version = current_tag.version + 1;

        // Store old version with version suffix for history
        let history_key = MetaKey::tag_version(space_id, tag_id, current_tag.version);
        let history_value = serde_json::to_vec(&current_tag)?;
        // Create new tag with updated fields and version
        let new_tag = TagSchema {
            id: tag_id,
            space_id,
            name: name.to_string(),
            version: new_version,
            fields: new_fields,
        };

        // Update in-memory
        self.tags.insert((space_id, tag_id), new_tag.clone());

        // Decode keys and values for batch
        let key = MetaKey::tag(space_id, tag_id);
        let value = serde_json::to_vec(&new_tag)?;

        // Persist history and new version atomically
        self.kvstore
            .batch_put(vec![(history_key, history_value), (key, value)])
            .await?;

        debug!(
            "Altered tag {}, new version {} with {} fields",
            name,
            new_version,
            new_tag.fields.len()
        );
        Ok(new_version)
    }

    /// Get all versions of a tag schema
    pub async fn get_tag_versions(&self, space_id: u32, name: &str) -> Result<Vec<TagSchema>> {
        let tag_id = self
            .tag_names
            .get(&(space_id, name.to_string()))
            .map(|r| *r.value())
            .ok_or_else(|| MetaError::TagNotFound(name.to_string()))?;

        let current_tag = self
            .tags
            .get(&(space_id, tag_id))
            .map(|r| r.value().clone())
            .ok_or_else(|| MetaError::TagNotFound(name.to_string()))?;

        let mut versions = Vec::new();

        // Load historical versions from KV store
        for version in 1..current_tag.version {
            let key = MetaKey::tag_version(space_id, tag_id, version);
            if let Some(value) = self.kvstore.get(&key).await? {
                match serde_json::from_slice::<TagSchema>(&value) {
                    Ok(tag) => versions.push(tag),
                    Err(e) => {
                        warn!(
                            "Failed to deserialize history version {} for tag {}: {}",
                            version, name, e
                        );
                        // Skip corrupted version
                    }
                }
            }
        }

        // Add current version
        versions.push(current_tag);

        Ok(versions)
    }
}
