//! GPUI-free implementers of the service-manager reactor traits.
//!
//! Maps the GPUI shapes onto tokio / `Arc<parking_lot::Mutex>`:
//!
//! | trait              | GPUI                          | daemon                        |
//! |--------------------|-------------------------------|-------------------------------|
//! | `ServiceCx`        | `Context<'_, ServiceManager>` | [`DaemonServiceCx`]           |
//! | `ServiceHandle`    | `WeakEntity<ServiceManager>`  | [`DaemonServiceHandle`]       |
//! | `ServiceAsyncCx`   | `AsyncApp`                     | [`DaemonServiceAsyncCx`]      |
//!
//! The handle is an `Arc<Mutex<ServiceManager>>` plus the bits a re-spawned task
//! needs (the tokio [`Handle`] and the service notify channel). `update` re-locks
//! the mutex instead of upgrading a `WeakEntity` — the manager is never "gone",
//! so it always returns `Some`.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use okena_services::manager::{ServiceAsyncCx, ServiceCx, ServiceHandle, ServiceManager};
use parking_lot::Mutex;
use tokio::runtime::Handle;
use tokio::sync::watch;

/// Cloneable bundle of everything a spawned service task / reentry needs: the
/// shared manager, the tokio runtime handle, and the service notify channel.
///
/// Held by [`DaemonServiceHandle`] and [`DaemonServiceAsyncCx`] (both `'static`)
/// and re-borrowed by the short-lived [`DaemonServiceCx`]. Cloning is cheap
/// (three `Arc`/sender clones).
#[derive(Clone)]
struct ServiceReactor {
    manager: Arc<Mutex<ServiceManager>>,
    runtime: Handle,
    service_tick: watch::Sender<u64>,
}

impl ServiceReactor {
    fn bump_notify(&self) {
        self.service_tick.send_modify(|v| *v += 1);
    }
}

/// Cloneable, `'static` handle a spawned task captures to re-enter the manager.
///
/// GPUI's `WeakEntity<ServiceManager>` equivalent. `update` re-locks the shared
/// mutex and runs the callback against the manager plus a fresh
/// [`DaemonServiceCx`].
#[derive(Clone)]
pub struct DaemonServiceHandle {
    reactor: ServiceReactor,
}

impl DaemonServiceHandle {
    /// Build a handle from the shared manager + reactor bits.
    pub fn new(
        manager: Arc<Mutex<ServiceManager>>,
        runtime: Handle,
        service_tick: watch::Sender<u64>,
    ) -> Self {
        Self {
            reactor: ServiceReactor {
                manager,
                runtime,
                service_tick,
            },
        }
    }
}

impl ServiceHandle for DaemonServiceHandle {
    type AsyncCx = DaemonServiceAsyncCx;

    fn update<R>(
        &self,
        _cx: &mut Self::AsyncCx,
        f: impl FnOnce(&mut ServiceManager, &mut DaemonServiceCx<'_>) -> R,
    ) -> Option<R> {
        // Re-lock the shared manager (the GPUI `WeakEntity::update` reentry). The
        // manager can't be "gone" behind an `Arc<Mutex>`, so this always runs and
        // returns `Some` — the `Option` exists only to mirror GPUI's released-
        // entity case.
        let mut manager = self.reactor.manager.lock();
        let mut reentry = DaemonServiceCx {
            reactor: &self.reactor,
        };
        Some(f(&mut manager, &mut reentry))
    }
}

/// Synchronous, main-context reentry context. GPUI's
/// `Context<'_, ServiceManager>` equivalent. Borrows the reactor bits for the
/// duration of one reentry callback.
pub struct DaemonServiceCx<'a> {
    reactor: &'a ServiceReactor,
}

/// Owned, `'static` holder of the reactor bits, so callers outside this module
/// can mint a top-level [`DaemonServiceCx`] without naming the private
/// [`ServiceReactor`]. Construct once at the service-method call site.
pub struct ServiceReactorRef(ServiceReactor);

impl ServiceReactorRef {
    /// Build the holder from the shared manager + reactor bits.
    pub fn new(
        manager: Arc<Mutex<ServiceManager>>,
        runtime: Handle,
        service_tick: watch::Sender<u64>,
    ) -> Self {
        Self(ServiceReactor {
            manager,
            runtime,
            service_tick,
        })
    }

