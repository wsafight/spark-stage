use super::*;
use crate::domain::BudgetContract;

impl WorkerRuntime {
    pub(super) fn update_budget(
        &mut self,
        request: &ClientRequest,
        contract: BudgetContract,
    ) -> WorkerReply {
        let Some(expected_revision) = request.expected_revision else {
            return missing_revision(request);
        };
        let store = match self.project_store(request.project_id.as_deref()) {
            Ok(store) => store,
            Err(error) => return worker_failure(request, error),
        };
        match store.update_budget_contract(
            contract,
            expected_revision,
            &request.command_id,
            &timestamp(),
        ) {
            Ok(state) => match self.snapshot(&store, state) {
                Ok(snapshot) => success(
                    request,
                    Some(snapshot.revision),
                    Some(snapshot),
                    "budget contract updated; prior overrun grants were cleared",
                ),
                Err(error) => worker_failure(request, error),
            },
            Err(error) => store_failure(request, error),
        }
    }
}
