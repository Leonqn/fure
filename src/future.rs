use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use crate::RetryPolicy;

#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct ConcurrentRetry<R, T, E, F, CF>
where
    R: RetryPolicy<T, E>,
{
    retry: Option<R>,
    retry_fut: Option<R::RetryFuture>,
    create_f: CF,
    running_futs: Vec<F>,
    delay: Option<R::ForceRetryFuture>,
    first_run: bool,
}

impl<R, T, E, F, CF> ConcurrentRetry<R, T, E, F, CF>
where
    R: RetryPolicy<T, E>,
{
    pub(crate) fn new(
        retry: R,
        create_f: CF,
        running_futs: Vec<F>,
        delay: Option<R::ForceRetryFuture>,
    ) -> Self {
        Self {
            retry: Some(retry),
            create_f,
            running_futs,
            delay,
            retry_fut: None,
            first_run: true,
        }
    }
}

impl<R, T, E, F, CF> ConcurrentRetry<R, T, E, F, CF>
where
    R: RetryPolicy<T, E>,
    F: Future<Output = Result<T, E>> + Unpin,
    CF: FnMut() -> F,
{
    fn poll_retry(&mut self, cx: &mut Context) {
        if let Some(Poll::Ready(retry)) = self.retry_fut.as_mut().map(|x| poll_unpin(x, cx)) {
            self.retry_fut = None;
            let f = (self.create_f)();
            self.running_futs.push(f);
            self.delay = Some(retry.force_retry_after());
            self.retry = Some(retry);
        }
    }
}

impl<R, T, E, F, CF> Unpin for ConcurrentRetry<R, T, E, F, CF>
where
    R: RetryPolicy<T, E>,
    F: Unpin,
{
}

impl<R, T, E, F, CF> Future for ConcurrentRetry<R, T, E, F, CF>
where
    R: RetryPolicy<T, E>,
    F: Future<Output = Result<T, E>> + Unpin,
    CF: FnMut() -> F,
{
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.first_run {
            self.first_run = false;
            let f = (self.create_f)();
            self.running_futs.push(f)
        }

        self.poll_retry(cx);
        loop {
            while let Some(r) = poll_vec(&mut self.running_futs, cx) {
                match self.retry.as_ref().and_then(|x| x.retry(Some(&r))) {
                    Some(retry) => {
                        self.retry_fut = Some(retry);
                        self.retry = None;
                        self.delay = None;
                        self.poll_retry(cx);
                    }
                    None => return Poll::Ready(r),
                }
            }
            match self.delay.as_mut() {
                Some(delay) => match poll_unpin(delay, cx) {
                    Poll::Ready(_) => match self.retry.as_ref().and_then(|x| x.retry(None)) {
                        Some(retry) => {
                            self.retry_fut = Some(retry);
                            self.retry = None;
                            self.delay = None;
                            self.poll_retry(cx);
                        }
                        None => {
                            self.retry = None;
                            self.delay = None;
                        }
                    },
                    Poll::Pending => {
                        return Poll::Pending;
                    }
                },
                None => {
                    self.delay = self.retry.as_ref().map(|x| x.force_retry_after());
                    if self.delay.is_none() {
                        return Poll::Pending;
                    }
                }
            }
        }
    }
}

fn poll_unpin<F>(f: &mut F, cx: &mut Context<'_>) -> Poll<F::Output>
where
    F: Future + Unpin,
{
    Pin::new(f).poll(cx)
}

fn poll_vec<F: Future + Unpin>(v: &mut Vec<F>, cx: &mut Context<'_>) -> Option<F::Output> {
    v.iter_mut()
        .enumerate()
        .find_map(|(i, f)| match poll_unpin(f, cx) {
            Poll::Pending => None,
            Poll::Ready(e) => Some((i, e)),
        })
        .map(|(idx, r)| {
            v.swap_remove(idx);
            r
        })
}
