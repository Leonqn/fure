#[cfg(any(all(feature = "tokio", feature = "async-std"),))]
compile_error!("`tokio` and `async-std` features must not be enabled together");

use future::ConcurrentRetry;
use std::future::Future;

pub mod future;
pub mod policies;

pub fn retry<R, T, E, F, CF>(create_f: CF, retry: R) -> ConcurrentRetry<R, T, E, F, CF>
where
    R: RetryPolicy<T, E>,
    F: Future<Output = Result<T, E>> + Unpin,
    CF: CreateFuture<F>,
{
    ConcurrentRetry::new(retry, create_f, vec![], None)
}

pub trait CreateFuture<F> {
    fn create(&mut self) -> F;
}

impl<F, FN> CreateFuture<F> for FN
where
    FN: FnMut() -> F,
{
    fn create(&mut self) -> F {
        self()
    }
}

pub trait RetryPolicy<T, E>: Sized {
    type ForceRetryFuture: Future + Unpin;
    type RetryFuture: Future<Output = Self> + Unpin;

    fn force_retry_after(&self) -> Self::ForceRetryFuture;

    fn retry(&self, result: Option<&Result<T, E>>) -> Option<Self::RetryFuture>;
}
