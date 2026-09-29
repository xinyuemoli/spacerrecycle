use super::{finding_for_dir, ScanCtx, Scanner};
use crate::model::{Category, Finding};
use std::path::PathBuf;
use std::sync::atomic::Ordering;

pub struct TempJunkScanner;

impl Scanner for TempJunkScanner {
    fn name(&self) -> &'static str {
        "Temp junk"
    }

    fn scan(&self, ctx: &ScanCtx) -> Vec<Finding> {
        let mut out = Vec::new();

        // Whole-directory targets: everything inside is junk by definition.
        let mut dirs: Vec<(PathBuf, &str)> = Vec::new();
        dirs.push((std::env::temp_dir(), "temp.user"));
        if let Some(windir) = std::env::var_os("SystemRoot") {
            dirs.push((PathBuf::from(&windir).join("Temp"), "temp.windows"));
            dirs.push((PathBuf::from(&windir).join("Prefetch"), "temp.prefetch"));
            dirs.push((
                PathBuf::from(&windir).join("Logs").join("CBS"),
                "temp.cbs-logs",
            ));
        }
        if let Some(local) = dirs::data_local_dir() {
            dirs.push((
                local.join("Microsoft").join("Windows").join("WER"),
                "temp.error-reports",
            ));
            dirs.push((
                local.join("CrashDumps"),
                "temp.crash-dumps",
            ));
        }
        if let Some(pd) = std::env::var_os("ProgramData") {
            dirs.push((
                PathBuf::from(&pd).join("Microsoft").join("Windows").join("WER"),
                "temp.error-reports",
            ));
        }

        for (dir, rule) in dirs {
            if ctx.counters.cancelled() {
                break;
            }
            if !dir.is_dir() {
                continue;
            }
            let (la, cr) = super::walk::timestamps(&dir);
            if let Some(f) = finding_for_dir(ctx, &dir, Category::TempJunk, rule, la, cr) {
                out.push(f);
            }
        }

        // Individual large dump files, which are pure garbage when old.
        // CrashDumps and Minidump are already covered by the whole-directory
        // pass above; listing them again here measured each tree twice and
        // produced two indistinguishable rows in the UI.
        let mut dumps: Vec<PathBuf> = Vec::new();
        if let Some(windir) = std::env::var_os("SystemRoot") {
            dumps.push(PathBuf::from(windir).join("MEMORY.DMP"));
        }
        for d in dumps {
            if ctx.counters.cancelled() {
                break;
            }
            if !d.exists() {
                continue;
            }
            if let Some(f) = self.file_finding(&d) {
                ctx.counters.findings.fetch_add(1, Ordering::Relaxed);
                out.push(f);
            }
        }

        out
    }
}

impl TempJunkScanner {
    fn file_finding(&self, file: &std::path::Path) -> Option<Finding> {
        use std::os::windows::fs::MetadataExt;
        let md = std::fs::metadata(file).ok()?;
        let logical = md.len();
        if logical == 0 {
            return None;
        }
        let on_disk = crate::winapi::ntfs::on_disk_size(file).unwrap_or(logical);
        let la = super::walk::filetime_to_unix(md.last_access_time() as u64);
        let cr = super::walk::filetime_to_unix(md.creation_time() as u64);
        let (age_days, age_basis) = super::walk::age_of(la, cr);
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        Some(Finding {
            id: uuid::Uuid::new_v4().to_string(),
            rule_id: "temp.dump".into(),
            name,
            path: file.to_string_lossy().to_string(),
            is_dir: false,
            logical_size: logical,
            on_disk_size: on_disk,
            last_access_unix: la,
            created_unix: cr,
            age_days,
            age_basis,
            category: Category::TempJunk,
            risk: Category::TempJunk.risk(),
            disposition: Category::TempJunk.default_disposition(),
            child_count: 0,
        })
    }
}
