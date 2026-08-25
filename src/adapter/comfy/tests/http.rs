use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;

use futures_util::SinkExt;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::accept_async;

use super::super::*;
use crate::adapter::CameraAdapter;

struct MockResponse {
    status: &'static str,
    body: Value,
}

async fn adapter_for(endpoint: String) -> ComfyAdapter {
    let mut config = super::config(PathBuf::from("unused-workflow.json"));
    config.endpoint = endpoint;
    ComfyAdapter::new(config).unwrap()
}

fn prepared_job() -> PreparedJob {
    PreparedJob {
        request_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_owned(),
        client_id: "client-1".to_owned(),
        workflow_hash: "workflow-hash".to_owned(),
        output_node: "120".to_owned(),
        output_prefix: "sparkstage/01ARZ3NDEKTSV4RRFFQ69G5FAV".to_owned(),
        workflow: json!({"1": {"class_type": "Test", "inputs": {}}}),
    }
}

async fn spawn_http_sequence(
    responses: Vec<MockResponse>,
) -> (String, mpsc::UnboundedReceiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/", listener.local_addr().unwrap());
    let (request_tx, request_rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut responses = VecDeque::from(responses);
        while let Some(response) = responses.pop_front() {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_http_request(&mut stream).await;
            let _ = request_tx.send(request);
            write_json_response(&mut stream, response).await;
        }
    });
    (endpoint, request_rx)
}

async fn read_http_request(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut chunk = [0_u8; 1024];
        let read = stream.read(&mut chunk).await.unwrap();
        assert!(read > 0, "client closed before sending HTTP headers");
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let header = String::from_utf8_lossy(&bytes[..header_end]);
    let content_length = header
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    while bytes.len() < header_end + content_length {
        let mut chunk = [0_u8; 1024];
        let read = stream.read(&mut chunk).await.unwrap();
        assert!(read > 0, "client closed before sending HTTP body");
        bytes.extend_from_slice(&chunk[..read]);
    }
    String::from_utf8(bytes).unwrap()
}

async fn write_json_response(stream: &mut TcpStream, response: MockResponse) {
    let body = serde_json::to_vec(&response.body).unwrap();
    write_response(stream, response.status, "application/json", &body).await;
}

async fn write_response(stream: &mut TcpStream, status: &str, content_type: &str, body: &[u8]) {
    let header = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).await.unwrap();
    stream.write_all(body).await.unwrap();
}

#[tokio::test]
async fn submit_posts_prompt_with_stable_request_identity() {
    let (endpoint, mut requests) = spawn_http_sequence(vec![MockResponse {
        status: "200 OK",
        body: json!({"prompt_id": "prompt-1"}),
    }])
    .await;
    let adapter = adapter_for(endpoint).await;

    let backend_id = adapter.submit(&prepared_job()).await.unwrap();

    assert_eq!(backend_id, BackendJobId("prompt-1".to_owned()));
    let request = requests.recv().await.unwrap();
    let (head, body) = request.split_once("\r\n\r\n").unwrap();
    assert!(head.starts_with("POST /prompt HTTP/1.1"));
    let body: Value = serde_json::from_str(body).unwrap();
    assert_eq!(body["client_id"], "client-1");
    assert_eq!(
        body["extra_data"]["sparkstage_request_id"],
        "01ARZ3NDEKTSV4RRFFQ69G5FAV"
    );
    assert_eq!(body["prompt"], prepared_job().workflow);
}

#[tokio::test]
async fn submit_rejects_empty_prompt_id() {
    let (endpoint, _) = spawn_http_sequence(vec![MockResponse {
        status: "200 OK",
        body: json!({"prompt_id": "  "}),
    }])
    .await;
    let adapter = adapter_for(endpoint).await;

    let error = adapter.submit(&prepared_job()).await.unwrap_err();

    assert!(matches!(error, AdapterError::Backend(message) if message.contains("empty prompt_id")));
}

#[tokio::test]
async fn submit_connection_drop_is_reported_as_http_uncertainty() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_http_request(&mut stream).await;
        assert!(request.starts_with("POST /prompt HTTP/1.1"));
    });
    let adapter = adapter_for(endpoint).await;

    let error = adapter.submit(&prepared_job()).await.unwrap_err();

    assert!(matches!(error, AdapterError::Http(_)));
}

#[tokio::test]
async fn reconcile_maps_history_success_and_execution_failure() {
    let prompt_id = BackendJobId("prompt-1".to_owned());
    let (success_endpoint, _) = spawn_http_sequence(vec![MockResponse {
        status: "200 OK",
        body: json!({"prompt-1": {
            "status": {"status_str": "success", "completed": true}
        }}),
    }])
    .await;
    let success = adapter_for(success_endpoint)
        .await
        .reconcile(&prompt_id)
        .await
        .unwrap();
    assert_eq!(success, BackendState::Succeeded);

    let (failure_endpoint, _) = spawn_http_sequence(vec![MockResponse {
        status: "200 OK",
        body: json!({"prompt-1": {
            "status": {
                "status_str": "error",
                "completed": false,
                "messages": [["execution_error", {"exception_message": "CUDA OOM"}]]
            }
        }}),
    }])
    .await;
    let failed = adapter_for(failure_endpoint)
        .await
        .reconcile(&prompt_id)
        .await
        .unwrap();
    assert!(matches!(failed, BackendState::Failed { message } if message.contains("CUDA OOM")));
}

