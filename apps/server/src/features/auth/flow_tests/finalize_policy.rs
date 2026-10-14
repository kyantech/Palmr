use super::finalize::{bytes, png, Rig};
use super::folders::HOST_A;
use super::transfers::{pathed, request_body, request_in, sized, SESSIONS};
use super::tus::{header, metadata, small_limits, Fault, Planned, TusReq, TUS};
use super::tus_patch::patch;
use super::*;
use crate::features::transfers::finalize::{FinalizeOutcome, Point};

impl Rig {
    async fn push_meta(
        &self,
        planned: Planned,
        data: &[u8],
        extra: &[(&str, &str)],
    ) -> super::finalize::Pushed {
        let created = self
            .stack
            .tus_send(
                TusReq::new(Method::POST, TUS, &self.alice)
                    .header(
                        "upload-metadata",
                        &metadata(&planned.session, &planned.item, &planned.name, extra),
                    )
                    .header("upload-length", &data.len().to_string()),
            )
            .await;
        assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
        let upload = super::tus::upload_id_of(&created);
        if !data.is_empty() {
            let patched = self
                .stack
                .tus_send(patch(&self.alice, &upload, 0).bytes(data))
                .await;
            assert_eq!(patched.status, StatusCode::NO_CONTENT, "{}", patched.text());
        }
        super::finalize::Pushed { planned, upload }
    }

    async fn push_hinted(&self, name: &str, data: &[u8], hint: &str) -> super::finalize::Pushed {
        let planned = self.plan(name, Some(data.len() as u64)).await;
        self.push_meta(planned, data, &[("filetype", hint)]).await
    }

    pub(super) async fn failed_with(&self, item: &str) -> String {
        let (state, _, _, code) = self.item_row(item).await;
        assert_eq!(state, "failed", "{item}");
        code.unwrap()
    }

    async fn tombstoned(&self, item: &str) -> (String, String, i64) {
        let found = self.object_state(item).await.unwrap();
        assert_eq!(found.0, "tombstoned");
        found
    }
}

#[tokio::test]
async fn it_finalize_keep_both_name() {
    let rig = Rig::new().await;
    for name in [
        "README",
        "README",
        ".env",
        ".env",
        "archive.tar.gz",
        "archive.tar.gz",
        "photo.jpg",
        "photo (1).jpg",
        "photo.jpg",
        "photo.jpg",
    ] {
        rig.push_and_settle(name, &bytes(300)).await;
    }
    let mut names: Vec<String> = rig
        .file_rows()
        .await
        .into_iter()
        .map(|row| row.name)
        .collect();
    names.sort();
    let mut expected = vec![
        "README",
        "README (1)",
        ".env",
        ".env (1)",
        "archive.tar.gz",
        "archive.tar (1).gz",
        "photo.jpg",
        "photo (1).jpg",
        "photo (2).jpg",
        "photo (3).jpg",
    ];
    expected.sort_unstable();
    assert_eq!(names, expected, "deterministic keep-both names");
    assert_eq!(
        rig.stack.counts().await.objects,
        10,
        "no object key repeats"
    );
    assert_eq!(rig.used().await, 3_000, "each file charged exactly once");
}

#[tokio::test]
async fn it_finalize_keep_both_name_under_concurrency_and_unicode() {
    let rig = Rig::new().await;
    let first = rig.plan("report.txt", Some(900)).await;
    let second = rig.plan("report.txt", Some(900)).await;
    let (a, b) = (bytes(900), {
        let mut other = bytes(900);
        other.reverse();
        other
    });
    rig.push_planned(first.clone(), &a).await;
    rig.push_planned(second.clone(), &b).await;
    rig.settle().await;
    let rows = rig.file_rows().await;
    assert_eq!(rows.len(), 2, "two distinct files");
    let mut names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["report (1).txt", "report.txt"]);
    assert_ne!(rows[0].object, rows[1].object);
    let digests: Vec<Vec<u8>> = {
        let mut out = Vec::new();
        for planned in [&first, &second] {
            let path = rig.final_path(&planned.item).await;
            out.push(Rig::digest_of(&path).unwrap().1);
        }
        out
    };
    assert_eq!(digests[0], Rig::digest(&a));
    assert_eq!(digests[1], Rig::digest(&b));
    assert_eq!(rig.used().await, 1_800);

    let composed = "Caf\u{e9}.txt";
    let decomposed = "Cafe\u{301}.txt";
    rig.push_and_settle(composed, &bytes(10)).await;
    rig.push_and_settle(decomposed, &bytes(10)).await;
    let normalized: std::collections::HashSet<String> =
        sqlx::query_scalar("SELECT name_normalized FROM files WHERE name LIKE 'Caf%'")
            .fetch_all(rig.stack.pools.reader().executor())
            .await
            .unwrap()
            .into_iter()
            .collect();
    assert_eq!(normalized.len(), 2, "the normalized names are distinct");
}

