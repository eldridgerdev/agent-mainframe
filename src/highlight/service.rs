use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};

use super::detect::{HighlightLanguage, detect_language};
use super::model::{HighlightRequest, HighlightedLine, HighlightedText};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CacheKey {
    language: Option<HighlightLanguage>,
    source_hash: u64,
}

static CACHE: OnceLock<Mutex<HashMap<CacheKey, HighlightedText>>> = OnceLock::new();

pub fn highlight_source(request: HighlightRequest<'_>) -> HighlightedText {
    let language = detect_language(request.path, request.language_hint, request.source);
    let key = CacheKey {
        language,
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
