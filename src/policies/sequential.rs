use std::{
    future::{pending, ready, Future, Pending, Ready},
    pin::Pin,
    task::{Context, Poll},
};

use super::Attempts;
use crate::Policy;
use pin_project_lite::pin_project;

/// Creates a policy to run futures sequentially until [`SequentialRetry::retry`] returns [`None`].
/// ## Example
/// Sends at most 4 requests and returns the first rusult for which [`SequentialRetry::retry`] returns [`None`] otherwise returns last [`Err`] result.
/// ```
/// # async fn run() -> Result<(), reqwest::Error> {
/// use fure::policies::{sequential, Attempts};
///
/// let get_body = || async {
///     reqwest::get("https://www.rust-lang.org")
///         .await?
///         .text()
///         .await
/// };
/// let body = fure::retry(get_body, sequential(Attempts::new(3))).await?;
/// println!("body = {}", body);
/// # Ok(())
/// # }
/// ```
pub fn sequential<P>(policy: P) -> SequentialRetryPolicy<P> {
    SequentialRetryPolicy { policy }
}

/// A helper policy trait which is used for [`sequential`] and [`backoff`] functions.
pub trait SequentialRetry<T, E>: Sized {
    // Future type returned by [`SequentialRetry::retry`]
    type RetryFuture: Future<Output = Self>;

    /// Checks the policy if a new futures should be created.
    ///
    /// This method is passed a reference to the future's result.
    ///
    /// If a new future should be created and polled return [`Some`] with a new policy, otherwise return [`None`].

    fn retry(self, result: Result<&T, &E>) -> Option<Self::RetryFuture>;
}

impl<T, E> SequentialRetry<T, E> for Attempts<T, E> {
    type RetryFuture = Ready<Self>;

    fn retry(self, result: Result<&T, &E>) -> Option<Self::RetryFuture> {
        self.retry(Some(result)).map(ready)
    }
}

/// A policy is created by [`sequential`] function
pub struct SequentialRetryPolicy<P> {
    policy: P,
}

impl<P, T, E> Policy<T, E> for SequentialRetryPolicy<P>
where
    P: SequentialRetry<T, E>,
{
    type ForceRetryFuture = Pending<()>;
    type RetryFuture = SeqMap<P, T, E>;

    fn force_retry_after(&self) -> Self::ForceRetryFuture {
        pending()
    }

    fn retry(self, result: Option<Result<&T, &E>>) -> Option<Self::RetryFuture> {
        let result = result.expect("Result must be some because of pending() above");
        let policy_f = self.policy.retry(result)?;
        Some(SeqMap { policy_f })
    }
}

#[cfg(any(feature = "tokio", feature = "async-std"))]
mod retry_backoff {
    use super::*;
    use std::time::Duration;

    /// Creates a [`crate::Policy`] to run futures sequentially with specified backoff until [`SequentialRetry::retry`] returns [`None`].
    /// ## Example
    /// Sends at most 4 requests and returns the first rusult for which [`SequentialRetry::retry`] returns [`None`] otherwise returns last [`Err`] result.
    ///
    /// Each next request will be sent only after specified backoff.
    /// ```
    /// # async fn run() -> Result<(), reqwest::Error> {
    /// use fure::{backoff::fixed, policies::{backoff, Attempts}};
    /// use std::time::Duration;
    ///
    /// let get_body = || async {
    ///     reqwest::get("https://www.rust-lang.org")
    ///         .await?
    ///         .text()
    ///         .await
    /// };
    /// let fixed = fixed(Duration::from_secs(3));
    /// let body = fure::retry(get_body, backoff(Attempts::new(3), fixed)).await?;
    /// println!("body = {}", body);
    /// # Ok(())
    /// # }
    /// ```
    pub fn backoff<P, I>(policy: P, backoff: I) -> BackoffRetry<P, I> {
        BackoffRetry { policy, backoff }
    }

    /// A policy is created by [`backoff`] function
    pub struct BackoffRetry<P, I> {
        policy: P,
        backoff: I,
    }

    impl<'a, P, I, T, E> Policy<T, E> for BackoffRetry<P, I>
    where
        P: SequentialRetry<T, E>,
        I: Iterator<Item = Duration>,
    {
        type ForceRetryFuture = Pending<()>;
        type RetryFuture = SeqDelay<P::RetryFuture, I>;

        fn force_retry_after(&self) -> Self::ForceRetryFuture {
            pending()
        }

        fn retry(self, result: Option<Result<&T, &E>>) -> Option<Self::RetryFuture> {
            let policy = self
                .policy
                .retry(result.expect("Result must be some because of pending() above"))?;
            Some(SeqDelay {
                backoff: Some(self.backoff),
                state: SeqDelayState::RetryFut { f: policy },
            })
        }
    }

