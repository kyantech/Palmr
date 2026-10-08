use std::collections::HashSet;

use futures_util::future::join_all;

use super::files::{write_surface, FileRow, FILES};
use super::folders::{HOST_A, HOST_B, SEEDED_AT};
use super::profile::assert_code;
use super::*;
use crate::domain::naming::NameCandidate;

const LATER: &str = "2026-09-25T12:01:00.000Z";

fn validation_fields(fetched: &Fetched) -> Value {
    fetched.json()["error"]["details"]["fields"].clone()
}

impl Stack {
    async fn ids_named_like(&self, owner: UserId, pattern: &str) -> Vec<String> {
        sqlx::query_scalar("SELECT id FROM files WHERE owner_id = ?1 AND name LIKE ?2 ORDER BY id")
            .bind(owner.to_string())
            .bind(pattern)
            .fetch_all(self.pools.reader().executor())
            .await
            .unwrap()
    }

    async fn names_in(&self, owner: UserId, folder: Option<&str>) -> Vec<String> {
        let rows: Vec<String> = match folder {
            Some(folder) => sqlx::query_scalar(
                "SELECT name FROM files WHERE owner_id = ?1 AND folder_id = ?2 ORDER BY name",
            )
            .bind(owner.to_string())
            .bind(folder)
            .fetch_all(self.pools.reader().executor())
            .await
            .unwrap(),
            None => sqlx::query_scalar(
                "SELECT name FROM files WHERE owner_id = ?1 AND folder_id IS NULL ORDER BY name",
            )
            .bind(owner.to_string())
            .fetch_all(self.pools.reader().executor())
            .await
            .unwrap(),
        };
        rows
    }

    async fn assert_names_are_sound(&self) {
        let duplicates = self
            .scalar_i64(
                "SELECT COUNT(*) FROM (
                     SELECT 1 FROM files GROUP BY owner_id, folder_id, name_normalized
                     HAVING COUNT(*) > 1)",
            )
            .await;
        assert_eq!(duplicates, 0, "two siblings share a normalized name");
        for row in self.file_rows().await {
            let candidate = NameCandidate::new(row.2.as_str()).unwrap();
            assert_eq!(row.3, candidate.normalized(), "{}: name_normalized", row.2);
            assert_eq!(row.4, candidate.extension(), "{}: extension", row.2);
        }
    }
}

fn identity(row: &FileRow) -> (String, i64, String, String) {
    (row.6.clone(), row.7, row.8.clone(), row.9.clone())
}

fn stamp(minute: u32) -> String {
    format!("2026-09-25T12:{minute:02}:00.000Z")
}

