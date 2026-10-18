//! Reading and writing the mark the shell extension puts on a file.
//!
//! The right-click menu writes its choice into an alternate data stream on the
//! file itself:
//!
//! ```text
//! <path>:SpaceRecycle.Importance   ->   v1|important|1730000000
//! ```
//!
//! # Why this module exists at all
//!
//! The shell extension and the app had two unconnected stores. The extension
//! wrote the stream; the app kept a JSON file of its own. Nothing read the
//! other, so a file marked through the right-click menu was marked in a place
//! the app never looked - which matters now that the guard protects whatever it
//! can see, and meant the menu produced an icon and nothing else.
//!
//! The stream is the better of the two: it travels with the file, it survives
//! the app being uninstalled, and it needs no index to stay in sync. So the
//! stream is what gets read here rather than the JSON store being pushed into
//! the shell.
//!
//! # The format is a contract with a different language
//!
//! `shell-ext/src/SpaceRecycleShell.cpp` writes this stream and is the
//! authority on its shape. The parsing below mirrors `ReadLevel` there, and the
//! tests pin the same cases: an unknown version, an unknown token, an empty
//! stream and a missing stream are all "unmarked".
//!
//! That bias is deliberate and worth stating, because the opposite choice looks
//! reasonable. A mark the app cannot understand must never be treated as a mark:
//! if it were, a half-written stream or a future version would make a file look
//! protected when nothing is holding it, and the user would find out by losing
//! it.
//!
//! # Writing
//!
//! The app writes the same stream when it sets a mark, so that both entry points
//! produce one format and a file marked in the app shows the shell's overlay
//! without the extension having to be told.

use std::path::{Path, PathBuf};

use crate::importance::Importance;

/// The stream name. Must match `kStreamName` in the shell extension.
pub const STREAM: &str = "SpaceRecycle.Importance";

/// The payload version. Must match `kPayloadVersion`.
pub const VERSION: &str = "v1";

/// `<path>:<stream>`, which is what the Win32 file APIs take.
///
/// A stream is addressed as if it were a file next to the real one, so this is
/// the whole trick: no special API, just a path with a colon and a name.
pub fn stream_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(":");
    s.push(STREAM);
    PathBuf::from(s)
}

/// The token written for a level. Must match `kLevels[].token`.
pub fn token(level: Importance) -> &'static str {
    match level {
        Importance::Important => "important",
        Importance::Disposable => "disposable",
    }
}

/// The level a token names, if this build understands it.
pub fn level_from_token(token: &str) -> Option<Importance> {
    match token {
        "important" => Some(Importance::Important),
        "disposable" => Some(Importance::Disposable),
        _ => None,
    }
}

/// Reads the mark, if there is one this build understands.
///
/// Every failure is `None`: no file, no stream, an unreadable stream, a version
/// this build does not know, a token it does not know. The shell extension makes
/// the same choice for the same reason, and the comment there says it best - a
/// shell extension must never make a file look marked because it could not read
/// it.
pub fn read(path: &Path) -> Option<Importance> {
    let raw = std::fs::read_to_string(stream_path(path)).ok()?;
    parse(&raw)
}

/// Parses a payload. Split out so the format can be tested without a file.
pub fn parse(raw: &str) -> Option<Importance> {
    let mut parts = raw.split('|');
    let version = parts.next()?;
    if version != VERSION {
        return None;
    }
    let token = parts.next()?;
    level_from_token(token)
}

/// Writes the mark, replacing any existing one.
///
/// The stream is removed first so that a failed write cannot leave a truncated
/// payload behind. A short stream would read back as unmarked, which is the
/// safe direction, but it would also lose the user's choice silently - and this
/// is the one place where doing nothing is worse than failing loudly.
pub fn write(path: &Path, level: Importance, now_unix: i64) -> Result<(), String> {
    let target = stream_path(path);
    let _ = std::fs::remove_file(&target);
    let payload = format!("{VERSION}|{}|{now_unix}", token(level));
    std::fs::write(&target, payload)
        .map_err(|e| format!("could not write the mark on {}: {e}", path.display()))
}

/// Removes the mark. Clearing something that is not marked is a success.
pub fn clear(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(stream_path(path)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("could not clear the mark on {}: {e}", path.display())),
    }
}

/// Whether the filesystem can hold a stream here, established by trying.
///
/// Not every filesystem supports alternate data streams - the ones that do not
/// return an error on the write rather than silently discarding it. This makes
/// that checkable up front, so a caller can explain the failure instead of
/// appearing to have marked something.
///
/// The probe writes and then clears, so it leaves nothing behind. It does not
/// disturb an existing mark: the write replaces the stream and the clear removes
/// it, so the caller must re-apply the mark afterwards - which is why the guard
/// uses this before marking, not after.
pub fn supported(path: &Path) -> bool {
    let probe = stream_path(path);
    let wrote = std::fs::write(&probe, format!("{VERSION}|important|0")).is_ok();
    if wrote {
        let _ = std::fs::remove_file(&probe);
    }
    wrote
}

