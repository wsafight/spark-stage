use std::path::Path;

use super::*;
use crate::domain::ReferenceSubjectKind;

impl WorkerRuntime {
    pub(super) fn list_references(&self, request: &ClientRequest) -> WorkerReply {
        if request.expected_revision.is_some() {
            return failure(
                request,
                "INVALID_ARGUMENT",
                "reference list is read-only and does not accept expected_revision".to_owned(),
                false,
                None,
            );
        }
        let store = match self.project_store(request.project_id.as_deref()) {
            Ok(store) => store,
            Err(error) => return worker_failure(request, error),
        };
        match store.read_state() {
            Ok(state) => success_payload(
                request,
                Some(state.revision),
                WorkerPayload::ReferenceList {
                    references: state.references.into_values().collect(),
                },
                "reference list loaded",
            ),
            Err(error) => store_failure(request, error),
        }
    }

    pub(super) fn reference_impact(
        &self,
        request: &ClientRequest,
        subject_kind: ReferenceSubjectKind,
        subject_id: &str,
    ) -> WorkerReply {
        if request.expected_revision.is_some() {
            return failure(
                request,
                "INVALID_ARGUMENT",
                "reference impact is read-only and does not accept expected_revision".to_owned(),
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
        match store.reference_impact(subject_kind, subject_id) {
            Ok(impact) => success_payload(
                request,
                Some(state.revision),
                WorkerPayload::ReferenceImpact(impact),
                "reference impact loaded",
            ),
            Err(error) => store_failure(request, error),
        }
    }

    pub(super) fn import_reference(
        &mut self,
        request: &ClientRequest,
        subject_kind: ReferenceSubjectKind,
        subject_id: &str,
        source: &Path,
        accept_impact: bool,
    ) -> WorkerReply {
        let Some(expected_revision) = request.expected_revision else {
            return missing_revision(request);
        };
        let store = match self.project_store(request.project_id.as_deref()) {
            Ok(store) => store,
            Err(error) => return worker_failure(request, error),
        };
        match store.import_reference(
            subject_kind,
            subject_id,
            ReferenceWriteRequest {
                source,
                accept_impact,
                expected_revision,
                command_id: &request.command_id,
                now: &timestamp(),
            },
        ) {
            Ok((state, reference, impact)) => success_payload(
                request,
                Some(state.revision),
                WorkerPayload::ReferenceImpact(impact),
                &format!("reference {} imported", reference.reference_id),
            ),
            Err(error) => store_failure(request, error),
        }
    }

    pub(super) fn replace_reference(
        &mut self,
        request: &ClientRequest,
        reference_id: &str,
        source: &Path,
        accept_impact: bool,
    ) -> WorkerReply {
        let Some(expected_revision) = request.expected_revision else {
            return missing_revision(request);
        };
        let store = match self.project_store(request.project_id.as_deref()) {
            Ok(store) => store,
            Err(error) => return worker_failure(request, error),
        };
        match store.replace_reference(
            reference_id,
            ReferenceWriteRequest {
                source,
                accept_impact,
                expected_revision,
                command_id: &request.command_id,
                now: &timestamp(),
            },
        ) {
            Ok((state, reference, impact)) => success_payload(
                request,
                Some(state.revision),
                WorkerPayload::ReferenceImpact(impact),
                &format!(
                    "reference {} replaced by {}",
                    reference_id, reference.reference_id
                ),
            ),
            Err(error) => store_failure(request, error),
        }
    }

    pub(super) fn verify_references(&self, request: &ClientRequest) -> WorkerReply {
        if request.expected_revision.is_some() {
            return failure(
                request,
                "INVALID_ARGUMENT",
                "reference verification is read-only and does not accept expected_revision"
                    .to_owned(),
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
        match store.verify_references() {
            Ok(report) => success_payload(
                request,
                Some(state.revision),
                WorkerPayload::ReferenceVerification(report),
                "references verified",
            ),
            Err(error) => store_failure(request, error),
        }
    }
}
