use super::AmfDb;
use crate::screenshot_evidence::EvidenceOwner;
use anyhow::{Result, ensure};
use rusqlite::{OptionalExtension, params};

impl AmfDb {
    pub(crate) fn evidence_scopes(
        &self,
        include_retired: bool,
    ) -> Result<Vec<(EvidenceOwner, bool)>> {
        let mut statement = self.conn.prepare("SELECT owner_json, retired FROM screenshot_scopes WHERE retired=0 OR ?1 ORDER BY scope_id")?;
        let rows = statement.query_map([include_retired], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))
        })?;
        rows.map(|row| {
            let (json, retired) = row?;
            let owner: EvidenceOwner = serde_json::from_str(&json)?;
            owner.validate()?;
            Ok((owner, retired))
        })
        .collect()
    }
    pub(crate) fn register_evidence(&self, proposed: EvidenceOwner) -> Result<EvidenceOwner> {
        proposed.validate()?;
        let tx = self.conn.unchecked_transaction()?;
        let json:Option<String>=tx.query_row("SELECT owner_json FROM screenshot_scopes WHERE project_id=?1 AND feature_id=?2 AND session_id=?3 AND workdir=?4 AND retired=0",params![proposed.project_id,proposed.feature_id,proposed.session_id,proposed.workdir.to_string_lossy()],|row|row.get(0)).optional()?;
        let owner = if let Some(json) = json {
            serde_json::from_str(&json)?
        } else {
            tx.execute("INSERT INTO screenshot_scopes(scope_id,project_id,feature_id,session_id,workdir,owner_json) VALUES(?1,?2,?3,?4,?5,?6)",params![proposed.scope_id,proposed.project_id,proposed.feature_id,proposed.session_id,proposed.workdir.to_string_lossy(),serde_json::to_string(&proposed)?])?;
            proposed
        };
        crate::screenshot_evidence::ensure_directory(&owner)?;
        tx.commit()?;
        Ok(owner)
    }
    pub(crate) fn evidence_scope_active(&self, id: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT retired=0 FROM screenshot_scopes WHERE scope_id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }
    pub(crate) fn retire_evidence(&self, id: &str) -> Result<()> {
        ensure!(
            self.conn.execute(
                "UPDATE screenshot_scopes SET retired=1 WHERE scope_id=?1",
                [id]
            )? == 1,
            "Unknown screenshot scope"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_scopes_survive_store_deletion_and_retirement_does_not_reassign() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("db");
        let owner = EvidenceOwner {
            version: 1,
            scope_id: "scope".into(),
            project_id: "project".into(),
            feature_id: "feature".into(),
            session_id: "session".into(),
            project_name: "Project".into(),
            feature_name: "Feature".into(),
            session_label: "Codex 1".into(),
            // Canonical, as production stores it: macOS temp dirs sit behind a symlink.
            workdir: temp.path().canonicalize().unwrap(),
            is_worktree: false,
            created_at: chrono::Utc::now(),
        };
        let db = AmfDb::open(&path).unwrap();
        db.register_evidence(owner.clone()).unwrap();
        db.save_store(&crate::project::ProjectStore::empty())
            .unwrap();
        drop(db);
        let db = AmfDb::open(&path).unwrap();
        assert_eq!(db.evidence_scopes(false).unwrap()[0].0, owner);
        db.retire_evidence("scope").unwrap();
        assert!(!db.evidence_scope_active("scope").unwrap());
        let mut next = owner;
        next.scope_id = "new-scope".into();
        assert_eq!(db.register_evidence(next).unwrap().scope_id, "new-scope");
        assert_eq!(db.evidence_scopes(true).unwrap().len(), 2);
    }
}
