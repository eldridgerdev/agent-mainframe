use super::AmfDb;
use crate::model_evidence::ResearchNote;
use anyhow::{Result, ensure};
use rusqlite::params;
use std::path::Path;

impl AmfDb {
    pub(crate) fn save_model_research(
        &self,
        project_id: &str,
        repo: &Path,
        notes: &[ResearchNote],
    ) -> Result<()> {
        ensure!(
            !project_id.is_empty() && !repo.as_os_str().is_empty(),
            "missing project evidence identity"
        );
        for note in notes {
            note.validate_provenance()?;
        }
        let tx = self.conn.unchecked_transaction()?;
        for note in notes {
            tx.execute("INSERT INTO model_research (project_id,repo,evidence_id,provenance_json) VALUES (?1,?2,?3,?4) ON CONFLICT(project_id,repo,evidence_id) DO UPDATE SET provenance_json=excluded.provenance_json", params![project_id,repo.to_string_lossy(),note.id,serde_json::to_string(note)?])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn load_model_research(
        &self,
        project_id: &str,
        repo: &Path,
    ) -> Result<Vec<ResearchNote>> {
        let mut stmt = self.conn.prepare("SELECT provenance_json FROM model_research WHERE project_id=?1 AND repo=?2 ORDER BY evidence_id")?;
        let rows = stmt.query_map(params![project_id, repo.to_string_lossy()], |row| {
            row.get::<_, String>(0)
        })?;
        let mut notes = vec![];
        for row in rows {
            let note: ResearchNote = serde_json::from_str(&row?)?;
            note.validate_provenance()?;
            notes.push(note);
        }
        Ok(notes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_evidence::research_notes;
    #[test]
    fn provenance_reopens_and_is_reused_only_in_same_project() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let repo = dir.path().join("repo");
        {
            let db = AmfDb::open(&path).unwrap();
            // Simulate an existing installation before new model research.
            db.save_model_research("project", &repo, &research_notes()[..2])
                .unwrap();
        }
        let db = AmfDb::open(&path).unwrap();
        db.save_model_research("project", &repo, &research_notes())
            .unwrap();
        drop(db);
        let db = AmfDb::open(&path).unwrap();
        // Loading orders by evidence ID; the registry is in review order.
        let mut expected = research_notes();
        expected.sort_by(|a, b| a.id.cmp(&b.id));
        assert_eq!(db.load_model_research("project", &repo).unwrap(), expected);
        assert!(db.load_model_research("other", &repo).unwrap().is_empty());
        assert!(
            db.load_model_research("project", &dir.path().join("other"))
                .unwrap()
                .is_empty()
        );
        // No feature identity: any feature in this project loads the same notes.
        db.save_model_research("project", &repo, &research_notes())
            .unwrap();
        assert_eq!(
            db.load_model_research("project", &repo).unwrap().len(),
            research_notes().len()
        );
    }
    #[test]
    fn write_failures_and_invented_provenance_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let db = AmfDb::open(&path).unwrap();
        let mut notes = research_notes();
        notes[0].statement = "generated claim".into();
        assert!(db.save_model_research("p", dir.path(), &notes).is_err());
        let ro = AmfDb::open_read_only(&path).unwrap();
        assert!(
            ro.save_model_research("p", dir.path(), &research_notes())
                .is_err()
        );
        db.conn
            .execute(
                "INSERT INTO model_research VALUES ('p',?1,'fake','{}')",
                [dir.path().to_string_lossy()],
            )
            .unwrap();
        assert!(db.load_model_research("p", dir.path()).is_err());
    }
    #[test]
    fn upgrades_existing_schema_without_replacing_project_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let db = AmfDb::open(&path).unwrap();
        db.conn.execute_batch("DROP TABLE screenshot_scopes; DROP TABLE model_research; ALTER TABLE feature_sessions DROP COLUMN stopped; DELETE FROM schema_version WHERE version>=44; CREATE TABLE upgrade_sentinel(value TEXT); INSERT INTO upgrade_sentinel VALUES ('keep');").unwrap();
        drop(db);
        let db = AmfDb::open(&path).unwrap();
        assert_eq!(
            db.conn
                .query_row("SELECT value FROM upgrade_sentinel", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "keep"
        );
        db.save_model_research("p", dir.path(), &research_notes())
            .unwrap();
    }
}
