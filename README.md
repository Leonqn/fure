[![Crates.io](https://img.shields.io/crates/v/fure.svg)](https://crates.io/crates/fure)
[![Docs.rs](https://img.shields.io/docsrs/fure.svg)](https://docs.rs/fure)
[![Workflow Status](https://github.com/Leonqn/fure/workflows/CI/badge.svg)](https://github.com/Leonqn/fure/actions?query=workflow%3A%22CI%22)

# fure

A crate for retrying futures.

[`Policy`] trait will help you define different retry policies.

Some builtin policies can be found in [`policies`] module.

By default this create uses `tokio` timers for [`crate::policies::interval`] and [`crate::policies::backoff`] policies,
but `async-std` is also available as feature `async-std`.
## Examples.
### Interval retry.
Starts with sending a request, setting up a 1 second timer, and waits for either of them.

If the timer completes first (it means that the request didn't complete in 1 second) one more request fires.

If the request completes first and it has an [`Ok`] response it is returned, if request has an [`Err`] response, timer resets and a new request fires.

At most 4 requests will be fired.
```rust
use fure::policies::{interval, attempts};
use std::time::Duration;

let get_body = || async {
    reqwest::get("https://www.rust-lang.org")
        .await?
        .text()
        .await
};
let policy = attempts(interval(Duration::from_secs(1)), 3);
let body = fure::retry(get_body, policy).await?;
println!("body = {}", body);
```
### Sequential retry with backoff.
Retries attempts requests with an exponential backoff and a jitter.
```rust
use fure::{policies::{backoff, cond}, backoff::{exponential, jitter}};
use std::time::Duration;

let get_body = || async {
    reqwest::get("https://www.rust-lang.org")
        .await?
        .text()
        .await
};
let exp_backoff = exponential(Duration::from_secs(1), 2, Some(Duration::from_secs(10)))
    .map(jitter);
let policy = cond(backoff(exp_backoff), |result| !matches!(result, Some(Ok(_))));
let body = fure::retry(get_body, policy).await?;
println!("body = {}", body);
```
### Implementing your own policy.
It behaves like the interval policy above, but if it hits `TOO_MANY_REQUESTS` it will wait for some seconds before sending next request.
```rust
use std::{future::{Future, ready}, pin::Pin, time::Duration};
use fure::Policy;
use reqwest::{Error, Response, StatusCode};

struct RetryPolicy;

impl Policy<Response, Error> for RetryPolicy {
    type ForceRetryFuture = tokio::time::Sleep;

    type RetryFuture = Pin<Box<dyn Future<Output = Self>>>;

    fn force_retry_after(&self) -> Self::ForceRetryFuture {
        tokio::time::sleep(Duration::from_millis(100))
    }

    fn retry(
        self,
        result: Option<Result<&Response, &Error>>,
    ) -> Option<Self::RetryFuture> {
        match result {
            Some(Ok(response)) => match response.status() {
                StatusCode::OK => None,
                StatusCode::TOO_MANY_REQUESTS => {
                    let retry_after_secs: u64 = response
                        .headers()
                        .get("Retry-After")
                        .and_then(|x| x.to_str().ok()?.parse().ok())
                        .unwrap_or(1);
                    Some(Box::pin(async move {
                        tokio::time::sleep(Duration::from_secs(retry_after_secs)).await;
                        self
                    }))
                }
                _ => Some(Box::pin(ready(self))),
            },
            _ => Some(Box::pin(ready(self))),
        }
    }
}

let get_response = || reqwest::get("https://www.rust-lang.org");
let response = fure::retry(get_response, RetryPolicy).await?;
println!("body = {}", response.text().await?);
```

License: MIT