#[tokio::test]
async fn it_file_patch_absent_vs_null() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let folder = stack.seed_folder(alice.id, None, "Docs", 0).await;
    let id = stack
        .put_file(alice.id, Some(&folder), "Report.txt", 11)
        .await;
    stack
        .execute(&format!(
            "UPDATE files SET description = 'first' WHERE id = '{id}'"
        ))
        .await;
    let sibling = stack
        .put_file(alice.id, Some(&folder), "Other.txt", 1)
        .await;
    let stored = identity(&stack.file_row(&id).await);
    let mut minute = 0;
    let mut tick = || {
        minute += 1;
        stack.clock.advance(Duration::from_secs(60));
        stamp(minute)
    };

    let current = stack.read(&format!("{FILES}/{id}"), &alice).await.json();
    tick();
    let untouched = stack.rename_file(&alice, &id, &json!({})).await;
    assert_eq!(untouched.status, StatusCode::OK, "{}", untouched.text());
    let mut expected = current.clone();
    expected["renamedTo"] = Value::Null;
    assert_eq!(
        untouched.json(),
        expected,
        "an empty patch returns the file"
    );
    assert_eq!(stack.file_row(&id).await.5, SEEDED_AT, "and writes nothing");

    let first_write = tick();
    let described = stack
        .rename_file(&alice, &id, &json!({ "description": "second" }))
        .await;
    assert_eq!(described.status, StatusCode::OK, "{}", described.text());
    let body = described.json();
    assert_eq!(body["description"], "second");
    assert_eq!(body["name"], "Report.txt", "an absent name is unchanged");
    assert_eq!(body["renamedTo"], Value::Null);
    assert_eq!(body["updatedAt"], first_write);

    tick();
    let same = stack
        .rename_file(&alice, &id, &json!({ "description": "second" }))
        .await
        .json();
    assert_eq!(
        same["updatedAt"], first_write,
        "an unchanged description is not a write"
    );

    let cleared_at = tick();
    let cleared = stack
        .rename_file(&alice, &id, &json!({ "description": null }))
        .await
        .json();
    assert_eq!(cleared["description"], Value::Null);
    assert_eq!(cleared["name"], "Report.txt");
    assert_eq!(cleared["updatedAt"], cleared_at);
    tick();
    let cleared_again = stack
        .rename_file(&alice, &id, &json!({ "description": null }))
        .await
        .json();
    assert_eq!(
        cleared_again["updatedAt"], cleared_at,
        "clearing nothing is not a write"
    );

    let renamed_at = tick();
    let renamed = stack
        .rename_file(&alice, &id, &json!({ "name": "Renamed.txt" }))
        .await
        .json();
    assert_eq!(renamed["name"], "Renamed.txt");
    assert_eq!(
        renamed["description"],
        Value::Null,
        "an absent description is unchanged"
    );
    assert_eq!(renamed["renamedTo"], Value::Null);
    assert_eq!(renamed["updatedAt"], renamed_at);

    tick();
    let exact = stack
        .rename_file(&alice, &id, &json!({ "name": "Renamed.txt" }))
        .await
        .json();
    assert_eq!(
        exact["name"], "Renamed.txt",
        "self-rename never becomes (1)"
    );
    assert_eq!(exact["renamedTo"], Value::Null);
    assert_eq!(exact["updatedAt"], renamed_at);

    let case_at = tick();
    let case_only = stack
        .rename_file(&alice, &id, &json!({ "name": "RENAMED.TXT" }))
        .await
        .json();
    assert_eq!(case_only["name"], "RENAMED.TXT");
    assert_eq!(case_only["renamedTo"], Value::Null);
    assert_eq!(case_only["updatedAt"], case_at);
    let row = stack.file_row(&id).await;
    assert_eq!(row.3, "renamed.txt");
    assert_eq!(row.4, "txt");

    tick();
    let collided = stack
        .rename_file(&alice, &id, &json!({ "name": "other.TXT" }))
        .await
        .json();
    assert_eq!(collided["name"], "other (1).TXT");
    assert_eq!(collided["renamedTo"], "other (1).TXT");
    assert_eq!(stack.file_row(&sibling).await.2, "Other.txt");
    assert_eq!(stack.file_row(&id).await.4, "txt");

    let both_at = tick();
    let both = stack
        .rename_file(
            &alice,
            &id,
            &json!({ "name": "Both.md", "description": "both at once" }),
        )
        .await
        .json();
    assert_eq!(both["name"], "Both.md");
    assert_eq!(both["description"], "both at once");
    assert_eq!(both["updatedAt"], both_at);
    assert_eq!(stack.file_row(&id).await.4, "md");

    let before = stack.file_rows().await;
    tick();
    for (body, code, status, fields) in [
        (
            json!({ "name": null }),
            "VALIDATION_ERROR",
            StatusCode::UNPROCESSABLE_ENTITY,
            Some(json!(["name"])),
        ),
        (
            json!({ "name": 5 }),
            "VALIDATION_ERROR",
            StatusCode::UNPROCESSABLE_ENTITY,
            Some(json!(["name"])),
        ),
        (
            json!({ "description": 5 }),
            "VALIDATION_ERROR",
            StatusCode::UNPROCESSABLE_ENTITY,
            Some(json!(["description"])),
        ),
        (
            json!({ "folderId": null }),
            "VALIDATION_ERROR",
            StatusCode::UNPROCESSABLE_ENTITY,
            Some(json!(["body"])),
        ),
        (
            json!({ "storageObjectId": "x" }),
            "VALIDATION_ERROR",
            StatusCode::UNPROCESSABLE_ENTITY,
            Some(json!(["body"])),
        ),
        (
            json!({ "description": "x".repeat(2001) }),
            "VALIDATION_ERROR",
            StatusCode::UNPROCESSABLE_ENTITY,
            Some(json!(["description"])),
        ),
        (
            json!({ "name": "" }),
            "NAME_INVALID",
            StatusCode::UNPROCESSABLE_ENTITY,
            None,
        ),
        (
            json!({ "name": "a/b" }),
            "NAME_INVALID",
            StatusCode::UNPROCESSABLE_ENTITY,
            None,
        ),
        (
            json!({ "name": ".." }),
            "NAME_INVALID",
            StatusCode::UNPROCESSABLE_ENTITY,
            None,
        ),
        (
            json!({ "name": "x".repeat(256) }),
            "NAME_INVALID",
            StatusCode::UNPROCESSABLE_ENTITY,
            None,
        ),
        (
            json!({ "name": "tab\there" }),
            "NAME_INVALID",
            StatusCode::UNPROCESSABLE_ENTITY,
            None,
        ),
        (
            json!(["name"]),
            "VALIDATION_ERROR",
            StatusCode::UNPROCESSABLE_ENTITY,
            Some(json!(["body"])),
        ),
    ] {
        let refused = stack.rename_file(&alice, &id, &body).await;
        assert_code(&refused, status, code);
        if let Some(fields) = fields {
            assert_eq!(validation_fields(&refused), fields, "{body}");
        }
    }
    assert_eq!(
        stack.file_rows().await,
        before,
        "refused patches change nothing"
    );

    let long = "é".repeat(2000);
    let fits = stack
        .rename_file(&alice, &id, &json!({ "description": long }))
        .await;
    assert_eq!(fits.status, StatusCode::OK, "2000 characters fit");
    assert_eq!(
        fits.json()["description"].as_str().unwrap().chars().count(),
        2000
    );

    let row = stack.file_row(&id).await;
    assert_eq!(
        (row.6.clone(), row.7, row.8.clone(), row.9.clone()),
        stored,
        "storage identity, size and detected type never change"
    );
    assert_eq!(
        row.1.as_deref(),
        Some(folder.as_str()),
        "patch never moves the file"
    );
    stack.assert_names_are_sound().await;
    stack.stop().await;
}

