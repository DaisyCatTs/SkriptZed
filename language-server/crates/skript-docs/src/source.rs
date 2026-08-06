//! Where the syntax database comes from.
//!
//! `docs.json` is generated from SkriptLang/Skript, which is **GPL-3.0**. This
//! project is MIT, so the database is never vendored: it is downloaded on first
//! use and cached, exactly the way Zed itself handles language server binaries.
//! That also means the docs always match the Skript version the user targets,
//! instead of whatever was current when the extension was released.
//!
//! Everything degrades rather than fails. No network on first run leaves the
//! server working with tree-sitter highlighting and a small built-in fallback;
//! a corrupt cache is discarded and refetched.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::model::Docs;

/// The current release, tracking whatever Skript version is newest.
pub const LATEST_URL: &str = "https://docs.skriptlang.org/docs.json";

/// SkriptHub's addon syntax catalog. Keyless, and the only comprehensive source
/// of addon syntax that exists.
pub const SKRIPTHUB_URL: &str = "https://skripthub.net/api/v1/addonsyntaxlist/";

/// The oldest archive that is actually machine-readable.
///
/// Every published archive from 2.6.4 through 2.9.5, plus 2.10.2, contains
/// `""region name""` in the `region` type's pattern list — invalid JSON. Asking
/// for one of those should say so plainly rather than surfacing a parse error
/// from somewhere deep in serde.
pub const OLDEST_USABLE_ARCHIVE: crate::version::Version = crate::version::Version::new(2, 10, 0);

/// How long a cached copy is trusted before we re-check for a newer one.
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Refuse absurdly large downloads rather than filling the user's disk. The
/// real file is ~1.1 MB; addon dumps can reach ~8 MB.
const MAX_DOWNLOAD_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug)]
pub enum LoadError {
    Io(std::io::Error),
    Http(String),
    Parse(serde_json::Error),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Io(error) => write!(f, "{error}"),
            LoadError::Http(error) => write!(f, "{error}"),
            LoadError::Parse(error) => write!(f, "malformed docs.json: {error}"),
        }
    }
}

impl std::error::Error for LoadError {}

/// Where to get the database from.
#[derive(Debug, Clone)]
pub enum DocsSource {
    /// The newest published release.
    Latest,
    /// A specific Skript version, from the published archives.
    Version(String),
    /// A file the user generated themselves with `/sk gen-docs`, which pins the
    /// catalog to their server's exact Skript build.
    ///
    /// This covers Skript itself only. `/sk gen-docs` runs
    /// `JSONGenerator.of(Skript.instance())`, scoped to a single addon, so a
    /// generated file never carries a third-party addon's syntax.
    Local(PathBuf),
    /// An arbitrary URL, for a private mirror.
    Url(String),
    /// SkriptHub's addon catalog. A different schema from Skript's own.
    SkriptHub,
    /// A user-supplied syntax file, in either schema. The shape is sniffed.
    Custom(PathBuf),
}

impl DocsSource {
    pub fn url(&self) -> Option<String> {
        match self {
            DocsSource::Latest => Some(LATEST_URL.to_string()),
            DocsSource::Version(version) => Some(format!(
                "https://docs.skriptlang.org/archives/{version}/docs.json"
            )),
            DocsSource::Url(url) => Some(url.clone()),
            DocsSource::SkriptHub => Some(SKRIPTHUB_URL.to_string()),
            DocsSource::Local(_) | DocsSource::Custom(_) => None,
        }
    }

    /// Rejects archives that are known not to parse, with a message that
    /// explains why rather than failing later inside the deserializer.
    pub fn validate(&self) -> Result<(), LoadError> {
        let DocsSource::Version(version) = self else {
            return Ok(());
        };
        let Some(parsed) = crate::version::Version::parse(version) else {
            return Err(LoadError::Http(format!(
                "{version:?} is not a Skript version number"
            )));
        };
        if parsed < OLDEST_USABLE_ARCHIVE {
            return Err(LoadError::Http(format!(
                "Skript {version} cannot be used: every published archive below \
                 {OLDEST_USABLE_ARCHIVE} is invalid JSON upstream (the `region` \
                 type contains an unescaped quote). The oldest usable version is \
                 {OLDEST_USABLE_ARCHIVE}."
            )));
        }
        Ok(())
    }

