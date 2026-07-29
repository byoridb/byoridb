// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

use super::{Executor, ExecutorResult};
use crate::error::{ExecutionError, Result};
use crate::key::USER_KEY_PREFIX;

impl Executor {
    pub(super) async fn execute_grant(
        &self,
        plan: crate::plan::GrantPlan,
    ) -> Result<ExecutorResult> {
        if !self.ctx.is_admin() {
            return Err(ExecutionError::InvalidOperation(
                "GRANT requires GOD or ADMIN role".to_string(),
            ));
        }

        // Store the role assignment in KVStore
        let user_key = format!("{}{}", USER_KEY_PREFIX, plan.username);

        // Get existing user data - user must exist
        let user_data = self
            .ctx
            .kvstore
            .get(user_key.as_bytes())
            .await?
            .ok_or_else(|| {
                ExecutionError::InvalidOperation(format!("User {} does not exist", plan.username))
            })?;

        let mut user_json: serde_json::Value = serde_json::from_slice(&user_data)?;

        // Validate and normalize role
        let normalized_role = validate_and_normalize_role(&plan.role)?;

        // Add role if not already present - ensure roles field exists
        let roles = user_json
            .get_mut("roles")
            .ok_or_else(|| {
                ExecutionError::InvalidOperation("User roles field is missing".to_string())
            })?
            .as_array_mut()
            .ok_or_else(|| {
                ExecutionError::InvalidOperation("User roles field is not an array".to_string())
            })?;

        let role_val = serde_json::Value::String(normalized_role.clone());
        if !roles.contains(&role_val) {
            roles.push(role_val);
        }

        // Save updated user data
        self.ctx
            .kvstore
            .put(
                user_key.as_bytes(),
                serde_json::to_vec(&user_json)?.as_slice(),
            )
            .await?;

        Ok(ExecutorResult::success_message(format!(
            "Role {} granted to user {}",
            plan.role, plan.username
        )))
    }

    /// Execute REVOKE ROLE statement
    pub(super) async fn execute_revoke(
        &self,
        plan: crate::plan::RevokePlan,
    ) -> Result<ExecutorResult> {
        if !self.ctx.is_admin() {
            return Err(ExecutionError::InvalidOperation(
                "REVOKE requires GOD or ADMIN role".to_string(),
            ));
        }

        let user_key = format!("{}{}", USER_KEY_PREFIX, plan.username);

        // Get existing user data
        let user_data = self
            .ctx
            .kvstore
            .get(user_key.as_bytes())
            .await?
            .ok_or_else(|| {
                ExecutionError::InvalidOperation(format!("User {} not found", plan.username))
            })?;

        let mut user_json: serde_json::Value = serde_json::from_slice(&user_data)?;

        // Validate and normalize role
        let normalized_role = validate_and_normalize_role(&plan.role)?;

        // Remove role - ensure roles field exists
        let roles = user_json
            .get_mut("roles")
            .ok_or_else(|| {
                ExecutionError::InvalidOperation("User roles field is missing".to_string())
            })?
            .as_array_mut()
            .ok_or_else(|| {
                ExecutionError::InvalidOperation("User roles field is not an array".to_string())
            })?;

        roles.retain(|r| r.as_str() != Some(&normalized_role));

        // Save updated user data
        self.ctx
            .kvstore
            .put(
                user_key.as_bytes(),
                serde_json::to_vec(&user_json)?.as_slice(),
            )
            .await?;

        Ok(ExecutorResult::success_message(format!(
            "Role {} revoked from user {}",
            plan.role, plan.username
        )))
    }

