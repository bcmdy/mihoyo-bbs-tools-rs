use mihoyo_bbs_tools::http::HttpClient;
use reqwest::Url;

/// 只验证生产 HTTPS 证书、平台验证器和 ring provider 能完成握手，不携带任何凭据。
#[tokio::test]
#[ignore = "需要访问公开生产 HTTPS 端点"]
async fn public_miyoushe_https_handshake_works_with_ring() {
    let client = HttpClient::builder().build().unwrap();
    let url = Url::parse("https://bbs-api.miyoushe.com/user/api/getUserFullInfo?uid=0").unwrap();
    let response: serde_json::Value = client.get_json(url).await.unwrap();
    assert!(response.get("retcode").is_some());
}
