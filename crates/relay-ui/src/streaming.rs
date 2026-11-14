//! Bridges Tokio-based provider streams into the GPUI event loop.
//!
//! The provider client is async and requires Tokio. GPUI runs its own executor
//! on the UI thread, so Relay keeps a Tokio runtime for network work and hands
//! events back over a `smol` channel. That channel is executor-agnostic and is
//! what the GPUI task awaits.

use relay_providers::{ChatRequest, ProviderClient, StreamEvent};
use smol::channel::{Receiver, unbounded};
use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;

/// A provider request that has been handed to the network runtime.
pub struct StartedStream {
    /// Stream events, in order. The channel closes when the request finishes.
    pub events: Receiver<StreamEvent>,
    /// A single error message, sent after the event channel closes when the
    /// request failed.
    pub error: Receiver<String>,
    /// Ask the provider to stop and release the connection.
    pub cancel: CancellationToken,
}

/// Start a provider stream without blocking the UI thread.
pub fn start(handle: &Handle, request: ChatRequest) -> Result<StartedStream, String> {
    let client = ProviderClient::new().map_err(|error| error.to_string())?;
    let (events_tx, events_rx) = unbounded::<StreamEvent>();
    let (error_tx, error_rx) = unbounded::<String>();
    let cancel = CancellationToken::new();
    let token = cancel.clone();

    handle.spawn(async move {
        let result = client.stream(&request, &events_tx, &token).await;
        // Closing the event channel first lets the UI consume every partial
        // event before it observes the terminal error.
        drop(events_tx);
        if let Err(error) = result {
            let _ = error_tx.send(error.to_string()).await;
        }
    });

    Ok(StartedStream {
        events: events_rx,
        error: error_rx,
        cancel,
    })
}
