//! The full distribution's executable; workspace owns its handlers and state.
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    cctui_server_workspace::run().await
}
