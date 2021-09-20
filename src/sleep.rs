#[cfg(feature = "tokio")]
pub fn sleep(duration: std::time::Duration) -> Sleep {
    tokio::time::sleep(duration)
}

#[cfg(feature = "async-std")]
pub fn sleep(duration: std::time::Duration) -> Sleep {
    #[cfg(feature = "async-std")]
    Sleep {
        timer: Box::pin(async_std::task::sleep(duration)),
    }
}

#[cfg(feature = "tokio")]
pub use tokio::time::Sleep;

#[cfg(feature = "async-std")]
pub struct Sleep {
    timer: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>>,
}

#[cfg(feature = "async-std")]
impl std::future::Future for Sleep {
    type Output = ();

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        std::pin::Pin::new(&mut self.timer).poll(cx)
    }
}
