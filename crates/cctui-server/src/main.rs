#[tokio::main]
async fn main() -> anyhow::Result<()> {
    cctui_server::run().await
}
