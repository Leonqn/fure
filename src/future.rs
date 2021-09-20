use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use crate::{CreateFuture, RetryPolicy};

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
        #[pin]
        running_futs: Futs<F>,
        #[pin]
        delay: Option<R::ForceRetryFuture>,
    }
}

impl<R, T, E, F, CF> ConcurrentRetry<R, T, E, F, CF>
where
    R: RetryPolicy<T, E>,
    CF: CreateFuture<F>,
{
    pub(crate) fn new(retry: R, mut create_f: CF) -> Self {
        Self {
            retry: Some(retry),
            running_futs: Futs::new(create_f.create()),
            create_f,
            delay: None,
            retry_fut: None,
        }
    }
}

impl<R, T, E, F, CF> ConcurrentRetry<R, T, E, F, CF>
where
    R: RetryPolicy<T, E>,
    F: Future<Output = Result<T, E>>,
    CF: CreateFuture<F>,
{
    fn poll_retry(mut self: Pin<&mut Self>, cx: &mut Context) {
        let mut this = self.as_mut().project();
        if let Some(Poll::Ready(retry)) = this.retry_fut.as_mut().as_pin_mut().map(|x| x.poll(cx)) {
            let f = this.create_f.create();
            this.running_futs.push(f);
            this.retry_fut.set(None);
            this.delay.set(Some(retry.force_retry_after()));
            *this.retry = Some(retry);
        }
    }
}

impl<R, T, E, F, CF> Future for ConcurrentRetry<R, T, E, F, CF>
where
    R: RetryPolicy<T, E>,
    F: Future<Output = Result<T, E>>,
    CF: CreateFuture<F>,
{
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.as_mut().poll_retry(cx);
        loop {
            {
                while let Poll::Ready(r) = self.as_mut().project().running_futs.poll(cx) {
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
            match this.delay.as_mut().as_pin_mut() {
                Some(delay) => match delay.poll(cx) {
                    Poll::Ready(_) => match this.retry.take().and_then(|x| x.retry(None)) {
                        Some(retry) => {
                            this.retry_fut.set(Some(retry));
                            this.delay.set(None);
                            self.as_mut().poll_retry(cx);
                        }
                        None => {
                            *this.retry = None;
                            this.delay.set(None)
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
                        this.delay.set(new_delay)
                    }
                }
            }
        }
    }
}

pin_project_lite::pin_project! {
    struct Futs<F> {
        #[pin]
        fut: Option<F>,
        rest: Vec<Pin<Box<F>>>,
    }
}

impl<F> Futs<F> {
    fn new(f: F) -> Self {
        Self {
            fut: Some(f),
            rest: vec![],
        }
    }

    fn push(self: Pin<&mut Self>, f: F) {
        let mut this = self.project();
        if this.fut.is_none() {
            this.fut.set(Some(f))
        } else {
            this.rest.push(Box::pin(f))
        }
    }
}

impl<F, T, E> Future for Futs<F>
where
    F: Future<Output = Result<T, E>>,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();
        match this.fut.as_mut().as_pin_mut().map(|x| x.poll(cx)) {
            Some(Poll::Ready(r)) => {
                this.fut.set(None);
                Poll::Ready(r)
            }
            Some(Poll::Pending) => poll_vec(this.rest, cx),
            None => poll_vec(this.rest, cx),
        }
    }
}

fn poll_vec<F: Future + Unpin>(v: &mut Vec<F>, cx: &mut Context<'_>) -> Poll<F::Output> {
    v.iter_mut()
        .enumerate()
        .find_map(|(i, f)| match Pin::new(f).poll(cx) {
            Poll::Pending => None,
            Poll::Ready(e) => Some((i, e)),
        })
        .map(|(idx, r)| {
            v.swap_remove(idx);
            Poll::Ready(r)
        })
        .unwrap_or(Poll::Pending)
}
