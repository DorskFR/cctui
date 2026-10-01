//! `GET /sessions/{id}/images/{image_id}` — the blob behind a `cctui-img://`
//! marker in a transcript.

use crate::error::ClientError;
use crate::rest::{Client, FileRead};

impl Client {
    pub async fn session_image(
        &self,
        session_id: &str,
        image_id: &str,
    ) -> Result<FileRead, ClientError> {
        self.get_blob(
            "get_sessions_by_id_images_by_image",
            &[("id", session_id), ("image_id", image_id)],
            "image/png",
        )
        .await
    }
}
