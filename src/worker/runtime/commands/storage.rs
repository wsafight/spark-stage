use super::*;

impl WorkerRuntime {
    pub(super) fn storage_status(&self, request: &ClientRequest) -> WorkerReply {
        if request.expected_revision.is_some() {
            return failure(
                request,
                "INVALID_ARGUMENT",
                "storage status is read-only and does not accept expected_revision".to_owned(),
                false,
                None,
            );
        }
        let store = match self.project_store(request.project_id.as_deref()) {
            Ok(store) => store,
            Err(error) => return worker_failure(request, error),
        };
        match store.storage_report() {
            Ok(report) => {
                let state = match store.read_state() {
                    Ok(state) => state,
                    Err(error) => return store_failure(request, error),
                };
                success_payload(
                    request,
                    Some(state.revision),
                    WorkerPayload::StorageReport(report),
                    "storage status loaded",
                )
            }
            Err(error) => store_failure(request, error),
        }
    }

    pub(super) fn create_cleanup_plan(&mut self, request: &ClientRequest) -> WorkerReply {
        let Some(expected_revision) = request.expected_revision else {
            return missing_revision(request);
        };
        let store = match self.project_store(request.project_id.as_deref()) {
            Ok(store) => store,
            Err(error) => return worker_failure(request, error),
        };
        match store.create_cleanup_plan(expected_revision, &request.command_id, &timestamp()) {
            Ok((state, plan)) => success_payload(
                request,
                Some(state.revision),
                WorkerPayload::CleanupPlan(plan),
                "cleanup plan created",
            ),
            Err(error) => store_failure(request, error),
        }
    }

    pub(super) fn apply_cleanup_plan(
        &mut self,
        request: &ClientRequest,
        plan_id: &str,
    ) -> WorkerReply {
        let Some(expected_revision) = request.expected_revision else {
            return missing_revision(request);
        };
        let store = match self.project_store(request.project_id.as_deref()) {
            Ok(store) => store,
            Err(error) => return worker_failure(request, error),
        };
        match store.apply_cleanup_plan(
            plan_id,
            expected_revision,
            &request.command_id,
            &timestamp(),
        ) {
            Ok((state, plan)) => success_payload(
                request,
                Some(state.revision),
                WorkerPayload::CleanupPlan(plan),
                "cleanup plan applied",
            ),
            Err(error) => store_failure(request, error),
        }
    }

    pub(super) fn restore_cleanup_plan(
        &mut self,
        request: &ClientRequest,
        plan_id: &str,
    ) -> WorkerReply {
        let Some(expected_revision) = request.expected_revision else {
            return missing_revision(request);
        };
        let store = match self.project_store(request.project_id.as_deref()) {
            Ok(store) => store,
            Err(error) => return worker_failure(request, error),
        };
        match store.restore_cleanup_plan(
            plan_id,
            expected_revision,
            &request.command_id,
            &timestamp(),
        ) {
            Ok((state, plan)) => success_payload(
                request,
                Some(state.revision),
                WorkerPayload::CleanupPlan(plan),
                "cleanup plan restored",
            ),
            Err(error) => store_failure(request, error),
        }
    }
}
