#![cfg(feature = "server")]

use byoridb_graph::grpc::graph_proto::{
    graph_service_client::GraphServiceClient, graph_service_server::GraphServiceServer,
    value::Value, AuthenticateRequest, ExecuteRequest,
};
use byoridb_graph::grpc::GrpcService;
use byoridb_graph::{AuthManager, GraphService};
use byoridb_kvstore::MemoryKVStore;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::codec::CompressionEncoding;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::{Endpoint, Server};

// Exercise the real protobuf/service boundary, including both a large request
// and a large response. Merely enabling codec features would not detect a
// negotiation regression or a broken encoder/decoder dependency upgrade.
async fn round_trip(encoding: Option<CompressionEncoding>, expected_header: Option<&'static str>) {
    let service = Arc::new(GraphService::with_auth(
        Arc::new(MemoryKVStore::new()),
        AuthManager::with_config("compression-test-password", Duration::from_secs(60)),
    ));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let request_count = Arc::new(AtomicUsize::new(0));
    let received = request_count.clone();
    let wire_service = GraphServiceServer::new(GrpcService::new(service))
        .accept_compressed(CompressionEncoding::Gzip)
        .accept_compressed(CompressionEncoding::Zstd)
        .send_compressed(CompressionEncoding::Gzip)
        .send_compressed(CompressionEncoding::Zstd);
    let wire_service = InterceptedService::new(wire_service, move |request: tonic::Request<()>| {
        assert_eq!(
            request
                .metadata()
                .get("grpc-encoding")
                .map(|value| value.to_str().unwrap()),
            expected_header,
            "the request must actually use the selected wire encoding"
        );
        received.fetch_add(1, Ordering::Relaxed);
        Ok(request)
    });
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        Server::builder()
            .add_service(wire_service)
            .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                let _ = shutdown_rx.await;
            })
            .await
            .unwrap();
    });

    let channel = Endpoint::from_shared(format!("http://{address}"))
        .unwrap()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(5))
        .connect()
        .await
        .unwrap();
    let mut client = GraphServiceClient::new(channel);
    if let Some(encoding) = encoding {
        client = client.send_compressed(encoding).accept_compressed(encoding);
    }
    let authenticated = client
        .authenticate(AuthenticateRequest {
            username: "root".to_owned(),
            password: "compression-test-password".to_owned(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(authenticated.error_code, 0, "{}", authenticated.error_msg);
    let session_id = authenticated.session_id;
    // Large individual literals exercise the iterative lexer as well as codec
    // streaming. Keep the dual legacy/protobuf response below the default 4MiB.
    let payload = format!(
        "agent evidence 한글 🐈\n{}",
        "abcdefghijklmnopqrstuvwxyz".repeat(8192)
    );
    for statement in [
        "CREATE SPACE compression (partition_num=1, replica_factor=1)".to_owned(),
        "USE compression".to_owned(),
        "CREATE TAG document (body string)".to_owned(),
    ]
    .into_iter()
    .chain((1..=4).map(|id| {
        format!(
            "INSERT VERTEX document(body) VALUES {id}:({})",
            serde_json::to_string(&payload).unwrap()
        )
    })) {
        let response = client
            .execute(ExecuteRequest {
                session_id,
                statement,
                read_only: false,
            })
            .await
            .unwrap()
            .into_inner();
        assert_eq!(response.error_code, 0, "{}", response.error_msg);
    }
    let response = client
        .execute(ExecuteRequest {
            session_id,
            statement: "MATCH (n:document) RETURN n.document.body AS body".to_owned(),
            read_only: true,
        })
        .await
        .unwrap();
    assert_eq!(
        response
            .metadata()
            .get("grpc-encoding")
            .map(|value| value.to_str().unwrap()),
        expected_header,
        "the response must negotiate the encoding the client accepts"
    );
    let response = response.into_inner();
    assert_eq!(response.error_code, 0, "{}", response.error_msg);
    let result = response.result.unwrap();
    assert_eq!(result.column_names, ["body"]);
    assert_eq!(result.rows.len(), 4);
    for row in result.rows {
        assert_eq!(row.values.len(), 1);
        assert_eq!(
            row.values[0].value,
            Some(Value::StringValue(payload.clone()))
        );
    }
    assert_eq!(request_count.load(Ordering::Relaxed), 9);

    drop(client);
    shutdown_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn gzip_request_and_response_round_trip() {
    round_trip(Some(CompressionEncoding::Gzip), Some("gzip")).await;
}

#[tokio::test]
async fn zstd_request_and_response_round_trip() {
    round_trip(Some(CompressionEncoding::Zstd), Some("zstd")).await;
}

#[tokio::test]
async fn uncompressed_client_remains_compatible() {
    round_trip(None, None).await;
}
