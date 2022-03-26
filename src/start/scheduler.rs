use crate::start::requests::{InvokeRequest, ServerError};
use axum::{body::Body, response::Response};
use std::{
    collections::{hash_map::Entry, HashMap, VecDeque},
    sync::Arc,
};
use tokio::{
    process::Command,
    sync::{
        mpsc::{self, Receiver, Sender},
        oneshot, Mutex,
    },
};
use tokio_graceful_shutdown::SubsystemHandle;
use tracing::{error, info};

#[derive(Clone)]
pub(crate) struct RequestQueue {
    inner: Arc<Mutex<VecDeque<InvokeRequest>>>,
}

impl RequestQueue {
    pub fn new() -> RequestQueue {
        RequestQueue {
            inner: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    pub async fn pop(&self) -> Option<InvokeRequest> {
        let mut queue = self.inner.lock().await;
        queue.pop_front()
    }

    pub async fn push(&self, req: InvokeRequest) {
        let mut queue = self.inner.lock().await;
        queue.push_back(req);
    }
}

#[derive(Clone)]
pub(crate) struct RequestCache {
    server_addr: String,
    inner: Arc<Mutex<HashMap<String, RequestQueue>>>,
}

impl RequestCache {
    pub fn new(server_addr: String) -> RequestCache {
        RequestCache {
            server_addr,
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn upsert(&self, req: InvokeRequest) -> Option<(String, String)> {
        let mut inner = self.inner.lock().await;
        let name = req.function_name.clone();

        match inner.entry(name) {
            Entry::Vacant(v) => {
                let name = req.function_name.clone();
                let runtime_api = format!("{}/{}", &self.server_addr, &name);

                let stack = RequestQueue::new();
                stack.push(req).await;
                v.insert(stack);

                Some((name, runtime_api))
            }
            Entry::Occupied(o) => {
                o.into_mut().push(req).await;
                None
            }
        }
    }

    pub async fn pop(&self, function_name: &str) -> Option<InvokeRequest> {
        let inner = self.inner.lock().await;
        let stack = match inner.get(function_name) {
            None => return None,
            Some(s) => s,
        };

        stack.pop().await
    }

    async fn clean(&self, function_name: &str) {
        let mut inner = self.inner.lock().await;
        inner.remove(function_name);
    }
}

#[derive(Clone)]
pub(crate) struct ResponseCache {
    inner: Arc<Mutex<HashMap<String, oneshot::Sender<Response<Body>>>>>,
}

impl ResponseCache {
    pub fn new() -> ResponseCache {
        ResponseCache {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn pop(&self, req_id: &str) -> Option<oneshot::Sender<Response<Body>>> {
        let mut cache = self.inner.lock().await;
        cache.remove(req_id)
    }

    pub async fn push(&self, req_id: &str, resp_tx: oneshot::Sender<Response<Body>>) {
        let mut cache = self.inner.lock().await;
        cache.insert(req_id.into(), resp_tx);
    }
}

pub(crate) async fn init_scheduler(
    subsys: &SubsystemHandle,
    req_cache: RequestCache,
) -> Sender<InvokeRequest> {
    let (req_tx, req_rx) = mpsc::channel::<InvokeRequest>(100);

    subsys.start("lambda scheduler", move |s| async {
        start_scheduler(s, req_cache, req_rx).await;
        Ok::<(), std::convert::Infallible>(())
    });

    req_tx
}

async fn start_scheduler(
    subsys: SubsystemHandle,
    req_cache: RequestCache,
    mut req_rx: Receiver<InvokeRequest>,
) {
    let (gc_tx, mut gc_rx) = mpsc::channel::<String>(10);

    loop {
        tokio::select! {
            Some(req) = req_rx.recv() => {
                if let Some((name, api)) = req_cache.upsert(req).await {
                    let name = name.clone();
                    let api = api.clone();
                    let gc_tx = gc_tx.clone();
                    subsys.start("lambda runtime", |s| start_function(s, name, api, gc_tx));
                }
            },
            Some(gc) = gc_rx.recv() => {
                req_cache.clean(&gc).await;
            },
            _ = subsys.on_shutdown_requested() => {
                info!("terminating Lambda scheduler");
                return;
            },

        };
    }
}

async fn start_function(
    subsys: SubsystemHandle,
    name: String,
    runtime_api: String,
    gc_tx: Sender<String>,
) -> Result<(), ServerError> {
    info!(function = ?name, "starting lambda function");

    let mut child = Command::new("cargo")
        .args(["watch", "--", "cargo", "run", "--bin", &name])
        .env("RUST_LOG", std::env::var("RUST_LOG").unwrap_or_default())
        .env("AWS_LAMBDA_RUNTIME_API", &runtime_api)
        .env("AWS_LAMBDA_FUNCTION_NAME", &name)
        .env("AWS_LAMBDA_FUNCTION_VERSION", "1")
        .env("AWS_LAMBDA_FUNCTION_MEMORY_SIZE", "4096")
        .spawn()
        .map_err(ServerError::SpawnCommand)?;

    tokio::select! {
        _ = child.wait() => {
            if let Err(err) = gc_tx.send(name.clone()).await {
                error!(error = %err, function = ?name, "failed to send message to cleanup dead function");
            }
        },
        _ = subsys.on_shutdown_requested() => {
            info!(function = ?name, "terminating Lambda function");
            let _ = child.kill().await;
        }
    }

    Ok(())
}
