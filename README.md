# fure

A crate for retrying futures.

[`Policy`] trait will help you define different retry policies.

Some builtin policies can be found in [`policies`] module.
## Examples.
### Interval retry.
Starts with sending a request, setting up a 1 second timer, and waits for either of them.

If the timer completes first (it means that the request didn't complete in 1 second) one more request fires.

If the request completes first and it has an [`Ok`] response it is returned, if request has an [`Err`] response, timer resets and a new request fires.

At most 4 requests will be fired.

When one of runninng requests completes with an [`Ok`] result it will be returned.
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
Retries failed requests with an exponential backoff and a jitter.
```rust
use fure::{policies::{backoff, retry_if}, backoff::exponential};
use std::time::Duration;

let get_body = || async {
    reqwest::get("https://www.rust-lang.org")
        .await?
        .text()
        .await
};
let exp_backoff = exponential(Duration::from_secs(1), 2, Some(Duration::from_secs(10)))
    .map(fure::backoff::jitter);
let policy = retry_if(backoff(exp_backoff), |result| !matches!(result, Some(Ok(_))));
let body = fure::retry(get_body, policy).await?;
println!("body = {}", body);
```

License: MIT
