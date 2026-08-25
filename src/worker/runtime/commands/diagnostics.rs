use super::super::*;

impl WorkerRuntime {
    pub(super) fn retry_probe(&self, request: &ClientRequest, probe_id: &str) -> WorkerReply {
        if probe_id != "worker" {
            return failure(
                request,
                "PROBE_NOT_FOUND",
                format!("diagnostic probe `{probe_id}` does not exist"),
                false,
                self.project_revision(request.project_id.as_deref()),
            );
        }
        match self.snapshot_for(request.project_id.as_deref()) {
            Ok(snapshot) => success(
                request,
                Some(snapshot.revision),
                Some(snapshot),
                "worker diagnostic refreshed",
            ),
            Err(error) => worker_failure(request, error),
        }
    }

    pub(super) fn open_logs(&self, request: &ClientRequest) -> WorkerReply {
        let store = match self.project_store(request.project_id.as_deref()) {
            Ok(store) => store,
            Err(error) => return worker_failure(request, error),
        };
        let state = match store.read_state() {
            Ok(state) => state,
            Err(error) => return store_failure(request, error),
        };
        let logs = store.root().join("logs");
        if !logs.is_dir() {
            return failure(
                request,
                "ARTIFACT_NOT_FOUND",
                format!("project log directory is missing at {}", logs.display()),
                false,
                Some(state.revision),
            );
        }
        WorkerReply {
            protocol_version: IPC_PROTOCOL_VERSION.to_owned(),
            command_id: request.command_id.clone(),
            ok: true,
            revision: Some(state.revision),
            snapshot: None,
            artifact_path: Some(logs),
            message: Some("project logs ready".to_owned()),
            error: None,
        }
    }
}
