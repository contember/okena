//! Reactor abstraction for the service manager.
//!
//! `ServiceManager`'s methods need a handful of reactor capabilities from their
//! context: mark the manager dirty (`notify`), spawn a main-context async task
//! that gets a cloneable handle to re-enter the manager (`spawn_main`), and,
//! from inside that task, offload blocking subprocess work to a background
//! thread (`spawn_blocking`), sleep (`timer`), and re-enter the manager to
//! mutate it (`ServiceHandle::update`).
//!
//! Today the only implementer is GPUI: [`ServiceCx`] for
//! `gpui::Context<'_, ServiceManager>`, [`ServiceHandle`] for
//! `gpui::WeakEntity<ServiceManager>`, and [`ServiceAsyncCx`] for
//! `gpui::AsyncApp`. A future GPUI-free daemon will add a second set of
//! implementers backed by a tokio reactor: the handle becomes an
//! `Arc<Mutex<ServiceManager>>` and `update` re-locks the mutex instead of
//! upgrading a `WeakEntity`.
//!
//! Manager methods take `cx: &mut impl ServiceCx` instead of
//! `&mut Context<Self>`. This is a non-breaking change for existing GUI callers:
//! they keep passing `&mut Context<ServiceManager>`, which satisfies the trait
//! via the impls below.

use super::ServiceManager;
use gpui::{AsyncApp, Context, WeakEntity};
use std::future::Future;
use std::time::Duration;

/// The reactor capabilities a [`ServiceManager`] method needs from its
/// (synchronous, main-context) context.
///
/// The associated [`Handle`](ServiceCx::Handle) and
/// [`AsyncCx`](ServiceCx::AsyncCx) types are what a spawned task receives so it
/// can re-enter the manager. For GPUI these are `WeakEntity<ServiceManager>` and
/// `AsyncApp`; for the future daemon they will be `Arc<Mutex<ServiceManager>>`
/// and the daemon's async handle.
pub trait ServiceCx {
    /// A cloneable, `'static` handle to the manager, captured by spawned tasks
    /// so they can re-enter it (the GPUI `WeakEntity::update` reentry).
    type Handle: ServiceHandle<AsyncCx = Self::AsyncCx>;

    /// The async context held across await points inside a spawned task.
    type AsyncCx: ServiceAsyncCx;

    /// Mark the manager dirty so observers (and cached views) re-evaluate.
    ///
    /// GPUI: `Context::notify`. Daemon: invoke registered change callbacks.
    fn notify(&mut self);

    /// Spawn a main-context async task. The task receives a cloned, `'static`
    /// [`Handle`](ServiceCx::Handle) (to re-enter the manager) and a mutable
    /// [`AsyncCx`](ServiceCx::AsyncCx) (to spawn blocking work, sleep, and drive
    /// the reentry). The task runs detached.
    ///
    /// GPUI: `cx.spawn(async move |this, cx| ...).detach()`, handing the task
    /// the `WeakEntity` as the handle.
    fn spawn_main<F>(&self, f: F)
    where
        F: AsyncFnOnce(Self::Handle, &mut Self::AsyncCx) + 'static;
}

/// A cloneable, `'static` handle to the manager that a spawned task uses to
/// re-enter it and mutate state.
pub trait ServiceHandle: Clone + 'static {
    /// The async context this handle is driven with (matches
    /// [`ServiceCx::AsyncCx`]).
    type AsyncCx: ServiceAsyncCx;

    /// Re-enter the manager and run `f` against it. The callback receives the
    /// manager plus a fresh synchronous [`ServiceCx`] (so it can `notify` and
    /// `spawn_main` again). Returns `None` if the manager is gone (the GPUI
    /// entity was released), mirroring `WeakEntity::update`'s error case.
    ///
    /// GPUI: `WeakEntity::update`, whose callback gets `&mut Context<_>` — itself
    /// a `ServiceCx`.
    fn update<R>(
        &self,
        cx: &mut Self::AsyncCx,
        f: impl FnOnce(&mut ServiceManager, &mut <Self::AsyncCx as ServiceAsyncCx>::ReentryCx<'_>) -> R,
    ) -> Option<R>;
}

/// The async context held across await points inside a spawned task: offload
/// blocking work, sleep, and (via [`ServiceHandle::update`]) re-enter the
/// manager.
pub trait ServiceAsyncCx {
    /// The synchronous context a reentry callback sees. For GPUI this is
    /// `Context<'_, ServiceManager>`, the same type that implements
    /// [`ServiceCx`] — so reentry code can `notify`/`spawn_main` exactly like
    /// the top-level methods.
    type ReentryCx<'a>: ServiceCx
    where
        Self: 'a;

    /// Offload blocking work (subprocess calls) to a background thread and await
    /// the result.
    ///
    /// GPUI: `cx.background_executor().spawn(fut)`.
    fn spawn_blocking<T>(
        &self,
        fut: impl Future<Output = T> + Send + 'static,
    ) -> impl Future<Output = T>
    where
        T: Send + 'static;

    /// Async sleep.
    ///
    /// GPUI: `cx.background_executor().timer(d)`.
    fn timer(&self, duration: Duration) -> impl Future<Output = ()>;
}

// --- GPUI implementers ---------------------------------------------------

impl ServiceCx for Context<'_, ServiceManager> {
    type Handle = WeakEntity<ServiceManager>;
    type AsyncCx = AsyncApp;

    fn notify(&mut self) {
        // The inherent `Context::notify` shadows this trait method during method
        // resolution (inherent methods win), so this is a direct call into GPUI,
        // not recursion back into the trait impl.
        self.notify();
    }

    fn spawn_main<F>(&self, f: F)
    where
        F: AsyncFnOnce(Self::Handle, &mut Self::AsyncCx) + 'static,
    {
        // `Context::spawn` hands the closure the `WeakEntity` and `&mut AsyncApp`
        // directly — the same shape as our trait method.
        self.spawn(async move |this, cx| f(this, cx).await).detach();
    }
}

impl ServiceHandle for WeakEntity<ServiceManager> {
    type AsyncCx = AsyncApp;

    fn update<R>(
        &self,
        cx: &mut Self::AsyncCx,
        f: impl FnOnce(&mut ServiceManager, &mut Context<'_, ServiceManager>) -> R,
    ) -> Option<R> {
        // `WeakEntity::update` returns Err if the entity was released; map that
        // to None (callers already treat it as "manager gone, stop").
        WeakEntity::update(self, cx, f).ok()
    }
}

impl ServiceAsyncCx for AsyncApp {
    type ReentryCx<'a> = Context<'a, ServiceManager>;

    fn spawn_blocking<T>(
        &self,
        fut: impl Future<Output = T> + Send + 'static,
    ) -> impl Future<Output = T>
    where
        T: Send + 'static,
    {
        self.background_executor().spawn(fut)
    }

    fn timer(&self, duration: Duration) -> impl Future<Output = ()> {
        self.background_executor().timer(duration)
    }
}