    pin_project! {
        /// A future for [`backoff`] policy
        pub struct SeqDelay<F, I>
        where F: Future {
            backoff: Option<I>,
            #[pin]
            state: SeqDelayState<F>,
        }
    }

    pin_project! {
        #[project = SeqDelayStateProj]
        enum SeqDelayState<F>
            where F: Future
        {
            RetryFut {#[pin] f: F },
            Delay { #[pin] delay: crate::sleep::Sleep, result: Option<F::Output> }
        }
    }

    impl<F, I> Future for SeqDelay<F, I>
    where
        F: Future,
        I: Iterator<Item = Duration>,
    {
        type Output = BackoffRetry<F::Output, I>;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            loop {
                let this = self.as_mut().project();
                let new_state = match this.state.project() {
                    SeqDelayStateProj::RetryFut { f } => match f.poll(cx) {
                        Poll::Ready(result) => {
                            match this.backoff.as_mut().expect("Iter must be some").next() {
                                Some(d) => SeqDelayState::Delay {
                                    delay: crate::sleep::sleep(d),
                                    result: Some(result),
                                },
                                None => {
                                    return Poll::Ready(backoff(
                                        result,
                                        this.backoff.take().expect("Iter must be some"),
                                    ))
                                }
                            }
                        }
                        Poll::Pending => return Poll::Pending,
                    },
                    SeqDelayStateProj::Delay { delay, result } => match delay.poll(cx) {
                        Poll::Ready(_) => {
                            return Poll::Ready(backoff(
                                result.take().expect("Result must be some"),
                                this.backoff.take().expect("Iter must be some"),
                            ))
                        }
                        Poll::Pending => return Poll::Pending,
                    },
                };
                self.as_mut().project().state.set(new_state)
            }
        }
    }
}
#[cfg(any(feature = "tokio", feature = "async-std"))]
pub use retry_backoff::*;

pin_project! {
    /// A future for [`sequential`] policy
    pub struct SeqMap<P, T, E>
    where
        P: SequentialRetry<T, E>,
    {
        #[pin]
        policy_f: P::RetryFuture,
    }
}

impl<P, T, E> Future for SeqMap<P, T, E>
where
    P: SequentialRetry<T, E>,
{
    type Output = SequentialRetryPolicy<P>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.project().policy_f.poll(cx) {
            Poll::Ready(policy) => Poll::Ready(SequentialRetryPolicy { policy }),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(all(any(feature = "tokio", feature = "async-std"), test))]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::tests::run_test;
    use crate::{policies::Attempts, retry};

    mod retry_backoff {
        use std::time::{Duration, Instant};

        use crate::{backoff::exponential, policies::sequential::backoff};

        use super::*;

        #[test]
        fn should_run_next_after_backoff() {
            run_test(async {
                let create_fut = || async move {
                    crate::tests::yield_now().await;
                    Err::<(), ()>(())
                };
                let now = Instant::now();

                let _result = crate::tests::spawn(retry(
                    create_fut,
                    backoff(
                        Attempts::new(2),
                        exponential(Duration::from_millis(50), 2, Some(Duration::from_secs(1))),
                    ),
                ))
                .await;

                assert!(now.elapsed() > Duration::from_millis(150))
            })
        }
    }

    mod retry_failed {
        use std::time::Duration;

        use crate::policies::sequential::sequential;

        use super::*;

        #[test]
        fn should_run_futures_sequentially() {
            run_test(async {
                let call_count = Arc::new(Mutex::new(0));
                let pass_allowed = Arc::new(Mutex::new(false));
                let create_fut = {
                    let call_count = call_count.clone();
                    let pass_allowed = pass_allowed.clone();
                    move || {
                        let call_count = call_count.clone();
                        let pass_allowed = pass_allowed.clone();
                        async move {
                            {
                                let mut mutex_guard = call_count.lock().unwrap();
                                *mutex_guard += 1;
                            }
                            loop {
                                {
                                    if *pass_allowed.lock().unwrap() {
                                        break;
                                    }
                                }
                                crate::tests::yield_now().await
                            }

                            Err::<(), ()>(())
                        }
                    }
                };

                crate::tests::spawn(async move {
                    retry(create_fut, sequential(Attempts::new(1))).await
                });
                crate::sleep::sleep(Duration::from_millis(10)).await;

                {
                    let guard = call_count.lock().unwrap();
                    assert_eq!(*guard, 1);
                }
                *pass_allowed.lock().unwrap() = true;
                crate::sleep::sleep(Duration::from_millis(10)).await;
                let guard = call_count.lock().unwrap();
                assert_eq!(*guard, 2);
            })
        }
    }
}
