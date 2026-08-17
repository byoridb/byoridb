use config::{Config, Environment, File};
use serde::Deserialize;
use std::net::SocketAddr;

#[derive(Debug, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub storage: StorageConfig,
    #[serde(default)]
    pub cluster: ClusterConfig,
    #[serde(default)]
    pub auth: AuthConfig,
}

/// Cluster / distributed mode configuration.
///
/// When `peers` is empty (default) the server runs in single-node standalone
/// mode. Set `BYORIDB__CLUSTER__PEERS=host1:9559,host2:9559` to enable the
/// distributed launcher (G-2).
#[derive(Debug, Deserialize)]
pub struct ClusterConfig {
    /// Numeric node identity in the Raft group (default: 1).
    /// Must be unique within the cluster.
    /// Env: `BYORIDB__CLUSTER__NODE_ID`
    pub node_id: u32,
    /// Peer meta-server addresses, comma-separated.
    /// Env: `BYORIDB__CLUSTER__PEERS`  (e.g. `node2:9559,node3:9559`)
    pub peers: Vec<String>,
    /// Advertise address for this node's Meta gRPC endpoint.
    /// Env: `BYORIDB__CLUSTER__ADVERTISE_ADDR`
    pub advertise_addr: String,
    /// If true, this node bootstraps a new cluster on first start.
    /// Env: `BYORIDB__CLUSTER__BOOTSTRAP`
    #[allow(dead_code)]
    pub bootstrap: bool,
    /// Meta gRPC listen address.
    /// Env: `BYORIDB__CLUSTER__META_ADDR`
    pub meta_addr: String,
}

impl Default for ClusterConfig {
    fn default() -> Self {
        ClusterConfig {
            node_id: 1,
            peers: vec![],
            advertise_addr: "127.0.0.1:9559".to_string(),
            bootstrap: false,
            meta_addr: "0.0.0.0:9559".to_string(),
        }
    }
}

/// Brute-force login protection.
///
/// The defaults are the values the engine compiles in and are the right choice
/// for an exposed listener. They are settable because they are wrong for a
/// single-user deployment bound to a loopback address, where a mistyped secret
/// locks the only account and no second administrator exists to recover it.
///
/// **Relaxing any of these is only safe when the listener is restricted at the
/// network boundary.** Nothing is adjusted automatically from the bind address:
/// the trust decision belongs to the operator and is recorded here.
#[derive(Debug, Deserialize)]
pub struct AuthConfig {
    /// Sliding window over which login *failures* are counted, in seconds.
    /// Env: `BYORIDB__AUTH__LOGIN_WINDOW_SECS`
    pub login_window_secs: u64,
    /// Failures allowed per username per window.
    /// Env: `BYORIDB__AUTH__MAX_ACCOUNT_FAILURES_PER_WINDOW`
    pub max_account_failures_per_window: usize,
    /// Failures allowed per peer address per window, across all usernames.
    /// Env: `BYORIDB__AUTH__MAX_SOURCE_FAILURES_PER_WINDOW`
    pub max_source_failures_per_window: usize,
    /// Simultaneous Argon2 verifications. Logins beyond this queue rather than
    /// failing, so it bounds CPU cost and not the number of sessions.
    /// Env: `BYORIDB__AUTH__MAX_CONCURRENT_VERIFICATIONS`
    pub max_concurrent_verifications: usize,
    /// Consecutive failures that lock an existing account.
    /// Env: `BYORIDB__AUTH__MAX_FAILED_ATTEMPTS`
    pub max_failed_attempts: u32,
    /// How long that lockout lasts, in seconds. `0` disables the lockout,
    /// leaving the window budgets above as the only throttle — the reason a
    /// single-user deployment would set any of this.
    /// Env: `BYORIDB__AUTH__LOCKOUT_DURATION_SECS`
    pub lockout_duration_secs: u64,
}

impl Default for AuthConfig {
    fn default() -> Self {
        let engine = byoridb_graph::auth::LoginThrottleConfig::default();
        AuthConfig {
            login_window_secs: engine.window.as_secs(),
            max_account_failures_per_window: engine.max_account_failures_per_window,
            max_source_failures_per_window: engine.max_source_failures_per_window,
            max_concurrent_verifications: engine.max_concurrent_verifications,
            max_failed_attempts: engine.max_failed_attempts,
            lockout_duration_secs: engine.lockout_duration.as_secs(),
        }
    }
}

