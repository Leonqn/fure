/// Policies to run futures concurrently.
pub mod concurrent;

/// Policies to run futures sequentially.
pub mod sequential;

/// An attempts policy which can be used in both sequential and concurrent manners.
///
/// It simple retries all [`Err`] and [`None`] results given times.

#[derive(Debug, Clone, Copy)]
pub struct Attempts {
    max_retries: usize,
}

impl Attempts {
    pub fn new(max_retries: usize) -> Self {
        Self { max_retries }
    }

    fn retry<T, E>(self, result: Option<Result<&T, &E>>) -> Option<Self> {
        match result {
            Some(Ok(_)) => None,
            _ => self.max_retries.checked_sub(1).map(Self::new),
        }
    }
}
