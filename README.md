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

When one of runninng requests completes with an [`Ok`] result it will be returned.
```rust
use fure::policies::{interval, failed};
use std::time::Duration;

let get_body = || async {
    reqwest::get("https://www.rust-lang.org")
        .await?
        .text()
        .await
};
let policy = failed(interval(Duration::from_secs(1)), 3);
let body = fure::retry(get_body, policy).await?;
println!("body = {}", body);
```
### Sequential retry with backoff.
Retries failed requests with an exponential backoff and a jitter.
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

License: MIT
