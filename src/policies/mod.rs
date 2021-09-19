pub mod concurrent;
pub mod sequential;

#[derive(Debug, Clone, Copy)]
pub struct RetryFailed {
    max_retries: usize,
}

impl RetryFailed {
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
