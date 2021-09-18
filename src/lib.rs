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
        (self)()
    }
}

pub trait Attempt<T, E>: Sized {
    type DelayFuture: Future + Unpin;

    fn delay(&self) -> Self::DelayFuture;

    fn next(&self, result: Option<&Result<T, E>>) -> Option<Self>;
}

#[derive(Debug, Clone, Copy)]
pub struct Parallel {
    max_parallelism: usize,
}

impl Parallel {
    pub fn new(max_parallelism: usize) -> Self {
        Self { max_parallelism }
    }

    fn dec(self) -> Option<Self> {
        self.max_parallelism.checked_sub(1).map(Self::new)
    }
}

impl<T, E> Attempt<T, E> for Parallel {
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
pub struct DelayedParallel {
    parallel: Parallel,
    delay_attempt: Duration,
}

#[cfg(feature = "tokio")]
impl DelayedParallel {
    pub fn new(max_attempts: usize, delay_attempt: Duration) -> Self {
        Self {
            parallel: Parallel::new(max_attempts),
            delay_attempt,
        }
    }
}

#[cfg(feature = "tokio")]
impl<T, E> Attempt<T, E> for DelayedParallel {
    type DelayFuture = Pin<Box<Sleep>>;

    fn delay(&self) -> Self::DelayFuture {
        Box::pin(sleep(self.delay_attempt))
    }

    fn next(&self, result: Option<&Result<T, E>>) -> Option<Self> {
        Some(Self {
            parallel: self.parallel.next(result)?,
            delay_attempt: self.delay_attempt,
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

    use crate::{retry, DelayedParallel};

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
            DelayedParallel::new(1, Duration::from_secs(10000)),
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
            DelayedParallel::new(1, Duration::from_secs(10000)),
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
            DelayedParallel::new(1, Duration::from_millis(1)),
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
            DelayedParallel::new(1, Duration::from_millis(1)),
        )
        .await;

        let guard = call_count.lock().unwrap();
        assert_eq!(*guard, 1);
        assert!(result.is_ok());
    }
}
