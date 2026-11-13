//! Mechanical unexplained-change detectors. They run over the diff, cost no
//! tokens, and each hit with no decision covering its paths becomes an agenda
//! item.

use std::collections::BTreeSet;

use crate::diff::{DiffFile, DiffFileStatus, DiffLine, DiffLineKind};

use super::context::DecisionSummary;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DetectorKind {
    Dependency,
    Migration,
    NewModule,
    TestsDeleted,
    BuildConfig,
}

impl DetectorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DetectorKind::Dependency => "dependency",
            DetectorKind::Migration => "migration",
            DetectorKind::NewModule => "new_module",
            DetectorKind::TestsDeleted => "tests_deleted",
            DetectorKind::BuildConfig => "build_config",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectorHit {
    pub kind: DetectorKind,
    pub paths: Vec<String>,
    pub summary: String,
}

/// Run every detector. `base_dirs` lists the directories that exist at the
/// base commit (for example `src`, `src/db`), which is how a new top-level
/// directory is told from a new file in an old one.
pub fn detect(files: &[DiffFile], base_dirs: &[String]) -> Vec<DetectorHit> {
    let mut hits = Vec::new();
    hits.extend(dependency_hits(files));
    hits.extend(migration_hits(files));
    hits.extend(new_module_hits(files, base_dirs));
    hits.extend(tests_deleted_hits(files));
    hits.extend(build_config_hits(files));
    hits
}

/// A hit is covered when some decision names a path that is, contains or sits
/// under one of the hit's paths. A dependency decision covers a dependency hit
/// the same way. Anything left is unexplained.
pub fn uncovered(hits: &[DetectorHit], decisions: &[DecisionSummary]) -> Vec<DetectorHit> {
    hits.iter()
        .filter(|hit| {
            !decisions.iter().any(|d| {
                d.paths
                    .iter()
                    .any(|dp| hit.paths.iter().any(|hp| paths_intersect(dp, hp)))
            })
        })
        .cloned()
        .collect()
}

