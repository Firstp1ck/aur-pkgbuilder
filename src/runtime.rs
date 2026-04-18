use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::{Handle, Runtime};

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

pub fn init() {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("aur-pkgbuilder-worker")
            .build()
            .expect("failed to start Tokio runtime")
    });
}

pub fn handle() -> Handle {
    RUNTIME
        .get()
        .expect("runtime::init() was not called before runtime::handle()")
        .handle()
        .clone()
}

/// Spawn `fut` on the Tokio runtime and forward its result to `on_done`,
/// which runs on the GTK main thread.
pub fn spawn<F, T>(fut: F, on_done: impl FnOnce(T) + 'static)
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = async_channel::bounded::<T>(1);
    handle().spawn(async move {
        let _ = tx.send(fut.await).await;
    });
    glib::spawn_future_local(async move {
        if let Ok(value) = rx.recv().await {
            on_done(value);
        }
    });
}

/// Spawn `fut` on the Tokio runtime and deliver streaming events on the GTK
/// main thread. `on_event` runs for every event sent on the channel; when the
/// channel closes, `on_done` runs with the future's final value.
pub fn spawn_streaming<F, T, E>(
    fut: impl FnOnce(async_channel::Sender<E>) -> F + Send + 'static,
    mut on_event: impl FnMut(E) + 'static,
    on_done: impl FnOnce(T) + 'static,
) where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
    E: Send + 'static,
{
    let (evt_tx, evt_rx) = async_channel::unbounded::<E>();
    let (done_tx, done_rx) = async_channel::bounded::<T>(1);
    handle().spawn(async move {
        let value = fut(evt_tx.clone()).await;
        drop(evt_tx);
        let _ = done_tx.send(value).await;
    });
    glib::spawn_future_local(async move {
        while let Ok(evt) = evt_rx.recv().await {
            on_event(evt);
        }
        if let Ok(value) = done_rx.recv().await {
            on_done(value);
        }
    });
}
