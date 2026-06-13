//! Lifecycle of the background proxy server task.

use proxy_core::AppState;
use std::sync::{Arc, Mutex};
use tauri::async_runtime::JoinHandle;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

/// Handle to the background proxy task, kept in Tauri-managed state so the
/// listen-address command can restart it. Holds both the task handle and a
/// shutdown sender so the server can be stopped *gracefully* — the old server
/// releases its port before a replacement binds. Lives in the GUI crate to keep
/// `proxy-core` free of Tauri dependencies.
pub(crate) struct ProxyTask {
    handle: Mutex<Option<JoinHandle<()>>>,
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
}

impl ProxyTask {
    pub(crate) fn new() -> Self {
        Self {
            handle: Mutex::new(None),
            shutdown: Mutex::new(None),
        }
    }

    /// Spawns the proxy server on an already-bound listener, wiring up a
    /// graceful-shutdown channel. Replaces any previously stored handle/sender
    /// (call [`ProxyTask::stop`] first to tear the old one down cleanly).
    pub(crate) fn spawn(&self, std_listener: std::net::TcpListener, state: Arc<AppState>) {
        let (tx, rx) = oneshot::channel::<()>();
        let handle = tauri::async_runtime::spawn(async move {
            // Register the listener with the reactor *inside* the runtime:
            // `TcpListener::from_std` calls `Handle::current()` and panics if no
            // Tokio runtime is running, which is exactly the case in Tauri's
            // synchronous `setup` callback. Doing it here (on the spawned task)
            // keeps the std-bind error surfacing at the call site while moving
            // the reactor registration into a valid runtime context.
            let listener = match TcpListener::from_std(std_listener) {
                Ok(listener) => listener,
                Err(e) => {
                    tracing::error!("failed to register proxy listener: {e}");
                    return;
                }
            };
            // Resolves when the sender fires *or* is dropped — either way the
            // server shuts down.
            let shutdown = async {
                let _ = rx.await;
            };
            if let Err(e) = proxy_core::serve_with(listener, state, shutdown).await {
                tracing::error!("proxy server stopped: {e}");
            }
        });
        *self.handle.lock().unwrap() = Some(handle);
        *self.shutdown.lock().unwrap() = Some(tx);
    }

    /// Signals the running server to shut down and waits for the task to finish,
    /// guaranteeing the listening socket is released before returning. Locks are
    /// released before the `.await` so no `MutexGuard` is held across it.
    pub(crate) async fn stop(&self) {
        let tx = self.shutdown.lock().unwrap().take();
        drop(tx); // dropping (or sending on) the channel triggers shutdown
        let handle = self.handle.lock().unwrap().take();
        if let Some(handle) = handle {
            let _ = handle.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proxy_core::RuntimeConfig;

    /// `ProxyTask::stop` must wait for the server to fully shut down so the
    /// listening socket is released — otherwise a restart on the same address
    /// races into `EADDRINUSE`. We run on Tauri's async runtime (the same one
    /// `spawn` uses) so the listener and the server task share a runtime.
    #[test]
    fn proxy_task_stop_releases_port() {
        tauri::async_runtime::block_on(async {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let addr = listener.local_addr().unwrap();

            let state = Arc::new(AppState::new(RuntimeConfig::default()));
            let task = ProxyTask::new();
            task.spawn(listener, state);

            // Graceful stop must release the socket before returning.
            task.stop().await;

            std::net::TcpListener::bind(addr).expect("port should be free after ProxyTask::stop()");
        });
    }
}