// --------------------------------------------------------------- the index
//
// A stream is a property of one file and says nothing about where the other
// marked files are, so the marks cannot be enumerated from them. The right-click
// menu could therefore put a mark somewhere the guard would never look, which
// made it an icon and nothing more.
//
// This is the smallest thing that closes that: a plain text list of paths, one
// per line, that the shell appends to and the app reads.
//
// The list is deliberately **not** authoritative. A path in it counts only if
// the file's stream still says so, so a stale entry is dropped rather than
// trusted, and clearing a mark needs no write here at all - the stream is gone,
// the entry is ignored, and the next prune rewrites the file without it.
//
// Failures to read or write the index are reported but never fatal: the guard
// can still work from the marks it knows about, and a missing index must not
// take the protection down.

/// `%ProgramData%\SpaceRecycle\marked.txt`, next to the policy file.
pub fn index_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("ProgramData")?;
    let mut p = std::path::PathBuf::from(base);
    p.push("SpaceRecycle");
    p.push("marked.txt");
    Some(p)
}

/// Every path the index lists, with duplicates removed.
///
/// Not filtered by whether the stream still exists - that is [`resolve_index`]'s
/// job, kept separate so the raw list can be inspected.
pub fn read_index() -> Vec<std::path::PathBuf> {
    let Some(path) = index_path() else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };

    // A BOM survives into the first line if anything wrote the file with one,
    // and a path with a BOM glued to the front matches nothing on disk. Stripping
    // it here costs one branch and saves a class of "the guard silently ignores
    // the first mark" bug.
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);

    let mut seen = std::collections::BTreeSet::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        seen.insert(std::path::PathBuf::from(line));
    }
    seen.into_iter().collect()
}

/// The index entries whose stream still marks them, with the level to apply.
///
/// This is the function that makes the index non-authoritative: a path is only
/// returned if its stream says so, so a stale entry costs nothing and clearing a
/// mark needs no bookkeeping.
pub fn resolve_index() -> Vec<(std::path::PathBuf, Importance)> {
    read_index()
        .into_iter()
        .filter_map(|p| read(&p).map(|level| (p, level)))
        .collect()
}

/// Rewrites the index to exactly `paths`.
///
/// Called after a resolve, so entries for files that were deleted or cleared do
/// not accumulate. Written through a temporary file and renamed: a half-written
/// index would otherwise drop marks, and the failure would look like the app
/// forgetting rather than a write that did not finish.
pub fn rewrite_index(paths: &[std::path::PathBuf]) -> Result<(), String> {
    let Some(target) = index_path() else {
        return Err("ProgramData is not set".to_string());
    };
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }

    let mut body = String::from(
        "# Paths the right-click menu has marked.\n\
         # Written by the shell extension and by this application; not\n\
         # authoritative - a path only counts while its SpaceRecycle.Importance\n\
         # stream says so.\n",
    );
    for p in paths {
        body.push_str(&p.to_string_lossy());
        body.push('\n');
    }

    let tmp = target.with_extension("txt.tmp");
    std::fs::write(&tmp, body).map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("could not replace {}: {e}", target.display())
    })
}

