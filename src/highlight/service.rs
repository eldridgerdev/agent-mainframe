use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::UNIX_EPOCH;

use super::detect::{HighlightLanguage, detect_language};
use super::model::{HighlightRequest, HighlightedLine, HighlightedText};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CacheKey {
    language: Option<HighlightLanguage>,
    parser_state_hash: u64,
    source_hash: u64,
}

static CACHE: OnceLock<Mutex<HashMap<CacheKey, HighlightedText>>> = OnceLock::new();

pub fn highlight_source(request: HighlightRequest<'_>) -> HighlightedText {
    let language = detect_language(request.path, request.language_hint, request.source);
    let key = CacheKey {
        language,
        parser_state_hash: parser_state_hash(language),
        source_hash: hash_text(request.source),
    };

    if let Some(cached) = cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(&key).cloned())
    {
        return cached;
    }

    let highlighted = match language {
        Some(language) => super::tree_sitter::highlight_source(language, request.source)
            .unwrap_or_else(|_| {
                HighlightedText::plain(Some(language.display_name().to_string()), request.source)
            }),
        None => HighlightedText::plain(None, request.source),
    };

    if let Ok(mut cache) = cache().lock() {
        cache.insert(key, highlighted.clone());
    }

    highlighted
}

pub fn highlight_line(
    path: Option<&std::path::Path>,
    language_hint: Option<&str>,
    source: &str,
) -> HighlightedLine {
    highlight_source(HighlightRequest {
        path,
        language_hint,
        source,
    })
    .lines
    .into_iter()
    .next()
    .unwrap_or_default()
}

fn cache() -> &'static Mutex<HashMap<CacheKey, HighlightedText>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn clear_cache() {
    if let Ok(mut cache) = cache().lock() {
        cache.clear();
    }
}

fn hash_text(source: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    hasher.finish()
}

fn parser_state_hash(language: Option<HighlightLanguage>) -> u64 {
    let Some(language) = language else {
        return 0;
    };

    let mut hasher = DefaultHasher::new();
    match language.install_state() {
        super::detect::HighlightInstallState::Available => 0u8,
        super::detect::HighlightInstallState::Installed => 1u8,
        super::detect::HighlightInstallState::Broken => 2u8,
    }
    .hash(&mut hasher);
    hash_path_state(language.source_dir().as_path()).hash(&mut hasher);
    hash_path_state(language.library_path().as_path()).hash(&mut hasher);
    hasher.finish()
}

fn hash_path_state(path: &Path) -> u64 {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);

    match std::fs::metadata(path) {
        Ok(metadata) => {
            true.hash(&mut hasher);
            metadata.len().hash(&mut hasher);
            if let Ok(modified) = metadata.modified()
                && let Ok(duration) = modified.duration_since(UNIX_EPOCH)
            {
                duration.as_secs().hash(&mut hasher);
                duration.subsec_nanos().hash(&mut hasher);
            }
        }
        Err(_) => {
            false.hash(&mut hasher);
        }
    }

    hasher.finish()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn path_state_hash_changes_after_file_update() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("parser.so");

        std::fs::write(&path, "old").unwrap();
        let first = hash_path_state(&path);

        std::thread::sleep(Duration::from_millis(5));
        std::fs::write(&path, "newer-parser").unwrap();
        let second = hash_path_state(&path);

        assert_ne!(first, second);
    }
}
