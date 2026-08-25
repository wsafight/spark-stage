use super::super::*;

impl WorkerRuntime {
    pub(super) fn approve(&mut self, request: &ClientRequest, approval_id: &str) -> WorkerReply {
        let Some(expected_revision) = request.expected_revision else {
            return missing_revision(request);
        };
        let store = match self.project_store(request.project_id.as_deref()) {
            Ok(store) => store,
            Err(error) => return worker_failure(request, error),
        };
        let state = match store.read_state() {
            Ok(state) => state,
            Err(error) => return store_failure(request, error),
        };
        if state.revision != expected_revision {
            return store_failure(
                request,
                StoreError::RevisionConflict {
                    expected: expected_revision,
                    actual: state.revision,
                },
            );
        }
        let Some(approval) = state
            .pending_approvals
            .iter()
            .find(|approval| approval.approval_id == approval_id)
        else {
            return store_failure(
                request,
                StoreError::ApprovalNotFound(approval_id.to_owned()),
            );
        };
        match approval.kind {
            ApprovalKind::ScriptBundle => self.approve_script(request, Some(approval_id)),
            ApprovalKind::BuildReview | ApprovalKind::FinalVisualReview => {
                match store.approve_build_review(
                    approval_id,
                    expected_revision,
                    &request.command_id,
                    &timestamp(),
                ) {
                    Ok(state) => match self.snapshot(&store, state) {
                        Ok(snapshot) => success(
                            request,
                            Some(snapshot.revision),
                            Some(snapshot),
                            "build review approved",
                        ),
                        Err(error) => worker_failure(request, error),
                    },
                    Err(error) => store_failure(request, error),
                }
            }
            ApprovalKind::CandidateSelection => failure(
                request,
                "CANDIDATE_SELECTION_REQUIRED",
                "select a concrete take to resolve a candidate approval".to_owned(),
                false,
                Some(state.revision),
            ),
            ApprovalKind::BudgetOverrun => failure(
                request,
                "APPROVAL_NOT_IMPLEMENTED",
                "budget-overrun approval is not implemented yet".to_owned(),
                false,
                Some(state.revision),
            ),
        }
    }
}