    /// Borrow a top-level [`DaemonServiceCx`] for a `ServiceManager` method call.
    pub fn cx(&self) -> DaemonServiceCx<'_> {
        DaemonServiceCx { reactor: &self.0 }
    }
}

impl ServiceCx for DaemonServiceCx<'_> {
    type Handle = DaemonServiceHandle;
    type AsyncCx = DaemonServiceAsyncCx;

    fn notify(&mut self) {
        self.reactor.bump_notify();
    }

    fn spawn_main<F>(&self, f: F)
    where
        F: AsyncFnOnce(Self::Handle, &mut Self::AsyncCx) + 'static,
    {
        // GPUI spawns onto the *main* (foreground) executor: single-threaded, so
        // the captured future need not be `Send` — and the trait signature does
        // not bound it `Send`. The faithful tokio analogue of a single-threaded
        // foreground executor is `tokio::task::spawn_local`, which keeps the
        // (`!Send`) future on the current thread instead of moving it to a worker
        // (`Handle::spawn`/`spawn_blocking` would both require `Send`, which we do
        // not have, and adding the bound would be a type hack that changes the
        // trait contract).
        //
        // Requirement on the daemon's runtime: the reactor's "main loop" must run
        // inside a `tokio::task::LocalSet` on a current-thread runtime, the
        // structural mirror of GPUI's single-threaded main loop. `spawn_local`
        // panics if no `LocalSet` is active — the wiring step that owns the loop
        // is responsible for that, exactly as GPUI owns its main executor.
        let handle = DaemonServiceHandle {
            reactor: self.reactor.clone(),
        };
        let mut async_cx = DaemonServiceAsyncCx {
            reactor: self.reactor.clone(),
        };
        tokio::task::spawn_local(async move {
            f(handle, &mut async_cx).await;
        });
    }
}

/// The async context held across await points inside a spawned service task.
/// GPUI's `AsyncApp` equivalent. `'static` so it survives the spawned future.
pub struct DaemonServiceAsyncCx {
    reactor: ServiceReactor,
}

impl ServiceAsyncCx for DaemonServiceAsyncCx {
    type ReentryCx<'a>
        = DaemonServiceCx<'a>
    where
        Self: 'a;

    fn spawn_blocking<T>(
        &self,
        task: impl FnOnce() -> T + Send + 'static,
    ) -> impl Future<Output = T>
    where
        T: Send + 'static,
    {
        let join = self.reactor.runtime.spawn_blocking(task);
        async move { join.await.expect("daemon spawn_blocking: task panicked") }
    }

    fn timer(&self, duration: Duration) -> impl Future<Output = ()> {
        tokio::time::sleep(duration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use okena_terminal::backend::LocalBackend;
    use okena_terminal::pty_manager::PtyManager;
    use okena_terminal::session_backend::SessionBackend;

    #[test]
    fn blocking_service_work_does_not_stall_a_single_runtime_worker() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let (pty_manager, _events) = PtyManager::new(SessionBackend::None);
        let manager = Arc::new(Mutex::new(ServiceManager::new(
            Arc::new(LocalBackend::new(Arc::new(pty_manager))),
            Arc::new(Mutex::new(Default::default())),
        )));
        let (service_tick, _service_tick_rx) = watch::channel(0);
        let async_cx = DaemonServiceAsyncCx {
            reactor: ServiceReactor {
                manager,
                runtime: runtime.handle().clone(),
                service_tick,
            },
        };

        runtime.block_on(async {
            let (started_tx, started_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let blocked = async_cx.spawn_blocking(move || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok::<_, &'static str>(42)
            });
            started_rx.await.unwrap();

            let (regular_tx, regular_rx) = tokio::sync::oneshot::channel();
            runtime.handle().spawn(async move {
                regular_tx.send(()).unwrap();
            });
            tokio::time::timeout(Duration::from_secs(1), regular_rx)
                .await
                .expect("regular async work was stalled")
                .unwrap();

            release_tx.send(()).unwrap();
            assert_eq!(blocked.await, Ok(42));
            let error = async_cx
                .spawn_blocking(|| Err::<(), _>("expected error"))
                .await;
            assert_eq!(error, Err("expected error"));
        });
    }
}