    /// A stable filename for this source's cache entry.
    fn cache_key(&self) -> String {
        match self {
            DocsSource::Latest => "docs-latest.json".to_string(),
            DocsSource::Version(version) => format!("docs-{}.json", sanitize(version)),
            DocsSource::Url(url) => format!("docs-{:016x}.json", hash(url)),
            DocsSource::SkriptHub => "skripthub-addons.json".to_string(),
            DocsSource::Local(path) => format!("docs-local-{:016x}.json", hash(&path.display())),
            DocsSource::Custom(path) => format!("docs-custom-{:016x}.json", hash(&path.display())),
        }
    }
}

/// Loads the database, using the cache when it is fresh.
pub fn load(source: &DocsSource, cache_dir: &Path) -> Result<Docs, LoadError> {
    source.validate()?;

    if let DocsSource::Local(path) | DocsSource::Custom(path) = source {
        let text = fs::read_to_string(path).map_err(LoadError::Io)?;
        return parse_any(&text);
    }

    let cache_path = cache_dir.join(source.cache_key());

    if let Some(docs) = read_fresh_cache(&cache_path) {
        return Ok(docs);
    }

    let url = source.url().expect("non-local sources always have a URL");
    match download(&url) {
        Ok(text) => {
            let docs = parse_for(source, &text)?;
            // Only cache what parsed — a truncated download must not poison the
            // cache for the next 24 hours.
            let _ = fs::create_dir_all(cache_dir);
            let _ = fs::write(&cache_path, &text);
            Ok(docs)
        }
        Err(error) => {
            // A stale cache beats no docs at all.
            match read_any_cache(&cache_path) {
                Some(docs) => Ok(docs),
                None => Err(error),
            }
        }
    }
}

/// Fetches a source's raw text, using the cache when it is fresh.
///
/// Exposed because the SkriptHub catalog is filtered by addon *before* it is
/// converted: parsing all 8,210 records and discarding most of them wastes the
/// bulk of the work, and the caller is the only thing that knows which addons
/// were detected.
pub fn fetch_text(source: &DocsSource, cache_dir: &Path) -> Result<String, LoadError> {
    source.validate()?;

    if let DocsSource::Local(path) | DocsSource::Custom(path) = source {
        return fs::read_to_string(path).map_err(LoadError::Io);
    }

    let cache_path = cache_dir.join(source.cache_key());

    if let Some(text) = read_fresh_text(&cache_path) {
        return Ok(text);
    }

    let url = source.url().expect("non-local sources always have a URL");
    match download(&url) {
        Ok(text) => {
            // Only cache what parses. A truncated body or a 200-OK error page
            // written straight to disk would be served as "fresh" for the next
            // 24 hours, leaving syntax broken for a day with no retry.
            if serde_json::from_str::<serde_json::Value>(&text).is_err() {
                return Err(LoadError::Http(format!(
                    "{url} returned {} bytes that are not valid JSON — not caching it",
                    text.len()
                )));
            }
            let _ = fs::create_dir_all(cache_dir);
            let _ = fs::write(&cache_path, &text);
            Ok(text)
        }
        // A stale cache beats nothing at all.
        Err(error) => fs::read_to_string(&cache_path).map_err(|_| error),
    }
}

fn read_fresh_text(path: &Path) -> Option<String> {
    let metadata = fs::metadata(path).ok()?;
    let age = metadata.modified().ok()?.elapsed().ok()?;
    (age <= CACHE_TTL).then(|| fs::read_to_string(path).ok())?
}

/// Parses text according to the schema its source publishes.
fn parse_for(source: &DocsSource, text: &str) -> Result<Docs, LoadError> {
    match source {
        DocsSource::SkriptHub => crate::skripthub::parse_filtered(text, |_| true)
            .map(|catalog| catalog.docs)
            .map_err(LoadError::Parse),
        _ => Docs::parse(text).map_err(LoadError::Parse),
    }
}

