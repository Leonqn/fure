use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use crate::RetryPolicy;

pin_project_lite::pin_project! {
    #[must_use = "futures do nothing unless you `.await` or poll them"]
    pub struct ConcurrentRetry<R, T, E, F, CF>
    where
        R: RetryPolicy<T, E>,
    {
        retry: Option<R>,
        #[pin]
        retry_fut: Option<R::RetryFuture>,
        create_f: CF,
        running_futs: Vec<F>,
        #[pin]
        delay: Option<R::ForceRetryFuture>,
        first_run: bool,
    }
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
    fn poll_retry(mut self: Pin<&mut Self>, cx: &mut Context) {
        if let Some(Poll::Ready(retry)) = self
            .as_mut()
            .project()
            .retry_fut
            .as_pin_mut()
            .map(|x| x.poll(cx))
        {
            let mut this = self.as_mut().project();
            this.retry_fut.set(None);
            let f = (this.create_f)();
            this.running_futs.push(f);
            this.delay.set(Some(retry.force_retry_after()));
            *this.retry = Some(retry);
        }
    }
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
            let this = self.as_mut().project();
            *this.first_run = false;
            let f = (this.create_f)();
            this.running_futs.push(f)
        }
        self.as_mut().poll_retry(cx);
        loop {
            {
                while let Some(r) = poll_vec(self.as_mut().project().running_futs, cx) {
                    let mut this = self.as_mut().project();
                    match this.retry.take().and_then(|x| x.retry(Some(r.as_ref()))) {
                        Some(retry) => {
                            this.retry_fut.set(Some(retry));
                            this.delay.set(None);
                            self.as_mut().poll_retry(cx);
                        }
                        None => return Poll::Ready(r),
                    }
                }
            }
            let mut this = self.as_mut().project();
            match this.delay.as_pin_mut() {
                Some(delay) => match delay.poll(cx) {
                    Poll::Ready(_) => match this.retry.take().and_then(|x| x.retry(None)) {
                        Some(retry) => {
                            this.retry_fut.set(Some(retry));
                            self.as_mut().project().delay.set(None);
                            self.as_mut().poll_retry(cx);
                        }
                        None => {
                            *this.retry = None;
                            self.as_mut().project().delay.set(None)
                        }
                    },
                    Poll::Pending => {
                        return Poll::Pending;
                    }
                },
                None => {
                    let new_delay = this.retry.as_ref().map(|x| x.force_retry_after());
                    if new_delay.is_none() {
                        return Poll::Pending;
                    } else {
                        self.as_mut().project().delay.set(new_delay)
                    }
                }
            }
        }
    }
}

fn poll_vec<F: Future + Unpin>(v: &mut Vec<F>, cx: &mut Context<'_>) -> Option<F::Output> {
    v.iter_mut()
        .enumerate()
        .find_map(|(i, f)| match Pin::new(f).poll(cx) {
            Poll::Pending => None,
            Poll::Ready(e) => Some((i, e)),
        })
        .map(|(idx, r)| {
            v.swap_remove(idx);
            r
        })
}
