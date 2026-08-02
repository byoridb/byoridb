use anyhow::{anyhow, Result};
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;

pub mod graph_proto {
    tonic::include_proto!("graph");
}

use graph_proto::graph_service_client::GraphServiceClient;
use graph_proto::{AuthenticateRequest, ExecuteRequest, SignOutRequest};

/// Render a proto `DataSet` to a flat string for the legacy `execute()`
/// caller. Each row becomes one comma-separated line, columns header on
/// top. The structured-proto path through `execute_raw` is preferable for
/// programmatic consumers.
fn format_dataset(ds: &graph_proto::DataSet) -> String {
    use graph_proto::value::Value as PV;
    let mut out = String::new();
    if !ds.column_names.is_empty() {
        out.push_str(&ds.column_names.join(", "));
        out.push('\n');
    }
    for row in &ds.rows {
        let cells: Vec<String> = row
            .values
            .iter()
            .map(|v| match &v.value {
                None => "NULL".to_string(),
                Some(PV::NullValue(_)) => "NULL".to_string(),
                Some(PV::BoolValue(b)) => b.to_string(),
                Some(PV::IntValue(i)) => i.to_string(),
                Some(PV::FloatValue(f)) => f.to_string(),
                Some(PV::StringValue(s)) => s.clone(),
                Some(PV::JsonValue(j)) => j.clone(),
            })
            .collect();
        out.push_str(&cells.join(", "));
        out.push('\n');
    }
    out
}

pub struct Client {
    client: GraphServiceClient<Channel>,
    session_id: i64,
}

impl Client {
    /// Connect to the ByoriDB server with compression enabled.
    ///
    /// `username` and `password` are required — the client no longer falls
    /// back to built-in root credentials, which was unsafe outside of
    /// local development. Callers should source credentials from CLI flags,
    /// environment variables, or a configuration file.
    pub async fn connect(addr: String, username: String, password: String) -> Result<Self> {
        if username.is_empty() {
            return Err(anyhow!(
                "username is required (was empty); pass --user or set BYORIDB_USER"
            ));
        }
        if password.is_empty() {
            return Err(anyhow!(
                "password is required (was empty); pass --password or set BYORIDB_PASSWORD"
            ));
        }

        let endpoint = if addr.starts_with("http") {
            addr
        } else {
            format!("http://{}", addr)
        };

        // Connect with compression support (Gzip and Zstd)
        // This reduces network I/O by 30-50% for large query results
        let channel = Channel::from_shared(endpoint)?
            .connect()
            .await
            .map_err(|e| anyhow!("Failed to connect: {}", e))?;

        let mut client = GraphServiceClient::new(channel)
            .accept_compressed(CompressionEncoding::Gzip)
            .accept_compressed(CompressionEncoding::Zstd)
            .send_compressed(CompressionEncoding::Gzip);

        let request = tonic::Request::new(AuthenticateRequest { username, password });

        let response = client.authenticate(request).await?.into_inner();

        if response.error_code != 0 {
            return Err(anyhow!("Authentication failed: {}", response.error_msg));
        }

        Ok(Self {
            client,
            session_id: response.session_id,
        })
    }

    pub async fn execute(&mut self, stmt: &str) -> Result<String> {
        let response = self.execute_raw(stmt).await?;
        // Prefer the structured `result` field added in PR 9; fall back to
        // the legacy JSON-encoded `data` bytes for servers that haven't
        // upgraded yet.
        if let Some(result) = response.result {
            Ok(format_dataset(&result))
        } else {
            #[allow(deprecated)]
            let legacy_data = response.data;
            String::from_utf8(legacy_data)
                .map_err(|_| anyhow!("Failed to parse response data as utf8"))
        }
    }

    /// Execute a query and return the structured proto response. Lets
    /// callers walk rows / columns / typed values directly instead of
    /// re-parsing JSON.
    pub async fn execute_raw(&mut self, stmt: &str) -> Result<graph_proto::ExecuteResponse> {
        let request = tonic::Request::new(ExecuteRequest {
            session_id: self.session_id,
            statement: stmt.to_string(),
        });
        let response = self.client.execute(request).await?.into_inner();
        if response.error_code != 0 {
            return Err(anyhow!("Execution error: {}", response.error_msg));
        }
        Ok(response)
    }

    pub async fn execute_json(&mut self, stmt: &str) -> Result<serde_json::Value> {
        let request = tonic::Request::new(ExecuteRequest {
            session_id: self.session_id,
            statement: stmt.to_string(),
        });

        let response = self.client.execute_json(request).await?.into_inner();

        if response.error_code != 0 {
            return Err(anyhow!("Execution error: {}", response.error_msg));
        }

        let value: serde_json::Value = serde_json::from_str(&response.json_data)
            .map_err(|e| anyhow!("Failed to parse JSON response: {}", e))?;

        Ok(value)
    }

    pub async fn close(&mut self) -> Result<()> {
        let request = tonic::Request::new(SignOutRequest {
            session_id: self.session_id,
        });
        let response = self.client.sign_out(request).await?.into_inner();
        if response.error_code != 0 {
            return Err(anyhow!("Sign out error: {}", response.error_msg));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Empty credentials must Err *before* any network is contacted so
    /// misconfigured CI runs fail fast with a clear message instead of
    /// silently authenticating with hard-coded root credentials.
    #[tokio::test]
    async fn connect_rejects_empty_username() {
        let err = Client::connect(
            "127.0.0.1:0".to_string(),
            String::new(),
            "password".to_string(),
        )
        .await
        .err()
        .expect("empty username must fail before reaching the network");
        let msg = err.to_string();
        assert!(
            msg.contains("username"),
            "error should mention username: {}",
            msg
        );
    }

    #[test]
    fn format_dataset_renders_primitives_in_column_order() {
        use graph_proto::value::Value as PV;
        let ds = graph_proto::DataSet {
            column_names: vec!["a".into(), "b".into(), "c".into()],
            rows: vec![graph_proto::Row {
                values: vec![
                    graph_proto::Value {
                        value: Some(PV::IntValue(1)),
                    },
                    graph_proto::Value {
                        value: Some(PV::StringValue("hi".into())),
                    },
                    graph_proto::Value {
                        value: Some(PV::NullValue(0)),
                    },
                ],
            }],
        };
        let s = format_dataset(&ds);
        assert!(s.starts_with("a, b, c\n"), "missing header: {:?}", s);
        assert!(s.contains("1, hi, NULL"), "row not formatted: {:?}", s);
    }

    #[tokio::test]
    async fn connect_rejects_empty_password() {
        let err = Client::connect("127.0.0.1:0".to_string(), "user".to_string(), String::new())
            .await
            .err()
            .expect("empty password must fail before reaching the network");
        let msg = err.to_string();
        assert!(
            msg.contains("password"),
            "error should mention password: {}",
            msg
        );
    }
}