/// Parses a file whose schema is not known ahead of time.
///
/// The two shapes are trivially distinguishable: Skript's database is a JSON
/// object, SkriptHub's catalog is a JSON array.
pub fn parse_any(text: &str) -> Result<Docs, LoadError> {
    if text.trim_start().starts_with('[') {
        crate::skripthub::parse_filtered(text, |_| true)
            .map(|catalog| catalog.docs)
            .map_err(LoadError::Parse)
    } else {
        Docs::parse(text).map_err(LoadError::Parse)
    }
}

fn read_fresh_cache(path: &Path) -> Option<Docs> {
    let metadata = fs::metadata(path).ok()?;
    let age = metadata.modified().ok()?.elapsed().ok()?;
    if age > CACHE_TTL {
        return None;
    }
    read_any_cache(path)
}

fn read_any_cache(path: &Path) -> Option<Docs> {
    let text = fs::read_to_string(path).ok()?;
    // The cache directory holds both schemas, so sniff rather than assume.
    parse_any(&text).ok()
}

fn download(url: &str) -> Result<String, LoadError> {
    let response = ureq::get(url)
        .timeout(Duration::from_secs(30))
        .call()
        .map_err(|error| LoadError::Http(format!("could not fetch {url}: {error}")))?;

    let mut text = String::new();
    // Read one byte past the cap so an overrun is detectable. Silently
    // truncating would hand back a body that looks complete but is not.
    response
        .into_reader()
        .take(MAX_DOWNLOAD_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(LoadError::Io)?;

    if text.len() as u64 > MAX_DOWNLOAD_BYTES {
        return Err(LoadError::Http(format!(
            "{url} is larger than the {MAX_DOWNLOAD_BYTES} byte limit"
        )));
    }

    Ok(text)
}

/// Makes a user-supplied version string safe to use as a filename.
///
/// The version comes from the user's Zed settings, so it is untrusted input
/// that ends up concatenated into a path. Separators become `_`, and any run of
/// dots is collapsed so no `..` component can survive.
fn sanitize(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        let ch = if ch.is_ascii_alphanumeric() || ch == '.' {
            ch
        } else {
            '_'
        };
        if ch == '.' && out.ends_with('.') {
            out.pop();
            out.push('_');
            continue;
        }
        out.push(ch);
    }
    out
}

fn hash(value: &impl std::fmt::Display) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    value.to_string().hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_archive_urls() {
        assert_eq!(DocsSource::Latest.url().unwrap(), LATEST_URL);
        assert_eq!(
            DocsSource::Version("2.15.3".into()).url().unwrap(),
            "https://docs.skriptlang.org/archives/2.15.3/docs.json"
        );
        assert!(DocsSource::Local("x.json".into()).url().is_none());
    }

    #[test]
    fn cache_keys_are_distinct_and_filesystem_safe() {
        let keys = [
            DocsSource::Latest.cache_key(),
            DocsSource::Version("2.15.3".into()).cache_key(),
            DocsSource::Version("../../etc/passwd".into()).cache_key(),
            DocsSource::Url("https://example.com/a.json".into()).cache_key(),
        ];
        for key in &keys {
            assert!(!key.contains('/') && !key.contains('\\') && !key.contains(".."));
        }
        let unique: std::collections::HashSet<_> = keys.iter().collect();
        assert_eq!(unique.len(), keys.len());
    }

    #[test]
    fn a_corrupt_cache_is_ignored_rather_than_returned() {
        let dir = std::env::temp_dir().join("skript-docs-test-corrupt");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join(DocsSource::Latest.cache_key());
        fs::write(&path, "{ this is not json").unwrap();
        assert!(read_any_cache(&path).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn loads_a_local_file() {
        let dir = std::env::temp_dir().join("skript-docs-test-local");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("docs.json");
        fs::write(&path, r#"{"source":{"name":"Skript","version":"2.16.1"}}"#).unwrap();

        let docs = load(&DocsSource::Local(path), &dir).unwrap();
        assert_eq!(docs.source.version, "2.16.1");
        let _ = fs::remove_dir_all(&dir);
    }
}
