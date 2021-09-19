use std::{
    future::{pending, ready, Future, Pending, Ready},
    pin::Pin,
    task::{Context, Poll},
};

use crate::RetryPolicy;

use super::RetryFailed;

pub trait Retry<T, E>: Sized {
    type RetryFuture: Future<Output = Self> + Unpin;

    fn retry(&self, result: &Result<T, E>) -> Option<Self::RetryFuture>;
}

pub struct Sequential<R> {
    retry: R,
}

impl<R> Sequential<R> {
    pub fn new(retry: R) -> Self {
        Self { retry }
    }
}

impl<R, T, E> RetryPolicy<T, E> for Sequential<R>
where
    R: Retry<T, E>,
{
    type ForceRetryFuture = Pending<()>;
    type RetryFuture = SeqMap<R, T, E>;

    fn force_retry_after(&self) -> Self::ForceRetryFuture {
        pending()
    }

    fn retry(&self, result: Option<&Result<T, E>>) -> Option<Self::RetryFuture> {
        let result = result.expect("Result must be some");
        let retry_f = self.retry.retry(result)?;
        Some(SeqMap { retry_f })
    }
}

impl<T, E> Retry<T, E> for RetryFailed {
    type RetryFuture = Ready<Self>;

    fn retry(&self, result: &Result<T, E>) -> Option<Self::RetryFuture> {
        self.retry(Some(result)).map(ready)
    }
}

pub struct SeqMap<R, T, E>
where
    R: Retry<T, E>,
{
    retry_f: R::RetryFuture,
}

impl<R, T, E> Future for SeqMap<R, T, E>
where
    R: Retry<T, E>,
{
    type Output = Sequential<R>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.retry_f).poll(cx) {
            Poll::Ready(retry) => Poll::Ready(Sequential { retry }),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::{
        policies::{sequential::Sequential, RetryFailed},
        retry,
    };

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

        tokio::spawn(async move { retry(create_fut, Sequential::new(RetryFailed::new(1))).await });
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
