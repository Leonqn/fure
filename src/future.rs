use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use crate::Attempt;

pub struct ConcurrentRetry<A, T, E, F, CF>
where
    A: Attempt<T, E>,
{
    attempt: Option<A>,
    create_f: CF,
    running_futs: Vec<F>,
    delay: Option<A::DelayFuture>,
}

impl<A, T, E, F, CF> ConcurrentRetry<A, T, E, F, CF>
where
    A: Attempt<T, E>,
{
    pub(crate) fn new(
        attempt: A,
        create_f: CF,
        running_futs: Vec<F>,
        delay: Option<A::DelayFuture>,
    ) -> Self {
        Self {
            attempt: Some(attempt),
            create_f,
            running_futs,
            delay,
        }
    }
}

impl<A, T, E, F, CF> Unpin for ConcurrentRetry<A, T, E, F, CF>
where
    A: Attempt<T, E>,
    F: Unpin,
{
}

impl<A, T, E, F, CF> Future for ConcurrentRetry<A, T, E, F, CF>
where
    A: Attempt<T, E>,
    F: Future<Output = Result<T, E>> + Unpin,
    CF: FnMut() -> F,
{
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.running_futs.is_empty() {
            let f = (self.create_f)();
            self.running_futs.push(f)
        }

        loop {
            while let Some(r) = poll_vec(&mut self.running_futs, cx) {
                match self.attempt.as_ref().and_then(|a| a.next(Some(&r))) {
                    Some(a) => {
                        let f = (self.create_f)();
                        self.running_futs.push(f);
                        self.attempt = Some(a)
                    }
                    None => return Poll::Ready(r),
                }
            }
            match self.delay.as_mut() {
                Some(delay) => match poll_unpin(delay, cx) {
                    Poll::Ready(_) => {
                        self.attempt = self.attempt.as_ref().and_then(|x| x.next(None));
                        self.delay = None;
                        if self.attempt.is_some() {
                            let send_f = (self.create_f)();
                            self.running_futs.push(send_f);
                        }
                    }
                    Poll::Pending => {
                        return Poll::Pending;
                    }
                },
                None => {
                    self.delay = self.attempt.as_ref().map(|x| x.delay());
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