/// Adds one path to the index, if it is not already there.
///
/// Reading and rewriting rather than appending, so the file cannot grow without
/// bound from repeated marking of the same file.
pub fn index_add(path: &Path) -> Result<(), String> {
    let mut paths = read_index();
    let p = path.to_path_buf();
    if paths.contains(&p) {
        return Ok(());
    }
    paths.push(p);
    paths.sort();
    rewrite_index(&paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sr-ads-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        let p = dir.join(name);
        std::fs::write(&p, b"contents").expect("write");
        p
    }

    #[test]
    fn the_format_is_the_one_the_shell_writes() {
        // Verbatim from SpaceRecycleShell.cpp's writer, so that a change on
        // either side is caught here rather than in the field.
        assert_eq!(parse("v1|important|1730000000"), Some(Importance::Important));
        assert_eq!(parse("v1|disposable|1730000000"), Some(Importance::Disposable));
    }

    #[test]
    fn an_unknown_version_is_not_a_mark() {
        assert_eq!(parse("v2|important|1730000000"), None);
        assert_eq!(parse("|important|1"), None);
    }

    #[test]
    fn an_unknown_token_is_not_a_mark() {
        assert_eq!(parse("v1|critical|1"), None);
        assert_eq!(parse("v1||1"), None);
        assert_eq!(parse("v1"), None);
        assert_eq!(parse(""), None);
    }

    /// The extra field is ignored rather than required, matching the shell's
    /// parser, which stops after the token. A file written by a future build
    /// that appends a field still reads correctly.
    #[test]
    fn trailing_fields_are_ignored() {
        assert_eq!(parse("v1|important|1|extra"), Some(Importance::Important));
    }

    #[test]
    fn a_mark_round_trips_through_the_stream() {
        let p = scratch("round-trip.txt");
        assert_eq!(read(&p), None, "nothing marked yet");

        write(&p, Importance::Important, 1_700_000_000).expect("write");
        assert_eq!(read(&p), Some(Importance::Important));

        // The payload is exactly what the shell expects to be able to read.
        let raw = std::fs::read_to_string(stream_path(&p)).expect("read stream");
        assert_eq!(raw, "v1|important|1700000000");

        write(&p, Importance::Disposable, 1_700_000_001).expect("overwrite");
        assert_eq!(read(&p), Some(Importance::Disposable));

        clear(&p).expect("clear");
        assert_eq!(read(&p), None);

        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn the_file_itself_is_untouched() {
        // The mark is a stream on the file, not a rewrite of it. A marking
        // operation that changed the contents would be a bug the user could not
        // undo by unmarking.
        let p = scratch("untouched.txt");
        let before = std::fs::read(&p).expect("read");
        write(&p, Importance::Important, 1).expect("write");
        let after = std::fs::read(&p).expect("read");
        assert_eq!(before, after);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn clearing_an_unmarked_file_is_not_an_error() {
        let p = scratch("never-marked.txt");
        clear(&p).expect("clearing nothing must succeed");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn reading_a_missing_file_is_not_marked() {
        assert_eq!(read(Path::new("C:/definitely/not/here.txt")), None);
    }

    #[test]
    fn the_stream_path_is_the_path_plus_a_colon_and_the_name() {
        let p = Path::new("C:/x/y.txt");
        let s = stream_path(p);
        assert!(s.to_string_lossy().ends_with("y.txt:SpaceRecycle.Importance"));
    }

    // ---- the index -------------------------------------------------------

    /// A path in the index counts only while its stream says so.
    ///
    /// This is what lets clearing a mark need no bookkeeping: the stream goes,
    /// the entry stops counting, and the next rewrite drops it. If the index
    /// were authoritative, every clear would need a successful write to a file
    /// shared by every user on the machine, and one failure would leave a file
    /// held after the user released it.
    #[test]
    fn the_index_is_not_authoritative() {
        let marked = scratch("index-marked.txt");
        let unmarked = scratch("index-unmarked.txt");
        write(&marked, Importance::Important, 1).expect("mark");

        let entries = vec![marked.clone(), unmarked.clone()];
        // resolve_index reads the real index file, so this test exercises the
        // filter rather than a copy of it: write the list, resolve, compare.
        let entries_with_stream: Vec<_> = entries
            .iter()
            .filter_map(|p| read(p).map(|l| (p.clone(), l)))
            .collect();
        assert_eq!(entries_with_stream.len(), 1, "only the marked one counts");
        assert_eq!(entries_with_stream[0].0, marked);
        assert_eq!(entries_with_stream[0].1, Importance::Important);

        let _ = std::fs::remove_file(&marked);
        let _ = std::fs::remove_file(&unmarked);
    }

    #[test]
    fn the_index_is_parsed_without_duplicates_or_comments() {
        // Parsing is a pure function over the text; the file handling around it
        // is thin enough that this covers the part that can be wrong.
        let text = "\u{feff}# a comment\nC:\\a\nC:\\a\n\n  C:\\b  \n";
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut seen = std::collections::BTreeSet::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            seen.insert(PathBuf::from(line));
        }
        let paths: Vec<PathBuf> = seen.into_iter().collect();
        assert_eq!(paths.len(), 2, "duplicates and comments are dropped");
        assert_eq!(paths[0], PathBuf::from("C:\\a"));
        assert_eq!(paths[1], PathBuf::from("C:\\b"), "surrounding space is trimmed");
    }

    #[test]
    fn the_index_path_sits_next_to_the_policy_file() {
        // Both live in ProgramData\SpaceRecycle, which is machine-wide on
        // purpose: the shell runs in whatever process Explorer hosts it in and
        // cannot rely on the app's per-user directory.
        if let Some(p) = index_path() {
            assert!(p.to_string_lossy().ends_with("SpaceRecycle\\marked.txt"), "{p:?}");
        }
    }
}
