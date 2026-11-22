use super::*;
use crate::authz::{Action, Authn, Authz, IdFrom, ResourceKind};
use axum::body::Bytes;
use axum::http::Method;
use uuid::Uuid;

fn upload(name: &str, bytes: &[u8], content_type: Option<&str>) -> RawUpload {
    RawUpload {
        name: name.to_owned(),
        bytes: Bytes::copy_from_slice(bytes),
        content_type: content_type.map(str::to_owned),
    }
}

#[test]
fn listing_requires_session_read() {
    let descs = crate::build_api_routes().into_parts().2;
    let d = descs
        .iter()
        .find(|d| d.path == "/sessions/{id}/attachments" && d.method == Method::GET)
        .expect("attachments route registered");
    assert_eq!(d.authn, Authn::Bearer);
    assert!(
        matches!(d.authz, Authz::Resource(ResourceKind::Session, Action::Read, IdFrom::Path("id"))),
        "attachments listing must be session-read scoped"
    );
}

#[tokio::test]
async fn record_then_list_and_cascade_with_session() {
    let Some(url) = cctui_server_kernel::test_support::db_url("record_then_list") else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let uid = Uuid::new_v4();
    let machine = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, 'att-test', $2)")
        .bind(uid)
        .bind(format!("kh-{uid}"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
        .bind(machine)
        .bind(uid)
        .bind(machine.to_string())
        .bind(format!("kh-{machine}"))
        .execute(&pool)
        .await
        .unwrap();
    let sid = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO sessions (id, machine_id, working_dir, user_id, machine_uuid, adapter_id) \
         VALUES ($1, $2, '/w', $3, $4, 'claude-code')",
    )
    .bind(&sid)
    .bind(machine.to_string())
    .bind(uid)
    .bind(machine)
    .execute(&pool)
    .await
    .unwrap();

    let paste = format!("cct893 paste {sid}");
    let uploads = [
        upload("paste-1.txt", paste.as_bytes(), Some("text/plain")),
        upload("shot.png", &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1], None),
    ];
    let staged = [
        format!("/tmp/cctui-uploads/{sid}/paste-1.txt"),
        format!("/tmp/cctui-uploads/{sid}/shot-1.png"),
    ];
    let names: Vec<String> =
        staged.iter().map(|p| p.rsplit('/').next().unwrap().to_owned()).collect();
    let recorded =
        record_uploads::<crate::state::AppState>(&pool, &sid, &uploads, &names).await.unwrap();
    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[1].name, "shot-1.png", "the daemon's suffixed name wins");

    let listed = list_session_attachments(&pool, &sid).await.unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].name, "paste-1.txt");
    assert_eq!(listed[0].content_type.as_deref(), Some("text/plain"));
    assert_eq!(listed[0].size, i64::try_from(paste.len()).unwrap());
    assert_eq!(listed[1].content_type.as_deref(), Some("image/png"));
    assert!(listed[0].created_at > 0);

    let (blob_len,): (i64,) = sqlx::query_as("SELECT byte_len FROM daemon_blobs WHERE hash = $1")
        .bind(&listed[0].hash)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(blob_len, listed[0].size);

    sqlx::query("DELETE FROM sessions WHERE id = $1").bind(&sid).execute(&pool).await.unwrap();
    assert!(list_session_attachments(&pool, &sid).await.unwrap().is_empty());
    for a in &listed {
        sqlx::query("DELETE FROM daemon_blobs WHERE hash = $1")
            .bind(&a.hash)
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::query("DELETE FROM machines WHERE id = $1").bind(machine).execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.unwrap();
}