#[tokio::test]
async fn it_file_rename_recomputes_the_stored_extension() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    stack.put_file(alice.id, None, "photo.jpg", 1).await;
    let id = stack.put_file(alice.id, None, "README", 1).await;
    assert_eq!(stack.file_row(&id).await.4, "");

    for (requested, display, normalized, extension) in [
        ("report.tar.gz", "report.tar.gz", "report.tar.gz", "gz"),
        (".env", ".env", ".env", ""),
        ("photo.JPG", "photo (1).JPG", "photo (1).jpg", "jpg"),
        ("a.", "a.", "a.", ""),
        ("Data.CSV", "Data.CSV", "data.csv", "csv"),
        ("README", "README", "readme", ""),
        ("report (1).pdf", "report (1).pdf", "report (1).pdf", "pdf"),
        ("archive.TAR.Gz", "archive.TAR.Gz", "archive.tar.gz", "gz"),
        ("cafe\u{301}.TXT", "cafe\u{301}.TXT", "café.txt", "txt"),
    ] {
        stack.clock.advance(Duration::from_secs(1));
        let renamed = stack
            .rename_file(&alice, &id, &json!({ "name": requested }))
            .await;
        assert_eq!(
            renamed.status,
            StatusCode::OK,
            "{requested}: {}",
            renamed.text()
        );
        let body = renamed.json();
        assert_eq!(body["name"], display, "{requested}");
        let row = stack.file_row(&id).await;
        assert_eq!(row.2, display, "{requested}: name");
        assert_eq!(row.3, normalized, "{requested}: name_normalized");
        assert_eq!(row.4, extension, "{requested}: extension");
        if display == requested {
            assert_eq!(body["renamedTo"], Value::Null, "{requested}");
        } else {
            assert_eq!(body["renamedTo"], display, "{requested}");
        }
    }

    let long_extension = format!("blob.{}", "x".repeat(40));
    let renamed = stack
        .rename_file(&alice, &id, &json!({ "name": long_extension }))
        .await;
    assert_eq!(renamed.status, StatusCode::OK, "{}", renamed.text());
    assert_eq!(
        stack.file_row(&id).await.4,
        "",
        "an extension past the 32 character column bound is stored as none"
    );
    stack.assert_names_are_sound().await;
    stack.stop().await;
}

#[tokio::test]
async fn regression_309_duplicate_names_in_folder() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;
    let docs = stack.seed_folder(alice.id, None, "Docs", 0).await;
    let nested = stack.seed_folder(alice.id, Some(&docs), "Nested", 1).await;
    let bobs = stack.put_file(bob.id, None, "photo.jpg", 1).await;

    let first = stack.put_file(alice.id, None, "photo.jpg", 1).await;
    let second = stack.put_file(alice.id, None, "other-1.jpg", 1).await;
    let renamed = stack
        .rename_file(&alice, &second, &json!({ "name": "photo.jpg" }))
        .await
        .json();
    assert_eq!(
        renamed["name"], "photo (1).jpg",
        "rename into a root collision"
    );
    assert_eq!(renamed["renamedTo"], "photo (1).jpg");
    assert_eq!(
        stack.file_row(&first).await.2,
        "photo.jpg",
        "the occupant is untouched"
    );
    for (index, expected) in ["photo (2).jpg", "photo (3).jpg", "photo (4).jpg"]
        .into_iter()
        .enumerate()
    {
        let id = stack
            .put_file(alice.id, None, &format!("tmp-{index}.jpg"), 1)
            .await;
        let body = stack
            .rename_file(&alice, &id, &json!({ "name": "PHOTO.jpg" }))
            .await
            .json();
        assert_eq!(
            body["name"],
            expected.replace("photo", "PHOTO"),
            "the (n) progression keeps the requested spelling"
        );
        assert_eq!(stack.file_row(&id).await.4, "jpg");
    }
    assert_eq!(
        stack.file_row(&bobs).await.2,
        "photo.jpg",
        "another owner is a separate namespace"
    );

    let occupant = stack.put_file(alice.id, Some(&docs), "report.pdf", 1).await;
    let from_root = stack.put_file(alice.id, None, "report.pdf", 1).await;
    let from_nested = stack
        .put_file(alice.id, Some(&nested), "REPORT.PDF", 1)
        .await;
    let moved = stack.move_file_to(&alice, &from_root, Some(&docs)).await;
    assert_eq!(moved.status, StatusCode::OK, "{}", moved.text());
    assert_eq!(
        moved.json()["name"],
        "report (1).pdf",
        "move into a folder collision"
    );
    assert_eq!(moved.json()["renamedTo"], "report (1).pdf");
    let moved = stack
        .move_file_to(&alice, &from_nested, Some(&docs))
        .await
        .json();
    assert_eq!(
        moved["name"], "REPORT (2).PDF",
        "case-insensitive collision, spelling kept"
    );
    assert_eq!(stack.file_row(&occupant).await.2, "report.pdf");
    for id in [&from_root, &from_nested] {
        assert_eq!(stack.file_row(id).await.4, "pdf");
    }

    let into_nested = stack.put_file(alice.id, Some(&nested), "n.txt", 1).await;
    let from_docs = stack.put_file(alice.id, Some(&docs), "n.txt", 1).await;
    let body = stack
        .move_file_to(&alice, &from_docs, Some(&nested))
        .await
        .json();
    assert_eq!(body["name"], "n (1).txt", "nested collision");
    assert_eq!(stack.file_row(&into_nested).await.2, "n.txt");
    let to_root = stack.move_file_to(&alice, &from_docs, None).await.json();
    assert_eq!(
        to_root["name"], "n (1).txt",
        "root has no n (1).txt, so the name is kept"
    );
    assert_eq!(to_root["renamedTo"], Value::Null);

    let composed = stack.put_file(alice.id, None, "caf\u{e9}.txt", 1).await;
    let decomposed = stack
        .put_file(alice.id, Some(&docs), "cafe\u{301}.txt", 1)
        .await;
    let body = stack.move_file_to(&alice, &decomposed, None).await.json();
    assert_eq!(
        body["name"], "cafe\u{301} (1).txt",
        "NFC and NFD spellings are the same name"
    );
    assert_eq!(stack.file_row(&composed).await.2, "caf\u{e9}.txt");
    assert_eq!(stack.file_row(&decomposed).await.3, "café (1).txt");

    let folder_named_like_a_file = stack.seed_folder(alice.id, None, "docs.txt", 0).await;
    let free = stack.put_file(alice.id, Some(&docs), "docs.txt", 1).await;
    let body = stack.move_file_to(&alice, &free, None).await.json();
    assert_eq!(
        body["name"], "docs.txt",
        "a folder does not occupy a file name"
    );
    assert!(!folder_named_like_a_file.is_empty());

    let target = stack.seed_folder(alice.id, None, "Target", 0).await;
    stack.put_file(alice.id, Some(&target), "dup.bin", 1).await;
    let mut sources = Vec::new();
    for index in 0..12 {
        let folder = stack
            .seed_folder(alice.id, None, &format!("source-{index:02}"), 0)
            .await;
        sources.push(stack.put_file(alice.id, Some(&folder), "dup.bin", 1).await);
    }
    let concurrent = join_all(
        sources
            .iter()
            .map(|id| stack.move_file_to(&alice, id, Some(&target))),
    )
    .await;
    for fetched in &concurrent {
        assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
    }
    let mut landed = stack.names_in(alice.id, Some(&target)).await;
    landed.sort();
    let mut expected: Vec<String> = std::iter::once("dup.bin".to_owned())
        .chain((1..=12).map(|n| format!("dup ({n}).bin")))
        .collect();
    expected.sort();
    assert_eq!(
        landed, expected,
        "concurrent moves each get a distinct name"
    );

    let crowded = stack.seed_folder(alice.id, None, "Crowded", 0).await;
    let mut renamers: Vec<String> = Vec::new();
    for index in 0..8 {
        renamers.push(
            stack
                .put_file(alice.id, Some(&crowded), &format!("start-{index}.txt"), 1)
                .await,
        );
    }
    let same_name = json!({ "name": "same.txt" });
    let renamed = join_all(
        renamers
            .iter()
            .map(|id| stack.rename_file(&alice, id, &same_name)),
    )
    .await;
    let mut names: Vec<String> = renamed
        .iter()
        .map(|fetched| {
            assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
            fetched.json()["name"].as_str().unwrap().to_owned()
        })
        .collect();
    names.sort();
    let mut expected: Vec<String> = std::iter::once("same.txt".to_owned())
        .chain((1..8).map(|n| format!("same ({n}).txt")))
        .collect();
    expected.sort();
    assert_eq!(
        names, expected,
        "concurrent renames each get a distinct name"
    );

    stack.assert_names_are_sound().await;
    stack.stop().await;
}

