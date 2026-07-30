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
        let normalized_role = validate_and_normalize_assignable_role(&plan.role)?;

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
        let normalized_role = validate_and_normalize_revocable_role(&plan.role)?;

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
        let operation = match plan {
            crate::plan::BalancePlan::Leader => "LEADER",
            crate::plan::BalancePlan::Data => "DATA",
            crate::plan::BalancePlan::Status => "STATUS",
            crate::plan::BalancePlan::Stop => "STOP",
            crate::plan::BalancePlan::Reset => "RESET",
        };

        Err(ExecutionError::InvalidOperation(format!(
            "BALANCE {} is not supported: no balance-job control RPC is wired to the executor",
            operation
        )))
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
                "Username 'root' is reserved for the built-in superuser".to_string(),
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
            Some(r) => Some(validate_and_normalize_assignable_role(&r)?),
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
                "Root password is managed by process configuration".to_string(),
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
    byoridb_common::crypto::hash_password(password)
        .map_err(|e| ExecutionError::InvalidOperation(format!("Password hashing failed: {}", e)))
}

/// Roles that may be assigned to persisted users. GOD is reserved for the
/// process-owned root identity and must never be persisted on another user.
const ASSIGNABLE_ROLES: &[&str] = &["ADMIN", "DBA", "USER", "GUEST"];
const REVOCABLE_ROLES: &[&str] = &["GOD", "ADMIN", "DBA", "USER", "GUEST"];

/// Validate and normalize a role that is being assigned.
fn validate_and_normalize_assignable_role(role: &str) -> Result<String> {
    let normalized = role.to_uppercase();
    if !ASSIGNABLE_ROLES.contains(&normalized.as_str()) {
        return Err(ExecutionError::InvalidOperation(
            "Role cannot be assigned; valid roles are ADMIN, DBA, USER, and GUEST".to_string(),
        ));
    }
    Ok(normalized)
}

/// GOD remains revocable so operators can repair legacy records that predate
/// the root-only policy, but it cannot be created or granted.
fn validate_and_normalize_revocable_role(role: &str) -> Result<String> {
    let normalized = role.to_uppercase();
    if !REVOCABLE_ROLES.contains(&normalized.as_str()) {
        return Err(ExecutionError::InvalidOperation(
            "Role cannot be revoked; valid roles are GOD, ADMIN, DBA, USER, and GUEST".to_string(),
        ));
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ExecutionContext;
    use crate::plan::{BalancePlan, ExecutionPlan, GrantPlan, RevokePlan};
    use byoridb_kvstore::store::MemoryKVStore;
    use std::sync::Arc;

    fn admin_executor() -> Executor {
        let kvstore = Arc::new(MemoryKVStore::new());
        let ctx = Arc::new(
            ExecutionContext::new(kvstore)
                .with_space("default".to_string())
                .with_caller_roles(vec!["GOD".to_string()]),
        );
        Executor::new(ctx)
    }

    #[tokio::test]
    async fn every_balance_variant_returns_explicit_unsupported_error() {
        let executor = admin_executor();

        for plan in [
            BalancePlan::Leader,
            BalancePlan::Data,
            BalancePlan::Status,
            BalancePlan::Stop,
            BalancePlan::Reset,
        ] {
            let err = executor
                .execute(ExecutionPlan::Balance(plan))
                .await
                .expect_err("BALANCE must not report success until the control RPC exists");
            match err {
                ExecutionError::InvalidOperation(message) => {
                    assert!(message.contains("not supported"), "message was: {message}");
                    assert!(message.contains("balance-job control RPC"));
                }
                other => panic!("expected InvalidOperation, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn create_user_rejects_reserved_root_without_persisting_it() {
        let executor = admin_executor();

        let err = executor
            .handle_create_user(
                "root".to_string(),
                false,
                "not-the-root-password".to_string(),
                Some("GOD".to_string()),
            )
            .await
            .expect_err("the built-in root account must not be recreated in KV");

        match err {
            ExecutionError::InvalidOperation(message) => {
                assert!(message.contains("reserved"), "message was: {message}");
            }
            other => panic!("expected InvalidOperation, got {other:?}"),
        }
        assert!(executor
            .ctx
            .kvstore
            .get(format!("{USER_KEY_PREFIX}root").as_bytes())
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn create_and_grant_reject_god_without_mutating_user_records() {
        let executor = admin_executor();

        let create_error = executor
            .handle_create_user(
                "alice".to_string(),
                false,
                "alice-password".to_string(),
                Some("gOd".to_string()),
            )
            .await
            .unwrap_err();
        assert!(matches!(create_error, ExecutionError::InvalidOperation(_)));

        let user_key = format!("{USER_KEY_PREFIX}alice");
        assert!(executor
            .ctx
            .kvstore
            .get(user_key.as_bytes())
            .await
            .unwrap()
            .is_none());

        executor
            .handle_create_user(
                "alice".to_string(),
                false,
                "alice-password".to_string(),
                Some("USER".to_string()),
            )
            .await
            .unwrap();
        let before = executor
            .ctx
            .kvstore
            .get(user_key.as_bytes())
            .await
            .unwrap()
            .unwrap();
        let grant_error = executor
            .execute_grant(GrantPlan {
                role: "god".to_string(),
                username: "alice".to_string(),
            })
            .await
            .unwrap_err();
        assert!(matches!(grant_error, ExecutionError::InvalidOperation(_)));
        let after = executor
            .ctx
            .kvstore
            .get(user_key.as_bytes())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after, before);

        // Revocation remains available to clean records written by older
        // versions, even though new GOD assignments are rejected.
        executor
            .execute_revoke(RevokePlan {
                role: "GOD".to_string(),
                username: "alice".to_string(),
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn drop_and_alter_user_require_admin_role() {
        let executor = Executor::new(Arc::new(
            ExecutionContext::new(Arc::new(MemoryKVStore::new()))
                .with_caller_roles(vec!["USER".to_string()]),
        ));

        for err in [
            executor
                .handle_drop_user("alice".to_string(), false)
                .await
                .unwrap_err(),
            executor
                .execute_alter_user("alice", Some("new-password".to_string()))
                .await
                .unwrap_err(),
        ] {
            match err {
                ExecutionError::InvalidOperation(message) => {
                    assert!(message.contains("GOD or ADMIN"), "message was: {message}");
                }
                other => panic!("expected InvalidOperation, got {other:?}"),
            }
        }
    }
}
