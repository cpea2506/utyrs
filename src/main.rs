mod capabilities;
mod document_storage;
mod server;

use std::error::Error;
use tracing::{error, info};

/// Start Unity LS over stdio.
fn main() -> Result<(), Box<dyn Error + Sync + Send>> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    info!("Starting Unity LS");

    if let Err(err) = server::run_stdio() {
        error!("[Unity LS] Server failed: {err}");
        return Err(err);
    }

    info!("Unity LS stopped");

    Ok(())
}
