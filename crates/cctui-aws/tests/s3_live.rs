use std::time::{SystemTime, UNIX_EPOCH};

use cctui_aws::AwsCredentials;
use cctui_aws::s3::{S3Client, sha256_hex};

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

fn live_client() -> Option<S3Client> {
    let endpoint = var("CCTUI_TEST_S3_ENDPOINT")?;
    let bucket = var("CCTUI_TEST_S3_BUCKET").expect("CCTUI_TEST_S3_BUCKET");
    let key_id = var("CCTUI_TEST_S3_ACCESS_KEY_ID").expect("CCTUI_TEST_S3_ACCESS_KEY_ID");
    let secret = var("CCTUI_TEST_S3_SECRET_ACCESS_KEY").expect("CCTUI_TEST_S3_SECRET_ACCESS_KEY");
    let region = var("CCTUI_TEST_S3_REGION").unwrap_or_else(|| "us-east-1".to_owned());
    let _ = rustls::crypto::ring::default_provider().install_default();
    Some(S3Client::new(&endpoint, &bucket, &region, AwsCredentials::new(key_id, secret)).unwrap())
}

#[tokio::test]
async fn s3_live_put_head_get_delete() {
    let Some(s3) = live_client() else {
        eprintln!("skipping s3_live_put_head_get_delete: CCTUI_TEST_S3_ENDPOINT is not set");
        return;
    };
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let bytes = format!("cctui s3 live test {nanos}").into_bytes();
    let hash = sha256_hex(&bytes);
    let key = format!("cctui-live-test/{hash}");

    assert_eq!(s3.head_object(&key).await.unwrap(), None);
    assert_eq!(s3.get_object(&key).await.unwrap(), None);
    s3.put_object(&key, &bytes, &hash).await.unwrap();
    assert_eq!(s3.head_object(&key).await.unwrap(), Some(bytes.len() as u64));
    assert_eq!(s3.get_object(&key).await.unwrap(), Some(bytes));
    s3.delete_object(&key).await.unwrap();
    s3.delete_object(&key).await.unwrap();
    assert_eq!(s3.head_object(&key).await.unwrap(), None);
    assert_eq!(s3.get_object(&key).await.unwrap(), None);
}