#[tokio::test]
async fn it_finalize_overrun_tombstones_blob() {
    let rig = Rig::new().await;
    let data = bytes(8_000);
    let pushed = rig.plan("big.bin", Some(8_000)).await;
    let item = pushed.item.clone();
    let session = pushed.session.clone();
    let before_commit = rig.hooks.gate_at(Point::BeforeCommit);
    rig.push_planned(pushed, &data).await;
    before_commit.wait_until_entered().await;
    rig.stack.set_quota(rig.alice.id, Some(1_000)).await;
    before_commit.open();
    rig.settle().await;

    assert!(rig.file_rows().await.is_empty(), "no visible file");
    let (state, key, size) = rig.tombstoned(&item).await;
    assert_eq!(
        key,
        rig.final_key(&item).await,
        "the recorded key is tombstoned"
    );
    assert_eq!(size, 8_000);
    assert_eq!(rig.used().await, 0, "no usage increment");
    assert_eq!(rig.failed_with(&item).await, "QUOTA_EXCEEDED");
    assert_eq!(state, "tombstoned");
    assert_eq!(rig.queue_rows().await, 1, "one deletion queue entry");
    assert_eq!(
        rig.jobs_for_delete().await,
        1,
        "one deduplicated delete job"
    );
    let reason: String = sqlx::query_scalar("SELECT reason FROM file_deletion_queue")
        .fetch_one(rig.stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(reason, "quota_rejected");
    let (_, reserved, committed, _) = rig.stack.reservation(&session).await;
    assert_eq!((reserved, committed), (0, None), "the share was settled");
    assert_eq!(rig.audit_uploads().await, 0, "a rejection is not an upload");
    assert_eq!(rig.stack.session_row(&session).await.0, "failed");
    let tus_state: String = sqlx::query_scalar("SELECT state FROM tus_uploads")
        .fetch_one(rig.stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(tus_state, "terminated");

    let replay = rig
        .finalizer()
        .finalize(item.parse().unwrap(), None)
        .await
        .unwrap();
    assert!(matches!(replay, FinalizeOutcome::NotReady), "{replay:?}");
    assert_eq!(rig.queue_rows().await, 1, "a replay queues nothing more");
    assert_eq!(rig.jobs_for_delete().await, 1);
}

#[tokio::test]
async fn it_abandoned_placed_object_tombstone_inserted() {
    let rig = Rig::new().await;
    let data = bytes(2_000);
    let after_place = rig.hooks.gate_at(Point::AfterPlace);
    let pushed = rig.push("abandon.bin", &data).await;
    let item = pushed.planned.item.clone();
    after_place.wait_until_entered().await;
    let bytes_at_key = rig.final_path(&item).await;
    assert!(bytes_at_key.exists());

    let canceled = rig
        .stack
        .cancel_session(&rig.alice, &pushed.planned.session)
        .await;
    assert_eq!(
        canceled.status,
        StatusCode::NO_CONTENT,
        "{}",
        canceled.text()
    );
    let (state, key, _) = rig.tombstoned(&item).await;
    assert_eq!(
        (state.as_str(), key.as_str()),
        ("tombstoned", rig.final_key(&item).await.as_str())
    );
    assert_eq!(rig.queue_rows().await, 1);
    assert_eq!(rig.jobs_for_delete().await, 1);
    let reason: String = sqlx::query_scalar("SELECT reason FROM file_deletion_queue")
        .fetch_one(rig.stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(reason, "upload_abandoned");

    after_place.open();
    rig.settle().await;
    assert!(
        rig.file_rows().await.is_empty(),
        "the late commit created no file"
    );
    assert_eq!(rig.used().await, 0);
    assert_eq!(rig.queue_rows().await, 1, "still one queue entry");
    assert_eq!(rig.jobs_for_delete().await, 1, "still one job");
    assert_eq!(rig.item_row(&item).await.0, "canceled");
    assert!(
        bytes_at_key.exists(),
        "the bytes stay until the deletion lifecycle removes them"
    );
}

#[tokio::test]
async fn it_finalize_zero_byte_file_and_creation_with_upload() {
    let rig = Rig::new().await;
    let empty = rig.plan("empty.txt", Some(0)).await;
    rig.push_planned(empty.clone(), &[]).await;
    let nameless = rig.plan("nothing", Some(0)).await;
    let created = rig
        .stack
        .tus_send(
            rig.stack
                .create_req(&rig.alice, &nameless, Some(0))
                .raw_body(Body::empty())
                .header("content-type", super::tus::OCTET),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    rig.settle().await;
    let mut rows = rig.file_rows().await;
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(rows.len(), 2);
    assert_eq!(
        (
            rows[0].name.as_str(),
            rows[0].size,
            rows[0].mime.as_str(),
            rows[0].mime_source.as_str()
        ),
        ("empty.txt", 0, "text/plain", "extension")
    );
    assert_eq!(
        (
            rows[1].name.as_str(),
            rows[1].size,
            rows[1].mime.as_str(),
            rows[1].mime_source.as_str()
        ),
        ("nothing", 0, "application/octet-stream", "fallback")
    );
    assert_eq!(rig.used().await, 0);
    assert_eq!(rig.stack.counts().await.objects, 2);
    assert_eq!(
        Rig::digest_of(&rig.final_path(&empty.item).await)
            .unwrap()
            .0,
        0,
        "a real zero-byte object exists"
    );
    assert_eq!(rig.audit_uploads().await, 2);

    let planned = rig.plan("with-body.bin", Some(5_000)).await;
    let data = bytes(5_000);
    let created = rig
        .stack
        .tus_send(
            rig.stack
                .create_req(&rig.alice, &planned, Some(5_000))
                .bytes(&data),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    assert_eq!(header(&created, "upload-offset"), "5000");
    rig.settle().await;
    let found = rig
        .file_rows()
        .await
        .into_iter()
        .find(|row| row.name == "with-body.bin")
        .unwrap();
    assert_eq!(found.size, 5_000);
    assert_eq!(
        Rig::digest_of(&rig.final_path(&planned.item).await).unwrap(),
        (5_000, Rig::digest(&data))
    );
    assert_eq!(rig.used().await, 5_000);
}

type MimeCase = (
    &'static str,
    Vec<u8>,
    Option<&'static str>,
    &'static str,
    &'static str,
);

#[tokio::test]
async fn it_finalize_resolves_mime_from_durable_bytes() {
    let rig = Rig::new().await;
    let mut pdf = b"%PDF-1.4\n".to_vec();
    pdf.extend(bytes(200));
    let cases: Vec<MimeCase> = vec![
        ("photo.txt", png(), None, "image/png", "sniffed"),
        ("doc", pdf.clone(), None, "application/pdf", "sniffed"),
        (
            "notes.txt",
            b"plain words".to_vec(),
            None,
            "text/plain",
            "extension",
        ),
        (
            "blob.xyzunknown",
            bytes(200),
            None,
            "application/octet-stream",
            "fallback",
        ),
        (
            "noext",
            bytes(200),
            None,
            "application/octet-stream",
            "fallback",
        ),
        ("liar.pdf", png(), Some("text/html"), "image/png", "sniffed"),
        (
            "hinted",
            bytes(200),
            Some("text/csv"),
            "text/csv",
            "client_hint",
        ),
        (
            "empty",
            Vec::new(),
            None,
            "application/octet-stream",
            "fallback",
        ),
    ];
    for (name, data, hint, _, _) in &cases {
        match hint {
            Some(hint) => {
                rig.push_hinted(name, data, hint).await;
            }
            None => {
                rig.push(name, data).await;
            }
        }
        rig.settle().await;
    }
    for (name, _, _, mime, source) in cases {
        let row = rig
            .file_rows()
            .await
            .into_iter()
            .find(|row| row.name == name)
            .unwrap_or_else(|| panic!("{name}"));
        assert_eq!(
            (row.mime.as_str(), row.mime_source.as_str()),
            (mime, source),
            "{name}"
        );
    }
}

#[tokio::test]
async fn it_finalize_over_reservation_commits_the_measured_size() {
    let rig = Rig::new().await;
    rig.stack.set_max_file_size(100).await;
    let planned = rig.plan("small.bin", None).await;
    let (session, item) = (planned.session.clone(), planned.item.clone());
    let upload = rig.stack.tus_created(&rig.alice, &planned, None).await;
    assert_eq!(rig.stack.reservation(&session).await.1, 100);
    let data = bytes(75);
    let patched = rig
        .stack
        .tus_send(
            patch(&rig.alice, &upload, 0)
                .header("upload-length", "75")
                .bytes(&data),
        )
        .await;
    assert_eq!(patched.status, StatusCode::NO_CONTENT, "{}", patched.text());
    rig.settle().await;

    assert_eq!(
        rig.used().await,
        75,
        "the measured size, not the reservation"
    );
    let rows = rig.file_rows().await;
    assert_eq!(rows[0].size, 75);
    let (state, reserved, committed, _) = rig.stack.reservation(&session).await;
    assert_eq!(
        (state.as_str(), reserved, committed),
        ("committed", 0, Some(75)),
        "the excess hold was released"
    );
    assert_eq!(rig.item_row(&item).await.0, "completed");
    assert_eq!(rig.stack.session_row(&session).await.0, "completed");
}

#[tokio::test]
async fn it_finalize_quota_exact_boundary_and_overflow() {
    let rig = Rig::new().await;
    rig.stack.set_quota(rig.alice.id, Some(1_500)).await;
    rig.push_and_settle("fits.bin", &bytes(1_000)).await;
    assert_eq!(rig.used().await, 1_000);
    rig.push_and_settle("exact.bin", &bytes(500)).await;
    assert_eq!(rig.used().await, 1_500, "usage may reach the quota exactly");
    assert_eq!(rig.file_rows().await.len(), 2);

    assert!(
        rig.stack
            .open(
                &rig.alice,
                &request_body(None, &[sized("c2", "over.bin", 1)])
            )
            .await
            .status
            == StatusCode::INSUFFICIENT_STORAGE,
        "admission already refuses one byte more"
    );
}

#[tokio::test]
async fn it_finalize_completes_the_session_and_settles_the_reservation() {
    let rig = Rig::new().await;
    let pushed = rig.push_and_settle("only.bin", &bytes(4_000)).await;
    let session = &pushed.planned.session;
    assert_eq!(rig.stack.session_row(session).await.0, "completed");
    let (state, reserved, committed, reason) = rig.stack.reservation(session).await;
    assert_eq!(
        (state.as_str(), reserved, committed, reason),
        ("committed", 0, Some(4_000), None)
    );
    let counters: (i64, i64) = sqlx::query_as(
        "SELECT completed_file_count, completed_bytes FROM transfer_sessions WHERE id = ?1",
    )
    .bind(session)
    .fetch_one(rig.stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(counters, (1, 4_000));
    let detail = rig.stack.session_detail(&rig.alice, session).await;
    assert_eq!(detail.status, StatusCode::OK);
    let body = detail.json();
    assert_eq!(body["state"], "completed");
    let file = &body["files"][0];
    assert_eq!(file["state"], "completed");
    assert_eq!(file["fileId"], rig.file_rows().await[0].id);
    assert_eq!(file["uploadedBytes"], 4_000);
    let leaked = body.to_string();
    assert!(!leaked.contains("objects/"), "no storage key leaks");
    assert!(!leaked.contains("staging"), "no staging path leaks");
    let fetched = rig
        .stack
        .read(
            &format!("/api/v1/files/{}", rig.file_rows().await[0].id),
            &rig.alice,
        )
        .await;
    assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
    assert_eq!(fetched.json()["sizeBytes"], 4_000);
}

#[tokio::test]
async fn it_finalize_partial_session_settles_each_item_once() {
    let rig = Rig::new().await;
    let created = rig
        .stack
        .opened(
            &rig.alice,
            &[
                sized("a", "a.bin", 1_000),
                sized("b", "b.bin", 2_000),
                sized("c", "c.bin", 4_000),
            ],
        )
        .await;
    let session = created["id"].as_str().unwrap().to_owned();
    let items = rig.stack.item_ids(&session).await;
    let plan = |index: usize, name: &str| Planned {
        session: session.clone(),
        item: items[index].clone(),
        name: name.to_owned(),
    };
    assert_eq!(rig.stack.reservation(&session).await.1, 7_000);

    rig.push_planned(plan(0, "a.bin"), &bytes(1_000)).await;
    rig.settle().await;
    assert_eq!(rig.used().await, 1_000, "only A is charged");
    assert_eq!(
        rig.stack.reservation(&session).await.1,
        6_000,
        "A's share settled"
    );
    assert_eq!(rig.stack.session_row(&session).await.0, "uploading");

    let upload_b = rig
        .stack
        .tus_created(&rig.alice, &plan(1, "b.bin"), Some(2_000))
        .await;
    let hold = rig.hooks.gate_at(Point::BeforePlace);
    rig.stack
        .tus_send(patch(&rig.alice, &upload_b, 0).bytes(&bytes(2_000)))
        .await;
    hold.wait_until_entered().await;
    let upload_c = rig
        .stack
        .tus_created(&rig.alice, &plan(2, "c.bin"), Some(4_000))
        .await;
    let data_c = bytes(4_001);
    let overrun = rig
        .stack
        .tus_send(super::tus_patch::framed_patch(
            &rig.alice,
            &upload_c,
            0,
            super::tus::stream_of(super::tus::frames(&data_c, &[2_000, 2_001])),
        ))
        .await;
    assert_eq!(
        overrun.status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "{}",
        overrun.text()
    );
    assert_eq!(rig.stack.session_row(&session).await.0, "failed");
    hold.open();
    rig.settle().await;

    assert_eq!(rig.used().await, 3_000, "A and B are charged, C is not");
    let (state, reserved, committed, _) = rig.stack.reservation(&session).await;
    assert_eq!(
        (state.as_str(), reserved, committed),
        ("held", 4_000, None),
        "C keeps its retryable share; B's share is settled"
    );
    assert_eq!(
        rig.stack.item_states(&session).await,
        ["completed", "completed", "failed"]
    );
    assert_eq!(rig.stack.session_row(&session).await.0, "failed");
    assert_eq!(rig.file_rows().await.len(), 2);

    let closed = rig.stack.complete_session(&rig.alice, &session).await;
    assert_eq!(closed.status, StatusCode::OK, "{}", closed.text());
    let (state, _, committed, _) = rig.stack.reservation(&session).await;
    assert_eq!((state.as_str(), committed), ("committed", Some(3_000)));
}

#[tokio::test]
async fn it_finalize_cancel_after_one_file_completed_keeps_it() {
    let rig = Rig::new().await;
    let created = rig
        .stack
        .opened(
            &rig.alice,
            &[sized("a", "a.bin", 1_000), sized("b", "b.bin", 2_000)],
        )
        .await;
    let session = created["id"].as_str().unwrap().to_owned();
    let items = rig.stack.item_ids(&session).await;
    rig.push_planned(
        Planned {
            session: session.clone(),
            item: items[0].clone(),
            name: "a.bin".into(),
        },
        &bytes(1_000),
    )
    .await;
    rig.settle().await;
    let canceled = rig.stack.cancel_session(&rig.alice, &session).await;
    assert_eq!(canceled.status, StatusCode::NO_CONTENT);
    assert_eq!(rig.file_rows().await.len(), 1, "A stays content");
    assert_eq!(rig.used().await, 1_000);
    let (state, _, committed, _) = rig.stack.reservation(&session).await;
    assert_eq!((state.as_str(), committed), ("committed", Some(1_000)));
}

#[tokio::test]
async fn it_finalize_nested_destination_and_destination_changes() {
    let rig = Rig::new().await;
    let body = request_body(None, &[pathed("n", "docs/2026/plan.txt", 120)]);
    let created = rig.stack.open(&rig.alice, &body).await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.text());
    let session = created.json()["id"].as_str().unwrap().to_owned();
    let item = rig.stack.item_ids(&session).await.remove(0);
    let planned = Planned {
        session,
        item: item.clone(),
        name: "plan.txt".to_owned(),
    };
    rig.push_meta(
        planned,
        &bytes(120),
        &[("relativePath", "docs/2026/plan.txt")],
    )
    .await;
    rig.settle().await;
    let row = rig.file_rows().await.remove(0);
    let leaf: String = sqlx::query_scalar("SELECT name FROM folders WHERE id = ?1")
        .bind(row.folder.as_deref().unwrap())
        .fetch_one(rig.stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(leaf, "2026");

    let folder = rig.stack.make(&rig.alice, "inbox", None).await;
    let folder_id = folder["id"].as_str().unwrap().to_owned();
    let created = rig
        .stack
        .open(
            &rig.alice,
            &request_in(&folder_id, &[sized("d", "dest.bin", 50)]),
        )
        .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let session = created.json()["id"].as_str().unwrap().to_owned();
    let item = rig.stack.item_ids(&session).await.remove(0);
    let gate = rig.hooks.gate_at(Point::Measured);
    rig.push_planned(
        Planned {
            session: session.clone(),
            item: item.clone(),
            name: "dest.bin".into(),
        },
        &bytes(50),
    )
    .await;
    gate.wait_until_entered().await;
    rig.stack
        .execute(&format!(
            "UPDATE folders SET deleting = 1 WHERE id = '{folder_id}'"
        ))
        .await;
    gate.open();
    rig.settle().await;

    assert_eq!(rig.failed_with(&item).await, "FOLDER_DELETING");
    assert_eq!(rig.file_rows().await.len(), 1, "no redirect to the root");
    assert!(
        !rig.final_path(&item).await.exists(),
        "the blob was never placed"
    );
    assert!(
        rig.object_state(&item).await.is_none(),
        "nothing was tombstoned"
    );
    assert_eq!(
        rig.stack.reservation(&session).await.1,
        0,
        "the share is released"
    );
    assert!(!rig.staging_exists_for(&item).await, "staging is cleaned");
}

impl Rig {
    async fn staging_exists_for(&self, item: &str) -> bool {
        let upload: Option<String> =
            sqlx::query_scalar("SELECT id FROM tus_uploads WHERE transfer_session_file_id = ?1")
                .bind(item)
                .fetch_optional(self.stack.pools.reader().executor())
                .await
                .unwrap();
        upload.is_some_and(|upload| self.staging_exists(&upload))
    }
}

#[tokio::test]
async fn it_finalize_rechecks_account_and_expiry_under_the_writer() {
    let rig = Rig::new().await;
    let done = rig.push_and_settle("kept.bin", &bytes(100)).await;
    let planned = rig.plan("late.bin", Some(100)).await;
    let item = planned.item.clone();
    let gate = rig.hooks.gate_at(Point::BeforeCommit);
    rig.push_planned(planned, &bytes(100)).await;
    gate.wait_until_entered().await;
    rig.stack
        .execute(&format!("UPDATE users SET is_active = 0, deactivated_at = '2026-09-25T12:00:00.000Z' WHERE id = '{}'", rig.alice.id))
        .await;
    gate.open();
    rig.settle().await;
    assert_eq!(rig.failed_with(&item).await, "AUTH_ACCOUNT_INACTIVE");
    assert_eq!(
        rig.file_rows().await.len(),
        1,
        "completed files are preserved"
    );
    assert_eq!(rig.item_row(&done.planned.item).await.0, "completed");
    rig.tombstoned(&item).await;

    rig.stack
        .execute(&format!(
            "UPDATE users SET is_active = 1, deactivated_at = NULL WHERE id = '{}'",
            rig.alice.id
        ))
        .await;
    let planned = rig.plan("expiring.bin", Some(100)).await;
    let item = planned.item.clone();
    let gate = rig.hooks.gate_at(Point::BeforeCommit);
    rig.push_planned(planned, &bytes(100)).await;
    gate.wait_until_entered().await;
    rig.stack
        .clock
        .advance(std::time::Duration::from_secs(8 * 24 * 60 * 60));
    gate.open();
    rig.settle().await;
    assert_eq!(rig.failed_with(&item).await, "TRANSFER_SESSION_EXPIRED");
    assert_eq!(
        rig.file_rows().await.len(),
        1,
        "an expired upload is not committed"
    );
}

#[tokio::test]
async fn it_finalize_storage_faults_are_retryable_and_resume_without_retransmission() {
    for (fault, code) in [
        (Fault::NoSpace, "STORAGE_WRITE_FAILED"),
        (Fault::Denied, "STORAGE_UNAVAILABLE"),
        (Fault::Io, "STORAGE_UNAVAILABLE"),
    ] {
        let rig = Rig::new().await;
        *rig.staging.place_fault.lock().unwrap() = Some((1, fault));
        let data = bytes(3_000);
        let pushed = rig.push("fault.bin", &data).await;
        let item = pushed.planned.item.clone();
        rig.settle().await;
        assert_eq!(rig.failed_with(&item).await, code);
        assert!(rig.file_rows().await.is_empty());
        assert_eq!(rig.used().await, 0);
        assert!(
            rig.staging_exists(&pushed.upload),
            "staging is retained for the retry"
        );
        assert_eq!(
            rig.stack.reservation(&pushed.planned.session).await.1,
            3_000
        );
        assert!(
            rig.object_state(&item).await.is_none(),
            "a retryable failure tombstones nothing"
        );

        let retried = rig
            .stack
            .retry_item(&rig.alice, &pushed.planned.session, &item)
            .await;
        assert_eq!(retried.status, StatusCode::OK, "{}", retried.text());
        rig.settle().await;
        assert_eq!(rig.file_rows().await.len(), 1);
        assert_eq!(rig.used().await, 3_000);
        assert_eq!(
            Rig::digest_of(&rig.final_path(&item).await).unwrap(),
            (3_000, Rig::digest(&data))
        );
        assert_eq!(
            rig.staging
                .place_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            2
        );
        rig.assert_no_overlap();
    }
}

#[tokio::test]
async fn it_finalize_measure_fault_is_retryable() {
    let rig = Rig::new().await;
    *rig.staging.measure_fault.lock().unwrap() = Some((1, Fault::Io));
    let pushed = rig.push("m.bin", &bytes(500)).await;
    rig.settle().await;
    assert_eq!(
        rig.failed_with(&pushed.planned.item).await,
        "STORAGE_UNAVAILABLE"
    );
    rig.stack
        .retry_item(&rig.alice, &pushed.planned.session, &pushed.planned.item)
        .await;
    rig.settle().await;
    assert_eq!(rig.file_rows().await.len(), 1);
}

#[tokio::test]
async fn it_finalize_never_overwrites_a_foreign_object_at_the_key() {
    let rig = Rig::new().await;
    let planned = rig.plan("clash.bin", Some(600)).await;
    let item = planned.item.clone();
    let key = rig.final_path(&item).await;
    std::fs::create_dir_all(key.parent().unwrap()).unwrap();
    std::fs::write(&key, b"someone else's bytes").unwrap();
    let pushed = rig.push_planned(planned, &bytes(600)).await;
    rig.settle().await;
    assert_eq!(rig.failed_with(&item).await, "INTERNAL_ERROR");
    assert_eq!(
        Rig::digest_of(&key).unwrap(),
        (20, Rig::digest(b"someone else's bytes")),
        "the foreign object is untouched"
    );
    assert!(rig.file_rows().await.is_empty());
    assert!(rig.object_state(&item).await.is_none());
    assert!(
        rig.staging_exists(&pushed.upload),
        "the staged bytes are kept"
    );
}

#[tokio::test]
async fn it_finalize_adopts_only_identical_bytes_at_the_key() {
    let rig = Rig::new().await;
    let data = bytes(600);
    let planned = rig.plan("twin.bin", Some(600)).await;
    let item = planned.item.clone();
    let key = rig.final_path(&item).await;
    std::fs::create_dir_all(key.parent().unwrap()).unwrap();
    std::fs::write(&key, &data).unwrap();
    rig.push_planned(planned, &data).await;
    rig.settle().await;
    assert_eq!(rig.file_rows().await.len(), 1);
    assert_eq!(
        rig.staging
            .place_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "an identical object at the key is not placed over"
    );
}

#[tokio::test]
async fn it_finalize_missing_bytes_fail_without_inventing_content() {
    let rig = Rig::new().await;
    let gate = rig.hooks.gate_at(Point::Claimed);
    let pushed = rig.push("gone.bin", &bytes(700)).await;
    gate.wait_until_entered().await;
    std::fs::remove_dir_all(
        rig.root
            .join("uploads")
            .join(pushed.upload.replace('-', "")),
    )
    .unwrap();
    gate.open();
    rig.settle().await;
    assert_eq!(
        rig.failed_with(&pushed.planned.item).await,
        "UPLOAD_LENGTH_MISMATCH"
    );
    assert!(rig.file_rows().await.is_empty());
    assert_eq!(rig.used().await, 0);
    assert!(rig.object_state(&pushed.planned.item).await.is_none());
}

#[tokio::test]
async fn it_finalize_head_reports_the_full_offset_while_finalizing_and_after() {
    let rig = Rig::new().await;
    let gate = rig.hooks.gate_at(Point::BeforePlace);
    let pushed = rig.push("head.bin", &bytes(900)).await;
    gate.wait_until_entered().await;
    for _ in 0..2 {
        let head = rig
            .stack
            .tus_send(super::finalize::tus_head(&rig, &pushed.upload))
            .await;
        assert_eq!(head.status, StatusCode::OK);
        assert_eq!(header(&head, "upload-offset"), "900");
        gate.open();
        rig.settle().await;
    }
    let again = rig
        .stack
        .tus_send(patch(&rig.alice, &pushed.upload, 900).bytes(&[]))
        .await;
    assert_eq!(
        again.status,
        StatusCode::CONFLICT,
        "a finalized upload takes no more PATCH"
    );
    let del = rig
        .stack
        .tus_send(TusReq::new(
            Method::DELETE,
            &format!("{TUS}/{}", pushed.upload),
            &rig.alice,
        ))
        .await;
    assert_eq!(del.status, StatusCode::NO_CONTENT);
    assert_eq!(
        rig.file_rows().await.len(),
        1,
        "terminate never deletes a finalized file"
    );
    let _ = (HOST_A, SESSIONS, small_limits());
}

#[tokio::test]
async fn it_finalize_a_failure_after_the_rename_resumes_without_a_second_placement() {
    let rig = Rig::new().await;
    *rig.staging.place_after_fault.lock().unwrap() = Some((1, Fault::Io));
    let data = bytes(7_000);
    let pushed = rig.push("synced.bin", &data).await;
    let item = pushed.planned.item.clone();
    rig.settle().await;

    assert_eq!(rig.failed_with(&item).await, "STORAGE_UNAVAILABLE");
    assert!(
        rig.file_rows().await.is_empty(),
        "no file before the commit"
    );
    assert!(
        rig.object_state(&item).await.is_none(),
        "a retryable failure tombstones nothing"
    );
    assert_eq!(
        Rig::digest_of(&rig.final_path(&item).await).unwrap(),
        (7_000, Rig::digest(&data)),
        "the renamed object is complete at the recorded key"
    );
    assert!(
        !rig.staging_exists(&pushed.upload),
        "the staging blob moved"
    );

    let retried = rig
        .stack
        .retry_item(&rig.alice, &pushed.planned.session, &item)
        .await;
    assert_eq!(retried.status, StatusCode::OK, "{}", retried.text());
    rig.settle().await;
    assert_eq!(rig.file_rows().await.len(), 1);
    assert_eq!(rig.used().await, 7_000);
    assert_eq!(
        rig.staging
            .place_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the retry committed the placed object instead of placing again"
    );
    assert_eq!(rig.audit_uploads().await, 1);
}

#[tokio::test]
async fn it_finalize_never_leaks_another_owners_files_or_sessions() {
    let rig = Rig::new().await;
    let pushed = rig.push_and_settle("private.bin", &bytes(500)).await;
    let bob = rig.stack.member("bob", super::folders::HOST_B).await;
    let file_id = rig.file_rows().await[0].id.clone();

    let file = rig
        .stack
        .read(&format!("/api/v1/files/{file_id}"), &bob)
        .await;
    assert_eq!(file.status, StatusCode::NOT_FOUND);
    let session = rig
        .stack
        .session_detail(&bob, &pushed.planned.session)
        .await;
    assert_eq!(session.status, StatusCode::NOT_FOUND);
    let head = rig
        .stack
        .tus_send(TusReq::new(
            Method::HEAD,
            &format!("{TUS}/{}", pushed.upload),
            &bob,
        ))
        .await;
    assert_eq!(head.status, StatusCode::NOT_FOUND);
    let del = rig
        .stack
        .tus_send(TusReq::new(
            Method::DELETE,
            &format!("{TUS}/{}", pushed.upload),
            &bob,
        ))
        .await;
    assert_eq!(del.status, StatusCode::NOT_FOUND);
    let listing = rig.stack.read("/api/v1/files", &bob).await;
    assert_eq!(listing.status, StatusCode::OK);
    assert!(!listing.text().contains("private.bin"));
    assert_eq!(
        rig.stack.used_bytes(bob.id).await,
        0,
        "bob is never charged"
    );
}
