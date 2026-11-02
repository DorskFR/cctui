//! Workspace provisioning without changing the legacy migration ledger.
use cctui_server_core::schema::{Migration, MigrationConnection, Origin};

/// The workspace baseline corresponding to legacy schema 191.
pub const BOOTSTRAP: &str = include_str!("../schema/0001_workspace.sql");

pub const MIGRATIONS: &[Migration] = &[
    Migration { version: 1, sql: BOOTSTRAP },
    Migration { version: 2, sql: include_str!("../schema/0002_dispatch_identities.sql") },
];

/// Expand a core installation, or adopt the already migrated full schema.
pub async fn provision(conn: &mut MigrationConnection) -> Result<(), sqlx::Error> {
    let adopted = cctui_server_core::schema::origin(conn).await? == Some(Origin::LegacyFull);
    cctui_server_core::schema::install_package(conn, "workspace", MIGRATIONS, adopted).await?;
    Ok(())
}
