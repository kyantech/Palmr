pub mod support;

use palmr_server::lifecycle::Drain;

use support::TestApplication;

#[tokio::test(flavor = "multi_thread")]
async fn it_harness_boots_and_serves_health() -> anyhow::Result<()> {
    let application = TestApplication::start("it_harness_boots_and_serves_health").await?;
    assert!(application.data_dir().join("instance.key").is_file());
    assert!(application.data_dir().join("palmr.db").is_file());

    let response = application
        .client()
        .get(application.url("/health/live")?)
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);

    let ready = application
        .client()
        .get(application.url("/health/ready")?)
        .send()
        .await?;
    assert_eq!(ready.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = serde_json::from_str(&ready.text().await?)?;
    assert_eq!(body["database"], "ok");
    assert_eq!(body["migrations"], "current");

    assert_eq!(application.shutdown().await, Drain::Completed);
    Ok(())
}
