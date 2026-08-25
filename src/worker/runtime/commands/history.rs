use super::*;

impl WorkerRuntime {
    pub(super) fn decision_history(&self, request: &ClientRequest, limit: u32) -> WorkerReply {
        if request.expected_revision.is_some() {
            return failure(
                request,
                "INVALID_ARGUMENT",
                "decision history is read-only and does not accept expected_revision".to_owned(),
                false,
                None,
            );
        }
        let store = match self.project_store(request.project_id.as_deref()) {
            Ok(store) => store,
            Err(error) => return worker_failure(request, error),
        };
        let state = match store.read_state() {
            Ok(state) => state,
            Err(error) => return store_failure(request, error),
        };
        match store.decision_history(limit) {
            Ok(decisions) => success_payload(
                request,
                Some(state.revision),
                WorkerPayload::DecisionHistory { decisions },
                "decision history loaded",
            ),
            Err(error) => store_failure(request, error),
        }
    }
}
