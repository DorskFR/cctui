//! `GET /api/v1/harnesses` — the harness table both clients derive their
//! pickers, capability gates and icons from.

use axum::Json;
use cctui_proto::adapter::HarnessDescriptor;

pub async fn list_harnesses() -> Json<Vec<HarnessDescriptor>> {
    Json(cctui_proto::adapter::harnesses())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_three_harnesses_are_served_in_table_order() {
        let Json(rows) = list_harnesses().await;
        let ids: Vec<&str> = rows.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(ids, ["claude-code", "codex", "opencode"]);
        assert!(rows.iter().all(|h| h.default_enabled));
    }
}
