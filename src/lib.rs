//! A crate for retrying futures using different strategies.
//!

#[cfg(all(feature = "tokio", feature = "async-std"))]
compile_error!("`tokio` and `async-std` features must not be enabled together");

use std::future::Future;

/// Backoff utilities for [`crate::policies::sequential::backoff`] policy.
pub mod backoff;

/// Some builtin implementations of [`Policy`].
pub mod policies;

/// Sleep functions for `tokio` and `async-std`.
#[cfg(any(feature = "tokio", feature = "async-std"))]
pub mod sleep;

pub use future::Retry;

mod future;

/// Run futures created by [`CreateFuture`] accorrding to [`Policy`].
/// ## Simple concurrent policy
/// Run at most 4 concurrent futures and wait a successful one.
/// ```
/// # async fn run() -> Result<(), reqwest::Error> {
/// use fure::policies::{concurrent::parallel, Attempts};
///
/// let get_body = || async {
///     reqwest::get("https://www.rust-lang.org")
///         .await?
///         .text()
///         .await
/// };
/// let body = fure::retry(get_body, parallel(Attempts::new(3))).await?;
/// println!("body = {}", body);
/// # Ok(())
/// # }
/// ```
pub fn retry<P, T, E, CF>(create_f: CF, policy: P) -> Retry<P, T, E, CF>
where
    P: Policy<T, E>,
    CF: CreateFuture<T, E>,
{
    Retry::new(policy, create_f)
}

/// A trait is used to create futures which then will be run.
pub trait CreateFuture<T, E> {
    type Output: Future<Output = Result<T, E>>;

    fn create(&mut self) -> Self::Output;
}

impl<T, E, F, FN> CreateFuture<T, E> for FN
where
    FN: FnMut() -> F,
    F: Future<Output = Result<T, E>>,
{
    type Output = F;

    fn create(&mut self) -> Self::Output {
        self()
    }
}

/// A generic policy used to determine how a future should be retried.
pub trait Policy<T, E>: Sized {
    /// Future type returned by [`Policy::force_retry_after`].
    type ForceRetryFuture: Future;

    /// Future type returned by [`Policy::retry`].
    type RetryFuture: Future<Output = Self>;

    /// When completes a [`Policy::retry`] will be called with [`None`] argument.
    ///
    /// All previous futures won't be cancelled.
    fn force_retry_after(&self) -> Self::ForceRetryFuture;

    /// Check the policy if a new futures should be created using [`CreateFuture`].
    ///
    /// This method is passed a reference to the future's result or [`None`] if it was called after [`Policy::force_retry_after`].
    ///
    /// If a new future should be created and polled return [`Some`] with a new policy, otherwise return [`None`].
    fn retry(self, result: Option<Result<&T, &E>>) -> Option<Self::RetryFuture>;
}

#[cfg(test)]
mod tests {
    use crate::{retry, Policy};
    use std::future::pending;
    use std::{
        future::{ready, Future, Ready},
        pin::Pin,
        sync::{Arc, Mutex},
    };

    #[cfg(all(not(feature = "async-std"), test))]
    pub fn run_test(f: impl std::future::Future + Send + 'static) {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(f);
    }

    #[cfg(all(feature = "async-std", test))]
    pub fn run_test(f: impl std::future::Future + Send + 'static) {
        async_std::task::block_on(f);
    }

    #[cfg(all(not(feature = "async-std"), test))]
    pub use tokio::task::spawn;

    #[cfg(all(feature = "async-std", test))]
    pub use async_std::task::spawn;

    #[cfg(all(not(feature = "async-std"), test))]
    pub use tokio::task::yield_now;

    #[cfg(all(feature = "async-std", test))]
    pub use async_std::task::yield_now;

    #[test]
    fn should_drop_previous_delay_after_retry() {
        struct RetryTest {
            retry: usize,
        }

        impl<T, E> Policy<T, E> for RetryTest {
            type ForceRetryFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;
            type RetryFuture = Ready<Self>;

            fn force_retry_after(&self) -> Self::ForceRetryFuture {
                if self.retry == 1 {
                    Box::pin(pending())
                } else {
                    Box::pin(ready(()))
                }
            }

            fn retry(self, _result: Option<Result<&T, &E>>) -> Option<Self::RetryFuture> {
                if self.retry == 5 {
                    return None;
                } else {
                    Some(ready(Self {
                        retry: self.retry + 1,
                    }))
                }
            }
        }
        run_test(async {
            let call_count = Arc::new(Mutex::new(0));
            let create_fut = || {
                let call_count = call_count.clone();
                async move {
                    let call = {
                        let mut mutex_guard = call_count.lock().unwrap();
                        *mutex_guard += 1;
                        *mutex_guard
                    };
                    if call == 2 {
                        crate::tests::yield_now().await;
                        Err(())
                    } else if call == 3 {
                        pending().await
                    } else {
                        Err::<(), ()>(())
                    }
                }
            };

            let _ = retry(create_fut, RetryTest { retry: 0 }).await;

            let guard = call_count.lock().unwrap();
            assert_eq!(*guard, 6);
        })
    }
}
