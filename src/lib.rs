//! A crate for retrying futures.
//!
//! [`Policy`] trait will help you define different retry policies.
//!
//! Some builtin policies can be found in [`policies`] module.
//!
//! By default this create uses `tokio` timers for [`crate::policies::interval`] and [`crate::policies::backoff`] policies,
//! but `async-std` is also available as feature `async-std`.
//! # Examples.
//! ## Interval retry.
//! Starts with sending a request, setting up a 1 second timer, and waits for either of them.
//!
//! If the timer completes first (it means that the request didn't complete in 1 second) one more request fires.
//!
//! If the request completes first and it has an [`Ok`] response it is returned, if request has an [`Err`] response, timer resets and a new request fires.
//!
//! At most 4 requests will be fired.
//! ```
//! # async fn run() -> Result<(), reqwest::Error> {
//! use fure::policies::{interval, attempts};
//! use std::time::Duration;
//!
//! let get_body = || async {
//!     reqwest::get("https://www.rust-lang.org")
//!         .await?
//!         .text()
//!         .await
//! };
//! let policy = attempts(interval(Duration::from_secs(1)), 3);
//! let body = fure::retry(get_body, policy).await?;
//! println!("body = {}", body);
//! # Ok(())
//! # }
//! ```
//! ## Sequential retry with backoff.
//! Retries attempts requests with an exponential backoff and a jitter.
//! ```
//! # async fn run() -> Result<(), reqwest::Error> {
//! use fure::{policies::{backoff, cond}, backoff::{exponential, jitter}};
//! use std::time::Duration;
//!
//! let get_body = || async {
//!     reqwest::get("https://www.rust-lang.org")
//!         .await?
//!         .text()
//!         .await
//! };
//! let exp_backoff = exponential(Duration::from_secs(1), 2, Some(Duration::from_secs(10)))
//!     .map(jitter);
//! let policy = cond(backoff(exp_backoff), |result| !matches!(result, Some(Ok(_))));
//! let body = fure::retry(get_body, policy).await?;
//! println!("body = {}", body);
//! # Ok(())
//! # }
//! ```
//! ## Implementing your own policy.
//! It behaves like the interval policy above, but if it hits `TOO_MANY_REQUESTS` it will wait for some seconds before sending next request.
//! ```
//! # async fn run() -> Result<(), reqwest::Error> {
//! use std::{future::{Future, ready}, pin::Pin, time::Duration};
//! use fure::Policy;
//! use reqwest::{Error, Response, StatusCode};
//!
//! struct RetryPolicy;
//!
//! impl Policy<Response, Error> for RetryPolicy {
//!     type ForceRetryFuture = tokio::time::Sleep;
//!
//!     type RetryFuture = Pin<Box<dyn Future<Output = Self>>>;
//!
//!     fn force_retry_after(&self) -> Self::ForceRetryFuture {
//!         tokio::time::sleep(Duration::from_millis(100))
//!     }
//!
//!     fn retry(
//!         self,
//!         result: Option<Result<&Response, &Error>>,
//!     ) -> Option<Self::RetryFuture> {
//!         match result {
//!             Some(Ok(response)) => match response.status() {
//!                 StatusCode::OK => None,
//!                 StatusCode::TOO_MANY_REQUESTS => {
//!                     let retry_after_secs: u64 = response
//!                         .headers()
//!                         .get("Retry-After")
//!                         .and_then(|x| x.to_str().ok()?.parse().ok())
//!                         .unwrap_or(1);
//!                     Some(Box::pin(async move {
//!                         tokio::time::sleep(Duration::from_secs(retry_after_secs)).await;
//!                         self
//!                     }))
//!                 }
//!                 _ => Some(Box::pin(ready(self))),
//!             },
//!             _ => Some(Box::pin(ready(self))),
//!         }
//!     }
//! }
//!
//! let get_response = || async {
//!     reqwest::get("https://www.rust-lang.org").await
//! };
//! let response = fure::retry(get_response, RetryPolicy).await?;
//! println!("body = {}", response.text().await?);
//! # Ok(())
//! # }
//! ```

#[cfg(all(feature = "tokio", feature = "async-std"))]
compile_error!("`tokio` and `async-std` features must not be enabled together");

use std::future::Future;

/// Backoff utilities for [`crate::policies::backoff`] policy.
pub mod backoff;

/// Some builtin implementations of [`Policy`].
pub mod policies;

/// Sleep functions for `tokio` and `async-std`.
#[cfg(any(feature = "tokio", feature = "async-std"))]
pub mod sleep;

pub use future::Retry;

mod future;

/// Runs futures created by [`CreateFuture`] accorrding to [`Policy`].
/// ## Simple concurrent policy
/// Runs at most 4 concurrent futures and waits a successful one.
/// ```
/// # async fn run() -> Result<(), reqwest::Error> {
/// use fure::policies::{parallel, attempts};
///
/// let get_body = || async {
///     reqwest::get("https://www.rust-lang.org")
///         .await?
///         .text()
///         .await
/// };
/// let body = fure::retry(get_body, attempts(parallel(), 3)).await?;
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

/// A generic policy is used to determine how a future should be retried.
pub trait Policy<T, E>: Sized {
    /// Future type returned by [`Policy::force_retry_after`].
    type ForceRetryFuture: Future;

    /// Future type returned by [`Policy::retry`].
    type RetryFuture: Future<Output = Self>;

    /// This method is called right after calling your future and
    /// if it completes before your future [`Policy::retry`] will be called with [`None`] argument
    /// without cancelling your future.
    ///
    /// If your future completes first, current [`Self::ForceRetryFuture`] will be dropped and this method will be called again if needs.
    fn force_retry_after(&self) -> Self::ForceRetryFuture;

    /// Checks the policy if a new future should be created and polled using [`CreateFuture`].
    ///
    /// If the future should be created return [`Some`] with a new policy, otherwise return [`None`].
    ///
    /// The future will be created only after this method resolves the new policy
    ///
    /// This method is passed a reference to the future's result or [`None`] if it is called after [`Policy::force_retry_after`].
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
