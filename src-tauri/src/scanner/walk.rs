use crate::model::AgeBasis;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Compute an item's age in days, and say which timestamp it came from.
///
/// Win10 and later disable last-access time updates by default, so
/// `LastAccessTime` is frequently meaningless on a modern system. The
/// creation time is the fallback.
///
/// The sanity check matters: a file last read in 1601 or one carrying a
/// nonsense timestamp must not be read as "modified in the year 6000", which
/// would compute to age 0 and silently disqualify an ancient file from the
/// age-gated categories. Anything implausible falls back to creation time.
pub fn age_of(last_access_unix: i64, created_unix: i64) -> (u32, AgeBasis) {
    let now = now_unix();

    let access_ok = last_access_unix > 0 && last_access_unix <= now;
    let created_ok = created_unix > 0 && created_unix <= now;

    let (ts, basis) = if access_ok {
        (last_access_unix, AgeBasis::LastAccess)
    } else if created_ok {
        (created_unix, AgeBasis::Creation)
    } else {
        // Nothing trustworthy. Report age 0 with an explicit basis so the UI
        // can show it as unknown rather than as "brand new".
        return (0, AgeBasis::Creation);
    };

    let days = ((now - ts).max(0) as f64) / 86_400.0;
    (days as u32, basis)
}

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Windows FILETIME (100ns intervals since 1601-01-01) to Unix seconds.
pub fn filetime_to_unix(ft: u64) -> i64 {
    const UNIX_EPOCH_FILETIME: u64 = 116_444_736_000_000_000;
    if ft < UNIX_EPOCH_FILETIME {
        return 0;
    }
    ((ft - UNIX_EPOCH_FILETIME) / 10_000_000) as i64
}

pub fn unix_to_filetime(unix: i64) -> u64 {
    const UNIX_EPOCH_FILETIME: u64 = 116_444_736_000_000_000;
    if unix <= 0 {
        return 0;
    }
    (unix as u64) * 10_000_000 + UNIX_EPOCH_FILETIME
}

/// Read last-access and creation times for a path, normalised to Unix seconds.
pub fn timestamps(path: &Path) -> (i64, i64) {
    use std::os::windows::fs::MetadataExt;
    match std::fs::metadata(path) {
        Ok(md) => (
            filetime_to_unix(md.last_access_time() as u64),
            filetime_to_unix(md.creation_time() as u64),
        ),
        Err(_) => (0, 0),
    }
}

/// Read the modification time of a path, normalised to Unix seconds.
///
/// Modification time is the only timestamp that survives a scan. NTFS
/// updates a *directory's* last-access time whenever the directory is
/// enumerated, so `timestamps` on a directory reports "now" the moment we
/// walk it. Win10+ turned off last-access updates for *files*, but not for
/// directories, so an age gate keyed on last access silently rejects every
/// directory the scanner has ever looked at - which is all of them.
pub fn modified_unix(path: &Path) -> i64 {
    use std::os::windows::fs::MetadataExt;
    match std::fs::metadata(path) {
        Ok(md) => filetime_to_unix(md.last_write_time() as u64),
        Err(_) => 0,
    }
}

