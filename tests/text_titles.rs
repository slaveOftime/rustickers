#![cfg(feature = "storage")]
use anyhow::Context as _;
use futures::executor::block_on;
use rustickers::text::{TextEntryId, TextRepository};
#[test]
fn required_titles_are_searchable_and_survive_edits_and_protection() -> anyhow::Result<()> {
    block_on(async {
        let dir = tempfile::tempdir()?;
        let repo = TextRepository::open(dir.path().join("stickers.db")).await?;
        repo.set_master_password("test-password").await?;
        let capture = repo
            .capture("private content", 1)
            .await?
            .capture_id()
            .context("expected a fresh clipboard capture")?;
        let entry = repo.get(TextEntryId::Clipboard(capture)).await?;
        assert!(
            repo.promote_titled(&entry, true, Some(" \t "))
                .await
                .is_err()
        );
        assert!(repo.get(entry.id()).await.is_ok());
        let id = repo
            .promote_titled(&entry, true, Some("Account notes"))
            .await?;
        let entry = repo.get(TextEntryId::Sticker(id)).await?;
        assert_eq!(entry.title(), "Account notes");
        assert_eq!(entry.multiline_preview(), "Account notes");
        assert!(!entry.multiline_preview().contains("private"));
        assert!(entry.matches("account"));
        assert!(!entry.matches("private"));
        repo.save_text(&entry, "updated private content").await?;
        let entry = repo.get(entry.id()).await?;
        assert_eq!(entry.title(), "Account notes");
        repo.update_title(entry.id(), "Renamed notes").await?;
        assert!(repo.update_title(entry.id(), "  ").await.is_err());
        // A draft acquired before rename must not replace the new title.
        repo.save_text(&entry, "another edit").await?;
        let entry = repo.get(entry.id()).await?;
        assert_eq!(entry.title(), "Renamed notes");
        repo.set_encryption(&entry, false, Some("test-password"))
            .await?;
        let entry = repo.get(entry.id()).await?;
        repo.save_text(&entry, "plaintext edit").await?;
        let entry = repo.get(entry.id()).await?;
        assert_eq!(entry.title(), "Renamed notes");
        assert!(
            repo.set_encryption_titled(&entry, true, None, Some(""))
                .await
                .is_err()
        );
        repo.set_encryption_titled(&entry, true, None, Some("Locked notes"))
            .await?;
        assert_eq!(repo.get(entry.id()).await?.title(), "Locked notes");
        Ok(())
    })
}
