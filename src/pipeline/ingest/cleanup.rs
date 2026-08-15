use log::{debug, info, warn};
use std::fs;

use crate::pipeline::media::{self, MediaDirs};
use crate::store::sqlite::repo::LibraryRepository;

pub(crate) async fn delete_item_for_book_path(
    repo: &LibraryRepository,
    book_path: &str,
    media_dirs: &MediaDirs,
    is_internal_server: bool,
    retain_missing: bool,
) -> bool {
    match repo.find_fingerprint_by_book_path(book_path).await {
        Ok(Some(fp)) => {
            delete_item_and_media(
                repo,
                &fp.item_id,
                media_dirs,
                is_internal_server,
                retain_missing,
                &format!("book removed: {book_path}"),
            )
            .await
        }
        Ok(None) => {
            debug!("No DB fingerprint for removed path: {}", book_path);
            false
        }
        Err(e) => {
            warn!("Failed to look up fingerprint for {}: {}", book_path, e);
            false
        }
    }
}

pub(crate) async fn delete_item_and_media(
    repo: &LibraryRepository,
    item_id: &str,
    media_dirs: &MediaDirs,
    is_internal_server: bool,
    retain_missing: bool,
    reason: &str,
) -> bool {
    // Retention keeps the row, and with it the annotations that delete_item
    // would cascade away -- the reading record outlives the file. The cover is
    // kept too, so the item still looks like a book in the library; only the
    // file symlink goes, because the file really is gone and a link that cannot
    // be served is worse than no link.
    if retain_missing {
        if let Err(e) = repo.mark_item_missing(item_id).await {
            warn!("Failed to mark item {} as missing: {}", item_id, e);
            return false;
        }

        if is_internal_server
            && let Err(e) = media::remove_item_files_by_id(item_id, &media_dirs.files_dir)
        {
            warn!("Failed to clean file symlinks for {}: {}", item_id, e);
        }

        info!("Item {} kept, file missing ({})", item_id, reason);
        return true;
    }

    if let Err(e) = repo.delete_item(item_id).await {
        warn!("Failed to delete removed item {}: {}", item_id, e);
        return false;
    }

    if media::is_canonical_item_id(item_id) {
        let cover_path = media_dirs.covers_dir.join(format!("{}.webp", item_id));
        let _ = fs::remove_file(&cover_path);
    } else {
        warn!(
            "Skipping cover cleanup for non-canonical item id: {}",
            item_id
        );
    }

    if is_internal_server
        && let Err(e) = media::remove_item_files_by_id(item_id, &media_dirs.files_dir)
    {
        warn!("Failed to clean file symlinks for {}: {}", item_id, e);
    }

    info!("Deleted item {} ({})", item_id, reason);
    true
}

#[cfg(test)]
mod tests {
    use super::delete_item_and_media;
    use crate::pipeline::media::{self, resolve_media_dirs};
    use crate::store::sqlite::repo::rows::FingerprintRow;
    use crate::store::sqlite::repo::tests::{sample_annotation, sample_item, test_repo};

    const CANONICAL_ID: &str = "0123456789abcdef0123456789abcdef";

    #[tokio::test]
    async fn delete_item_and_media_removes_db_item_and_canonical_cover() {
        let repo = test_repo().await;
        repo.upsert_item(&sample_item(CANONICAL_ID))
            .await
            .expect("item upsert");

        let output_dir = tempfile::Builder::new()
            .prefix("koshelf-cleanup-")
            .tempdir_in(std::env::current_dir().expect("cwd"))
            .expect("output dir");
        let media_dirs = resolve_media_dirs(output_dir.path(), false);
        std::fs::create_dir_all(&media_dirs.covers_dir).expect("covers dir");
        let cover_path = media_dirs.covers_dir.join(format!("{CANONICAL_ID}.webp"));
        std::fs::write(&cover_path, b"cover").expect("cover write");

        assert!(
            delete_item_and_media(
                &repo,
                CANONICAL_ID,
                &media_dirs,
                false,
                false,
                "test removal"
            )
            .await
        );

        assert!(
            repo.get_item(CANONICAL_ID)
                .await
                .expect("get item")
                .is_none()
        );
        assert!(!cover_path.exists());
    }

