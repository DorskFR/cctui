#[tokio::main]
async fn main() -> anyhow::Result<()> {
    cctui_dispatcher_core::cli::main::<cctui_dispatcher_apple::backend::Apple>().await
}
