use super::{finding_for_dir_cached, ScanCtx, Scanner};
use crate::model::{Category, Finding};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

/// Directory names that are dev artifacts wherever they appear. These are
/// rebuildable, so no age gate applies.
const REBUILDABLE_DIRS: &[&str] = &[
    "node_modules",
    "target",       // Rust / Maven-ish
    "__pycache__",
    ".venv",
    "venv",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".gradle",
    ".nuget",
    ".tox",
    ".parcel-cache",
    ".turbo",
    ".cache",
    // "dist" is deliberately absent: it is far too generic to be a build
    // artifact at any depth, and users do keep things in it. It is handled by
    // BUILD_OUTPUT_DIRS with a depth limit instead.
];

/// Contexts where a "dist"/"build"/"out" directory is a build artifact rather
/// than a user's project output worth keeping.
const BUILD_OUTPUT_DIRS: &[&str] = &["dist", "build", "out", ".next"];

pub struct DevArtifactScanner;

impl Scanner for DevArtifactScanner {
    fn name(&self) -> &'static str {
        "Dev artifacts"
    }

    fn scan(&self, ctx: &ScanCtx) -> Vec<Finding> {
        let cfg: &crate::config::Config = ctx.config.as_ref();
        let mut out = Vec::new();

        // Well-known global caches: fixed locations, cheap to check.
        let mut known: Vec<(PathBuf, &str)> = Vec::new();
        if let Some(local) = dirs::data_local_dir() {
            known.push((local.join("npm-cache"), "dev.npm-cache"));
            known.push((local.join("pnpm").join("store"), "dev.pnpm-store"));
            known.push((local.join("pip").join("Cache"), "dev.pip-cache"));
            known.push((local.join("Yarn").join("Cache"), "dev.yarn-cache"));
            known.push((local.join("NuGet").join("Cache"), "dev.nuget-cache"));
            known.push((local.join("Temp").join("nuget"), "dev.nuget-temp"));
            known.push((local.join("Microsoft").join("VisualStudio"), "dev.vs"));
            // Package-manager stores that are dev artifacts, not app
            // leftovers. Without these they surface via the leftover scanner
            // and get tagged red, which misdescribes a rebuildable cache.
            known.push((local.join("uv"), "dev.uv-cache"));
            known.push((local.join("vcpkg"), "dev.vcpkg"));
            known.push((local.join("ms-playwright"), "dev.playwright-browsers"));
        }
        if let Some(home) = dirs::home_dir() {
            known.push((home.join(".cargo").join("registry"), "dev.cargo-registry"));
            known.push((home.join(".cargo").join("git"), "dev.cargo-git"));
            known.push((home.join(".m2").join("repository"), "dev.maven"));
            known.push((home.join(".gradle").join("caches"), "dev.gradle"));
            known.push((home.join(".cache"), "dev.user-cache"));
        }

        let cache = super::SizeCache::default();
        for (dir, rule) in known {
            if ctx.counters.cancelled() {
                return out;
            }
            if !dir.is_dir() {
                continue;
            }
            let (la, cr) = super::walk::timestamps(&dir);
            if let Some(f) = finding_for_dir_cached(ctx, &dir, Category::DevArtifact, rule, la, cr, &cache)
            {
                out.push(f);
            }
        }

        // Project-local artifacts: walk the user's source roots looking for
        // rebuildable directory names.
        //
        // Roots are conventional code directories only. Deliberately NOT
        // included: Documents, Desktop, Downloads. Those are personal-data
        // territory (and partly denylisted), and walking them is both slow and
        // outside this scanner's remit.
        let mut roots: Vec<PathBuf> = Vec::new();
        if let Some(home) = dirs::home_dir() {
            for name in [
                "source", "src", "projects", "code", "repos", "dev", "work", "git", "Projects",
            ] {
                let p = home.join(name);
                if p.is_dir() {
                    roots.push(p);
                }
            }
        }
        for drive in ["C:", "D:", "E:", "F:"] {
            for name in ["dev", "code", "src", "projects", "repos", "source", "git"] {
                let p = PathBuf::from(drive).join(name);
                if p.is_dir() {
                    roots.push(p);
                }
            }
        }

        let cache = super::SizeCache::default();
        let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        for root in roots {
            if ctx.counters.cancelled() {
                break;
            }
            self.walk_for_artifacts(ctx, &root, 5, &mut out, &mut seen, cfg, &cache);
        }

        out
    }
}

impl DevArtifactScanner {
    #[allow(clippy::too_many_arguments)]
    fn walk_for_artifacts(
        &self,
        ctx: &ScanCtx,
        dir: &Path,
        depth: usize,
        out: &mut Vec<Finding>,
        seen: &mut std::collections::HashSet<PathBuf>,
        cfg: &crate::config::Config,
        cache: &super::SizeCache,
    ) {
        if depth == 0 || ctx.counters.cancelled() {
            return;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => {
                ctx.counters.skipped.fetch_add(1, Ordering::Relaxed);
                return;
            }
        };
        for entry in entries.flatten() {
            if ctx.counters.cancelled() {
                return;
            }
            let p = entry.path();
            let ft = match entry.file_type() {
                Ok(f) => f,
                Err(_) => continue,
            };
            if ft.is_dir() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') && !REBUILDABLE_DIRS.contains(&name.as_str()) {
                    continue;
                }
                let is_artifact = REBUILDABLE_DIRS.contains(&name.as_str())
                    || (BUILD_OUTPUT_DIRS.contains(&name.as_str()) && depth <= 2);
                if is_artifact {
                    if !crate::safety::is_allowed(&p, cfg) {
                        continue;
                    }
                    let key = p.clone();
                    if seen.insert(key) {
                        let (la, cr) = super::walk::timestamps(&p);
                        // Route through the shared constructor so the age comes
                        // from the subtree's newest mtime. Reading a directory
                        // rewrites its own last-access stamp on NTFS, so the
                        // naive age_of(la, cr) reported every project artifact
                        // as brand new.
                        if let Some(f) = super::finding_for_dir_cached(
                            ctx,
                            &p,
                            Category::DevArtifact,
                            "dev.project-artifact",
                            la,
                            cr,
                            cache,
                        ) {
                            out.push(f);
                        }
                    }
                    // No need to descend into something we already flagged.
                    continue;
                }
                self.walk_for_artifacts(ctx, &p, depth - 1, out, seen, cfg, cache);
            }
        }
    }
}
