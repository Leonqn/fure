use std::{
    future::{ready, Ready},
    time::Duration,
};

use crate::Policy;

use super::AttemptsPolicy;

pub trait ConcurrentPolicy<T, E>: Sized {
    fn retry(self, result: Option<Result<&T, &E>>) -> Option<Self>;
}

pub trait DelayedConcurrentPolicy<T, E>: ConcurrentPolicy<T, E> {
    fn force_retry_after(&self) -> Duration;
}

impl<T, E> ConcurrentPolicy<T, E> for AttemptsPolicy {
    fn retry(self, result: Option<Result<&T, &E>>) -> Option<Self> {
        self.retry(result)
    }
}

pub struct IntervalPolicy<P> {
    policy: P,
    force_delay_after: Duration,
}

impl<P> IntervalPolicy<P> {
    pub fn new(policy: P, force_delay_after: Duration) -> Self {
        Self {
            policy,
            force_delay_after,
        }
    }
}
impl<P, T, E> ConcurrentPolicy<T, E> for IntervalPolicy<P>
where
    P: ConcurrentPolicy<T, E>,
{
    fn retry(self, result: Option<Result<&T, &E>>) -> Option<Self> {
        let force_delay_after = self.force_delay_after;
        self.policy
            .retry(result)
            .map(|x| Self::new(x, force_delay_after))
    }
}