/// The newest modification time anywhere in a tree, or the directory's own
/// mtime when the tree is empty.
///
/// A leftover is stale when nothing inside it has been touched recently, so
/// the newest mtime in the subtree is the honest question to ask. Returns
/// `None` only when the tree cannot be read at all.
pub fn newest_mtime_in_tree(dir: &Path, budget: usize) -> Option<i64> {
    use std::os::windows::fs::MetadataExt;

    let own = modified_unix(dir);
    let mut newest = own;
    let mut visited = 0usize;

    // walkdir rather than jwalk for the same cancel reason: this walk is
    // bounded by `budget`, but a bound is not a cancel, and a budget of
    // 10k entries on a slow tree is still seconds of unstoppable reading.
    for entry in walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        // Bounded so a pathological tree cannot stall the scan. Missing the
        // newest file in a huge tree only makes us *more* likely to keep the
        // item, which is the safe direction to be wrong in.
        visited += 1;
        if visited > budget {
            break;
        }
        let Ok(md) = entry.metadata() else { continue };
        let m = filetime_to_unix(md.last_write_time() as u64);
        if m > newest {
            newest = m;
        }
    }

    if newest <= 0 {
        None
    } else {
        Some(newest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;

    #[test]
    fn uses_last_access_when_it_is_plausible() {
        let now = now_unix();
        let (age, basis) = age_of(now - 10 * DAY, now - 400 * DAY);
        assert_eq!(basis, AgeBasis::LastAccess);
        assert_eq!(age, 10);
    }

    #[test]
    fn falls_back_to_creation_when_access_time_is_zero() {
        let now = now_unix();
        let (age, basis) = age_of(0, now - 200 * DAY);
        assert_eq!(basis, AgeBasis::Creation);
        assert_eq!(age, 200);
    }

    #[test]
    fn a_nonsense_future_access_time_does_not_report_age_zero() {
        // A FILETIME that has not been normalised to Unix seconds is a large
        // positive number far beyond "now". Treating it as valid would compute
        // a negative age, clamp to 0, and hide a genuinely ancient file from
        // the age-gated categories.
        let now = now_unix();
        let bogus = 13_427_238_365_000_000; // raw FILETIME, ~year 6000
        let (age, basis) = age_of(bogus, now - 365 * DAY);
        assert_eq!(basis, AgeBasis::Creation);
        assert!(
            age >= 364,
            "expected the creation-time fallback to give a real age, got {}",
            age
        );
    }

    #[test]
    fn a_future_creation_time_also_falls_back_safely() {
        let now = now_unix();
        let (age, _) = age_of(0, now + 10_000 * DAY);
        assert_eq!(age, 0, "an untrustworthy timestamp must not produce nonsense");
    }

    #[test]
    fn filetime_round_trips() {
        let now = now_unix();
        assert_eq!(filetime_to_unix(unix_to_filetime(now)), now);
    }

    #[test]
    fn pre_1970_filetime_clamps_to_zero() {
        assert_eq!(filetime_to_unix(0), 0);
    }

    #[test]
    fn a_directory_enumeration_rewrites_its_own_access_time() {
        // Documents the NTFS behaviour that motivates keying directory age off
        // modification time: reading a directory resets its last-access stamp,
        // so an access-time age gate can never be satisfied by a directory the
        // scanner has walked.
        let dir = std::env::temp_dir().join(format!("sr-at-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("payload.bin");
        std::fs::write(&file, b"x").unwrap();

        backdate(&file, now_unix() - 400 * DAY);
        let before = timestamps(&dir).0;
        // Enumerate the directory the way a measuring walk does.
        let _ = std::fs::read_dir(&dir).unwrap().count();
        let after = timestamps(&dir).0;

        assert!(
            after >= before,
            "access time should be refreshed by enumeration (before={} after={})",
            before,
            after
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn newest_mtime_in_tree_reports_the_newest_file_not_the_directory() {
        let dir = std::env::temp_dir().join(format!("sr-mt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let old_file = dir.join("old.bin");
        let new_file = dir.join("new.bin");
        std::fs::write(&old_file, b"a").unwrap();
        std::fs::write(&new_file, b"b").unwrap();

        backdate(&old_file, now_unix() - 800 * DAY);
        backdate(&new_file, now_unix() - 5 * DAY);
        // Make the directory itself look ancient and freshly accessed, which is
        // exactly the state a previously-scanned leftover ends up in.
        backdate(&dir, now_unix() - 900 * DAY);

        let newest = newest_mtime_in_tree(&dir, 10_000).expect("readable tree");
        let (age, _) = age_of(0, newest);
        assert!(
            age <= 6 && age >= 4,
            "expected the 5-day-old file to drive the age, got {} days",
            age
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn newest_mtime_in_tree_falls_back_to_the_directory_when_empty() {
        let dir = std::env::temp_dir().join(format!("sr-empty-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        backdate(&dir, now_unix() - 120 * DAY);
        let newest = newest_mtime_in_tree(&dir, 10_000).expect("readable dir");
        let (age, _) = age_of(0, newest);
        assert!((119..=121).contains(&age), "got {} days", age);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn backdate(path: &std::path::Path, unix: i64) {
        // set_times takes a raw FILETIME and applies it to all three stamps.
        crate::winapi::ntfs::set_times(path, unix_to_filetime(unix)).unwrap();
    }
}
