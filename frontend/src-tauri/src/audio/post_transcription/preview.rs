//! Drain disabled preview audio without loading a model or retaining backlog.
pub fn discard_audio<T: Send + 'static>(
    receiver: tokio::sync::mpsc::UnboundedReceiver<T>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut receiver = receiver;
        while receiver.recv().await.is_some() {}
    })
}
