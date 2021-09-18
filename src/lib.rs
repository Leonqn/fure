use std::future::{ready, Future, Ready};
use std::{pin::Pin, time::Duration};

use future::ConcurrentRetry;
use tokio::time::{sleep, Sleep};

pub mod future;

pub fn retry<A, T, E, F, CF>(create_f: CF, attempt: A) -> ConcurrentRetry<A, T, E, F, CF>
where
    A: Attempt<T, E>,
    F: Future<Output = Result<T, E>> + Unpin,
    CF: CreateFuture<F>,
{
    ConcurrentRetry::new(attempt, create_f, vec![], None)
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

pub trait Attempt<T, E>: Sized {
    type DelayFuture: Future + Unpin;

    fn delay(&self) -> Self::DelayFuture;

    fn next(&self, result: Option<&Result<T, E>>) -> Option<Self>;
}

#[derive(Debug, Clone, Copy)]
pub struct Concurrent {
    max_additional_attempts: usize,
}

impl Concurrent {
    pub fn new(max_additional_attempts: usize) -> Self {
        Self {
            max_additional_attempts,
        }
    }

    fn dec(self) -> Option<Self> {
        self.max_additional_attempts.checked_sub(1).map(Self::new)
    }
}

impl<T, E> Attempt<T, E> for Concurrent {
    type DelayFuture = Ready<()>;

    fn delay(&self) -> Self::DelayFuture {
        ready(())
    }

    fn next(&self, result: Option<&Result<T, E>>) -> Option<Self> {
        match result {
            Some(Ok(_)) => None,
            _ => self.dec(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[cfg(feature = "tokio")]
pub struct DelayedConcurrent {
    parallel: Concurrent,
    next_attempt_delay: Duration,
}

#[cfg(feature = "tokio")]
impl DelayedConcurrent {
    pub fn new(max_additional_attempts: usize, next_attempt_delay: Duration) -> Self {
        Self {
            parallel: Concurrent::new(max_additional_attempts),
            next_attempt_delay,
        }
    }
}

#[cfg(feature = "tokio")]
impl<T, E> Attempt<T, E> for DelayedConcurrent {
    type DelayFuture = Pin<Box<Sleep>>;

    fn delay(&self) -> Self::DelayFuture {
        Box::pin(sleep(self.next_attempt_delay))
    }

    fn next(&self, result: Option<&Result<T, E>>) -> Option<Self> {
        Some(Self {
            parallel: self.parallel.next(result)?,
            next_attempt_delay: self.next_attempt_delay,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    use futures_util::FutureExt;
    use tokio::time::sleep;

    use crate::{retry, Concurrent, DelayedConcurrent};

    #[tokio::test]
    async fn concurrent_should_run_only_one_future_when_it_is_completed() {
        let call_count = Arc::new(Mutex::new(0));
        let create_fut = || {
            let call_count = call_count.clone();
            async move {
                let mut mutex_guard = call_count.lock().unwrap();
                *mutex_guard += 1;
                Ok::<(), ()>(())
            }
            .boxed()
        };

        let result = retry(create_fut, Concurrent::new(1)).await;

        let guard = call_count.lock().unwrap();
        assert_eq!(*guard, 1);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn concurrent_should_run_all_futures_when_not_ready() {
        let call_count = Arc::new(Mutex::new(0));
        let create_fut = || {
            let call_count = call_count.clone();
            async move {
                {
                    let mut mutex_guard = call_count.lock().unwrap();
                    *mutex_guard += 1;
                }
                sleep(Duration::from_millis(500)).await;
                Ok::<(), ()>(())
            }
            .boxed()
        };

        let result = retry(create_fut, Concurrent::new(2)).await;

        let guard = call_count.lock().unwrap();
        assert_eq!(*guard, 3);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn concurrent_should_run_futures_till_ready_one() {
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
                    Ok::<(), ()>(())
                } else {
                    sleep(Duration::from_millis(500)).await;
                    Ok::<(), ()>(())
                }
            }
            .boxed()
        };

        let result = retry(create_fut, Concurrent::new(2)).await;

        let guard = call_count.lock().unwrap();
        assert_eq!(*guard, 2);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn should_retry_when_failed() {
        let call_count = Arc::new(Mutex::new(0));
        let create_fut = || {
            let call_count = call_count.clone();
            async move {
                let mut mutex_guard = call_count.lock().unwrap();
                *mutex_guard += 1;
                if *mutex_guard == 1 {
                    Err(())
                } else {
                    Ok(())
                }
            }
            .boxed()
        };

        let result = retry(
            create_fut,
            DelayedConcurrent::new(1, Duration::from_secs(10000)),
        )
        .await;

        let guard = call_count.lock().unwrap();
        assert_eq!(*guard, 2);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn should_return_second_error_when_all_failed() {
        let call_count = Arc::new(Mutex::new(0));
        let create_fut = || {
            let call_count = call_count.clone();
            async move {
                let mut mutex_guard = call_count.lock().unwrap();
                *mutex_guard += 1;
                if *mutex_guard == 1 {
                    Err::<(), _>(*mutex_guard)
                } else {
                    Err(*mutex_guard)
                }
            }
            .boxed()
        };

        let result = retry(
            create_fut,
            DelayedConcurrent::new(1, Duration::from_secs(10000)),
        )
        .await;

        let guard = call_count.lock().unwrap();
        assert_eq!(*guard, 2);
        assert_eq!(result, Err(2));
    }

    #[tokio::test]
    async fn should_retry_after_delay() {
        let call_count = Arc::new(Mutex::new(0));
        let create_fut = || {
            let call_count = call_count.clone();
            async move {
                let call_count = {
                    let mut mutex_guard = call_count.lock().unwrap();
                    *mutex_guard += 1;
                    *mutex_guard
                };
                if call_count == 1 {
                    sleep(Duration::from_secs(10000000)).await;
                    Ok::<_, ()>(())
                } else {
                    Ok(())
                }
            }
            .boxed()
        };

        let result = retry(
            create_fut,
            DelayedConcurrent::new(1, Duration::from_millis(1)),
        )
        .await;

        let guard = call_count.lock().unwrap();
        assert_eq!(*guard, 2);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn should_not_retry_when_ok() {
        let call_count = Arc::new(Mutex::new(0));
        let create_fut = || {
            let call_count = call_count.clone();
            async move {
                let call_count = {
                    let mut mutex_guard = call_count.lock().unwrap();
                    *mutex_guard += 1;
                    *mutex_guard
                };
                if call_count == 1 {
                    Ok::<_, ()>(())
                } else {
                    Ok(())
                }
            }
            .boxed()
        };

        let result = retry(
            create_fut,
            DelayedConcurrent::new(1, Duration::from_millis(1)),
        )
        .await;

        let guard = call_count.lock().unwrap();
        assert_eq!(*guard, 1);
        assert!(result.is_ok());
    }
}
