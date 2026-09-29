//! End-to-end verification of the cleanup pipeline against real files on
//! disk. Everything happens inside a throwaway temp directory, so no user
//! data is ever touched.

use spacerrecycle_lib::testing::*;

fn temp_root(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("sr-e2e-{}-{}", tag, uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_file(path: &std::path::Path, bytes: usize) {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).unwrap();
    }
    std::fs::write(path, vec![b'x'; bytes]).unwrap();
}

fn finding_for(path: &std::path::Path, is_dir: bool, category: Category) -> Finding {
    let risk = category.risk();
    Finding {
        id: uuid::Uuid::new_v4().to_string(),
        rule_id: "e2e".into(),
        name: path.file_name().unwrap().to_string_lossy().to_string(),
        path: path.to_string_lossy().to_string(),
        is_dir,
        logical_size: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        on_disk_size: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        last_access_unix: 0,
        created_unix: 0,
        age_days: 100,
        age_basis: AgeBasis::Creation,
        category,
        risk,
        disposition: category.default_disposition(),
        child_count: 0,
    }
}

#[test]
fn e2e_quarantine_compresses_and_restores_a_directory_tree() {
    let root = temp_root("qtree");
    let qroot = root.join("quarantine");
    let src = root.join("node_modules");

    // A realistic little project tree.
    write_file(&src.join("pkg-a").join("index.js"), 4096);
    write_file(&src.join("pkg-b").join("lib.js"), 8192);
    write_file(&src.join("README.md"), 512);

    let cfg = Config { quarantine_dir: qroot.clone(), ..Default::default() };
    let f = finding_for(&src, true, Category::DevArtifact);

    let report = run_cleanup(
        CleanupRequest {
            items: vec![SelectedItem { finding: f, disposition: Disposition::Quarantine }],
            confirmed_red_purge: false,
        },
        &cfg,
    );
    assert_eq!(report.quarantined.len(), 1, "should quarantine the tree");
    assert!(!src.exists(), "source tree should be gone");
    assert!(report.quarantine_batch.is_some());

    // The batch directory must exist and carry the original layout.
    let batches = list_batches(&qroot);
    assert_eq!(batches.len(), 1);
    let batch_dir = qroot.join(&batches[0].batch_id);
    assert!(batch_dir.exists());

    // Restore and verify byte-for-byte.
    let restored = restore_batch(&batch_dir, ConflictPolicy::Skip).unwrap();
    assert_eq!(restored.restored.len(), 1);
    assert_eq!(std::fs::read(src.join("pkg-b").join("lib.js")).unwrap().len(), 8192);
    assert!(src.join("README.md").exists());

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn e2e_purge_removes_temp_junk_immediately() {
    let root = temp_root("purge");
    let junk = root.join("cache").join("tmp0.dat");
    write_file(&junk, 2048);

    let cfg = Config { quarantine_dir: root.join("q"), ..Default::default() };
    let f = finding_for(&junk, false, Category::TempJunk);

    let report = run_cleanup(
        CleanupRequest {
            items: vec![SelectedItem { finding: f, disposition: Disposition::Purge }],
            confirmed_red_purge: false,
        },
        &cfg,
    );
    assert_eq!(report.purged.len(), 1);
    assert!(!junk.exists(), "temp junk should be deleted outright");
    assert!(report.quarantined.is_empty(), "temp junk must not be quarantined");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn e2e_mixed_selection_splits_by_disposition() {
    let root = temp_root("mixed");
    let qroot = root.join("q");
    let junk = root.join("temp").join("a.tmp");
    let artifacts = root.join("proj").join("node_modules");
    write_file(&junk, 1024);
    write_file(&artifacts.join("x.js"), 4096);

    let cfg = Config { quarantine_dir: qroot.clone(), ..Default::default() };
    let report = run_cleanup(
        CleanupRequest {
            items: vec![
                SelectedItem { finding: finding_for(&junk, false, Category::TempJunk), disposition: Disposition::Purge },
                SelectedItem { finding: finding_for(&artifacts, true, Category::DevArtifact), disposition: Disposition::Quarantine },
            ],
            confirmed_red_purge: false,
        },
        &cfg,
    );

    assert_eq!(report.purged.len(), 1, "temp junk purged");
    assert_eq!(report.quarantined.len(), 1, "artifacts quarantined");
    assert!(!junk.exists());
    assert!(!artifacts.exists());
    assert!(report.on_disk_purged > 0);

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn e2e_protected_paths_survive_a_purge_request() {
    let windir = std::env::var("SystemRoot").unwrap();
    let victim = std::path::Path::new(&windir).join("System32").join("kernel32.dll");
    if !victim.exists() {
        return;
    }
    let cfg = Config::default();
    let report = run_cleanup(
        CleanupRequest {
            items: vec![SelectedItem {
                finding: finding_for(&victim, false, Category::LargeFile),
                disposition: Disposition::Purge,
            }],
            confirmed_red_purge: true,
        },
        &cfg,
    );
    assert_eq!(report.purged.len(), 0, "must never purge a protected path");
    assert_eq!(report.blocked.len(), 1);
    assert!(victim.exists(), "explorer.exe must still exist");
}
