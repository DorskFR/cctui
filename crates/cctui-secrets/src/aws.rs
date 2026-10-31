use aws_sdk_secretsmanager::Client;
use aws_sdk_secretsmanager::operation::get_secret_value::GetSecretValueError;

use crate::{Credential, SecretError, SecretRef};

pub async fn get(
    client: &Client,
    r: &SecretRef,
    name: &str,
    field: Option<&str>,
) -> Result<Credential, SecretError> {
    match client.get_secret_value().secret_id(name).send().await {
        Ok(out) => {
            let Some(value) = out.secret_string().filter(|v| !v.is_empty()) else {
                return Err(SecretError::not_found(r));
            };
            let Some(field) = field else { return Ok(Credential::new(value.to_owned())) };
            let json: serde_json::Value = serde_json::from_str(value).map_err(|e| {
                SecretError::Backend(anyhow::anyhow!("aws-sm {name} is not JSON: {e}"))
            })?;
            match json.get(field).and_then(serde_json::Value::as_str) {
                Some(v) if !v.is_empty() => Ok(Credential::new(v.to_owned())),
                _ => Err(SecretError::not_found(r)),
            }
        }
        Err(e)
            if e.as_service_error()
                .is_some_and(GetSecretValueError::is_resource_not_found_exception) =>
        {
            Err(SecretError::not_found(r))
        }
        Err(e) => Err(SecretError::Backend(anyhow::anyhow!(
            "aws secrets manager get {name}: {}",
            aws_sdk_secretsmanager::error::DisplayErrorContext(&e)
        ))),
    }
}

#[cfg(test)]
mod tests {
    use aws_sdk_secretsmanager::config::{BehaviorVersion, Credentials, Region};
    use axum::response::IntoResponse;

    use super::*;
    use crate::{Engines, RefResolver as _, resolve_value};

    async fn spawn_sm_mock() -> String {
        use axum::http::{HeaderMap, StatusCode};
        use axum::routing::post;

        const JSON: &str = "application/x-amz-json-1.1";
        let app = axum::Router::new().route(
            "/",
            post(|headers: HeaderMap, body: String| async move {
                let target = headers.get("x-amz-target").and_then(|v| v.to_str().ok());
                if target != Some("secretsmanager.GetSecretValue") {
                    return (StatusCode::BAD_REQUEST, "unexpected target").into_response();
                }
                let req: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                let secret = match req["SecretId"].as_str() {
                    Some("cctui/plain") => "sm-plain",
                    Some("cctui/json") => r#"{"password":"sm-pw","user":"u"}"#,
                    _ => {
                        return (
                            StatusCode::BAD_REQUEST,
                            [
                                ("content-type", JSON),
                                ("x-amzn-errortype", "ResourceNotFoundException"),
                            ],
                            r#"{"__type":"ResourceNotFoundException","Message":"no such secret"}"#,
                        )
                            .into_response();
                    }
                };
                let out = serde_json::json!({"Name": req["SecretId"], "SecretString": secret});
                ([("content-type", JSON)], out.to_string()).into_response()
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    fn sm_engines(base: String) -> Engines {
        let config = aws_sdk_secretsmanager::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .endpoint_url(base)
            .credentials_provider(Credentials::new("AKIDTEST", "secret-key", None, None, "test"))
            .build();
        Engines::new().with_aws_client(Client::from_conf(config))
    }

    #[tokio::test]
    async fn aws_sm_engine_reads_plain_and_json_field() {
        let engines = sm_engines(spawn_sm_mock().await);
        let plain = SecretRef::parse("aws-sm:cctui/plain").unwrap();
        assert_eq!(engines.resolve(&plain).await.unwrap().expose(), "sm-plain");
        let field = SecretRef::parse("aws-sm:cctui/json#password").unwrap();
        assert_eq!(engines.resolve(&field).await.unwrap().expose(), "sm-pw");
    }

    #[tokio::test]
    async fn aws_sm_engine_missing_secret_or_field_is_not_found() {
        let engines = sm_engines(spawn_sm_mock().await);
        for r in ["aws-sm:cctui/nope", "aws-sm:cctui/json#missing"] {
            let r = SecretRef::parse(r).unwrap();
            assert!(
                matches!(engines.resolve(&r).await, Err(SecretError::NotFound { .. })),
                "{r} must be NotFound"
            );
        }
    }

    #[tokio::test]
    async fn aws_sm_engine_field_on_non_json_fails_closed() {
        let engines = sm_engines(spawn_sm_mock().await);
        let r = SecretRef::parse("aws-sm:cctui/plain#password").unwrap();
        assert!(matches!(engines.resolve(&r).await, Err(SecretError::Backend(_))));
    }

    #[tokio::test]
    async fn aws_sm_value_error_names_the_ref_and_hides_the_secret() {
        let engines = sm_engines(spawn_sm_mock().await);
        let msg = resolve_value(&engines, "aws-sm:cctui/json#missing")
            .await
            .unwrap_err()
            .to_string();
        assert!(msg.contains("aws-sm:cctui/json#missing"), "{msg}");
        assert!(!msg.contains("sm-pw"), "{msg}");
    }
}
