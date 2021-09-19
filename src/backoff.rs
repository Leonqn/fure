use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub enum Backoff {
    Fixed {
        delay: Duration,
    },
    Exponential {
        delay: Duration,
        max_delay: Option<Duration>,
    },
}

impl Backoff {
    pub fn delay(self) -> Duration {
        match self {
            Backoff::Fixed { delay } => delay,
            Backoff::Exponential { delay, .. } => delay,
        }
    }

    pub fn next(self) -> Self {
        match self {
            Backoff::Fixed { delay } => Backoff::Fixed { delay },
            Backoff::Exponential { delay, max_delay } => match delay.checked_mul(2) {
                Some(new_delay) => Backoff::Exponential {
                    delay: max_delay.filter(|m| *m < new_delay).unwrap_or(new_delay),
                    max_delay,
                },
                None => Backoff::Exponential { delay, max_delay },
            },
        }
    }
}

impl Iterator for Backoff {
    type Item = Self;

    fn next(&mut self) -> Option<Self::Item> {
        let next = (*self).next();
        *self = next;
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::Backoff;

    #[test]
    fn fixed_backoff() {
        let backoff = Backoff::Fixed {
            delay: Duration::from_secs(1),
        };

        assert_eq!(
            vec![
                Duration::from_secs(1),
                Duration::from_secs(1),
                Duration::from_secs(1),
                Duration::from_secs(1),
                Duration::from_secs(1),
            ],
            backoff.take(5).map(Backoff::delay).collect::<Vec<_>>()
        )
    }

    #[test]
    fn exponential_backoff_without_max() {
        let backoff = Backoff::Exponential {
            delay: Duration::from_secs(1),
            max_delay: None,
        };

        assert_eq!(
            vec![
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8),
                Duration::from_secs(16),
                Duration::from_secs(32),
            ],
            backoff.take(5).map(Backoff::delay).collect::<Vec<_>>()
        )
    }

    #[test]
    fn exponential_backoff_with_max() {
        let backoff = Backoff::Exponential {
            delay: Duration::from_secs(1),
            max_delay: Some(Duration::from_secs(7)),
        };

        assert_eq!(
            vec![
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(7),
                Duration::from_secs(7),
                Duration::from_secs(7),
            ],
            backoff.take(5).map(Backoff::delay).collect::<Vec<_>>()
        )
    }
}