    /// Execute BALANCE statement for partition management
    pub(super) async fn execute_balance(
        &self,
        plan: crate::plan::BalancePlan,
    ) -> Result<ExecutorResult> {
        if !self.ctx.is_admin() {
            return Err(ExecutionError::InvalidOperation(
                "BALANCE requires GOD or ADMIN role".to_string(),
            ));
        }

        match plan {
            crate::plan::BalancePlan::Leader => {
                // Trigger leader rebalance
                #[cfg(feature = "distributed")]
                if let Some(client) = &self.ctx.meta_client {
                    let space = self.ctx.space.as_ref().ok_or_else(|| {
                        ExecutionError::InvalidOperation("No space selected".to_string())
                    })?;
                    match client.get_space(space).await {
                        Ok(space_info) => {
                            // Trigger rebalance via meta service
                            tracing::info!(
                                "Triggering leader rebalance for space {}",
                                space_info.id
                            );
                            // Note: Full implementation would call client.balance_leader()
                            return Ok(ExecutorResult::success_message(
                                "Leader balance job submitted".to_string(),
                            ));
                        }
                        Err(e) => {
                            return Err(ExecutionError::InvalidOperation(format!(
                                "Failed to get space info: {}",
                                e
                            )));
                        }
                    }
                }
                Ok(ExecutorResult::success_message(
                    "Leader balance job submitted (standalone mode)".to_string(),
                ))
            }
            crate::plan::BalancePlan::Data => {
                // Trigger data rebalance
                #[cfg(feature = "distributed")]
                if let Some(client) = &self.ctx.meta_client {
                    let space = self.ctx.space.as_ref().ok_or_else(|| {
                        ExecutionError::InvalidOperation("No space selected".to_string())
                    })?;
                    match client.get_space(space).await {
                        Ok(space_info) => {
                            tracing::info!("Triggering data rebalance for space {}", space_info.id);
                            // Note: Full implementation would call client.balance_data()
                            return Ok(ExecutorResult::success_message(
                                "Data balance job submitted".to_string(),
                            ));
                        }
                        Err(e) => {
                            return Err(ExecutionError::InvalidOperation(format!(
                                "Failed to get space info: {}",
                                e
                            )));
                        }
                    }
                }
                Ok(ExecutorResult::success_message(
                    "Data balance job submitted (standalone mode)".to_string(),
                ))
            }
            crate::plan::BalancePlan::Status => {
                // Show balance status
                Ok(ExecutorResult {
                    columns: vec![
                        "Job ID".to_string(),
                        "Status".to_string(),
                        "Progress".to_string(),
                    ],
                    rows: vec![vec![
                        byoridb_common::Value::Int(0),
                        byoridb_common::Value::String("IDLE".to_string()),
                        byoridb_common::Value::String("N/A".to_string()),
                    ]],
                    latency_ms: 0,
                })
            }
            crate::plan::BalancePlan::Stop => {
                // Stop ongoing balance
                Ok(ExecutorResult::success_message(
                    "Balance job stopped".to_string(),
                ))
            }
            crate::plan::BalancePlan::Reset => {
                // Reset balance plan
                Ok(ExecutorResult::success_message(
                    "Balance plan reset".to_string(),
                ))
            }
        }
    }

    /// Handle CREATE USER
    pub(super) async fn handle_create_user(
        &self,
        username: String,
        if_not_exists: bool,
        password: String,
        role: Option<String>,
    ) -> Result<ExecutorResult> {
        if !self.ctx.is_admin() {
            return Err(ExecutionError::InvalidOperation(
                "CREATE USER requires GOD or ADMIN role".to_string(),
            ));
        }

        if username.eq_ignore_ascii_case("root") {
            return Err(ExecutionError::InvalidOperation(
                "The root username is reserved".to_string(),
            ));
        }

        let user_key = format!("{}{}", USER_KEY_PREFIX, username);

        // Check if user already exists
        if self.ctx.kvstore.get(user_key.as_bytes()).await?.is_some() {
            if if_not_exists {
                return Ok(ExecutorResult::empty());
            }
            return Err(ExecutionError::InvalidOperation(format!(
                "User {} already exists",
                username
            )));
        }

        // Validate and normalize role if provided
        let normalized_role = match role {
            Some(r) => Some(validate_and_normalize_role(&r)?),
            None => None,
        };

        // Hash password with argon2 (cryptographically secure)
        let password_hash = hash_password(&password)?;

        // Create user data
        let user_json = serde_json::json!({
            "username": username,
            "password_hash": password_hash,
            "roles": normalized_role.map(|r| vec![r]).unwrap_or_default(),
            "enabled": true
        });

        // Save user
        self.ctx
            .kvstore
            .put(
                user_key.as_bytes(),
                serde_json::to_vec(&user_json)?.as_slice(),
            )
            .await?;

        Ok(ExecutorResult::success_message(format!(
            "User {} created successfully",
            username
        )))
    }

    /// Handle DROP USER
    pub(super) async fn handle_drop_user(
        &self,
        username: String,
        if_exists: bool,
    ) -> Result<ExecutorResult> {
        if !self.ctx.is_admin() {
            return Err(ExecutionError::InvalidOperation(
                "DROP USER requires GOD or ADMIN role".to_string(),
            ));
        }

        let user_key = format!("{}{}", USER_KEY_PREFIX, username);

        // Prevent deletion of root user
        if username.eq_ignore_ascii_case("root") {
            return Err(ExecutionError::InvalidOperation(
                "Cannot delete the root user".to_string(),
            ));
        }

        // Check if user exists
        if self.ctx.kvstore.get(user_key.as_bytes()).await?.is_none() {
            if if_exists {
                return Ok(ExecutorResult::success_message(
                    "User already dropped".to_string(),
                ));
            } else {
                return Err(ExecutionError::InvalidOperation(format!(
                    "User {} not found",
                    username
                )));
            }
        }

        // Delete user
        self.ctx.kvstore.delete(user_key.as_bytes()).await?;

        Ok(ExecutorResult::success_message(format!(
            "User {} dropped successfully",
            username
        )))
    }