    #[tokio::test]
    async fn delete_item_and_media_removes_internal_server_file_link() {
        let repo = test_repo().await;
        repo.upsert_item(&sample_item(CANONICAL_ID))
            .await
            .expect("item upsert");

        let output_dir = tempfile::Builder::new()
            .prefix("koshelf-cleanup-")
            .tempdir_in(std::env::current_dir().expect("cwd"))
            .expect("output dir");
        let media_dirs = resolve_media_dirs(output_dir.path(), true);
        std::fs::create_dir_all(&media_dirs.files_dir).expect("files dir");
        let source_path = output_dir.path().join("book.epub");
        std::fs::write(&source_path, b"book").expect("book write");
        media::sync_item_file_symlink(CANONICAL_ID, "epub", &source_path, &media_dirs.files_dir)
            .expect("file link");
        let link_path = media_dirs.files_dir.join(format!("{CANONICAL_ID}.epub"));

        assert!(link_path.exists());
        assert!(
            delete_item_and_media(
                &repo,
                CANONICAL_ID,
                &media_dirs,
                true,
                false,
                "test removal"
            )
            .await
        );
        assert!(!link_path.exists());
    }

    #[tokio::test]
    async fn retain_missing_keeps_the_item_its_annotations_and_its_cover() {
        let repo = test_repo().await;
        repo.upsert_item(&sample_item(CANONICAL_ID))
            .await
            .expect("item upsert");
        repo.replace_annotations(
            CANONICAL_ID,
            &[sample_annotation(CANONICAL_ID, "highlight", 0)],
        )
        .await
        .expect("annotations");

        let output_dir = tempfile::Builder::new()
            .prefix("koshelf-cleanup-")
            .tempdir_in(std::env::current_dir().expect("cwd"))
            .expect("output dir");
        let media_dirs = resolve_media_dirs(output_dir.path(), false);
        std::fs::create_dir_all(&media_dirs.covers_dir).expect("covers dir");
        let cover_path = media_dirs.covers_dir.join(format!("{CANONICAL_ID}.webp"));
        std::fs::write(&cover_path, b"cover").expect("cover write");

        assert!(
            delete_item_and_media(
                &repo,
                CANONICAL_ID,
                &media_dirs,
                false,
                true,
                "test removal"
            )
            .await
        );

        // The whole point: the file went, the reading record did not.
        assert!(repo.item_exists(CANONICAL_ID).await.expect("item exists"));
        assert_eq!(
            repo.get_annotations(CANONICAL_ID, None)
                .await
                .expect("annotations")
                .len(),
            1
        );
        assert!(cover_path.exists());
    }

    #[tokio::test]
    async fn retain_missing_still_removes_the_file_link() {
        let repo = test_repo().await;
        repo.upsert_item(&sample_item(CANONICAL_ID))
            .await
            .expect("item upsert");

        let output_dir = tempfile::Builder::new()
            .prefix("koshelf-cleanup-")
            .tempdir_in(std::env::current_dir().expect("cwd"))
            .expect("output dir");
        let media_dirs = resolve_media_dirs(output_dir.path(), true);
        std::fs::create_dir_all(&media_dirs.files_dir).expect("files dir");
        let source_path = output_dir.path().join("book.epub");
        std::fs::write(&source_path, b"book").expect("book write");
        media::sync_item_file_symlink(CANONICAL_ID, "epub", &source_path, &media_dirs.files_dir)
            .expect("file link");
        let link_path = media_dirs.files_dir.join(format!("{CANONICAL_ID}.epub"));

        assert!(link_path.exists());
        assert!(
            delete_item_and_media(&repo, CANONICAL_ID, &media_dirs, true, true, "test removal")
                .await
        );
        // A link that cannot be served is worse than no link -- see #94.
        assert!(!link_path.exists());
        assert!(repo.item_exists(CANONICAL_ID).await.expect("item exists"));
    }

    #[tokio::test]
    async fn delete_item_and_media_skips_noncanonical_cover_cleanup() {
        let repo = test_repo().await;
        let item_id = "not-canonical";
        repo.upsert_item(&sample_item(item_id))
            .await
            .expect("item upsert");
        repo.upsert_fingerprint(&FingerprintRow {
            item_id: item_id.to_string(),
            book_path: "/books/not-canonical.epub".to_string(),
            book_size_bytes: 1,
            book_modified_unix_ms: 1,
            metadata_path: None,
            metadata_size_bytes: None,
            metadata_modified_unix_ms: None,
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        })
        .await
        .expect("fingerprint upsert");

        let output_dir = tempfile::tempdir().expect("output dir");
        let media_dirs = resolve_media_dirs(output_dir.path(), false);
        std::fs::create_dir_all(&media_dirs.covers_dir).expect("covers dir");
        let cover_path = media_dirs.covers_dir.join(format!("{item_id}.webp"));
        std::fs::write(&cover_path, b"cover").expect("cover write");

        assert!(
            delete_item_and_media(&repo, item_id, &media_dirs, false, false, "test removal").await
        );

        assert!(repo.get_item(item_id).await.expect("get item").is_none());
        assert!(cover_path.exists());
    }
}
