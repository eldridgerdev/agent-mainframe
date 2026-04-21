use anyhow::Result;
use rusqlite::{Connection, params};

use crate::token_tracking::{
    DbTokenCacheEntry, SessionTokenUsage, TokenUsageProvider, TokenUsageSource,
};

fn provider_to_str(p: &TokenUsageProvider) -> &'static str {
    match p {
        TokenUsageProvider::Claude => "claude",
        TokenUsageProvider::Opencode => "opencode",
        TokenUsageProvider::Codex => "codex",
    }
}

fn provider_from_str(s: &str) -> TokenUsageProvider {
    match s {
        "opencode" => TokenUsageProvider::Opencode,
        "codex" => TokenUsageProvider::Codex,
        _ => TokenUsageProvider::Claude,
    }
}

pub fn load(conn: &Connection) -> Result<Vec<DbTokenCacheEntry>> {
    let mut stmt = conn.prepare(
        "SELECT source_provider, source_id, signature,
                has_usage, input_tokens, output_tokens,
                cache_read_tokens, cache_write_tokens,
                reasoning_tokens, total_tokens
         FROM token_usage_cache",
    )?;

    let entries = stmt
        .query_map([], |row| {
            let provider = provider_from_str(&row.get::<_, String>(0)?);
            let id: String = row.get(1)?;
            let signature: Option<i64> = row.get(2)?;
            let has_usage: bool = row.get(3)?;
            let input_tokens: i64 = row.get(4)?;
            let output_tokens: i64 = row.get(5)?;
            let cache_read_tokens: i64 = row.get(6)?;
            let cache_write_tokens: i64 = row.get(7)?;
            let reasoning_tokens: i64 = row.get(8)?;
            let total_tokens: i64 = row.get(9)?;

            let source = TokenUsageSource {
                provider: provider.clone(),
                id: id.clone(),
            };
            let usage = if has_usage {
                Some(SessionTokenUsage {
                    source: TokenUsageSource { provider, id },
                    input_tokens: input_tokens as u64,
                    output_tokens: output_tokens as u64,
                    cache_read_tokens: cache_read_tokens as u64,
                    cache_write_tokens: cache_write_tokens as u64,
                    reasoning_tokens: reasoning_tokens as u64,
                    total_tokens: total_tokens as u64,
                })
            } else {
                None
            };

            Ok(DbTokenCacheEntry {
                source,
                signature: signature.map(|s| s as u64),
                usage,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(entries)
}

pub fn save(conn: &Connection, entries: &[DbTokenCacheEntry]) -> Result<()> {
    conn.execute_batch("BEGIN IMMEDIATE;")?;
    match do_save(conn, entries) {
        Ok(()) => {
            conn.execute_batch("COMMIT;")?;
            Ok(())
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK;");
            Err(e)
        }
    }
}

fn do_save(conn: &Connection, entries: &[DbTokenCacheEntry]) -> Result<()> {
    for entry in entries {
        let (has_usage, input, output, cache_read, cache_write, reasoning, total) =
            match &entry.usage {
                Some(u) => (
                    1i32,
                    u.input_tokens as i64,
                    u.output_tokens as i64,
                    u.cache_read_tokens as i64,
                    u.cache_write_tokens as i64,
                    u.reasoning_tokens as i64,
                    u.total_tokens as i64,
                ),
                None => (0i32, 0, 0, 0, 0, 0, 0),
            };

        conn.execute(
            "INSERT OR REPLACE INTO token_usage_cache (
                source_provider, source_id, signature, has_usage,
                input_tokens, output_tokens, cache_read_tokens,
                cache_write_tokens, reasoning_tokens, total_tokens, updated_at
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,datetime('now'))",
            params![
                provider_to_str(&entry.source.provider),
                entry.source.id,
                entry.signature.map(|s| s as i64),
                has_usage,
                input,
                output,
                cache_read,
                cache_write,
                reasoning,
                total,
            ],
        )?;
    }
    Ok(())
}

pub fn evict_stale(conn: &Connection) -> Result<()> {
    conn.execute(
        "DELETE FROM token_usage_cache
         WHERE updated_at < datetime('now', '-7 days')",
        [],
    )?;
    Ok(())
}