#[tokio::test]
async fn reconcile_falls_back_to_queue_when_history_is_missing() {
    let (endpoint, mut requests) = spawn_http_sequence(vec![
        MockResponse {
            status: "200 OK",
            body: json!({}),
        },
        MockResponse {
            status: "200 OK",
            body: json!({"queue_pending": [[1, "prompt-1", {}]], "queue_running": []}),
        },
    ])
    .await;
    let adapter = adapter_for(endpoint).await;

    let state = adapter
        .reconcile(&BackendJobId("prompt-1".to_owned()))
        .await
        .unwrap();

    assert_eq!(state, BackendState::Queued);
    assert!(
        requests
            .recv()
            .await
            .unwrap()
            .starts_with("GET /history/prompt-1 HTTP/1.1")
    );
    assert!(
        requests
            .recv()
            .await
            .unwrap()
            .starts_with("GET /queue HTTP/1.1")
    );
}

#[tokio::test]
async fn websocket_disconnect_reconciles_terminal_history() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut websocket = accept_async(stream).await.unwrap();
        websocket
            .send(tokio_tungstenite::tungstenite::Message::Close(None))
            .await
            .unwrap();
        drop(websocket);

        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_http_request(&mut stream).await;
        assert!(request.starts_with("GET /history/prompt-1 HTTP/1.1"));
        write_json_response(
            &mut stream,
            MockResponse {
                status: "200 OK",
                body: json!({"prompt-1": {
                    "status": {"status_str": "success", "completed": true}
                }}),
            },
        )
        .await;
    });
    let adapter = adapter_for(endpoint).await;

    let state = adapter
        .wait_websocket(
            "client-1",
            &BackendJobId("prompt-1".to_owned()),
            Duration::from_secs(2),
        )
        .await
        .unwrap();

    assert_eq!(state, BackendState::Succeeded);
}

#[tokio::test]
async fn fetch_outputs_rejects_unsafe_history_artifact() {
    let (endpoint, _) = spawn_http_sequence(vec![MockResponse {
        status: "200 OK",
        body: json!({"prompt-1": {
            "status": {"status_str": "success", "completed": true},
            "outputs": {"120": {"videos": [{
                "filename": "escape.mp4",
                "subfolder": "../outside",
                "type": "output"
            }]}}
        }}),
    }])
    .await;
    let adapter = adapter_for(endpoint).await;

    let error = adapter
        .fetch_outputs(&BackendJobId("prompt-1".to_owned()))
        .await
        .unwrap_err();

    assert!(matches!(error, AdapterError::UnsafeOutput(_)));
}

#[tokio::test]
async fn download_output_writes_exact_bytes_to_staging() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/", listener.local_addr().unwrap());
    let (request_tx, mut requests) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        request_tx
            .send(read_http_request(&mut stream).await)
            .unwrap();
        write_response(&mut stream, "200 OK", "video/mp4", b"video-bytes").await;
    });
    let adapter = adapter_for(endpoint).await;
    let directory = tempfile::tempdir().unwrap();
    let artifact = OutputArtifact {
        node_id: "120".to_owned(),
        filename: "clip one.mp4".to_owned(),
        subfolder: "shot/one".to_owned(),
        kind: "output".to_owned(),
    };

    let downloaded = adapter
        .download_output(&artifact, directory.path(), "take.mp4")
        .await
        .unwrap();

    assert_eq!(downloaded.bytes, 11);
    assert_eq!(
        tokio::fs::read(downloaded.path).await.unwrap(),
        b"video-bytes"
    );
    let request = requests.recv().await.unwrap();
    assert!(request.starts_with("GET /view?"));
    assert!(request.contains("filename=clip+one.mp4"));
    assert!(request.contains("subfolder=shot%2Fone"));
}

#[cfg(unix)]
#[tokio::test]
async fn download_output_rejects_symlink_staging_directory() {
    let directory = tempfile::tempdir().unwrap();
    let real = directory.path().join("real");
    let linked = directory.path().join("linked");
    std::fs::create_dir(&real).unwrap();
    std::os::unix::fs::symlink(&real, &linked).unwrap();
    let adapter = adapter_for("http://127.0.0.1:9/".to_owned()).await;
    let artifact = OutputArtifact {
        node_id: "120".to_owned(),
        filename: "clip.mp4".to_owned(),
        subfolder: String::new(),
        kind: "output".to_owned(),
    };

    let error = adapter
        .download_output(&artifact, &linked, "take.mp4")
        .await
        .unwrap_err();

    assert!(matches!(error, AdapterError::UnsafeOutput(message) if message.contains("symlink")));
}
