//! Audit findings, each written as a failing test first.

use spacerrecycle_lib::safety;
use spacerrecycle_lib::testing::*;

fn temp_root(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("sr-audit-{}-{}", tag, uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_file(path: &std::path::Path, bytes: usize) {
    if let Some(p) = path.parent() { std::fs::create_dir_all(p).unwrap(); }
    std::fs::write(path, vec![b'x'; bytes]).unwrap();
}

fn finding_for(path: &std::path::Path, is_dir: bool, category: Category) -> Finding {
    Finding {
        id: uuid::Uuid::new_v4().to_string(),
        rule_id: "audit".into(),
        name: path.file_name().unwrap_or(path.as_os_str()).to_string_lossy().to_string(),
        path: path.to_string_lossy().to_string(),
        is_dir,
        logical_size: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        on_disk_size: std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        last_access_unix: 0,
        created_unix: 0,
        age_days: 100,
        age_basis: AgeBasis::Creation,
        category,
        risk: category.risk(),
        disposition: category.default_disposition(),
        child_count: 0,
    }
}

/// FINDING 1: the safety gate guards purge but not quarantine.
///
/// engine.rs applies is_allowed inside the purge loop only. The quarantine
/// loop hands findings straight to quarantine_items. The excluded root here
/// is a real directory holding a real file, so the only thing that can stop
/// the move is the safety gate itself.
#[test]
fn quarantine_does_not_bypass_the_safety_gate() {
    let root = temp_root("qsafety");
    let qroot = root.join("quarantine");
    let protected = root.join("protected");
    let victim = protected.join("keep-me");
    write_file(&victim.join("data.bin"), 4096);
    let cfg = Config {
        quarantine_dir: qroot.clone(),
        extra_excluded_roots: vec![protected.clone()],
        ..Default::default()
    };

    // Precondition: the gate does deny this path, and the file really exists.
    assert!(victim.exists(), "precondition: the victim must exist on disk");
    assert!(
        !safety::is_allowed(&victim, &cfg),
        "precondition: an excluded root must be denied"
    );

    let report = run_cleanup(
        CleanupRequest {
            items: vec![SelectedItem {
                finding: finding_for(&victim, true, Category::AppLeftover),
                disposition: Disposition::Quarantine,
            }],
            confirmed_red_purge: false,
        },
        &cfg,
    );

    assert_eq!(
        report.quarantined.len(),
        0,
        "a denylisted path was moved into the quarantine store"
    );
    assert!(victim.exists(), "the protected item was moved out of place");
    let _ = std::fs::remove_dir_all(&root);
}

/// FINDING 2: risk and disposition are taken from the client, not derived.
///
/// The red-confirmation gate keys on Finding.risk, and the category is only
/// used to build the preview. Nothing recomputes them server-side, so the
/// tier that protects the user is whatever the request says it is.
#[test]
fn a_client_cannot_relabel_a_dangerous_finding_as_safe() {
    let root = temp_root("relabel");
    let qroot = root.join("quarantine");
    let victim = root.join("important-user-data");
    write_file(&victim.join("thesis.docx"), 2048);
    let cfg = Config { quarantine_dir: qroot.clone(), ..Default::default() };

    // A leftover - the most dangerous category - but the request claims it is
    // a harmless temp file, so no red confirmation is ever asked for.
    let mut f = finding_for(&victim, true, Category::AppLeftover);
    f.category = Category::TempJunk;
    f.risk = RiskTier::Safe;

    let report = run_cleanup(
        CleanupRequest {
            items: vec![SelectedItem { finding: f, disposition: Disposition::Purge }],
            confirmed_red_purge: false,
        },
        &cfg,
    );

    assert_eq!(
        report.purged.len(),
        0,
        "an AppLeftover path was permanently deleted because the request relabelled it"
    );
    assert!(victim.exists(), "the victim directory was destroyed");
    let _ = std::fs::remove_dir_all(&root);
}
