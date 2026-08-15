//! Kimi OAuth Device Authorization Grant（RFC 8628）：
//! 请求设备码 → 给用户链接/授权码 → 浏览器授权 → 轮询换 token → 组装成本地凭证 blob。
//! 端点与 client_id 与官方 Kimi CLI 及 CLIProxyAPI 的实现一致。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use kimi_switch_core::error::{Error, Result};

/// Kimi Code 官方 OAuth client_id（公开常量，官方 CLI 内置同款）。
const CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";
/// 服务端建议间隔的下限，避免高频轮询触发风控。
const MIN_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// 授权等待总上限（设备码自身的 expires_in 优先）。
const MAX_POLL_DURATION: Duration = Duration::from_secs(15 * 60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

/// 设备码授权响应：展示给用户的链接与授权码。
#[derive(Debug, Clone)]
pub struct DeviceAuthorization {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    pub interval: u64,
}

/// 进程级设备 ID（X-Msh-Device-Id），随机生成一次即可，无需持久化。
fn device_id() -> String {
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id() as u128;
        let mixed = nanos ^ (pid << 64);
        format!(
            "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
            (mixed & 0xffff_ffff) as u32,
            ((mixed >> 32) & 0xffff) as u16,
            ((mixed >> 48) & 0x0fff) as u16,
            (((mixed >> 64) & 0x3fff) as u16) | 0x8000,
            (mixed & 0xffff_ffff_ffff) as u64
        )
    })
    .clone()
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| Error::Provider(format!("kimi device flow client failed: {e}")))
}

/// 官方客户端同款 X-Msh-* 头。
fn with_common_headers(builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    builder
        .header("Accept", "application/json")
        .header("User-Agent", "kimi-switch")
        .header("X-Msh-Platform", "kimi-switch")
        .header("X-Msh-Device-Id", device_id())
        .header("X-Msh-Device-Model", "Windows")
}

/// 请求设备码（使用真实 OAuth host）。
pub async fn request_device_code() -> Result<DeviceAuthorization> {
    request_device_code_at(&crate::oauth::oauth_host()).await
}

async fn request_device_code_at(oauth_base: &str) -> Result<DeviceAuthorization> {
    let url = format!(
        "{}/api/oauth/device_authorization",
        oauth_base.trim_end_matches('/')
    );
    let form = [("client_id", CLIENT_ID)];
    let resp = with_common_headers(http_client()?.post(&url))
        .form(&form)
        .send()
        .await
        .map_err(|e| Error::Provider(format!("kimi device code request failed: {e}")))?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(Error::Provider(format!(
            "kimi device code HTTP {status}: {body}"
        )));
    }
    let v: serde_json::Value = serde_json::from_str(&body)
        .map_err(|e| Error::Provider(format!("kimi device code parse failed: {e}")))?;
    let get_str = |key: &str| v.get(key).and_then(|x| x.as_str()).map(String::from);
    let device_code = get_str("device_code")
        .ok_or_else(|| Error::Provider("kimi device code response missing device_code".into()))?;
    Ok(DeviceAuthorization {
        device_code,
        user_code: get_str("user_code").unwrap_or_default(),
        verification_uri: get_str("verification_uri")
            .or_else(|| get_str("verification_url"))
            .unwrap_or_default(),
        verification_uri_complete: get_str("verification_uri_complete"),
        expires_in: v.get("expires_in").and_then(|x| x.as_u64()).unwrap_or(900),
        interval: v.get("interval").and_then(|x| x.as_u64()).unwrap_or(5),
    })
}

/// 轮询等待用户授权，成功返回可入库的凭证 blob（kimi-code.json 同构 JSON）。
/// `cancel` 置 true 时尽快中止。
pub async fn poll_for_token(
    auth: &DeviceAuthorization,
    cancel: Arc<AtomicBool>,
) -> Result<String> {
    poll_for_token_at(auth, &crate::oauth::oauth_host(), cancel).await
}

