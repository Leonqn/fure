use std::{
    future::{ready, Ready},
    pin::Pin,
    time::Duration,
};

use crate::RetryPolicy;

use super::RetryFailed;

pub trait Retry<T, E>: Sized {
    fn retry(&self, result: Option<&Result<T, E>>) -> Option<Self>;
}

pub trait RetryDelayed<T, E>: Retry<T, E> {
    fn force_retry_after(&self) -> Duration;
}

impl<T, E> Retry<T, E> for RetryFailed {
    fn retry(&self, result: Option<&Result<T, E>>) -> Option<Self> {
        self.retry(result)
    }
}

pub struct RetryFailedDelayed<R> {
    retry: R,
    force_delay_after: Duration,
}

impl<R> RetryFailedDelayed<R> {
    pub fn new(retry: R, force_delay_after: Duration) -> Self {
        Self {
            retry,
            force_delay_after,
        }
    }
}
impl<R, T, E> Retry<T, E> for RetryFailedDelayed<R>
where
    R: Retry<T, E>,
{
    fn retry(&self, result: Option<&Result<T, E>>) -> Option<Self> {
        self.retry
            .retry(result)
            .map(|x| Self::new(x, self.force_delay_after))
    }
}

impl<R: Retry<T, E>, T, E> RetryDelayed<T, E> for RetryFailedDelayed<R> {
    fn force_retry_after(&self) -> Duration {
        self.force_delay_after
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Concurrent<R> {
    retry: R,
}

impl<R> Concurrent<R> {
    pub fn new(retry: R) -> Self {
        Self { retry }
    }
}

impl<R, T, E> RetryPolicy<T, E> for Concurrent<R>
where
    R: Retry<T, E>,
{
    type ForceRetryFuture = Ready<()>;
    type RetryFuture = Ready<Self>;

    fn force_retry_after(&self) -> Self::ForceRetryFuture {
        ready(())
    }

    fn retry(&self, result: Option<&Result<T, E>>) -> Option<Self::RetryFuture> {
        Some(ready(Self {
            retry: self.retry.retry(result)?,
        }))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DelayedConcurrent<R> {
    retry: R,
}

impl<R> DelayedConcurrent<R> {
    pub fn new(retry: R) -> Self {
        Self { retry }
    }
}

impl<R, T, E> RetryPolicy<T, E> for DelayedConcurrent<R>
where
    R: RetryDelayed<T, E>,
{
    #[cfg(feature = "tokio")]
    type ForceRetryFuture = Pin<Box<tokio::time::Sleep>>;
    #[cfg(feature = "tokio")]
    fn force_retry_after(&self) -> Self::ForceRetryFuture {
        Box::pin(tokio::time::sleep(self.retry.force_retry_after()))
    }

    #[cfg(feature = "async-std")]
    type DelayFuture = Pin<Box<dyn std::future::Future<Output = ()>>>;
    #[cfg(feature = "async-std")]
    fn force_retry_after(&self) -> Self::ForceRetryFuture {
        Box::pin(async_std::task::sleep(self.retry.force_retry_after()))
    }

    type RetryFuture = Ready<Self>;

    fn retry(&self, result: Option<&Result<T, E>>) -> Option<Self::RetryFuture> {
        Some(ready(Self {
            retry: self.retry.retry(result)?,
        }))
    }
}

#[cfg(test)]
mod test {
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    use super::{super::RetryFailed, Concurrent, DelayedConcurrent, RetryFailedDelayed};
    use crate::retry;
    use std::future::pending;
    use std::time::Instant;

    mod concurrent {
        use super::*;

        #[tokio::test]
        async fn should_run_only_one_future_when_first_completed() {
            let call_count = Arc::new(Mutex::new(0));
            let create_fut = || {
                let call_count = call_count.clone();
                Box::pin(async move {
                    let mut mutex_guard = call_count.lock().unwrap();
                    *mutex_guard += 1;
                    Ok::<(), ()>(())
                })
            };

            let result = retry(create_fut, Concurrent::new(RetryFailed::new(2))).await;

            let guard = call_count.lock().unwrap();
            assert_eq!(*guard, 1);
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn should_run_all_non_ready_futures() {
            let call_count = Arc::new(Mutex::new(0));
            let create_fut = || {
                let call_count = call_count.clone();
                Box::pin(async move {
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
                })
            };

            let result = retry(create_fut, Concurrent::new(RetryFailed::new(2))).await;

            let guard = call_count.lock().unwrap();
            assert_eq!(*guard, 3);
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn should_run_futures_till_ready_one() {
            let call_count = Arc::new(Mutex::new(0));
            let create_fut = || {
                let call_count = call_count.clone();
                Box::pin(async move {
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
                })
            };

            let result = retry(create_fut, Concurrent::new(RetryFailed::new(2))).await;

            let guard = call_count.lock().unwrap();
            assert_eq!(*guard, 2);
            assert!(result.is_ok());
        }
    }

    mod delayed_concurrent {

        use super::*;

        #[tokio::test]
        async fn should_retry_when_failed() {
            let call_count = Arc::new(Mutex::new(0));
            let create_fut = || {
                let call_count = call_count.clone();
                Box::pin(async move {
                    let mut mutex_guard = call_count.lock().unwrap();
                    *mutex_guard += 1;
                    if *mutex_guard == 1 {
                        Err(())
                    } else {
                        Ok(())
                    }
                })
            };

            let result = retry(
                create_fut,
                DelayedConcurrent::new(RetryFailedDelayed::new(
                    RetryFailed::new(2),
                    Duration::from_secs(10000),
                )),
            )
            .await;

            let guard = call_count.lock().unwrap();
            assert_eq!(*guard, 2);
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn should_return_last_error_when_all_failed() {
            let call_count = Arc::new(Mutex::new(0));
            let create_fut = || {
                let call_count = call_count.clone();
                Box::pin(async move {
                    let mut mutex_guard = call_count.lock().unwrap();
                    *mutex_guard += 1;
                    if *mutex_guard == 1 {
                        Err::<(), _>(*mutex_guard)
                    } else {
                        Err(*mutex_guard)
                    }
                })
            };

            let result = retry(
                create_fut,
                DelayedConcurrent::new(RetryFailedDelayed::new(
                    RetryFailed::new(2),
                    Duration::from_secs(10000),
                )),
            )
            .await;

            let guard = call_count.lock().unwrap();
            assert_eq!(*guard, 3);
            assert_eq!(result, Err(3));
        }

        #[tokio::test]
        async fn should_retry_after_delay() {
            let call_count = Arc::new(Mutex::new(0));
            let create_fut = || {
                let call_count = call_count.clone();
                Box::pin(async move {
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
                })
            };
            let now = Instant::now();
            let result = retry(
                create_fut,
                DelayedConcurrent::new(RetryFailedDelayed::new(
                    RetryFailed::new(2),
                    Duration::from_millis(50),
                )),
            )
            .await;

            let guard = call_count.lock().unwrap();
            assert_eq!(*guard, 2);
            assert!(now.elapsed() >= Duration::from_millis(50));
            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn should_not_retry_when_ok() {
            let call_count = Arc::new(Mutex::new(0));
            let create_fut = || {
                let call_count = call_count.clone();
                Box::pin(async move {
                    let mut mutex_guard = call_count.lock().unwrap();
                    *mutex_guard += 1;
                    Ok::<_, ()>(())
                })
            };

            let result = retry(
                create_fut,
                DelayedConcurrent::new(RetryFailedDelayed::new(
                    RetryFailed::new(2),
                    Duration::from_secs(10000),
                )),
            )
            .await;

            let guard = call_count.lock().unwrap();
            assert_eq!(*guard, 1);
            assert!(result.is_ok());
        }
    }
}
