use std::{
    future::{pending, ready, Pending, Ready},
    pin::Pin,
    time::Duration,
};

use crate::RetryPolicy;

#[derive(Debug, Clone, Copy)]
pub struct Sequential {
    retry: RetryFailed,
}

impl Sequential {
    pub fn new(max_retries: usize) -> Self {
        Self {
            retry: RetryFailed::new(max_retries),
        }
    }
}

impl<T, E> RetryPolicy<T, E> for Sequential {
    type DelayFuture = Pending<()>;

    fn force_retry_after(&self) -> Self::DelayFuture {
        pending()
    }

    fn retry(&self, result: Option<&Result<T, E>>) -> Option<Self> {
        Some(Self {
            retry: self.retry.next(result)?,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Concurrent {
    retry: RetryFailed,
}

impl Concurrent {
    pub fn new(max_retries: usize) -> Self {
        Self {
            retry: RetryFailed::new(max_retries),
        }
    }
}

impl<T, E> RetryPolicy<T, E> for Concurrent {
    type DelayFuture = Ready<()>;

    fn force_retry_after(&self) -> Self::DelayFuture {
        ready(())
    }

    fn retry(&self, result: Option<&Result<T, E>>) -> Option<Self> {
        Some(Self {
            retry: self.retry.next(result)?,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DelayedConcurrent {
    retry: RetryFailed,
    force_retry_after: Duration,
}

impl DelayedConcurrent {
    pub fn new(max_retries: usize, force_retry_after: Duration) -> Self {
        Self {
            retry: RetryFailed::new(max_retries),
            force_retry_after,
        }
    }
}

impl<T, E> RetryPolicy<T, E> for DelayedConcurrent {
    #[cfg(feature = "tokio")]
    type DelayFuture = Pin<Box<tokio::time::Sleep>>;
    #[cfg(feature = "tokio")]
    fn force_retry_after(&self) -> Self::DelayFuture {
        Box::pin(tokio::time::sleep(self.force_retry_after))
    }

    #[cfg(feature = "async-std")]
    type DelayFuture = Pin<Box<dyn std::future::Future<Output = ()>>>;
    #[cfg(feature = "async-std")]
    fn force_retry_after(&self) -> Self::DelayFuture {
        Box::pin(async_std::task::sleep(self.force_retry_after))
    }

    fn retry(&self, result: Option<&Result<T, E>>) -> Option<Self> {
        Some(Self {
            retry: self.retry.next(result)?,
            force_retry_after: self.force_retry_after,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct RetryFailed {
    max_retries: usize,
}

impl RetryFailed {
    fn new(max_retries: usize) -> Self {
        Self { max_retries }
    }
    fn next<T, E>(&self, result: Option<&Result<T, E>>) -> Option<Self> {
        match result {
            Some(Ok(_)) => None,
            _ => self.max_retries.checked_sub(1).map(Self::new),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        future::{ready, Future},
        pin::Pin,
        sync::{Arc, Mutex},
        time::Duration,
    };

    use super::{Concurrent, DelayedConcurrent, Sequential};
    use crate::{retry, RetryPolicy};
    use std::future::pending;

    #[tokio::test]
    async fn should_drop_previous_delay_after_retry() {
        struct RetryTest {
            retry: usize,
        }

        impl<T, E> RetryPolicy<T, E> for RetryTest {
            type DelayFuture = Pin<Box<dyn Future<Output = ()>>>;

            fn force_retry_after(&self) -> Self::DelayFuture {
                if self.retry == 1 {
                    Box::pin(pending())
                } else {
                    Box::pin(ready(()))
                }
            }

            fn retry(&self, _result: Option<&Result<T, E>>) -> Option<Self> {
                if self.retry == 5 {
                    return None;
                } else {
                    Some(Self {
                        retry: self.retry + 1,
                    })
                }
            }
        }
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
                    tokio::task::yield_now().await;
                    Err(())
                } else if call == 3 {
                    pending().await
                } else {
                    Err::<(), ()>(())
                }
            })
        };

        let _ = retry(create_fut, RetryTest { retry: 0 }).await;

        let guard = call_count.lock().unwrap();
        assert_eq!(*guard, 6);
    }

    mod sequential {
        use super::*;

        #[tokio::test]
        async fn should_run_futures_sequentially() {
            let call_count = Arc::new(Mutex::new(0));
            let pass_allowed = Arc::new(Mutex::new(false));
            let create_fut = {
                let call_count = call_count.clone();
                let pass_allowed = pass_allowed.clone();
                move || {
                    let call_count = call_count.clone();
                    let pass_allowed = pass_allowed.clone();
                    Box::pin(async move {
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
                            tokio::task::yield_now().await
                        }

                        Err::<(), ()>(())
                    })
                }
            };

            tokio::spawn(async move { retry(create_fut, Sequential::new(1)).await });
            tokio::task::yield_now().await;

            {
                let guard = call_count.lock().unwrap();
                assert_eq!(*guard, 1);
            }
            *pass_allowed.lock().unwrap() = true;
            tokio::task::yield_now().await;
            let guard = call_count.lock().unwrap();
            assert_eq!(*guard, 2);
        }
    }

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

            let result = retry(create_fut, Concurrent::new(2)).await;

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

            let result = retry(create_fut, Concurrent::new(2)).await;

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

            let result = retry(create_fut, Concurrent::new(2)).await;

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
                DelayedConcurrent::new(2, Duration::from_secs(10000)),
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
                DelayedConcurrent::new(2, Duration::from_secs(10000)),
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

            let result = retry(
                create_fut,
                DelayedConcurrent::new(2, Duration::from_millis(1)),
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
                Box::pin(async move {
                    let mut mutex_guard = call_count.lock().unwrap();
                    *mutex_guard += 1;
                    Ok::<_, ()>(())
                })
            };

            let result = retry(
                create_fut,
                DelayedConcurrent::new(2, Duration::from_millis(100000000)),
            )
            .await;

            let guard = call_count.lock().unwrap();
            assert_eq!(*guard, 1);
            assert!(result.is_ok());
        }
    }
}