impl AuthConfig {
    /// Translate into the engine's policy type.
    ///
    /// Validation lives on that type, so a value that would degenerate a
    /// control is rejected identically whether it arrived from `byoridb.toml`,
    /// the environment, or an embedded caller.
    pub fn to_throttle(&self) -> byoridb_graph::auth::LoginThrottleConfig {
        byoridb_graph::auth::LoginThrottleConfig {
            window: std::time::Duration::from_secs(self.login_window_secs),
            max_account_failures_per_window: self.max_account_failures_per_window,
            max_source_failures_per_window: self.max_source_failures_per_window,
            max_concurrent_verifications: self.max_concurrent_verifications,
            max_failed_attempts: self.max_failed_attempts,
            lockout_duration: std::time::Duration::from_secs(self.lockout_duration_secs),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    pub graph_addr: SocketAddr,
    pub http_addr: SocketAddr,
    #[allow(dead_code)]
    pub storage_addr: SocketAddr,
}

#[derive(Debug, Deserialize)]
pub struct StorageConfig {
    pub data_paths: Vec<String>,
}

impl AppConfig {
    pub fn load() -> Result<Self, config::ConfigError> {
        let auth_defaults = AuthConfig::default();
        let builder = Config::builder()
            // Start with default values
            .set_default("server.graph_addr", "0.0.0.0:9669")?
            .set_default("server.http_addr", "0.0.0.0:19669")?
            .set_default("server.storage_addr", "0.0.0.0:44500")?
            .set_default("storage.data_paths", vec!["data/storage"])?
            .set_default("cluster.node_id", 1)?
            .set_default("cluster.peers", Vec::<String>::new())?
            .set_default("cluster.advertise_addr", "127.0.0.1:9559")?
            .set_default("cluster.bootstrap", false)?
            .set_default("cluster.meta_addr", "0.0.0.0:9559")?
            .set_default("auth.login_window_secs", auth_defaults.login_window_secs)?
            .set_default(
                "auth.max_account_failures_per_window",
                auth_defaults.max_account_failures_per_window as u64,
            )?
            .set_default(
                "auth.max_source_failures_per_window",
                auth_defaults.max_source_failures_per_window as u64,
            )?
            .set_default(
                "auth.max_concurrent_verifications",
                auth_defaults.max_concurrent_verifications as u64,
            )?
            .set_default(
                "auth.max_failed_attempts",
                auth_defaults.max_failed_attempts as u64,
            )?
            .set_default(
                "auth.lockout_duration_secs",
                auth_defaults.lockout_duration_secs,
            )?
            // Add configuration file (optional)
            .add_source(File::with_name("byoridb").required(false))
            // Environment variables: BYORIDB__SECTION__KEY → section.key.
            // prefix_separator="__" ensures K8s service auto-injected envs
            // (BYORIDB_PUBLIC_SERVICE_HOST, BYORIDB_ROOT_PASSWORD, ...) are
            // ignored rather than misparsed.
            // list_separator + with_list_parse_key lets `Vec<String>` fields
            // be supplied as comma-separated env values.
            .add_source(
                Environment::with_prefix("BYORIDB")
                    .prefix_separator("__")
                    .separator("__")
                    .try_parsing(true)
                    .list_separator(",")
                    .with_list_parse_key("storage.data_paths")
                    .with_list_parse_key("cluster.peers"),
            );

        let config: Self = builder.build()?.try_deserialize()?;
        // Reject a degenerate throttle here rather than at first login. A zero
        // window or a zero budget refuses every login including a correct one,
        // so it must fail at startup where an operator sees it, not silently
        // become the lockout it was meant to prevent.
        config
            .auth
            .to_throttle()
            .validate()
            .map_err(|problem| config::ConfigError::Message(format!("[auth] {problem}")))?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serializes all tests that touch process-level env vars to prevent races
    // when cargo runs tests on multiple threads within the same binary.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    fn with_env<F: FnOnce()>(vars: &[(&str, &str)], f: F) {
        let _guard = ENV_MUTEX.lock().unwrap();
        for (k, v) in vars {
            std::env::set_var(k, v);
        }
        f();
        for (k, _) in vars {
            std::env::remove_var(k);
        }
    }

    #[test]
    fn defaults_load_without_env() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let cfg = AppConfig::load().expect("defaults must load");
        assert_eq!(cfg.storage.data_paths, vec!["data/storage".to_string()]);
        assert_eq!(cfg.server.graph_addr.to_string(), "0.0.0.0:9669");
    }

    #[test]
    fn data_paths_parses_single_value_from_env() {
        with_env(&[("BYORIDB__STORAGE__DATA_PATHS", "/app/data")], || {
            let cfg = AppConfig::load().expect("single-value list must parse");
            assert_eq!(cfg.storage.data_paths, vec!["/app/data".to_string()]);
        });
    }

    #[test]
    fn data_paths_parses_comma_separated_list_from_env() {
        with_env(
            &[("BYORIDB__STORAGE__DATA_PATHS", "/data/a,/data/b,/data/c")],
            || {
                let cfg = AppConfig::load().expect("multi-value list must parse");
                assert_eq!(
                    cfg.storage.data_paths,
                    vec![
                        "/data/a".to_string(),
                        "/data/b".to_string(),
                        "/data/c".to_string(),
                    ]
                );
            },
        );
    }

    #[test]
    fn environment_keys_remain_case_insensitive_for_scalars_and_lists() {
        // config 0.15 restored case-sensitive config paths, while its
        // Environment source still normalizes environment keys to lowercase.
        // Guard both a scalar and an explicitly list-parsed key because our
        // deployment convention uses uppercase BYORIDB__SECTION__KEY names.
        with_env(
            &[
                ("bYoRiDb__SeRvEr__HtTp_AdDr", "127.0.0.1:29669"),
                (
                    "bYoRiDb__ClUsTeR__pEeRs",
                    "node2.example:9559,node3.example:9559",
                ),
            ],
            || {
                let cfg = AppConfig::load().expect("mixed-case environment keys must parse");
                assert_eq!(cfg.server.http_addr.to_string(), "127.0.0.1:29669");
                assert_eq!(
                    cfg.cluster.peers,
                    vec![
                        "node2.example:9559".to_string(),
                        "node3.example:9559".to_string(),
                    ]
                );
            },
        );
    }

    #[test]
    fn auth_defaults_are_the_engine_policy_unchanged() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let cfg = AppConfig::load().expect("defaults must load");
        let engine = byoridb_graph::auth::LoginThrottleConfig::default();

        // Defaults must not drift from the engine's compiled-in policy: making
        // these settable is not an occasion to change what they are.
        assert_eq!(cfg.auth.to_throttle(), engine);
        assert_eq!(cfg.auth.login_window_secs, 60);
        assert_eq!(cfg.auth.max_account_failures_per_window, 20);
        assert_eq!(cfg.auth.max_source_failures_per_window, 60);
        assert_eq!(cfg.auth.max_concurrent_verifications, 4);
        assert_eq!(cfg.auth.max_failed_attempts, 5);
        assert_eq!(cfg.auth.lockout_duration_secs, 300);
    }

