//! The iOS companion's core, called from Swift through UniFFI.
//!
//! The phone is one more paired device: it pairs with a Mac running Bomb Code
//! or a `bombd` server using the same `bomb://pair` links and pinned TLS as a
//! Mac does ([`pairing`]), then keeps one connection per machine ([`machine`]).
//! Events are folded by the same reducer the desktop app uses
//! (`bomb_transcript`), and handed to Swift as plain records ([`view`]), so
//! the two apps never disagree about what a thread shows.
//!
//! Everything async runs on one tokio runtime owned by this library; Swift
//! awaits the results without needing to know.

pub mod machine;
pub mod pairing;
mod preview;
pub mod view;

use std::future::Future;
use std::sync::OnceLock;

uniffi::setup_scaffolding!();

/// Every failure reaches Swift as a sentence it can show as is.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum MobileError {
    #[error("{message}")]
    Failed { message: String },
}

impl From<String> for MobileError {
    fn from(message: String) -> Self {
        Self::Failed { message }
    }
}

impl From<&str> for MobileError {
    fn from(message: &str) -> Self {
        Self::Failed { message: message.to_string() }
    }
}

pub type Result<T, E = MobileError> = std::result::Result<T, E>;

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("bomb-mobile")
            .enable_all()
            .build()
            .expect("start the tokio runtime")
    })
}

/// Run `future` on the library's runtime. Swift's executor polls the returned future; tokio does the I/O.
async fn on_runtime<F>(future: F) -> F::Output
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    match runtime().spawn(future).await {
        Ok(output) => output,
        Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
        // The runtime lives for the whole process, so its tasks are never cancelled.
        Err(error) => unreachable!("runtime task cancelled: {error}"),
    }
}
