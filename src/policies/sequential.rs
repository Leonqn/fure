use std::{
    future::{pending, ready, Future, Pending, Ready},
    pin::Pin,
    task::{Context, Poll},
};

use super::AttemptsPolicy;
use crate::Policy;
use pin_project_lite::pin_project;

pub trait SequentialPolicy<T, E>: Sized {
    type RetryFuture: Future<Output = Self>;

    fn retry(self, result: Result<&T, &E>) -> Option<Self::RetryFuture>;
}

#[cfg(any(feature = "tokio", feature = "async-std"))]
mod retry_backoff {
    use pin_project_lite::pin_project;

    use super::*;
    use std::time::Duration;

    pub struct BackoffPolicy<P, I> {
        policy: P,
        backoff: I,
    }

    impl<P, I> BackoffPolicy<P, I> {
        pub fn new(policy: P, backoff: I) -> Self {
            Self { policy, backoff }
        }
    }

    impl<'a, P, I, T, E> SequentialPolicy<T, E> for BackoffPolicy<P, I>
    where
        P: SequentialPolicy<T, E>,
        I: Iterator<Item = Duration>,
    {
        type RetryFuture = SeqDelay<P::RetryFuture, I>;

        fn retry(self, result: Result<&T, &E>) -> Option<Self::RetryFuture> {
            let policy = self.policy.retry(result)?;
            Some(SeqDelay {
                backoff: Some(self.backoff),
                state: SeqDelayState::RetryFut { f: policy },
            })
        }
    }

    pin_project! {
        pub struct SeqDelay<F, I>
        where F: Future {
            backoff: Option<I>,
            #[pin]
            state: SeqDelayState<F>,
        }
    }

    pin_project! {
        #[project = SeqDelayStateProj]
        pub enum SeqDelayState<F>
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
        type Output = BackoffPolicy<F::Output, I>;

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
                                    return Poll::Ready(BackoffPolicy::new(
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
                            return Poll::Ready(BackoffPolicy::new(
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

pub struct SequentialRetry<P> {
    policy: P,
}

impl<P> SequentialRetry<P> {
    pub fn new(policy: P) -> Self {
        Self { policy }
    }
}

impl<P, T, E> Policy<T, E> for SequentialRetry<P>
where
    P: SequentialPolicy<T, E>,
{
    type ForceRetryFuture = Pending<()>;
    type RetryFuture = SeqMap<P, T, E>;

    fn force_retry_after(&self) -> Self::ForceRetryFuture {
        pending()
    }

    fn retry(self, result: Option<Result<&T, &E>>) -> Option<Self::RetryFuture> {
        let result = result.expect("Result in sequential policy must be some");
        let policy_f = self.policy.retry(result)?;
        Some(SeqMap { policy_f })
    }
}

impl<T, E> SequentialPolicy<T, E> for AttemptsPolicy {
    type RetryFuture = Ready<Self>;

    fn retry(self, result: Result<&T, &E>) -> Option<Self::RetryFuture> {
        self.retry(Some(result)).map(ready)
    }
}

pin_project! {
    pub struct SeqMap<P, T, E>
    where
        P: SequentialPolicy<T, E>,
    {
        #[pin]
        policy_f: P::RetryFuture,
    }
}

impl<P, T, E> Future for SeqMap<P, T, E>
where
    P: SequentialPolicy<T, E>,
{
    type Output = SequentialRetry<P>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.project().policy_f.poll(cx) {
            Poll::Ready(policy) => Poll::Ready(SequentialRetry { policy }),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(all(any(feature = "tokio", feature = "async-std"), test))]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::tests::run_test;
    use crate::{
        policies::{sequential::SequentialRetry, AttemptsPolicy},
        retry,
    };

    mod retry_backoff {
        use std::time::{Duration, Instant};

        use crate::{backoff, policies::sequential::BackoffPolicy};

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
                    SequentialRetry::new(BackoffPolicy::new(
                        AttemptsPolicy::new(2),
                        backoff::exponential(
                            Duration::from_millis(50),
                            2,
                            Some(Duration::from_secs(1)),
                        ),
                    )),
                ))
                .await;

                assert!(now.elapsed() > Duration::from_millis(150))
            })
        }
    }

    mod retry_failed {
        use std::time::Duration;

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
                    retry(create_fut, SequentialRetry::new(AttemptsPolicy::new(1))).await
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
