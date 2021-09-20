#[cfg(all(feature = "tokio", feature = "async-std"))]
compile_error!("`tokio` and `async-std` features must not be enabled together");

use future::ConcurrentRetry;
use std::future::Future;

pub mod backoff;
pub mod future;
pub mod policies;

pub fn retry<R, T, E, F, CF>(create_f: CF, retry: R) -> ConcurrentRetry<R, T, E, F, CF>
where
    R: RetryPolicy<T, E>,
    F: Future<Output = Result<T, E>>,
    CF: CreateFuture<F>,
{
    ConcurrentRetry::new(retry, create_f)
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
    type ForceRetryFuture: Future;
    type RetryFuture: Future<Output = Self>;

    fn force_retry_after(&self) -> Self::ForceRetryFuture;

    fn retry(self, result: Option<Result<&T, &E>>) -> Option<Self::RetryFuture>;
}

#[cfg(test)]
mod tests {
    use crate::{retry, RetryPolicy};
    use std::future::pending;
    use std::{
        future::{ready, Future, Ready},
        pin::Pin,
        sync::{Arc, Mutex},
    };

    #[tokio::test]
    async fn should_drop_previous_delay_after_retry() {
        struct RetryTest {
            retry: usize,
        }

        impl<T, E> RetryPolicy<T, E> for RetryTest {
            type ForceRetryFuture = Pin<Box<dyn Future<Output = ()>>>;
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
                    tokio::task::yield_now().await;
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
    }
}
