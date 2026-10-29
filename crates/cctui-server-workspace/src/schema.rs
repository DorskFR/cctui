//! Workspace provisioning without changing the legacy migration ledger.
use cctui_server_core::schema::{Migration, MigrationConnection, Origin};

/// The workspace baseline corresponding to legacy schema 191.
pub const BOOTSTRAP: &str = include_str!("../schema/0001_workspace.sql");

/// Expand a core installation, or adopt the already migrated full schema.
pub async fn provision(conn: &mut MigrationConnection) -> Result<(), sqlx::Error> {
    let adopted = cctui_server_core::schema::origin(conn).await? == Some(Origin::LegacyFull);
    cctui_server_core::schema::install_package(
        conn,
        "workspace",
        &[Migration { version: 1, sql: BOOTSTRAP }],
        adopted,
    )
    .await?;
    Ok(())
}