#[tokio::test]
async fn records_before_the_session_row_exists_and_still_cascades() {
    let Some(url) = cctui_server_kernel::test_support::db_url("record_before_session") else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let uid = Uuid::new_v4();
    let machine = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, 'att-spawn', $2)")
        .bind(uid)
        .bind(format!("kh-{uid}"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
        .bind(machine)
        .bind(uid)
        .bind(machine.to_string())
        .bind(format!("kh-{machine}"))
        .execute(&pool)
        .await
        .unwrap();

    // The spawn path records under the pre-minted session id, before the
    // worker has registered and created the `sessions` row.
    let sid = Uuid::new_v4().to_string();
    let body = format!("cct916 bootstrap {sid}");
    let uploads = [upload("paste-1.txt", body.as_bytes(), Some("text/plain"))];
    let recorded =
        record_uploads::<crate::state::AppState>(&pool, &sid, &uploads, &[]).await.unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].name, "paste-1.txt", "no staged name yet: the upload name stands");

    let listed = list_session_attachments(&pool, &sid).await.unwrap();
    assert_eq!(listed.len(), 1);
    let hash = listed[0].hash.clone();
    let (blob_len,): (i64,) = sqlx::query_as("SELECT byte_len FROM daemon_blobs WHERE hash = $1")
        .bind(&hash)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(blob_len, i64::try_from(body.len()).unwrap());

    sqlx::query(
        "INSERT INTO sessions (id, machine_id, working_dir, user_id, machine_uuid, adapter_id) \
         VALUES ($1, $2, '/w', $3, $4, 'claude-code')",
    )
    .bind(&sid)
    .bind(machine.to_string())
    .bind(uid)
    .bind(machine)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("DELETE FROM sessions WHERE id = $1").bind(&sid).execute(&pool).await.unwrap();
    assert!(
        list_session_attachments(&pool, &sid).await.unwrap().is_empty(),
        "the delete trigger replaces the dropped FK cascade"
    );

    sqlx::query("DELETE FROM daemon_blobs WHERE hash = $1")
        .bind(&hash)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM machines WHERE id = $1").bind(machine).execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.unwrap();
}

#[tokio::test]
async fn an_uploaded_blob_is_servable_to_its_own_session_only() {
    let Some(url) = cctui_server_kernel::test_support::db_url("upload_blob_servable") else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");

    let sid = Uuid::new_v4().to_string();
    let other = Uuid::new_v4().to_string();
    let mut png =
        vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, b'I', b'H', b'D'];
    png.extend_from_slice(sid.as_bytes());
    let uploads = [upload("Screenshot 2026-09-25 at 13.23.11.png", &png, None)];
    let recorded =
        record_uploads::<crate::state::AppState>(&pool, &sid, &uploads, &[]).await.unwrap();
    let hash = recorded[0].hash.clone();
    assert_eq!(recorded[0].content_type.as_deref(), Some("image/png"));

    let (linked,): (bool,) =
        sqlx::query_as("SELECT EXISTS (SELECT 1 FROM session_blob_links WHERE hash = $1)")
            .bind(&hash)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!linked, "no link row is created, and none is needed");

    let served = crate::routes::blobs::fetch_session_blob(&pool, &sid, &hash)
        .await
        .unwrap()
        .expect("the uploading session serves its own attachment");
    assert_eq!(served.1, png);
    let resp = crate::routes::blobs::blob_response(&hash, served.1);
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get(axum::http::header::CONTENT_TYPE).unwrap(),
        "image/png",
        "the upload is served as an image, so the thumbnail renders"
    );

    assert!(
        crate::routes::blobs::fetch_session_blob(&pool, &other, &hash).await.unwrap().is_none(),
        "another session cannot fetch the same hash"
    );

    delete_attachments(&pool, &[recorded[0].id]).await.unwrap();
    assert!(
        crate::routes::blobs::fetch_session_blob(&pool, &sid, &hash).await.unwrap().is_none(),
        "authz follows the attachment row"
    );
    sqlx::query("DELETE FROM daemon_blobs WHERE hash = $1")
        .bind(&hash)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn delete_attachments_undoes_a_recorded_batch() {
    let Some(url) = cctui_server_kernel::test_support::db_url("delete_attachments") else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let sid = Uuid::new_v4().to_string();
    let body = format!("cct1056 rollback {sid}");
    let uploads = [upload("note.txt", body.as_bytes(), Some("text/plain"))];
    let recorded =
        record_uploads::<crate::state::AppState>(&pool, &sid, &uploads, &[]).await.unwrap();
    let ids: Vec<Uuid> = recorded.iter().map(|a| a.id).collect();

    set_attachment_names(&pool, &recorded, &["note-1.txt".to_owned()]).await.unwrap();
    assert_eq!(list_session_attachments(&pool, &sid).await.unwrap()[0].name, "note-1.txt");

    delete_attachments(&pool, &ids).await.unwrap();
    assert!(list_session_attachments(&pool, &sid).await.unwrap().is_empty());

    sqlx::query("DELETE FROM daemon_blobs WHERE hash = $1")
        .bind(&recorded[0].hash)
        .execute(&pool)
        .await
        .unwrap();
}
