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
async fn upload_project_file_posts_safe_project_input() {
    let (endpoint, mut requests) = spawn_http_sequence(vec![MockResponse {
        status: "200 OK",
        body: json!({"name": "frame.png", "subfolder": "sparkstage/refs"}),
    }])
    .await;
    let adapter = adapter_for(endpoint).await;
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join("refs")).unwrap();
    std::fs::write(project.path().join("refs/frame.png"), b"png-data").unwrap();

    let remote = adapter
        .upload_project_file(project.path(), "refs/frame.png")
        .await
        .unwrap();

    assert_eq!(remote, "sparkstage/refs/frame.png");
    let request = requests.recv().await.unwrap();
    assert!(request.starts_with("POST /upload/image HTTP/1.1"));
    assert!(request.contains("frame.png"));
    assert!(request.contains("png-data"));
}

#[tokio::test]
async fn upload_project_file_rejects_traversal_and_symlink_inputs() {
    let adapter = adapter_for("http://127.0.0.1:1/".to_owned()).await;
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("inside.png"), b"png-data").unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("outside.png"), b"png-data").unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("outside.png"),
        project.path().join("link.png"),
    )
    .unwrap();

    assert!(matches!(
        adapter
            .upload_project_file(project.path(), "../outside.png")
            .await,
        Err(AdapterError::UnsafeOutput(_))
    ));
    assert!(matches!(
        adapter
            .upload_project_file(project.path(), "link.png")
            .await,
        Err(AdapterError::UnsafeOutput(_))
    ));
}

#[tokio::test]
async fn upload_project_file_rejects_invalid_backend_filename() {
    let (endpoint, _) = spawn_http_sequence(vec![MockResponse {
        status: "200 OK",
        body: json!({"name": "", "subfolder": "sparkstage/refs"}),
    }])
    .await;
    let adapter = adapter_for(endpoint).await;
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("frame.png"), b"png-data").unwrap();

    assert!(matches!(
        adapter
            .upload_project_file(project.path(), "frame.png")
            .await,
        Err(AdapterError::UnsafeOutput(_))
    ));
}

#[tokio::test]
async fn prepare_builds_dynamic_h3_reference_image_nodes() {
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("one.png"), b"one").unwrap();
    std::fs::write(project.path().join("two.png"), b"two").unwrap();
    let workflow_path = project.path().join("workflow.json");
    std::fs::write(
        &workflow_path,
        serde_json::to_vec(&json!({
            "45": {"class_type": "Text", "inputs": {"text": ""}},
            "78": {"class_type": "Seed", "inputs": {"noise_seed": 0}},
            "90": {"class_type": "Size", "inputs": {"width": 1, "height": 1}},
            "5": {"class_type": "H3", "inputs": {"ref_images": {}}},
            "120": {"class_type": "Output", "inputs": {"filename_prefix": "out"}}
        }))
        .unwrap(),
    )
    .unwrap();
    let mut config = super::config(workflow_path.clone());
    config.output_node = "120".to_owned();
    config.workflow = workflow_path;
    config.optional_bindings.insert(
        "reference_images".to_owned(),
        WorkflowBinding {
            node: "5".to_owned(),
            input: "ref_images".to_owned(),
        },
    );
    let (endpoint, mut requests) = spawn_http_sequence(vec![
        MockResponse {
            status: "200 OK",
            body: json!({"name": "one.png", "subfolder": ""}),
        },
        MockResponse {
            status: "200 OK",
            body: json!({"name": "two.png", "subfolder": ""}),
        },
    ])
    .await;
    config.endpoint = endpoint;
    let adapter = ComfyAdapter::new(config).unwrap();

    let prepared = adapter
        .prepare(GenerationRequest {
            request_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_owned(),
            project_root: project.path().to_owned(),
            operation: Operation::R2v,
            prompt: "test".to_owned(),
            seed: 7,
            width: 1344,
            height: 768,
            fps: 24,
            duration_seconds: 10,
            profile: "audition".to_owned(),
            first_frame: None,
            last_frame: None,
            reference_images: vec!["one.png".to_owned(), "two.png".to_owned()],
            reference_video: None,
        })
        .await
        .unwrap();

    assert_eq!(prepared.workflow["121"]["class_type"], "LoadImage");
    assert_eq!(prepared.workflow["122"]["inputs"]["image"], "two.png");
    assert_eq!(
        prepared.workflow["5"]["inputs"]["ref_images.ref_image_0"],
        json!(["121", 0])
    );
    assert_eq!(
        prepared.workflow["5"]["inputs"]["ref_images.ref_image_1"],
        json!(["122", 0])
    );
    assert!(prepared.workflow["5"]["inputs"].get("ref_images").is_none());
    assert!(requests.recv().await.is_some());
    assert!(requests.recv().await.is_some());
}

#[tokio::test]
async fn prepare_builds_h3_reference_video_frame_stream() {
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("clip.mp4"), b"video").unwrap();
    let workflow_path = project.path().join("workflow.json");
    std::fs::write(
        &workflow_path,
        serde_json::to_vec(&json!({
            "45": {"class_type": "Text", "inputs": {"text": ""}},
            "78": {"class_type": "Seed", "inputs": {"noise_seed": 0}},
            "90": {"class_type": "Size", "inputs": {"width": 1, "height": 1}},
            "5": {"class_type": "H3", "inputs": {"ref_images": {}, "ref_videos": {}}},
            "120": {"class_type": "Output", "inputs": {"filename_prefix": "out"}}
        }))
        .unwrap(),
    )
    .unwrap();
    let mut config = super::config(workflow_path.clone());
    config.output_node = "120".to_owned();
    config.workflow = workflow_path;
    config.optional_bindings.insert(
        "reference_video".to_owned(),
        WorkflowBinding {
            node: "5".to_owned(),
            input: "ref_videos".to_owned(),
        },
    );
    let (endpoint, mut requests) = spawn_http_sequence(vec![MockResponse {
        status: "200 OK",
        body: json!({"name": "clip.mp4", "subfolder": ""}),
    }])
    .await;
    config.endpoint = endpoint;
    let adapter = ComfyAdapter::new(config).unwrap();

    let prepared = adapter
        .prepare(GenerationRequest {
            request_id: "01ARZ3NDEKTSV4RRFFQ69G5FAV".to_owned(),
            project_root: project.path().to_owned(),
            operation: Operation::R2v,
            prompt: "test".to_owned(),
            seed: 7,
            width: 1344,
            height: 768,
            fps: 24,
            duration_seconds: 10,
            profile: "audition".to_owned(),
            first_frame: None,
            last_frame: None,
            reference_images: Vec::new(),
            reference_video: Some("clip.mp4".to_owned()),
        })
        .await
        .unwrap();

    assert_eq!(prepared.workflow["121"]["class_type"], "LoadVideo");
    assert_eq!(prepared.workflow["121"]["inputs"]["file"], "clip.mp4");
    assert_eq!(
        prepared.workflow["122"]["inputs"]["video"],
        json!(["121", 0])
    );
    assert_eq!(
        prepared.workflow["5"]["inputs"]["ref_videos.ref_video_0"],
        json!(["122", 0])
    );
    assert!(prepared.workflow["5"]["inputs"].get("ref_videos").is_none());
    assert!(requests.recv().await.is_some());
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
