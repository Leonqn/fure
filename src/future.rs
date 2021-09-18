use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use crate::Attempt;

pub struct ParallelRetry<A, T, E, F, FN>
where
    A: Attempt<T, E>,
{
    attempt: Option<A>,
    create_f: FN,
    running_futs: Vec<F>,
    timeout: Option<A::DelayFuture>,
}

impl<A, T, E, F, FN> ParallelRetry<A, T, E, F, FN>
where
    A: Attempt<T, E>,
{
    pub(crate) fn new(
        attempt: A,
        create_f: FN,
        running_futs: Vec<F>,
        timeout: Option<A::DelayFuture>,
    ) -> Self {
        Self {
            attempt: Some(attempt),
            create_f,
            running_futs,
            timeout,
        }
    }
}

impl<A, T, E, F, FN> Unpin for ParallelRetry<A, T, E, F, FN>
where
    A: Attempt<T, E>,
    F: Unpin,
{
}

impl<A, T, E, F, FN> Future for ParallelRetry<A, T, E, F, FN>
where
    A: Attempt<T, E>,
    F: Future<Output = Result<T, E>> + Unpin,
    FN: FnMut() -> F,
{
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.running_futs.is_empty() {
            let f = (self.create_f)();
            self.running_futs.push(f)
        }

        loop {
            while let Poll::Ready(r) = poll_vec(&mut self.running_futs, cx) {
                match self.attempt.as_ref().and_then(|a| a.next(Some(&r))) {
                    Some(a) => {
                        let f = (self.create_f)();
                        self.running_futs.push(f);
                        self.attempt = Some(a)
                    }
                    None => return Poll::Ready(r),
                }
            }
            match self.timeout.as_mut() {
                Some(timeout) => match poll_unpin(timeout, cx) {
                    Poll::Ready(_) => {
                        self.attempt = self.attempt.as_ref().and_then(|x| x.next(None));
                        self.timeout = None;
                        let send_f = (self.create_f)();
                        self.running_futs.push(send_f);
                    }
                    Poll::Pending => {
                        return Poll::Pending;
                    }
                },
                None => self.timeout = self.attempt.as_ref().map(|x| x.delay()),
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

fn poll_vec<F: Future + Unpin>(v: &mut Vec<F>, cx: &mut Context<'_>) -> Poll<F::Output> {
    let item = v
        .iter_mut()
        .enumerate()
        .find_map(|(i, f)| match poll_unpin(f, cx) {
            Poll::Pending => None,
            Poll::Ready(e) => Some((i, e)),
        });
    match item {
        Some((idx, r)) => {
            v.swap_remove(idx);
            Poll::Ready(r)
        }
        None => Poll::Pending,
    }
}
