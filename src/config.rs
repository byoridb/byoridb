use config::{Config, Environment, File};
use serde::Deserialize;
use std::net::SocketAddr;

#[derive(Debug, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub storage: StorageConfig,
    #[serde(default)]
    pub cluster: ClusterConfig,
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

        builder.build()?.try_deserialize()
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
