use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

use super::{BuildRecipe, run};

#[derive(Debug, Clone)]
pub(crate) struct BuildRequest {
    pub project_id: String,
    pub project_root: PathBuf,
    pub command_id: String,
    pub recipe: BuildRecipe,
}

#[derive(Debug)]
pub(crate) enum BuildEvent {
    Started(BuildRequest),
    Completed(BuildRequest),
    Failed {
        request: BuildRequest,
        message: String,
    },
}

pub(crate) struct BuildExecutorHandle {
    requests: Sender<BuildRequest>,
    events: Receiver<BuildEvent>,
}

impl BuildExecutorHandle {
    pub(crate) fn spawn() -> std::io::Result<Self> {
        let (request_tx, request_rx) = mpsc::channel::<BuildRequest>();
        let (event_tx, event_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("sparkstage-build".to_owned())
            .spawn(move || {
                while let Ok(request) = request_rx.recv() {
                    if event_tx.send(BuildEvent::Started(request.clone())).is_err() {
                        break;
                    }
                    let event = match run(&request.project_root, &request.recipe) {
                        Ok(()) => BuildEvent::Completed(request),
                        Err(error) => BuildEvent::Failed {
                            request,
                            message: error.to_string(),
                        },
                    };
                    if event_tx.send(event).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            requests: request_tx,
            events: event_rx,
        })
    }

    pub(crate) fn send(&self, request: BuildRequest) -> Result<(), String> {
        self.requests
            .send(request)
            .map_err(|_| "build executor request channel is closed".to_owned())
    }

    pub(crate) fn try_recv(&self) -> Result<BuildEvent, TryRecvError> {
        self.events.try_recv()
    }
}