/// Equal, or one is a directory prefix of the other.
pub(crate) fn paths_intersect(a: &str, b: &str) -> bool {
    let norm = |p: &str| p.trim_start_matches("./").trim_end_matches('/').to_owned();
    let (a, b) = (norm(a), norm(b));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a == b || a.starts_with(&format!("{b}/")) || b.starts_with(&format!("{a}/"))
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn is_new(file: &DiffFile) -> bool {
    matches!(
        file.status,
        DiffFileStatus::Added | DiffFileStatus::Untracked
    )
}

fn lines_of(file: &DiffFile, kind: DiffLineKind) -> impl Iterator<Item = &DiffLine> {
    file.hunks
        .iter()
        .flat_map(|h| h.lines.iter())
        .filter(move |l| l.kind == kind)
}

/// Where a manifest line sits: the TOML table header above it (TOML files
/// only) and the lines that opened each still-open `{`/`[` block, innermost
/// last. A closing bracket pops, so a key after a closed block is not
/// attributed to it.
#[derive(Debug, Default)]
struct Scope {
    toml: bool,
    header: Option<String>,
    open: Vec<String>,
}

impl Scope {
    fn new(path: &str) -> Self {
        Self {
            toml: basename(path).ends_with(".toml"),
            ..Self::default()
        }
    }

    /// The innermost open block's line, else the table header.
    fn opener(&self) -> Option<&str> {
        self.open
            .last()
            .or(self.header.as_ref())
            .map(String::as_str)
    }

    fn header(&self) -> Option<&str> {
        self.header.as_deref()
    }

    /// True when the line sits directly in its table, outside any bracket.
    fn at_table_level(&self) -> bool {
        self.open.is_empty()
    }

    fn feed(&mut self, line: &str) {
        if self.toml && is_table_header(line) {
            self.header = Some(line.trim().to_owned());
            self.open.clear();
            return;
        }
        for c in unquoted_brackets(line, self.toml) {
            match c {
                '{' | '[' => self.open.push(line.to_owned()),
                _ => {
                    self.open.pop();
                }
            }
        }
    }
}

/// `{`, `[`, `}` and `]` outside string literals, in order. In TOML a `#`
/// outside a string starts a comment.
fn unquoted_brackets(line: &str, toml: bool) -> Vec<char> {
    let mut out = Vec::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in line.chars() {
        match quote {
            Some(q) => {
                if escaped {
                    escaped = false;
                } else if c == '\\' && q == '"' {
                    escaped = true;
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                '#' if toml => break,
                '{' | '[' | '}' | ']' => out.push(c),
                _ => {}
            },
        }
    }
    out
}

/// A TOML `[table]` or `[[array.of.tables]]` header line.
fn is_table_header(text: &str) -> bool {
    let t = text.split(" #").next().unwrap_or(text).trim();
    let inner = t.trim_start_matches('[').trim_end_matches(']');
    t.starts_with('[')
        && t.ends_with(']')
        && !t.contains(',')
        && inner
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || "_-\"'".contains(c))
}

/// Trimmed lines of `kind` matching `want`, which sees the line's scope.
/// Each side of the diff is tracked separately: added lines against the new
/// file, removed lines against the old one, each seeded from that side's
/// full content when the hunk starts mid-file.
fn lines_in_scope(
    file: &DiffFile,
    kind: DiffLineKind,
    want: &dyn Fn(&Scope, &str) -> bool,
) -> Vec<String> {
    let old_side = kind == DiffLineKind::Removed;
    let (content, own) = if old_side {
        (file.old_content.as_deref(), DiffLineKind::Removed)
    } else {
        (file.new_content.as_deref(), DiffLineKind::Added)
    };
    let mut out = Vec::new();
    for hunk in &file.hunks {
        let mut scope = Scope::new(&file.path);
        let start = if old_side {
            hunk.old_start
        } else {
            hunk.new_start
        };
        for line in content
            .into_iter()
            .flat_map(str::lines)
            .take(start.saturating_sub(1))
        {
            scope.feed(line);
        }
        for line in &hunk.lines {
            if line.kind != DiffLineKind::Context && line.kind != own {
                continue;
            }
            if line.kind == kind && want(&scope, &line.text) {
                out.push(line.text.trim().to_owned());
            }
            scope.feed(&line.text);
        }
    }
    out
}

fn is_toml_key(text: &str) -> bool {
    let t = text.trim();
    !t.is_empty()
        && !t.starts_with('#')
        && t.split_once('=').is_some_and(|(k, _)| {
            k.trim()
                .chars()
                .all(|c| c.is_alphanumeric() || "-_.\"'".contains(c))
                && !k.trim().is_empty()
        })
}

type LineFilter = Box<dyn Fn(&Scope, &str) -> bool>;

/// Which lines of the manifest named `name` declare a dependency.
fn dependency_line_filter(name: &str) -> Option<LineFilter> {
    let filter: LineFilter = match name {
        "Cargo.toml" => Box::new(|scope, text| {
            let t = text.trim();
            // `[dependencies.foo]` added as its own table.
            if is_table_header(t) && t.contains("dependencies.") {
                return true;
            }
            is_toml_key(text)
                && scope.at_table_level()
                && scope.header().is_some_and(|h| h.contains("dependencies"))
        }),
        "package.json" => Box::new(|scope, text| {
            let t = text.trim();
            t.starts_with('"')
                && t.contains("\":")
                && scope.opener().is_some_and(|o| o.contains("ependencies\""))
        }),
        "pyproject.toml" => Box::new(|scope, text| {
            let t = text.trim();
            let Some(o) = scope.opener() else {
                return false;
            };
            o.contains("dependencies")
                && (is_toml_key(text) || t.starts_with('"') || t.starts_with('\''))
        }),
        "go.mod" => Box::new(|_, text| {
            let t = text.trim().trim_start_matches("require ").trim();
            !text.contains("// indirect")
                && t.split_whitespace()
                    .nth(1)
                    .is_some_and(|v| v.starts_with('v'))
                && t.split_whitespace()
                    .next()
                    .is_some_and(|m| m.contains('.') || m.contains('/'))
        }),
        "Gemfile" => Box::new(|_, text| {
            let t = text.trim();
            t.starts_with("gem \"") || t.starts_with("gem '")
        }),
        _ if name.starts_with("requirements") && name.ends_with(".txt") => Box::new(|_, text| {
            let t = text.trim();
            !t.is_empty() && !t.starts_with('#') && !t.starts_with('-')
        }),
        _ => return None,
    };
    Some(filter)
}

/// The package a dependency line names, lowercased: the key, quoted name,
/// module path or requirement name before any version or extras.
fn dependency_name(line: &str) -> Option<String> {
    let t = line.trim();
    if t.starts_with('[') {
        let (_, name) = t.trim_matches(['[', ']']).rsplit_once("dependencies.")?;
        return Some(name.trim_matches(['"', '\'']).to_ascii_lowercase());
    }
    let t = t
        .strip_prefix("require ")
        .or_else(|| t.strip_prefix("gem "))
        .unwrap_or(t)
        .trim_start()
        .trim_start_matches(['"', '\'']);
    let name: String = t
        .chars()
        .take_while(|c| !c.is_whitespace() && !"=<>!~[;:,\"'(".contains(*c))
        .collect();
    (!name.is_empty()).then(|| name.to_ascii_lowercase())
}

/// Dependency lines added to `file`, less those whose package also appears
/// on a removed dependency line: a version bump rewrites the line but adds
/// no dependency.
fn added_dependencies(file: &DiffFile) -> Vec<String> {
    let Some(want) = dependency_line_filter(basename(&file.path)) else {
        return Vec::new();
    };
    let removed: BTreeSet<String> = lines_in_scope(file, DiffLineKind::Removed, &want)
        .iter()
        .filter_map(|l| dependency_name(l))
        .collect();
    lines_in_scope(file, DiffLineKind::Added, &want)
        .into_iter()
        .filter(|l| dependency_name(l).is_none_or(|n| !removed.contains(&n)))
        .collect()
}

fn dependency_hits(files: &[DiffFile]) -> Vec<DetectorHit> {
    files
        .iter()
        .filter_map(|file| {
            let deps = added_dependencies(file);
            (!deps.is_empty()).then(|| DetectorHit {
                kind: DetectorKind::Dependency,
                paths: vec![file.path.clone()],
                summary: format!(
                    "{} gained {} dependency line(s): {}",
                    file.path,
                    deps.len(),
                    deps.iter().take(5).cloned().collect::<Vec<_>>().join("; ")
                ),
            })
        })
        .collect()
}

/// A database or API schema file by name or extension, not any path that
/// merely contains "schema".
fn is_schema_file(path: &str) -> bool {
    let name = basename(path).to_ascii_lowercase();
    matches!(
        name.as_str(),
        "schema.rb" | "schema.prisma" | "structure.sql"
    ) || [".sql", ".prisma", ".graphql", ".gql"]
        .iter()
        .any(|ext| name.ends_with(ext))
}

fn migration_hits(files: &[DiffFile]) -> Vec<DetectorHit> {
    files
        .iter()
        .filter_map(|file| {
            let lower = file.path.to_ascii_lowercase();
            let by_path = is_new(file) && (lower.contains("migrations/") || is_schema_file(&lower));
            let by_const = file.path == "src/db/migrations.rs"
                && lines_of(file, DiffLineKind::Added).any(|l| l.text.contains("MIGRATION_"));
            (by_path || by_const).then(|| DetectorHit {
                kind: DetectorKind::Migration,
                paths: vec![file.path.clone()],
                summary: if by_const {
                    format!("{} gained a MIGRATION_ constant", file.path)
                } else {
                    format!("{} is a new migration or schema file", file.path)
                },
            })
        })
        .collect()
}

/// The directory a new file introduces: its first component, or its second
/// under `src/`. Files directly in `src/` (other than `lib.rs`/`main.rs`) are
/// modules themselves. `None` for repo-root files and dot-directories.
fn module_of(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.split('/').collect();
    match parts.as_slice() {
        [] | [_] => None,
        [first, ..] if first.starts_with('.') => None,
        ["src", file] if !matches!(*file, "lib.rs" | "main.rs" | "mod.rs") => {
            Some(format!("src/{file}"))
        }
        ["src", dir, _, ..] => Some(format!("src/{dir}")),
        ["src", _] => None,
        [dir, ..] => Some((*dir).to_owned()),
    }
}

fn new_module_hits(files: &[DiffFile], base_dirs: &[String]) -> Vec<DetectorHit> {
    let base: BTreeSet<&str> = base_dirs.iter().map(|d| d.trim_end_matches('/')).collect();
    let mut found: Vec<(String, Vec<String>)> = Vec::new();
    for file in files.iter().filter(|f| is_new(f)) {
        let Some(module) = module_of(&file.path) else {
            continue;
        };
        // `src/foo.rs` names a file module; it is new simply by being added.
        let is_file_module = module.ends_with(".rs") || module.ends_with(".ts");
        if !is_file_module && base.contains(module.as_str()) {
            continue;
        }
        match found.iter_mut().find(|(m, _)| *m == module) {
            Some((_, paths)) => paths.push(file.path.clone()),
            None => found.push((module, vec![file.path.clone()])),
        }
    }
    found
        .into_iter()
        .map(|(module, paths)| DetectorHit {
            kind: DetectorKind::NewModule,
            summary: format!("New module or directory {module} ({} file(s))", paths.len()),
            paths,
        })
        .collect()
}

fn is_test_path(path: &str) -> bool {
    let name = basename(path);
    path.split('/').any(|c| c == "tests" || c == "test")
        || name == "tests.rs"
        || name.contains("_test")
        || name.contains(".test.")
        || name.contains(".spec.")
        || name.starts_with("test_")
}

fn is_test_attr(text: &str) -> bool {
    let t = text.trim();
    t == "#[test]" || (t.starts_with("#[") && t.ends_with("::test]"))
}

fn tests_deleted_hits(files: &[DiffFile]) -> Vec<DetectorHit> {
    let mut hits = Vec::new();
    let deleted: Vec<String> = files
        .iter()
        .filter(|f| f.status == DiffFileStatus::Deleted && is_test_path(&f.path))
        .map(|f| f.path.clone())
        .collect();
    if !deleted.is_empty() {
        hits.push(DetectorHit {
            kind: DetectorKind::TestsDeleted,
            summary: format!("{} test file(s) deleted", deleted.len()),
            paths: deleted.clone(),
        });
    }
    // Net `#[test]` removal across files that were not deleted outright.
    let (mut removed, mut added) = (0usize, 0usize);
    let mut paths = Vec::new();
    for file in files.iter().filter(|f| f.status != DiffFileStatus::Deleted) {
        let r = lines_of(file, DiffLineKind::Removed)
            .filter(|l| is_test_attr(&l.text))
            .count();
        let a = lines_of(file, DiffLineKind::Added)
            .filter(|l| is_test_attr(&l.text))
            .count();
        removed += r;
        added += a;
        if r > a {
            paths.push(file.path.clone());
        }
    }
    if removed > added {
        hits.push(DetectorHit {
            kind: DetectorKind::TestsDeleted,
            summary: format!("Net {} #[test] function(s) removed", removed - added),
            paths,
        });
    }
    hits
}

fn is_build_path(path: &str) -> bool {
    let name = basename(path);
    path.starts_with(".github/workflows/")
        || name == "build.rs"
        || name.starts_with("Dockerfile")
        || name == "Makefile"
        || name.eq_ignore_ascii_case("justfile")
}

fn cargo_build_lines(file: &DiffFile) -> bool {
    if basename(&file.path) != "Cargo.toml" {
        return false;
    }
    [DiffLineKind::Added, DiffLineKind::Removed]
        .into_iter()
        .any(|kind| {
            !lines_in_scope(file, kind, &|scope, text| {
                let header = |t: &str| t.starts_with("[profile") || t == "[features]";
                header(text.trim()) || scope.header().is_some_and(header)
            })
            .is_empty()
        })
}

fn build_config_hits(files: &[DiffFile]) -> Vec<DetectorHit> {
    files
        .iter()
        .filter(|f| is_build_path(&f.path) || cargo_build_lines(f))
        .map(|f| DetectorHit {
            kind: DetectorKind::BuildConfig,
            paths: vec![f.path.clone()],
            summary: format!("Build or CI configuration changed: {}", f.path),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::DiffHunk;

    fn line(kind: DiffLineKind, text: &str) -> DiffLine {
        DiffLine {
            kind,
            text: text.to_owned(),
        }
    }

    /// `body` lines: `+x` added, `-x` removed, ` x` context.
    fn file(path: &str, status: DiffFileStatus, body: &[&str]) -> DiffFile {
        let lines = body
            .iter()
            .map(|l| {
                let kind = match l.as_bytes()[0] {
                    b'+' => DiffLineKind::Added,
                    b'-' => DiffLineKind::Removed,
                    _ => DiffLineKind::Context,
                };
                line(kind, &l[1..])
            })
            .collect();
        DiffFile {
            old_path: None,
            path: path.to_owned(),
            status,
            additions: 0,
            deletions: 0,
            is_binary: false,
            old_content: None,
            new_content: None,
            patch: String::new(),
            hunks: vec![DiffHunk {
                header: "@@ -1 +1 @@".into(),
                old_start: 1,
                old_lines: 1,
                new_start: 1,
                new_lines: 1,
                lines,
            }],
        }
    }

    fn modified(path: &str, body: &[&str]) -> DiffFile {
        file(path, DiffFileStatus::Modified, body)
    }

    fn kinds(hits: &[DetectorHit]) -> Vec<DetectorKind> {
        hits.iter().map(|h| h.kind).collect()
    }

    #[test]
    fn cargo_dependency_added_but_not_package_metadata() {
        let yes = modified(
            "Cargo.toml",
            &[" [dependencies]", " serde = \"1\"", "+regex = \"1\""],
        );
        assert_eq!(kinds(&detect(&[yes], &[])), [DetectorKind::Dependency]);
        let no = modified("Cargo.toml", &[" [package]", "+version = \"0.2.0\""]);
        assert!(detect(&[no], &[]).is_empty());
        let table = modified("Cargo.toml", &["+[dependencies.tokio]", "+version = \"1\""]);
        assert_eq!(kinds(&detect(&[table], &[])), [DetectorKind::Dependency]);
    }

    #[test]
    fn cargo_section_seeds_from_new_content_for_a_mid_file_hunk() {
        let mut f = modified("Cargo.toml", &["+regex = \"1\""]);
        f.new_content = Some("[package]\nname = \"x\"\n[dev-dependencies]\nregex = \"1\"\n".into());
        f.hunks[0].new_start = 4;
        assert_eq!(kinds(&detect(&[f], &[])), [DetectorKind::Dependency]);
    }

    #[test]
    fn other_manifests() {
        let pkg = modified(
            "web/package.json",
            &[" \"dependencies\": {", "+\"left-pad\": \"1.0.0\""],
        );
        let pkg_no = modified("web/package.json", &[" \"scripts\": {", "+\"x\": \"y\""]);
        let py = modified(
            "pyproject.toml",
            &[" dependencies = [", "+\"requests>=2\","],
        );
        let go = modified("go.mod", &[" require (", "+github.com/a/b v1.2.3"]);
        let go_indirect = modified("go.mod", &["+github.com/a/c v1.2.3 // indirect"]);
        let req = modified("requirements-dev.txt", &["+pytest==8", "+# note"]);
        let gem = modified("Gemfile", &["+gem 'rails'"]);
        for f in [&pkg, &py, &go, &req, &gem] {
            assert_eq!(
                kinds(&detect(std::slice::from_ref(f), &[])),
                [DetectorKind::Dependency],
                "{}",
                f.path
            );
        }
        assert!(detect(&[pkg_no, go_indirect], &[]).is_empty());
    }

    #[test]
    fn version_bumps_are_not_new_dependencies() {
        let bumps = [
            modified(
                "Cargo.toml",
                &[" [dependencies]", "-regex = \"1.9\"", "+regex = \"1.10\""],
            ),
            modified("requirements.txt", &["-pytest==7", "+pytest==8"]),
            modified(
                "package.json",
                &[
                    " \"dependencies\": {",
                    "-\"@types/node\": \"^20\",",
                    "+\"@types/node\": \"^22\",",
                ],
            ),
            modified(
                "go.mod",
                &[
                    "-require github.com/a/b v1.2.3",
                    "+require github.com/a/b v1.3.0",
                ],
            ),
            modified(
                "Gemfile",
                &["-gem 'rails', '~> 7.0'", "+gem 'rails', '~> 7.1'"],
            ),
        ];
        for f in &bumps {
            assert!(
                detect(std::slice::from_ref(f), &[]).is_empty(),
                "{}",
                f.path
            );
        }
        // A bump next to a genuinely new dependency reports only the new one.
        let mixed = modified(
            "Cargo.toml",
            &[
                " [dependencies]",
                "-regex = \"1.9\"",
                "+regex = \"1.10\"",
                "+serde = \"1\"",
            ],
        );
        let hits = detect(&[mixed], &[]);
        assert_eq!(kinds(&hits), [DetectorKind::Dependency]);
        assert!(
            hits[0]
                .summary
                .contains("gained 1 dependency line(s): serde")
        );
    }

    #[test]
    fn closing_brackets_end_a_dependency_block() {
        // A top-level key after the dependencies object has closed.
        let pkg = modified(
            "package.json",
            &[
                " {",
                "   \"dependencies\": {",
                "     \"left-pad\": \"1.0.0\"",
                "   },",
                "+  \"version\": \"1.0.1\"",
                " }",
            ],
        );
        assert!(detect(&[pkg], &[]).is_empty());
        // A multi-line inline table does not hide later keys in the table.
        let cargo = modified(
            "Cargo.toml",
            &[
                " [dependencies]",
                " serde = { version = \"1\", features = [",
                "     \"derive\",",
                " ] }",
                "+regex = \"1\"",
            ],
        );
        let hits = detect(&[cargo], &[]);
        assert_eq!(kinds(&hits), [DetectorKind::Dependency]);
        assert!(hits[0].summary.ends_with("regex = \"1\""));
        // Brackets inside strings and comments do not move the scope.
        let quoted = modified(
            "Cargo.toml",
            &[" [dependencies]", " a = \"{[\" # [ {", "+b = \"1\""],
        );
        assert_eq!(kinds(&detect(&[quoted], &[])), [DetectorKind::Dependency]);
        // A later table ends the dependencies table.
        let later = modified(
            "Cargo.toml",
            &[" [dependencies]", " a = \"1\"", " [[bin]]", "+name = \"x\""],
        );
        assert!(detect(&[later], &[]).is_empty());
    }

    #[test]
    fn migrations_and_schema_files() {
        let sql = file("db/001.sql", DiffFileStatus::Added, &["+create table t();"]);
        let dir = file("app/migrations/0002_x.py", DiffFileStatus::Added, &["+x"]);
        let schema = file("api/schema.graphql", DiffFileStatus::Added, &["+type T"]);
        let konst = modified(
            "src/db/migrations.rs",
            &["+const MIGRATION_040: &str = \"\";"],
        );
        for f in [sql, dir, schema, konst] {
            assert_eq!(
                kinds(&detect(
                    std::slice::from_ref(&f),
                    &["db".into(), "app".into(), "api".into()]
                )),
                [DetectorKind::Migration],
                "{}",
                f.path
            );
        }
        for path in [
            "src/json_schema.rs",
            "docs/schema.md",
            "tests/schema_validation.py",
        ] {
            let f = file(path, DiffFileStatus::Added, &["+x"]);
            let hits = detect(&[f], &["src".into(), "docs".into(), "tests".into()]);
            assert!(!kinds(&hits).contains(&DetectorKind::Migration), "{path}");
        }
        let prisma = file(
            "prisma/schema.prisma",
            DiffFileStatus::Added,
            &["+model T {}"],
        );
        assert_eq!(
            kinds(&detect(&[prisma], &["prisma".into()])),
            [DetectorKind::Migration]
        );
        let edit = modified("db/001.sql", &["+select 1;"]);
        let other = modified("src/db/migrations.rs", &["+// comment"]);
        assert!(detect(&[edit, other], &[]).is_empty());
    }

    #[test]
    fn new_module_directory_versus_existing() {
        let added = [
            file("src/billing/mod.rs", DiffFileStatus::Added, &["+x"]),
            file("src/billing/pay.rs", DiffFileStatus::Added, &["+x"]),
            file("src/db/new.rs", DiffFileStatus::Added, &["+x"]),
            file("tools/run.sh", DiffFileStatus::Added, &["+x"]),
            file("README.md", DiffFileStatus::Added, &["+x"]),
            file("src/lib.rs", DiffFileStatus::Added, &["+x"]),
        ];
        let hits = detect(&added, &["src".into(), "src/db".into()]);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].paths, ["src/billing/mod.rs", "src/billing/pay.rs"]);
        assert_eq!(hits[1].paths, ["tools/run.sh"]);
        let module_file = detect(
            &[file("src/pricing.rs", DiffFileStatus::Added, &["+x"])],
            &["src".into()],
        );
        assert_eq!(kinds(&module_file), [DetectorKind::NewModule]);
        // Modified files never count as new.
        assert!(detect(&[modified("src/billing/x.rs", &["+y"])], &[]).is_empty());
    }

    #[test]
    fn deleted_tests_and_net_test_removal() {
        let gone = file("tests/api.rs", DiffFileStatus::Deleted, &["-x"]);
        let gone2 = file("src/app/tests.rs", DiffFileStatus::Deleted, &["-x"]);
        let src_gone = file("src/util.rs", DiffFileStatus::Deleted, &["-x"]);
        let hits = detect(&[gone, gone2, src_gone], &[]);
        assert_eq!(kinds(&hits), [DetectorKind::TestsDeleted]);
        assert_eq!(hits[0].paths, ["tests/api.rs", "src/app/tests.rs"]);

        let net = modified("src/a.rs", &["-#[test]", "-fn a() {}", "+fn a() {}"]);
        assert_eq!(kinds(&detect(&[net], &[])), [DetectorKind::TestsDeleted]);
        // Moved tests (removed and re-added) are not a loss.
        let moved = [
            modified("src/a.rs", &["-#[test]"]),
            modified("src/b.rs", &["+#[test]"]),
        ];
        assert!(detect(&moved, &[]).is_empty());
    }

    #[test]
    fn build_and_ci_config() {
        for path in [
            ".github/workflows/ci.yml",
            "build.rs",
            "Dockerfile",
            "docker/Dockerfile.dev",
            "Makefile",
            "justfile",
        ] {
            let f = modified(path, &["+x"]);
            assert_eq!(
                kinds(&detect(&[f], &[])),
                [DetectorKind::BuildConfig],
                "{path}"
            );
        }
        let profile = modified("Cargo.toml", &[" [profile.release]", "+lto = true"]);
        let features = modified("Cargo.toml", &["+[features]", "+gui = []"]);
        assert_eq!(kinds(&detect(&[profile], &[])), [DetectorKind::BuildConfig]);
        assert_eq!(
            kinds(&detect(&[features], &[])),
            [DetectorKind::BuildConfig]
        );
        let plain = modified("Cargo.toml", &[" [package]", "+version = \"1\""]);
        assert!(detect(&[plain, modified("src/x.rs", &["+y"])], &[]).is_empty());
    }

    fn decision(kind: &str, paths: &[&str]) -> DecisionSummary {
        DecisionSummary {
            seq: 1,
            kind: kind.into(),
            title: "t".into(),
            paths: paths.iter().map(|p| p.to_string()).collect(),
        }
    }

    #[test]
    fn uncovered_filters_by_path_intersection() {
        let files = [
            modified("Cargo.toml", &[" [dependencies]", "+regex = \"1\""]),
            file("src/billing/mod.rs", DiffFileStatus::Added, &["+x"]),
            modified(".github/workflows/ci.yml", &["+x"]),
        ];
        let hits = detect(&files, &[]);
        assert_eq!(hits.len(), 3);
        // Exact path covers the dependency hit; a directory covers the module.
        let decisions = [
            decision("dependency", &["Cargo.toml"]),
            decision("architecture", &["src/billing"]),
        ];
        let left = uncovered(&hits, &decisions);
        assert_eq!(kinds(&left), [DetectorKind::BuildConfig]);
        assert_eq!(uncovered(&hits, &[]).len(), 3);
        // "src/bill" is not a directory prefix of "src/billing".
        assert!(!paths_intersect("src/bill", "src/billing/mod.rs"));
        assert!(paths_intersect("src/billing/", "src/billing/mod.rs"));
    }
}
