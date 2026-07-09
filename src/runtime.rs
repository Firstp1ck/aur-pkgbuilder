use std::any::Any;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::OnceLock;

use futures_util::FutureExt;
use tokio::runtime::{Handle, Runtime};

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

pub(crate) trait TaskFailure {
    fn from_task_failure(message: String) -> Self;
}

pub(crate) trait TaskOutput {
    fn from_task_failure(message: String) -> Self;
}

impl<T, E: TaskFailure> TaskOutput for Result<T, E> {
    fn from_task_failure(message: String) -> Self {
        Err(E::from_task_failure(message))
    }
}

impl TaskFailure for String {
    fn from_task_failure(message: String) -> Self {
        message
    }
}

impl TaskFailure for anyhow::Error {
    fn from_task_failure(message: String) -> Self {
        anyhow::anyhow!(message)
    }
}

impl TaskFailure for crate::workflow::admin::AdminError {
    fn from_task_failure(message: String) -> Self {
        Self::Other(anyhow::anyhow!(message))
    }
}

impl TaskFailure for crate::workflow::aur_account::AurAccountError {
    fn from_task_failure(message: String) -> Self {
        Self::Other(anyhow::anyhow!(message))
    }
}

impl TaskFailure for crate::workflow::pkgbase::PkgbaseNsError {
    fn from_task_failure(message: String) -> Self {
        Self::Pacman(message)
    }
}

impl TaskFailure for crate::workflow::pkgbuild_edit::PkgbuildEditError {
    fn from_task_failure(message: String) -> Self {
        Self::Msg(message)
    }
}

impl TaskFailure for crate::workflow::ssh_setup::SshSetupError {
    fn from_task_failure(message: String) -> Self {
        Self::Other(anyhow::anyhow!(message))
    }
}

impl TaskOutput for bool {
    fn from_task_failure(_message: String) -> Self {
        false
    }
}

impl TaskOutput for String {
    fn from_task_failure(message: String) -> Self {
        message
    }
}

impl<T> TaskOutput for Vec<T> {
    fn from_task_failure(_message: String) -> Self {
        Vec::new()
    }
}

impl<A: Default, B: Default> TaskOutput for (A, B) {
    fn from_task_failure(_message: String) -> Self {
        (A::default(), B::default())
    }
}

fn panic_message(payload: Box<dyn Any + Send>) -> String {
    let detail = payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic payload".into());
    format!("background task panicked: {detail}")
}

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
    T: TaskOutput + Send + 'static,
{
    let (tx, rx) = async_channel::bounded::<Result<T, String>>(1);
    handle().spawn(async move {
        let outcome = AssertUnwindSafe(fut)
            .catch_unwind()
            .await
            .map_err(panic_message);
        let _ = tx.send(outcome).await;
    });
    glib::spawn_future_local(async move {
        let value = match rx.recv().await {
            Ok(Ok(value)) => value,
            Ok(Err(message)) => T::from_task_failure(message),
            Err(error) => {
                T::from_task_failure(format!("background task ended without a result: {error}"))
            }
        };
        on_done(value);
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
    T: TaskOutput + Send + 'static,
    E: Send + 'static,
{
    let (evt_tx, evt_rx) = async_channel::unbounded::<E>();
    let (done_tx, done_rx) = async_channel::bounded::<Result<T, String>>(1);
    handle().spawn(async move {
        let outcome = AssertUnwindSafe(fut(evt_tx.clone()))
            .catch_unwind()
            .await
            .map_err(panic_message);
        drop(evt_tx);
        let _ = done_tx.send(outcome).await;
    });
    glib::spawn_future_local(async move {
        while let Ok(evt) = evt_rx.recv().await {
            on_event(evt);
        }
        let value = match done_rx.recv().await {
            Ok(Ok(value)) => value,
            Ok(Err(message)) => T::from_task_failure(message),
            Err(error) => T::from_task_failure(format!(
                "background streaming task ended without a result: {error}"
            )),
        };
        on_done(value);
    });
}
