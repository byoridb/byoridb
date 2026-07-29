// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! Security regressions for authentication, session revocation, and statement RBAC.

use byoridb_common::Value;
use byoridb_graph::error::GraphError;
use byoridb_graph::service::GraphService;
use byoridb_kvstore::{KVStoreOptions, RedbKVStore};
use std::sync::Arc;

const ROOT_PASSWORD: &str = "byoridb-security-test-root-password";

fn create_test_service() -> (GraphService, Arc<RedbKVStore>, tempfile::TempDir) {
    // SAFETY: every test in this binary writes the same process-global value.
    unsafe {
        std::env::set_var("BYORIDB_ROOT_PASSWORD", ROOT_PASSWORD);
    }

    let temp_dir = tempfile::TempDir::new().expect("create temporary database directory");
    let kvstore = Arc::new(
        RedbKVStore::open(temp_dir.path(), KVStoreOptions::default())
            .expect("open temporary database"),
    );
    let service = GraphService::new(kvstore.clone());
    (service, kvstore, temp_dir)
}

async fn authenticate_root(service: &GraphService) -> i64 {
    service
        .authenticate("root".to_string(), ROOT_PASSWORD.to_string())
        .await
        .expect("authenticate root")
}

async fn execute_ok(service: &GraphService, session_id: i64, query: &str) {
    service
        .execute(session_id, query.to_string())
        .await
        .unwrap_or_else(|error| panic!("query failed: {query}: {error}"));
}

#[tokio::test]
async fn guest_cannot_hide_mutations_in_compound_or_profile_statements() {
    let (service, _kvstore, _temp_dir) = create_test_service();
    let root_session = authenticate_root(&service).await;

    execute_ok(
        &service,
        root_session,
        r#"CREATE USER security_guest WITH PASSWORD "guest-pass" ROLE GUEST"#,
    )
    .await;
    let guest_session = service
        .authenticate("security_guest".to_string(), "guest-pass".to_string())
        .await
        .expect("authenticate guest");

    let compound = service
        .execute(
            guest_session,
            "SHOW SPACES; CREATE SPACE compound_security_bypass".to_string(),
        )
        .await;
    assert!(compound.is_err(), "compound mutation must be denied");

    let profile = service
        .execute(
            guest_session,
            "PROFILE CREATE SPACE profile_security_bypass".to_string(),
        )
        .await;
    assert!(profile.is_err(), "PROFILE mutation must be denied");

    let spaces = service
        .execute(root_session, "SHOW SPACES".to_string())
        .await
        .expect("show spaces as root");
    let leaked_space = spaces.rows.iter().flatten().any(|value| {
        matches!(
            value,
            Value::String(name)
                if name == "compound_security_bypass" || name == "profile_security_bypass"
        )
    });
    assert!(
        !leaked_space,
        "denied statements must not partially execute"
    );
}

#[tokio::test]
async fn session_listing_is_admin_only_and_never_returns_bearer_ids() {
    let (service, _kvstore, _temp_dir) = create_test_service();
    let root_session = authenticate_root(&service).await;
    execute_ok(
        &service,
        root_session,
        r#"CREATE USER sessions_guest WITH PASSWORD "guest-pass" ROLE GUEST"#,
    )
    .await;
    let guest_session = service
        .authenticate("sessions_guest".to_string(), "guest-pass".to_string())
        .await
        .expect("authenticate guest");

    assert!(
        service
            .execute(guest_session, "SHOW SESSIONS".to_string())
            .await
            .is_err(),
        "guest must not list sessions"
    );

    let sessions = service
        .execute(root_session, "SHOW SESSIONS".to_string())
        .await
        .expect("admin may list redacted session metadata");
    assert!(
        !sessions
            .column_names
            .iter()
            .any(|column| column.eq_ignore_ascii_case("sessionid")),
        "SHOW SESSIONS must not expose bearer credential columns"
    );
    assert!(
        !sessions.rows.iter().flatten().any(
            |value| matches!(value, Value::Int(id) if *id == root_session || *id == guest_session)
        ),
        "SHOW SESSIONS must not expose active bearer credentials"
    );
}