impl<P: ConcurrentPolicy<T, E>, T, E> DelayedConcurrentPolicy<T, E> for IntervalPolicy<P> {
    fn force_retry_after(&self) -> Duration {
        self.force_delay_after
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ConcurrentRetry<P> {
    policy: P,
}

impl<P> ConcurrentRetry<P> {
    pub fn new(policy: P) -> Self {
        Self { policy }
    }
}

impl<P, T, E> Policy<T, E> for ConcurrentRetry<P>
where
    P: ConcurrentPolicy<T, E>,
{
    type ForceRetryFuture = Ready<()>;
    type RetryFuture = Ready<Self>;

    fn force_retry_after(&self) -> Self::ForceRetryFuture {
        ready(())
    }

    fn retry(self, result: Option<Result<&T, &E>>) -> Option<Self::RetryFuture> {
        Some(ready(Self {
            policy: self.policy.retry(result)?,
        }))
    }
}
#[cfg(any(feature = "tokio", feature = "async-std"))]
mod delayed {
    use super::*;

    #[derive(Debug, Clone, Copy)]
    pub struct DelayedConcurrentRetry<P> {
        policy: P,
    }

    impl<P> DelayedConcurrentRetry<P> {
        pub fn new(policy: P) -> Self {
            Self { policy }
        }
    }

    impl<P, T, E> Policy<T, E> for DelayedConcurrentRetry<P>
    where
        P: DelayedConcurrentPolicy<T, E>,
    {
        type ForceRetryFuture = crate::sleep::Sleep;
        type RetryFuture = Ready<Self>;

        fn force_retry_after(&self) -> Self::ForceRetryFuture {
            crate::sleep::sleep(self.policy.force_retry_after())
        }

        fn retry(self, result: Option<Result<&T, &E>>) -> Option<Self::RetryFuture> {
            Some(ready(Self {
                policy: self.policy.retry(result)?,
            }))
        }
    }
}
#[cfg(any(feature = "tokio", feature = "async-std"))]
pub use delayed::*;

#[cfg(test)]
mod test {
    use std::sync::{Arc, Mutex};

    use super::{super::AttemptsPolicy, ConcurrentRetry};
    use crate::retry;
    use crate::tests::run_test;
    use std::future::pending;

    mod concurrent {

        use super::*;

        #[test]
        fn should_run_only_one_future_when_first_completed() {
            run_test(async {
                let call_count = Arc::new(Mutex::new(0));
                let create_fut = || {
                    let call_count = call_count.clone();
                    async move {
                        crate::tests::yield_now().await;
                        let mut mutex_guard = call_count.lock().unwrap();
                        *mutex_guard += 1;
                        Ok::<(), ()>(())
                    }
                };

                let result = retry(create_fut, ConcurrentRetry::new(AttemptsPolicy::new(2))).await;

                let guard = call_count.lock().unwrap();
                assert_eq!(*guard, 1);
                assert!(result.is_ok());
            })
        }

        #[test]
        fn should_run_all_non_ready_futures() {
            run_test(async {
                let call_count = Arc::new(Mutex::new(0));
                let create_fut = || {
                    let call_count = call_count.clone();
                    async move {
                        crate::tests::yield_now().await;
                        let call = {
                            let mut mutex_guard = call_count.lock().unwrap();
                            *mutex_guard += 1;
                            *mutex_guard
                        };
                        if call == 3 {
                            Ok::<(), ()>(())
                        } else {
                            pending().await
                        }
                    }
                };

                let result = retry(create_fut, ConcurrentRetry::new(AttemptsPolicy::new(2))).await;

                let guard = call_count.lock().unwrap();
                assert_eq!(*guard, 3);
                assert!(result.is_ok());
            })
        }

        #[test]
        fn should_run_futures_till_ready_one() {
            run_test(async {
                let call_count = Arc::new(Mutex::new(0));
                let create_fut = || {
                    let call_count = call_count.clone();
                    async move {
                        crate::tests::yield_now().await;
                        let call = {
                            let mut mutex_guard = call_count.lock().unwrap();
                            *mutex_guard += 1;
                            *mutex_guard
                        };
                        if call == 2 {
                            Ok::<(), ()>(())
                        } else {
                            pending().await
                        }
                    }
                };

                let result = retry(create_fut, ConcurrentRetry::new(AttemptsPolicy::new(2))).await;

                let guard = call_count.lock().unwrap();
                assert_eq!(*guard, 2);
                assert!(result.is_ok());
            })
        }
    }

    #[cfg(any(feature = "tokio", feature = "async-std"))]
    mod delayed_concurrent {

        use std::time::{Duration, Instant};

        use crate::policies::concurrent::{DelayedConcurrentRetry, IntervalPolicy};

        use super::*;

        #[test]
        fn should_retry_when_failed() {
            run_test(async {
                let call_count = Arc::new(Mutex::new(0));
                let create_fut = || {
                    let call_count = call_count.clone();
                    async move {
                        crate::tests::yield_now().await;
                        let mut mutex_guard = call_count.lock().unwrap();
                        *mutex_guard += 1;
                        if *mutex_guard == 1 {
                            Err(())
                        } else {
                            Ok(())
                        }
                    }
                };

                let result = retry(
                    create_fut,
                    DelayedConcurrentRetry::new(IntervalPolicy::new(
                        AttemptsPolicy::new(2),
                        Duration::from_secs(10000),
                    )),
                )
                .await;

                let guard = call_count.lock().unwrap();
                assert_eq!(*guard, 2);
                assert!(result.is_ok());
            })
        }

        #[test]
        fn should_return_last_error_when_all_failed() {
            run_test(async move {
                let call_count = Arc::new(Mutex::new(0));
                let create_fut = || {
                    let call_count = call_count.clone();
                    async move {
                        crate::tests::yield_now().await;
                        let mut mutex_guard = call_count.lock().unwrap();
                        *mutex_guard += 1;
                        if *mutex_guard == 1 {
                            Err::<(), _>(*mutex_guard)
                        } else {
                            Err(*mutex_guard)
                        }
                    }
                };

                let result = retry(
                    create_fut,
                    DelayedConcurrentRetry::new(IntervalPolicy::new(
                        AttemptsPolicy::new(2),
                        Duration::from_secs(10000),
                    )),
                )
                .await;

                let guard = call_count.lock().unwrap();
                assert_eq!(*guard, 3);
                assert_eq!(result, Err(3));
            })
        }

        #[test]
        fn should_retry_after_delay() {
            run_test(async {
                let call_count = Arc::new(Mutex::new(0));
                let create_fut = || {
                    let call_count = call_count.clone();
                    async move {
                        crate::tests::yield_now().await;
                        let call_count = {
                            let mut mutex_guard = call_count.lock().unwrap();
                            *mutex_guard += 1;
                            *mutex_guard
                        };
                        if call_count == 1 {
                            pending::<Result<(), ()>>().await
                        } else {
                            Ok(())
                        }
                    }
                };
                let now = Instant::now();
                let result = retry(
                    create_fut,
                    DelayedConcurrentRetry::new(IntervalPolicy::new(
                        AttemptsPolicy::new(2),
                        Duration::from_millis(50),
                    )),
                )
                .await;

                let guard = call_count.lock().unwrap();
                assert_eq!(*guard, 2);
                assert!(now.elapsed() >= Duration::from_millis(50));
                assert!(result.is_ok());
            })
        }

        #[test]
        fn should_not_retry_when_ok() {
            run_test(async {
                let call_count = Arc::new(Mutex::new(0));
                let create_fut = || {
                    let call_count = call_count.clone();
                    Box::pin(async move {
                        crate::tests::yield_now().await;

                        let mut mutex_guard = call_count.lock().unwrap();
                        *mutex_guard += 1;
                        Ok::<_, ()>(())
                    })
                };

                let result = retry(
                    create_fut,
                    DelayedConcurrentRetry::new(IntervalPolicy::new(
                        AttemptsPolicy::new(2),
                        Duration::from_secs(10000),
                    )),
                )
                .await;

                let guard = call_count.lock().unwrap();
                assert_eq!(*guard, 1);
                assert!(result.is_ok());
            })
        }
    }
}