async fn poll_for_token_at(
    auth: &DeviceAuthorization,
    oauth_base: &str,
    cancel: Arc<AtomicBool>,
) -> Result<String> {
    let url = format!("{}/api/oauth/token", oauth_base.trim_end_matches('/'));
    let client = http_client()?;
    let mut interval = MIN_POLL_INTERVAL.max(Duration::from_secs(auth.interval));
    let deadline = Instant::now()
        + MAX_POLL_DURATION.min(Duration::from_secs(auth.expires_in.max(1)));

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Provider("授权已取消".into()));
        }
        if Instant::now() >= deadline {
            return Err(Error::Provider("授权超时：设备码已过期，请重新添加".into()));
        }
        tokio::time::sleep(interval).await;

        let form = [
            ("client_id", CLIENT_ID),
            ("device_code", auth.device_code.as_str()),
            (
                "grant_type",
                "urn:ietf:params:oauth:grant-type:device_code",
            ),
        ];
        let resp = with_common_headers(client.post(&url))
            .form(&form)
            .send()
            .await
            .map_err(|e| Error::Provider(format!("kimi token poll failed: {e}")))?;
        let body = resp.text().await.unwrap_or_default();
        let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();

        match v.get("error").and_then(|x| x.as_str()) {
            Some("authorization_pending") => continue,
            Some("slow_down") => {
                interval += Duration::from_secs(5);
                continue;
            }
            Some("expired_token") => {
                return Err(Error::Provider("授权超时：设备码已过期，请重新添加".into()));
            }
            Some("access_denied") => {
                return Err(Error::Provider("授权被拒绝".into()));
            }
            Some(other) => {
                let desc = v
                    .get("error_description")
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                return Err(Error::Provider(format!("kimi OAuth error: {other} {desc}")));
            }
            None => {}
        }

        let access = v.get("access_token").and_then(|x| x.as_str());
        let Some(access) = access else {
            return Err(Error::Provider(
                "kimi token response missing access_token".into(),
            ));
        };
        let expires_in = v.get("expires_in").and_then(|x| x.as_i64()).unwrap_or(900);
        let now = chrono::Utc::now().timestamp();
        let mut blob = serde_json::json!({
            "access_token": access,
            "expires_at": now + expires_in,
            "expires_in": expires_in,
        });
        let obj = blob.as_object_mut().unwrap();
        for key in ["refresh_token", "scope", "token_type"] {
            if let Some(value) = v.get(key) {
                obj.insert(key.into(), value.clone());
            }
        }
        return Ok(blob.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::MockServer;

    fn auth() -> DeviceAuthorization {
        DeviceAuthorization {
            device_code: "dc-1".into(),
            user_code: "ABCD-EFGH".into(),
            verification_uri: "https://auth.kimi.com/device".into(),
            verification_uri_complete: None,
            expires_in: 900,
            interval: 0,
        }
    }

    #[tokio::test]
    async fn poll_succeeds_after_pending() {
        let server = MockServer::start(vec![
            ("200 OK", r#"{"error":"authorization_pending"}"#),
            (
                "200 OK",
                r#"{"access_token":"header.eyJ1c2VyX2lkIjoidS0xMjMifQ.sig","refresh_token":"R1","expires_in":900,"scope":"kimi-code","token_type":"Bearer"}"#,
            ),
        ]);
        let blob = poll_for_token_at(&auth(), server.base_url(), Arc::new(AtomicBool::new(false)))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&blob).unwrap();
        assert_eq!(v.get("refresh_token").unwrap(), "R1");
        assert_eq!(v.get("scope").unwrap(), "kimi-code");
        assert!(v.get("expires_at").unwrap().as_i64().unwrap() > 0);
        assert_eq!(server.finish().len(), 2);
    }

    #[tokio::test]
    async fn cancel_stops_polling() {
        let server = MockServer::start(vec![]);
        let blob = poll_for_token_at(&auth(), server.base_url(), Arc::new(AtomicBool::new(true)))
            .await;
        assert!(blob.unwrap_err().to_string().contains("取消"));
    }

    #[tokio::test]
    async fn access_denied_is_error() {
        let server = MockServer::start(vec![("200 OK", r#"{"error":"access_denied"}"#)]);
        let blob = poll_for_token_at(&auth(), server.base_url(), Arc::new(AtomicBool::new(false)))
            .await;
        assert!(blob.unwrap_err().to_string().contains("拒绝"));
    }

    #[tokio::test]
    async fn device_code_request_parses_fields() {
        let server = MockServer::start(vec![(
            "200 OK",
            r#"{"device_code":"dc-9","user_code":"WXYZ-1234","verification_uri":"https://auth.kimi.com/device","expires_in":600,"interval":3}"#,
        )]);
        let auth = request_device_code_at(server.base_url()).await.unwrap();
        assert_eq!(auth.device_code, "dc-9");
        assert_eq!(auth.user_code, "WXYZ-1234");
        assert_eq!(auth.expires_in, 600);
    }
}