#[tokio::test]
async fn role_password_and_user_changes_revoke_existing_sessions() {
    let (service, _kvstore, _temp_dir) = create_test_service();
    let root_session = authenticate_root(&service).await;

    execute_ok(
        &service,
        root_session,
        r#"CREATE USER mutable_admin WITH PASSWORD "old-pass" ROLE ADMIN"#,
    )
    .await;
    let admin_session = service
        .authenticate("mutable_admin".to_string(), "old-pass".to_string())
        .await
        .expect("authenticate admin");
    execute_ok(&service, admin_session, "CREATE SPACE before_admin_revoke").await;

    execute_ok(
        &service,
        root_session,
        "REVOKE ROLE ADMIN FROM mutable_admin",
    )
    .await;
    assert!(matches!(
        service
            .execute(admin_session, "SHOW SPACES".to_string())
            .await,
        Err(GraphError::SessionNotFound(_))
    ));

    execute_ok(&service, root_session, "GRANT ROLE USER TO mutable_admin").await;
    let user_session = service
        .authenticate("mutable_admin".to_string(), "old-pass".to_string())
        .await
        .expect("authenticate user before password change");
    execute_ok(
        &service,
        root_session,
        r#"ALTER USER mutable_admin WITH PASSWORD "new-pass""#,
    )
    .await;
    assert!(matches!(
        service
            .execute(user_session, "SHOW SPACES".to_string())
            .await,
        Err(GraphError::SessionNotFound(_))
    ));
    assert!(
        service
            .authenticate("mutable_admin".to_string(), "old-pass".to_string())
            .await
            .is_err(),
        "old password must stop working immediately"
    );

    let changed_session = service
        .authenticate("mutable_admin".to_string(), "new-pass".to_string())
        .await
        .expect("new password must work immediately");
    execute_ok(&service, root_session, "DROP USER mutable_admin").await;
    assert!(matches!(
        service
            .execute(changed_session, "SHOW SPACES".to_string())
            .await,
        Err(GraphError::SessionNotFound(_))
    ));
    assert!(
        service
            .authenticate("mutable_admin".to_string(), "new-pass".to_string())
            .await
            .is_err(),
        "dropped user must not authenticate"
    );
}

#[tokio::test]
async fn persisted_users_authenticate_after_service_recreation_and_root_is_reserved() {
    let (service, kvstore, _temp_dir) = create_test_service();
    let root_session = authenticate_root(&service).await;
    execute_ok(
        &service,
        root_session,
        r#"CREATE USER persisted_user WITH PASSWORD "persisted-pass" ROLE USER"#,
    )
    .await;

    let duplicate_root = service
        .execute(
            root_session,
            r#"CREATE USER root WITH PASSWORD "replacement" ROLE GOD"#.to_string(),
        )
        .await;
    assert!(duplicate_root.is_err(), "root username must stay reserved");

    let recreated = GraphService::new(kvstore);
    let persisted_session = recreated
        .authenticate("persisted_user".to_string(), "persisted-pass".to_string())
        .await
        .expect("persisted user must authenticate in a fresh service");
    recreated
        .execute(persisted_session, "SHOW SPACES".to_string())
        .await
        .expect("persisted USER role must be restored");
}

#[tokio::test]
async fn malformed_credential_statements_do_not_echo_passwords_in_errors() {
    let (service, _kvstore, _temp_dir) = create_test_service();
    let root_session = authenticate_root(&service).await;
    let secret = "must-not-appear-in-errors";

    let error = service
        .execute(
            root_session,
            format!("CREATE USER malformed WITH PASSWORD {secret}"),
        )
        .await
        .expect_err("unquoted password must fail parsing");

    assert!(
        !error.to_string().contains(secret),
        "credential parse errors must not echo the supplied password"
    );
}