#[tokio::test]
async fn it_file_move_single() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let one = stack.seed_folder(alice.id, None, "One", 0).await;
    let two = stack.seed_folder(alice.id, Some(&one), "Two", 1).await;
    let id = stack.put_file(alice.id, None, "move-me.txt", 5).await;
    let identity_before = identity(&stack.file_row(&id).await);

    stack.clock.advance(Duration::from_secs(60));
    let into = stack.move_file_to(&alice, &id, Some(&two)).await;
    assert_eq!(into.status, StatusCode::OK, "{}", into.text());
    let body = into.json();
    assert_eq!(body["folderId"], two);
    assert_eq!(body["renamedTo"], Value::Null);
    assert_eq!(body["name"], "move-me.txt");
    assert_eq!(body["updatedAt"], LATER);
    assert_eq!(into.headers.get("cache-control").unwrap(), "no-store");

    stack.clock.advance(Duration::from_secs(60));
    let same = stack.move_file_to(&alice, &id, Some(&two)).await.json();
    assert_eq!(
        same["updatedAt"], LATER,
        "moving to the current folder writes nothing"
    );
    assert_eq!(same["name"], "move-me.txt");
    assert_eq!(same["renamedTo"], Value::Null);

    stack.clock.advance(Duration::from_secs(60));
    let across = stack.move_file_to(&alice, &id, Some(&one)).await.json();
    assert_eq!(across["folderId"], one);
    let to_root = stack.move_file_to(&alice, &id, None).await.json();
    assert_eq!(to_root["folderId"], Value::Null);
    stack.clock.advance(Duration::from_secs(60));
    let root_again = stack.move_file_to(&alice, &id, None).await.json();
    assert_eq!(
        root_again["updatedAt"], to_root["updatedAt"],
        "root to root is a no-op"
    );
    assert_eq!(
        identity(&stack.file_row(&id).await),
        identity_before,
        "moving never touches storage identity"
    );

    let blocker = stack.put_file(alice.id, None, "full.txt", 1).await;
    let full = stack.seed_folder(alice.id, None, "Full", 0).await;
    stack.put_file(alice.id, Some(&full), "full.txt", 1).await;
    stack
        .seed_numbered_names(alice.id, Some(&full), "full", "txt", 1_000)
        .await;
    let rows = stack.file_rows().await;
    let refused = stack.move_file_to(&alice, &blocker, Some(&full)).await;
    assert_code(&refused, StatusCode::CONFLICT, "FILE_NAME_CONFLICT");
    assert_eq!(
        stack.file_rows().await,
        rows,
        "an exhausted move changes nothing"
    );
    let loose = stack.put_file(alice.id, Some(&full), "loose.txt", 1).await;
    let rows = stack.file_rows().await;
    let refused = stack
        .rename_file(&alice, &loose, &json!({ "name": "full.txt" }))
        .await;
    assert_code(&refused, StatusCode::CONFLICT, "FILE_NAME_CONFLICT");
    assert_eq!(
        stack.file_rows().await,
        rows,
        "an exhausted rename keeps the old name"
    );

    let phantom = stack.fresh_id();
    for (body, code, status) in [
        (
            json!({}),
            "VALIDATION_ERROR",
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({ "folderId": "nope" }),
            "FOLDER_NOT_FOUND",
            StatusCode::NOT_FOUND,
        ),
        (
            json!({ "folderId": phantom }),
            "FOLDER_NOT_FOUND",
            StatusCode::NOT_FOUND,
        ),
        (
            json!({ "folderId": null, "name": "x" }),
            "VALIDATION_ERROR",
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({ "folderId": 5 }),
            "VALIDATION_ERROR",
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let refused = stack
            .api(
                Method::POST,
                &format!("{FILES}/{id}/move"),
                &alice,
                Some(&body),
            )
            .await;
        assert_code(&refused, status, code);
    }
    let missing = stack.move_file_to(&alice, &phantom, None).await;
    assert_code(&missing, StatusCode::NOT_FOUND, "FILE_NOT_FOUND");
    let malformed = stack.move_file_to(&alice, "not-a-uuid", None).await;
    assert_code(&malformed, StatusCode::NOT_FOUND, "FILE_NOT_FOUND");
    stack.assert_names_are_sound().await;
    stack.stop().await;
}

