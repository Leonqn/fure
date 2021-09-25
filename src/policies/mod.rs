mod concurrent;
mod sequential;
pub use concurrent::*;
pub use sequential::*;

/// A policy for retrying tasks specified number of times.
#[derive(Clone, Copy)]
pub struct Attempts<T, E> {
    max_retries: usize,
    retry_if: fn(Option<Result<&T, &E>>) -> bool,
}

impl<T, E> Attempts<T, E> {
    /// Create a policy which simply retries [`Err`] and [`None`] results at most given times.
    pub fn new(max_retries: usize) -> Self {
        Self {
            max_retries,
            retry_if: |result| !matches!(result, Some(Ok(_))),
        }
    }

    /// Create a policy which retries if `retry_if` returns true and retries limit `max_retries` isn't exceeded.
    pub fn with_retry_if(max_retries: usize, retry_if: fn(Option<Result<&T, &E>>) -> bool) -> Self {
        Self {
            max_retries,
            retry_if,
        }
    }

    fn retry(self, result: Option<Result<&T, &E>>) -> Option<Self> {
        if (self.retry_if)(result) {
            self.max_retries.checked_sub(1).map(|retries_left| Self {
                max_retries: retries_left,
                retry_if: self.retry_if,
            })
        } else {
            None
        }
    }
}
