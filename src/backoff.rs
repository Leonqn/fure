use std::time::Duration;

/// Creates an iterator which yields fixed duration.
pub fn fixed(duration: Duration) -> impl Iterator<Item = Duration> + Clone {
    std::iter::repeat(duration)
}

/// Creates an iterator which yields an exponential sequence of durations.
/// ## Example
/// ```
/// use std::time::Duration;
/// use fure::backoff;
/// let durations = backoff::exponential(Duration::from_secs(1), 2, Some(Duration::from_secs(7)))
///     .take(5)
///     .collect::<Vec<_>>();
///
/// assert_eq!(
///     vec![
///         Duration::from_secs(1),
///         Duration::from_secs(2),
///         Duration::from_secs(4),
///         Duration::from_secs(7),
///         Duration::from_secs(7),
///     ],
///     durations
/// );
/// ```
pub fn exponential(
    mut initial: Duration,
    factor: u32,
    max: Option<Duration>,
) -> impl Iterator<Item = Duration> + Clone {
    std::iter::from_fn(move || match initial.checked_mul(factor) {
        Some(x) => {
            let new = max.filter(|m| *m < x).unwrap_or(x);
            let current = initial;
            initial = new;
            Some(current)
        }
        None => Some(initial),
    })
}

/// Adds some randomness to given duration.
#[cfg(feature = "rand")]
pub fn jitter(duration: Duration) -> Duration {
    let jitter = rand::random::<f64>();
    let secs = ((duration.as_secs() as f64) * jitter).ceil() as u64;
    let nanos = ((f64::from(duration.subsec_nanos())) * jitter).ceil() as u32;
    Duration::new(secs, nanos)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::backoff::{exponential, fixed};

    #[test]
    fn fixed_backoff() {
        assert_eq!(
            vec![
                Duration::from_secs(1),
                Duration::from_secs(1),
                Duration::from_secs(1),
                Duration::from_secs(1),
                Duration::from_secs(1),
            ],
            fixed(Duration::from_secs(1)).take(5).collect::<Vec<_>>()
        )
    }

    #[test]
    fn exponential_backoff_without_max() {
        assert_eq!(
            vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8),
                Duration::from_secs(16),
            ],
            exponential(Duration::from_secs(1), 2, None)
                .take(5)
                .collect::<Vec<_>>()
        )
    }

    #[test]
    fn exponential_backoff_with_max() {
        assert_eq!(
            vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(7),
                Duration::from_secs(7),
            ],
            exponential(Duration::from_secs(1), 2, Some(Duration::from_secs(7)))
                .take(5)
                .collect::<Vec<_>>()
        )
    }
}
