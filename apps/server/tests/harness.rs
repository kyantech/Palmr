pub mod support;

use palmr_server::lifecycle::Drain;

use support::TestApplication;

#[tokio::test(flavor = "multi_thread")]
async fn it_harness_boots_and_serves_health() -> anyhow::Result<()> {
    let application = TestApplication::start("it_harness_boots_and_serves_health").await?;
    assert!(application.data_dir().join("instance.key").is_file());

    let response = application
        .client()
        .get(application.url("/health/live")?)
        .send()
        .await?;
    assert_eq!(response.status(), reqwest::StatusCode::OK);

    assert_eq!(application.shutdown().await, Drain::Completed);
    Ok(())
}
