#[tokio::main]
async fn main() -> anyhow::Result<()> {
    Box::pin(cctui_dispatcher_core::cli::main::<cctui_dispatcher_kube::backend::Kube>()).await
}
