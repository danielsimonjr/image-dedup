//! `recycle` must never delete permanently. Windows only.
//!
//! These tests use files of their own under the system temp folder, which must be on a local
//! drive. The local test leaves one Recycle Bin item per run; nothing here deletes from the bin.
#![cfg(windows)]

use std::fs;
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::mpsc;
use std::time::Duration;

fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join("imgdedup-recycle-tests");
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// The drive letter of an absolute local path such as `C:\...`.
fn drive_letter(p: &Path) -> char {
    match p.components().next() {
        Some(Component::Prefix(pre)) => match pre.kind() {
            Prefix::Disk(d) | Prefix::VerbatimDisk(d) => d as char,
            other => panic!("temp folder is not on a local drive: {other:?}"),
        },
        _ => panic!("not an absolute path: {}", p.display()),
    }
}

fn unique(tag: &str) -> String {
    let ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("imgdedup-{tag}-{ns}-{}.txt", std::process::id())
}

fn make_file(name: &str) -> PathBuf {
    let p = scratch().join(name);
    fs::write(&p, b"recycle test file").unwrap();
    p
}

/// Run `recycle` on its own thread so a hang is reported as a failure instead of blocking the run.
fn recycle_bounded(p: PathBuf, secs: u64) -> Option<std::io::Result<()>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(imgdedup::recycle(&p));
    });
    rx.recv_timeout(Duration::from_secs(secs)).ok()
}

/// Names of Recycle Bin `$I` records on `drive` (any user folder we can read) that mention `needle`.
fn bin_records_mentioning(drive: char, needle: &str) -> Vec<PathBuf> {
    let mut hits = Vec::new();
    let Ok(dirs) = fs::read_dir(format!(r"{drive}:\$Recycle.Bin")) else {
        return hits;
    };
    for d in dirs.flatten() {
        let Ok(files) = fs::read_dir(d.path()) else {
            continue;
        };
        for f in files.flatten() {
            if !f.file_name().to_string_lossy().starts_with("$I") {
                continue;
            }
            let Ok(bytes) = fs::read(f.path()) else {
                continue;
            };
            let text: Vec<u16> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect();
            if String::from_utf16_lossy(&text).contains(needle) {
                hits.push(f.path());
            }
        }
    }
    hits
}

#[test]
fn admin_share_path_is_never_deleted_permanently() {
    // A network-form path has no Recycle Bin. The file must stay, and the call must fail.
    let name = unique("share");
    let local = make_file(&name);
    let drive = drive_letter(&local);
    let rest = local.strip_prefix(format!(r"{drive}:\")).unwrap();
    let share = PathBuf::from(format!(r"\\localhost\{drive}$")).join(rest);
    assert!(
        share.exists(),
        "admin share not reachable in this session: {}",
        share.display()
    );

    let result = recycle_bounded(share.clone(), 60);
    let survived = local.exists();
    let _ = fs::remove_file(&local); // test file of this test; not a bin item
    match result {
        None => panic!("recycle hung on a network path (60 s)"),
        Some(Ok(())) => panic!(
            "recycle reported success on a path with no Recycle Bin; file survived: {survived}"
        ),
        Some(Err(e)) => {
            assert!(survived, "recycle failed ({e}) but the file is gone");
            assert!(
                bin_records_mentioning(drive, &name).is_empty(),
                "a bin record exists for a file that was refused"
            );
        }
    }
}

#[test]
fn local_file_lands_in_the_recycle_bin() {
    let name = unique("local");
    let p = make_file(&name);
    recycle_bounded(p.clone(), 60)
        .expect("recycle hung")
        .expect("local recycle failed");
    assert!(!p.exists(), "file still on disk");
    let records = bin_records_mentioning(drive_letter(&p), &name);
    assert_eq!(
        records.len(),
        1,
        "expected one bin record for {name}, found {records:?}"
    );
    eprintln!("left in Recycle Bin: {:?}", records[0]);
}

#[test]
fn missing_file_is_an_error_not_a_success() {
    let p = scratch().join(unique("missing"));
    let r = recycle_bounded(p, 60).expect("recycle hung");
    assert!(r.is_err());
}