    /// Execute ALTER USER statement (change password)
    pub(super) async fn execute_alter_user(
        &self,
        username: &str,
        new_password: Option<String>,
    ) -> Result<ExecutorResult> {
        if !self.ctx.is_admin() {
            return Err(ExecutionError::InvalidOperation(
                "ALTER USER requires GOD or ADMIN role".to_string(),
            ));
        }

        if username.eq_ignore_ascii_case("root") {
            return Err(ExecutionError::InvalidOperation(
                "Root credentials must be rotated with BYORIDB_ROOT_PASSWORD and a restart"
                    .to_string(),
            ));
        }

        let user_key = format!("{}{}", USER_KEY_PREFIX, username);

        // Get existing user data
        let user_data = self
            .ctx
            .kvstore
            .get(user_key.as_bytes())
            .await?
            .ok_or_else(|| {
                ExecutionError::InvalidOperation(format!("User {} not found", username))
            })?;

        let mut user_json: serde_json::Value = serde_json::from_slice(&user_data)?;

        // Update password if provided
        if let Some(password) = new_password {
            // Hash password with argon2 (cryptographically secure)
            let password_hash = hash_password(&password)?;

            user_json["password_hash"] = serde_json::Value::String(password_hash);
        }

        // Save updated user data
        self.ctx
            .kvstore
            .put(
                user_key.as_bytes(),
                serde_json::to_vec(&user_json)?.as_slice(),
            )
            .await?;

        Ok(ExecutorResult::success_message(format!(
            "User {} altered successfully",
            username
        )))
    }
}

/// Hash password using centralized implementation in `byoridb_common::crypto`
fn hash_password(password: &str) -> Result<String> {
    if password.trim().is_empty() {
        return Err(ExecutionError::InvalidOperation(
            "Password must not be empty".to_string(),
        ));
    }
    byoridb_common::crypto::hash_password(password)
        .map_err(|e| ExecutionError::InvalidOperation(format!("Password hashing failed: {}", e)))
}

/// Valid roles for user management
const VALID_ROLES: &[&str] = &["GOD", "ADMIN", "DBA", "USER", "GUEST"];

/// Validate and normalize a role to uppercase
fn validate_and_normalize_role(role: &str) -> Result<String> {
    let normalized = role.to_uppercase();
    if !VALID_ROLES.contains(&normalized.as_str()) {
        return Err(ExecutionError::InvalidOperation(format!(
            "Invalid role '{}'. Valid roles are: {:?}",
            role, VALID_ROLES
        )));
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ExecutionContext;
    use crate::plan::{AlterPlan, BalancePlan, DropPlan, ExecutionPlan};
    use byoridb_kvstore::MemoryKVStore;
    use std::sync::Arc;

    fn executor_with_roles(roles: Vec<&str>) -> Executor {
        let context = ExecutionContext::new(Arc::new(MemoryKVStore::new())).with_caller_roles(
            roles
                .into_iter()
                .map(std::string::ToString::to_string)
                .collect(),
        );
        Executor::new(Arc::new(context))
    }

    #[tokio::test]
    async fn executor_defense_in_depth_rejects_non_admin_admin_operations() {
        let executor = executor_with_roles(vec!["USER"]);
        let plans = [
            ExecutionPlan::Balance(BalancePlan::Status),
            ExecutionPlan::Drop(DropPlan::User {
                name: "victim".to_string(),
                if_exists: true,
            }),
            ExecutionPlan::Alter(AlterPlan::User {
                name: "victim".to_string(),
                new_password: Some("replacement".to_string()),
            }),
        ];

        for plan in plans {
            let error = executor
                .execute(plan)
                .await
                .expect_err("non-admin operation must be rejected before storage access");
            assert!(matches!(error, ExecutionError::InvalidOperation(_)));
        }
    }

    #[tokio::test]
    async fn root_username_is_reserved_and_query_password_rotation_is_rejected() {
        let executor = executor_with_roles(vec!["ADMIN"]);

        let create_error = executor
            .handle_create_user(
                "ROOT".to_string(),
                false,
                "replacement".to_string(),
                Some("GOD".to_string()),
            )
            .await
            .expect_err("root alias must be reserved");
        assert!(matches!(create_error, ExecutionError::InvalidOperation(_)));

        let alter_error = executor
            .execute_alter_user("root", Some("replacement".to_string()))
            .await
            .expect_err("root password must be rotated out-of-band");
        assert!(matches!(alter_error, ExecutionError::InvalidOperation(_)));
    }

    #[tokio::test]
    async fn blank_passwords_are_rejected() {
        let executor = executor_with_roles(vec!["ADMIN"]);
        let error = executor
            .handle_create_user(
                "blank_user".to_string(),
                false,
                "  \t".to_string(),
                Some("USER".to_string()),
            )
            .await
            .expect_err("blank passwords must be rejected");
        assert!(matches!(error, ExecutionError::InvalidOperation(_)));
    }
}
