mod migrations;
pub mod store;

use anyhow::Result;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

pub struct AmfDb {
    conn: Connection,
    pub path: PathBuf,
}

impl AmfDb {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        migrations::run(&conn)?;
        Ok(Self {
            conn,
            path: path.to_path_buf(),
        })
    }

    /// Open `path`, seeding it first if it does not exist.
    ///
    /// Seeding priority:
    /// 1. Copy all data from `global_path` (worktree-local isolation).
    /// 2. Import from a `projects.json` file in the same directory.
    /// 3. Import from the global `~/.config/amf/projects.json`.
    pub fn open_or_seed(path: &Path, global_path: &Path) -> Result<Self> {
        if !path.exists() {
            if path != global_path && global_path.exists() {
                seed_from_db(path, global_path);
            } else {
                seed_from_json(path);
            }
        }
        Self::open(path)
    }

    pub fn load_store(&self) -> Result<crate::project::ProjectStore> {
        store::load(&self.conn)
    }

    pub fn save_store(&self, store: &crate::project::ProjectStore) -> Result<()> {
        store::save(&self.conn, store)
    }
}

fn seed_from_db(dest: &Path, source: &Path) {
    let Ok(source_db) = AmfDb::open(source) else {
        return;
    };
    let Ok(store) = source_db.load_store() else {
        return;
    };
    if let Ok(dest_db) = AmfDb::open(dest) {
        let _ = dest_db.save_store(&store);
    }
}

fn seed_from_json(db_path: &Path) {
    let json_candidates: Vec<PathBuf> = vec![
        db_path
            .parent()
            .map(|p| p.join("projects.json"))
            .unwrap_or_default(),
        dirs::config_dir()
            .unwrap_or_default()
            .join("amf")
            .join("projects.json"),
        dirs::config_dir()
            .unwrap_or_default()
            .join("claude-super-vibeless")
            .join("projects.json"),
    ];

    for json_path in json_candidates {
        if json_path.exists() {
            if let Ok(store) = crate::project::ProjectStore::load(&json_path) {
                if let Ok(db) = AmfDb::open(db_path) {
                    let _ = db.save_store(&store);
                }
            }
            return;
        }
    }
}