    #[test]
    fn auth_policy_is_settable_from_the_environment() {
        with_env(
            &[
                ("BYORIDB__AUTH__LOGIN_WINDOW_SECS", "30"),
                ("BYORIDB__AUTH__MAX_ACCOUNT_FAILURES_PER_WINDOW", "200"),
                ("BYORIDB__AUTH__MAX_SOURCE_FAILURES_PER_WINDOW", "500"),
                ("BYORIDB__AUTH__MAX_CONCURRENT_VERIFICATIONS", "8"),
                ("BYORIDB__AUTH__MAX_FAILED_ATTEMPTS", "50"),
                // The single-user case: no lockout at all.
                ("BYORIDB__AUTH__LOCKOUT_DURATION_SECS", "0"),
            ],
            || {
                let cfg = AppConfig::load().expect("a relaxed auth policy must load");
                let throttle = cfg.auth.to_throttle();
                assert_eq!(throttle.window, std::time::Duration::from_secs(30));
                assert_eq!(throttle.max_account_failures_per_window, 200);
                assert_eq!(throttle.max_source_failures_per_window, 500);
                assert_eq!(throttle.max_concurrent_verifications, 8);
                assert_eq!(throttle.max_failed_attempts, 50);
                assert_eq!(throttle.lockout_duration, std::time::Duration::ZERO);
            },
        );
    }

    #[test]
    fn a_degenerate_auth_policy_is_rejected_at_startup() {
        // Each of these would refuse every login, including a correct one, so
        // loading must fail where an operator sees it rather than turning into
        // the lockout the setting was meant to avoid.
        for key in [
            "BYORIDB__AUTH__LOGIN_WINDOW_SECS",
            "BYORIDB__AUTH__MAX_ACCOUNT_FAILURES_PER_WINDOW",
            "BYORIDB__AUTH__MAX_SOURCE_FAILURES_PER_WINDOW",
            "BYORIDB__AUTH__MAX_CONCURRENT_VERIFICATIONS",
            "BYORIDB__AUTH__MAX_FAILED_ATTEMPTS",
        ] {
            with_env(&[(key, "0")], || {
                let error = AppConfig::load()
                    .expect_err("a zero value must not load")
                    .to_string();
                assert!(
                    error.contains("[auth]"),
                    "the error must name the section responsible: {error}"
                );
            });
        }

        // A zero lockout duration is a supported way to disable the lockout.
        with_env(&[("BYORIDB__AUTH__LOCKOUT_DURATION_SECS", "0")], || {
            AppConfig::load().expect("disabling the lockout must be allowed");
        });
    }

    #[test]
    fn k8s_auto_injected_service_envs_are_ignored() {
        // Simulate K8s' enableServiceLinks behavior: prefix is BYORIDB_ (single
        // underscore), which must NOT collide with BYORIDB__SECTION__KEY.
        with_env(
            &[
                ("BYORIDB_PUBLIC_SERVICE_HOST", "10.0.0.42"),
                ("BYORIDB_HEADLESS_PORT", "tcp://10.0.0.43:9669"),
                ("BYORIDB_ROOT_PASSWORD", "ignored-by-config-crate"),
            ],
            || {
                let cfg = AppConfig::load().expect("k8s service envs must not break load");
                // Defaults still hold; auto-injected envs did not poison the tree.
                assert_eq!(cfg.storage.data_paths, vec!["data/storage".to_string()]);
            },
        );
    }
}