#[tokio::test]
async fn it_file_batch_move_atomic() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let bob = stack.member("bob", HOST_B).await;

    let dest = stack.seed_folder(alice.id, None, "Dest", 0).await;
    let left = stack.seed_folder(alice.id, None, "Left", 0).await;
    let right = stack.seed_folder(alice.id, None, "Right", 0).await;
    stack.put_file(alice.id, Some(&dest), "report.pdf", 1).await;
    let resident = stack.put_file(alice.id, Some(&dest), "stay.txt", 1).await;
    let a1 = stack.put_file(alice.id, Some(&left), "report.pdf", 1).await;
    let b1 = stack
        .put_file(alice.id, Some(&right), "report.pdf", 1)
        .await;
    let c1 = stack
        .put_file(alice.id, Some(&right), "unique.txt", 1)
        .await;
    let foreign_file = stack.put_file(bob.id, None, "bobs.txt", 1).await;
    let foreign_folder = stack.seed_folder(bob.id, None, "BobDir", 0).await;
    let phantom = stack.fresh_id();

    let surface = write_surface(&stack).await;
    for (case, body, status, code) in [
        (
            "foreign file last",
            json!({ "fileIds": [a1, b1, foreign_file], "targetFolderId": dest }),
            StatusCode::NOT_FOUND,
            "FILE_NOT_FOUND",
        ),
        (
            "missing file in the middle",
            json!({ "fileIds": [a1, phantom, b1], "targetFolderId": dest }),
            StatusCode::NOT_FOUND,
            "FILE_NOT_FOUND",
        ),
        (
            "malformed file id",
            json!({ "fileIds": [a1, "nope"], "targetFolderId": dest }),
            StatusCode::NOT_FOUND,
            "FILE_NOT_FOUND",
        ),
        (
            "missing target",
            json!({ "fileIds": [a1, b1], "targetFolderId": phantom }),
            StatusCode::NOT_FOUND,
            "FOLDER_NOT_FOUND",
        ),
        (
            "foreign target",
            json!({ "fileIds": [a1, b1], "targetFolderId": foreign_folder }),
            StatusCode::NOT_FOUND,
            "FOLDER_NOT_FOUND",
        ),
        (
            "missing folder",
            json!({ "fileIds": [a1], "folderIds": [phantom], "targetFolderId": dest }),
            StatusCode::NOT_FOUND,
            "FOLDER_NOT_FOUND",
        ),
        (
            "folder into itself after a good folder",
            json!({ "fileIds": [a1], "folderIds": [right, left], "targetFolderId": left }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "FOLDER_CYCLE",
        ),
    ] {
        stack.clock.advance(Duration::from_secs(1));
        let refused = stack.batch_move(&alice, &body).await;
        assert_code(&refused, status, code);
        assert_eq!(
            write_surface(&stack).await,
            surface,
            "{case}: something moved"
        );
    }

    let crowded = stack.seed_folder(alice.id, None, "Crowded", 0).await;
    stack.put_file(alice.id, Some(&crowded), "x.txt", 1).await;
    stack
        .seed_numbered_names(alice.id, Some(&crowded), "x", "txt", 1_000)
        .await;
    let doomed = stack.put_file(alice.id, None, "x.txt", 1).await;
    let surface = write_surface(&stack).await;
    let refused = stack
        .batch_move(
            &alice,
            &json!({ "fileIds": [a1, b1, doomed, c1], "targetFolderId": crowded }),
        )
        .await;
    assert_code(&refused, StatusCode::CONFLICT, "FILE_NAME_CONFLICT");
    assert_eq!(
        write_surface(&stack).await,
        surface,
        "naming exhaustion on one item rolls the others back"
    );

    stack
        .execute(&format!(
            "CREATE TRIGGER boom BEFORE UPDATE ON files WHEN OLD.id = '{c1}'
             BEGIN SELECT RAISE(ABORT, 'boom'); END"
        ))
        .await;
    let surface = write_surface(&stack).await;
    let failed = stack
        .batch_move(
            &alice,
            &json!({ "fileIds": [a1, b1, c1], "targetFolderId": dest }),
        )
        .await;
    assert_code(&failed, StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR");
    assert_eq!(
        write_surface(&stack).await,
        surface,
        "a database failure after earlier items moved rolls them back"
    );
    stack.execute("DROP TRIGGER boom").await;

    stack.clock.advance(Duration::from_secs(60));
    let moved = stack
        .batch_move(
            &alice,
            &json!({ "fileIds": [b1, a1, resident, c1], "targetFolderId": dest }),
        )
        .await;
    assert_eq!(moved.status, StatusCode::OK, "{}", moved.text());
    assert_eq!(moved.headers.get("cache-control").unwrap(), "no-store");
    let body = moved.json();
    assert_eq!(body["folders"], json!([]));
    assert_eq!(
        body["files"],
        json!([
            { "id": b1, "name": "report (1).pdf", "renamedTo": "report (1).pdf" },
            { "id": a1, "name": "report (2).pdf", "renamedTo": "report (2).pdf" },
            { "id": resident, "name": "stay.txt", "renamedTo": null },
            { "id": c1, "name": "unique.txt", "renamedTo": null },
        ]),
        "request order decides who gets which suffix"
    );
    assert_eq!(
        stack.file_row(&resident).await.5,
        SEEDED_AT,
        "a file already there is not written"
    );
    assert_ne!(stack.file_row(&b1).await.5, SEEDED_AT);
    stack.assert_names_are_sound().await;

    let tree = stack.seed_folder(alice.id, None, "Tree", 0).await;
    let tree_child = stack.seed_folder(alice.id, Some(&tree), "Child", 1).await;
    let wanderer = stack.seed_folder(alice.id, None, "Wanderer", 0).await;
    stack
        .seed_folder(alice.id, Some(&dest), "Wanderer", 1)
        .await;
    let loose = stack.put_file(alice.id, None, "loose.txt", 1).await;
    let mixed = stack
        .batch_move(
            &alice,
            &json!({ "fileIds": [loose], "folderIds": [wanderer, tree], "targetFolderId": dest }),
        )
        .await;
    assert_eq!(mixed.status, StatusCode::OK, "{}", mixed.text());
    let body = mixed.json();
    assert_eq!(
        body["folders"],
        json!([
            { "id": wanderer, "name": "Wanderer (1)", "renamedTo": "Wanderer (1)" },
            { "id": tree, "name": "Tree", "renamedTo": null },
        ])
    );
    assert_eq!(body["files"][0]["name"], "loose.txt");
    let depth = stack
        .scalar_i64(&format!(
            "SELECT depth FROM folders WHERE id = '{tree_child}'"
        ))
        .await;
    assert_eq!(depth, 2, "a moved subtree keeps consistent depths");
    stack.assert_names_are_sound().await;
    stack.stop().await;
}

#[tokio::test]
async fn it_file_batch_move_validates_before_moving() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let dest = stack.seed_folder(alice.id, None, "Dest", 0).await;
    let other = stack.seed_folder(alice.id, None, "Other", 0).await;
    let file = stack.put_file(alice.id, None, "one.txt", 1).await;
    let surface = write_surface(&stack).await;

    let ids = |count: usize| -> Vec<String> { (0..count).map(|_| stack.fresh_id()).collect() };
    for (case, body, status, code, fields) in [
        (
            "501 ids",
            json!({ "fileIds": ids(501), "targetFolderId": dest }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "BATCH_TOO_LARGE",
            None,
        ),
        (
            "501 across files and folders",
            json!({ "fileIds": ids(250), "folderIds": ids(251), "targetFolderId": dest }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "BATCH_TOO_LARGE",
            None,
        ),
        (
            "500 unknown ids pass the bound",
            json!({ "fileIds": ids(500), "targetFolderId": dest }),
            StatusCode::NOT_FOUND,
            "FILE_NOT_FOUND",
            None,
        ),
        (
            "no ids",
            json!({ "fileIds": [], "targetFolderId": dest }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
            Some(json!(["fileIds"])),
        ),
        (
            "duplicate file id",
            json!({ "fileIds": [file, file], "targetFolderId": dest }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
            Some(json!(["fileIds"])),
        ),
        (
            "duplicate folder id",
            json!({ "fileIds": [file], "folderIds": [other, other], "targetFolderId": dest }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
            Some(json!(["folderIds"])),
        ),
        (
            "missing target member",
            json!({ "fileIds": [file] }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
            Some(json!(["targetFolderId"])),
        ),
        (
            "missing fileIds member",
            json!({ "targetFolderId": dest }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
            Some(json!(["fileIds"])),
        ),
        (
            "unknown member",
            json!({ "fileIds": [file], "targetFolderId": dest, "storageKey": "x" }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
            Some(json!(["body"])),
        ),
        (
            "ids of the wrong type",
            json!({ "fileIds": [1, 2], "targetFolderId": dest }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
            Some(json!(["body"])),
        ),
        (
            "malformed target",
            json!({ "fileIds": [file], "targetFolderId": "nope" }),
            StatusCode::NOT_FOUND,
            "FOLDER_NOT_FOUND",
            None,
        ),
    ] {
        let refused = stack.batch_move(&alice, &body).await;
        assert_code(&refused, status, code);
        if let Some(fields) = fields {
            assert_eq!(validation_fields(&refused), fields, "{case}");
        }
        assert_eq!(write_surface(&stack).await, surface, "{case}");
    }
    let too_large = stack
        .batch_move(
            &alice,
            &json!({ "fileIds": ids(501), "targetFolderId": dest }),
        )
        .await;
    assert_eq!(too_large.json()["error"]["details"]["maxItems"], 500);

    let only_folders = stack
        .batch_move(
            &alice,
            &json!({ "fileIds": [], "folderIds": [other], "targetFolderId": dest }),
        )
        .await;
    assert_eq!(
        only_folders.status,
        StatusCode::OK,
        "{}",
        only_folders.text()
    );
    assert_eq!(only_folders.json()["folders"][0]["id"], other);
    stack.stop().await;
}

#[tokio::test]
async fn it_file_batch_move_handles_the_whole_bound_in_one_transaction() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let dest = stack.seed_folder(alice.id, None, "Dest", 0).await;
    stack
        .seed_numbered_names(alice.id, None, "bulk", "bin", 500)
        .await;
    let ids = stack.ids_named_like(alice.id, "bulk (%").await;
    assert_eq!(ids.len(), 500);

    stack.clock.advance(Duration::from_secs(60));
    let moved = stack
        .batch_move(&alice, &json!({ "fileIds": ids, "targetFolderId": dest }))
        .await;
    assert_eq!(moved.status, StatusCode::OK, "{}", moved.text());
    assert_eq!(moved.json()["files"].as_array().unwrap().len(), 500);
    let landed = stack.names_in(alice.id, Some(&dest)).await;
    assert_eq!(landed.len(), 500);
    assert_eq!(
        landed.iter().collect::<HashSet<_>>().len(),
        500,
        "no name repeated"
    );
    assert!(stack.names_in(alice.id, None).await.is_empty());
    let stamps = stack
        .scalar_i64("SELECT COUNT(DISTINCT updated_at) FROM files")
        .await;
    assert_eq!(stamps, 1, "one transaction, one timestamp");
    stack.assert_names_are_sound().await;
    stack.stop().await;
}

#[tokio::test]
async fn it_file_extension_length_boundary_is_stored_exactly_or_as_none() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let id = stack.put_file(alice.id, None, "start", 1).await;

    let accented = "é".repeat(32);
    let kanji = "日".repeat(32);
    for (requested, extension) in [
        (format!("blob.{}", "x".repeat(32)), "x".repeat(32)),
        (format!("blob.{}", "x".repeat(33)), String::new()),
        (format!("blob.{}", "X".repeat(32)), "x".repeat(32)),
        (format!("blob.{}", "X".repeat(33)), String::new()),
        (format!("blob.{}", "é".repeat(32)), accented.clone()),
        (format!("blob.{}", "É".repeat(32)), accented.clone()),
        (format!("blob.{}", "é".repeat(33)), String::new()),
        (format!("blob.{}", "日".repeat(32)), kanji.clone()),
        (format!("blob.{}", "日".repeat(33)), String::new()),
        (format!("blob.{}", "İ".repeat(32)), String::new()),
        ("README".to_owned(), String::new()),
        ("archive.TAR.GZ".to_owned(), "gz".to_owned()),
    ] {
        stack.clock.advance(Duration::from_secs(1));
        let renamed = stack
            .rename_file(&alice, &id, &json!({ "name": requested }))
            .await;
        assert_eq!(
            renamed.status,
            StatusCode::OK,
            "{requested}: {}",
            renamed.text()
        );
        let body = renamed.json();
        assert_eq!(body["name"], requested, "the display name is never altered");
        assert_eq!(body["renamedTo"], Value::Null);
        let row = stack.file_row(&id).await;
        assert_eq!(row.2, requested, "stored name");
        assert_eq!(row.4, extension, "{requested}: stored extension");
        assert!(row.4.chars().count() <= 32);
        assert_eq!(
            row.3,
            NameCandidate::new(requested.as_str()).unwrap().normalized()
        );
    }

    let overlong = format!("blob.{}", "é".repeat(33));
    stack
        .rename_file(&alice, &id, &json!({ "name": overlong }))
        .await;
    let sibling = stack.put_file(alice.id, None, "other", 1).await;
    stack.clock.advance(Duration::from_secs(1));
    let collided = stack
        .rename_file(&alice, &sibling, &json!({ "name": overlong }))
        .await
        .json();
    assert_eq!(collided["name"], format!("blob (1).{}", "é".repeat(33)));
    assert_eq!(
        stack.file_row(&sibling).await.4,
        "",
        "keep-both keeps the rule"
    );

    let moved_in = stack.seed_folder(alice.id, None, "Dest", 0).await;
    let carried = stack
        .put_file(alice.id, None, &format!("carried.{}", "x".repeat(32)), 1)
        .await;
    assert_eq!(stack.file_row(&carried).await.4, "x".repeat(32));
    let moved = stack.move_file_to(&alice, &carried, Some(&moved_in)).await;
    assert_eq!(moved.status, StatusCode::OK, "{}", moved.text());
    assert_eq!(stack.file_row(&carried).await.4, "x".repeat(32));
    stack.assert_names_are_sound().await;
    stack.stop().await;
}

#[tokio::test]
async fn it_file_batch_move_folder_only_selection_moves_the_folders() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let dest = stack.seed_folder(alice.id, None, "Dest", 0).await;
    stack.seed_folder(alice.id, Some(&dest), "Src", 1).await;
    let src = stack.seed_folder(alice.id, None, "Src", 0).await;
    let sub = stack.seed_folder(alice.id, Some(&src), "Sub", 1).await;
    let deep = stack.seed_folder(alice.id, Some(&sub), "Deep", 2).await;
    let inside = stack.put_file(alice.id, Some(&deep), "inside.txt", 3).await;
    let bystander = stack.put_file(alice.id, None, "bystander.txt", 1).await;
    let files_before = stack.file_rows().await;

    stack.clock.advance(Duration::from_secs(60));
    let moved = stack
        .batch_move(
            &alice,
            &json!({ "fileIds": [], "folderIds": [src], "targetFolderId": dest }),
        )
        .await;
    assert_eq!(moved.status, StatusCode::OK, "{}", moved.text());
    assert_eq!(
        moved.json(),
        json!({
            "files": [],
            "folders": [{ "id": src, "name": "Src (1)", "renamedTo": "Src (1)" }]
        })
    );
    let row: (Option<String>, String, String, i64) =
        sqlx::query_as("SELECT parent_id, name, name_normalized, depth FROM folders WHERE id = ?1")
            .bind(&src)
            .fetch_one(stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(
        row,
        (
            Some(dest.clone()),
            "Src (1)".to_owned(),
            "src (1)".to_owned(),
            1
        )
    );
    assert_eq!(stack.depth_in("folders", &sub).await, 2);
    assert_eq!(stack.depth_in("folders", &deep).await, 3);
    assert_eq!(
        stack.file_rows().await,
        files_before,
        "no file is written by a folder-only selection"
    );
    assert_eq!(
        stack.file_row(&inside).await.1.as_deref(),
        Some(deep.as_str())
    );
    assert_eq!(stack.file_row(&bystander).await.1, None);

    let back = stack
        .batch_move(
            &alice,
            &json!({ "fileIds": [], "folderIds": [deep], "targetFolderId": null }),
        )
        .await;
    assert_eq!(back.status, StatusCode::OK, "{}", back.text());
    assert_eq!(back.json()["folders"][0]["name"], "Deep");
    assert_eq!(stack.depth_in("folders", &deep).await, 0);

    let cycle = stack
        .batch_move(
            &alice,
            &json!({ "fileIds": [], "folderIds": [src], "targetFolderId": sub }),
        )
        .await;
    assert_code(&cycle, StatusCode::UNPROCESSABLE_ENTITY, "FOLDER_CYCLE");
    let nothing = stack
        .batch_move(
            &alice,
            &json!({ "fileIds": [], "folderIds": [], "targetFolderId": dest }),
        )
        .await;
    assert_code(
        &nothing,
        StatusCode::UNPROCESSABLE_ENTITY,
        "VALIDATION_ERROR",
    );
    stack.assert_names_are_sound().await;
    stack.stop().await;
}

#[tokio::test]
async fn it_file_batch_move_mixed_selection_is_one_transaction_for_both_kinds() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let alice = stack.member("alice", HOST_A).await;
    let dest = stack.seed_folder(alice.id, None, "Dest", 0).await;
    let folder_a = stack.seed_folder(alice.id, None, "A", 0).await;
    let folder_b = stack.seed_folder(alice.id, None, "B", 0).await;
    let child = stack
        .seed_folder(alice.id, Some(&folder_a), "Child", 1)
        .await;
    let file_one = stack.put_file(alice.id, None, "one.txt", 1).await;
    let file_two = stack
        .put_file(alice.id, Some(&folder_b), "two.txt", 1)
        .await;
    let phantom = stack.fresh_id();
    let surface = write_surface(&stack).await;

    stack.clock.advance(Duration::from_secs(60));
    let missing_file = stack
        .batch_move(
            &alice,
            &json!({
                "fileIds": [file_one, phantom],
                "folderIds": [folder_a, folder_b],
                "targetFolderId": dest
            }),
        )
        .await;
    assert_code(&missing_file, StatusCode::NOT_FOUND, "FILE_NOT_FOUND");
    assert_eq!(
        write_surface(&stack).await,
        surface,
        "folders already moved are rolled back with the files"
    );

    let crowded = stack.seed_folder(alice.id, None, "Crowded", 0).await;
    stack.put_file(alice.id, Some(&crowded), "z.txt", 1).await;
    stack
        .seed_numbered_names(alice.id, Some(&crowded), "z", "txt", 1_000)
        .await;
    let doomed = stack.put_file(alice.id, None, "z.txt", 1).await;
    let surface = write_surface(&stack).await;
    let exhausted = stack
        .batch_move(
            &alice,
            &json!({
                "fileIds": [file_one, doomed],
                "folderIds": [folder_a, folder_b],
                "targetFolderId": crowded
            }),
        )
        .await;
    assert_code(&exhausted, StatusCode::CONFLICT, "FILE_NAME_CONFLICT");
    assert_eq!(write_surface(&stack).await, surface);

    stack
        .execute(&format!(
            "CREATE TRIGGER boom BEFORE UPDATE ON files WHEN OLD.id = '{file_two}'
             BEGIN SELECT RAISE(ABORT, 'boom'); END"
        ))
        .await;
    let failed = stack
        .batch_move(
            &alice,
            &json!({
                "fileIds": [file_one, file_two],
                "folderIds": [folder_a, folder_b],
                "targetFolderId": dest
            }),
        )
        .await;
    assert_code(&failed, StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR");
    assert_eq!(
        write_surface(&stack).await,
        surface,
        "a failure after folders and a file moved leaves nothing moved"
    );
    stack.execute("DROP TRIGGER boom").await;

    let moved = stack
        .batch_move(
            &alice,
            &json!({
                "fileIds": [file_one, file_two],
                "folderIds": [folder_a, folder_b],
                "targetFolderId": dest
            }),
        )
        .await;
    assert_eq!(moved.status, StatusCode::OK, "{}", moved.text());
    let body = moved.json();
    assert_eq!(body["folders"].as_array().unwrap().len(), 2);
    assert_eq!(body["files"].as_array().unwrap().len(), 2);
    assert_eq!(stack.depth_in("folders", &folder_a).await, 1);
    assert_eq!(stack.depth_in("folders", &child).await, 2);
    let stamps = stack
        .scalar_i64(&format!(
            "SELECT COUNT(DISTINCT updated_at) FROM (
                 SELECT updated_at FROM folders WHERE id IN ('{folder_a}', '{folder_b}')
                 UNION ALL
                 SELECT updated_at FROM files WHERE id IN ('{file_one}', '{file_two}'))"
        ))
        .await;
    assert_eq!(stamps, 1, "both kinds share the one transaction timestamp");
    stack.assert_names_are_sound().await;
    stack.stop().await;
}

impl Stack {
    async fn depth_in(&self, table: &str, id: &str) -> i64 {
        self.scalar_i64(&format!("SELECT depth FROM {table} WHERE id = '{id}'"))
            .await
    }
}
