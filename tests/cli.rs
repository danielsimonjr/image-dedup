mod common;

use common::{scene, write_png};
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn run(args: &[&std::ffi::OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_imgdedup"))
        .args(args)
        .output()
        .expect("run imgdedup")
}

fn run_str(args: &[&str]) -> Output {
    let os: Vec<&std::ffi::OsStr> = args.iter().map(std::ffi::OsStr::new).collect();
    run(&os)
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn help_goes_to_stdout_with_exit_zero() {
    let o = run_str(&["--help"]);
    assert_eq!(o.status.code(), Some(0));
    assert!(text(&o.stdout).contains("usage: imgdedup"));
    assert!(o.stderr.is_empty());
}

#[test]
fn usage_errors_go_to_stderr_with_exit_one() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().to_str().unwrap();
    let cases: Vec<Vec<&str>> = vec![
        vec![],
        vec!["--apply"],
        vec![d, "--bogus"],
        vec![d, "--ssim-threshold", "1.5"],
        vec![d, "--ssim-threshold", "abc"],
        vec![d, "--phash-threshold", "65"],
        vec![d, "--threads", "0"],
        vec![d, "--min-width"],
        vec![d, "extra-root"],
        vec!["definitely-not-a-folder-xyz"],
    ];
    for args in cases {
        let o = run_str(&args);
        assert_eq!(
            o.status.code(),
            Some(1),
            "args {args:?}: stderr {}",
            text(&o.stderr)
        );
        assert!(o.stdout.is_empty(), "args {args:?} wrote to stdout");
        assert!(text(&o.stderr).contains("imgdedup:"), "args {args:?}");
    }
}

fn dup_tree(root: &Path) {
    fs::create_dir_all(root.join("sub")).unwrap();
    write_png(&root.join("a.png"), &scene(1, 200, 150));
    write_png(&root.join("sub").join("a_small.png"), &scene(1, 100, 75));
    write_png(&root.join("other.png"), &scene(2, 200, 150));
}

#[test]
fn dry_run_deletes_nothing_and_writes_the_report() {
    let dir = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    dup_tree(dir.path());
    let report = out.path().join("r.tsv");
    let o = run(&[
        dir.path().as_os_str(),
        "--report".as_ref(),
        report.as_os_str(),
    ]);
    assert_eq!(o.status.code(), Some(0), "stderr: {}", text(&o.stderr));
    let stdout = text(&o.stdout);
    assert!(stdout.contains("DRY RUN"), "{stdout}");
    assert!(stdout.contains("groups: 1"), "{stdout}");
    assert!(stdout.contains("copies to delete: 1"), "{stdout}");
    for f in ["a.png", "other.png"] {
        assert!(dir.path().join(f).exists());
    }
    assert!(
        dir.path().join("sub").join("a_small.png").exists(),
        "a dry run must delete nothing"
    );
    let rep = fs::read_to_string(&report).unwrap();
    let rows: Vec<Vec<&str>> = rep.lines().map(|l| l.split('\t').collect()).collect();
    assert_eq!(rows[0][..2], ["group", "action"]);
    assert_eq!(rows.iter().filter(|r| r[1] == "KEEP").count(), 1);
    let del: Vec<&Vec<&str>> = rows.iter().filter(|r| r[1] == "DELETE").collect();
    assert_eq!(del.len(), 1);
    assert!(del[0][7].ends_with("a_small.png"));
}

#[test]
fn flat_scans_only_the_root_folder() {
    let dir = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    dup_tree(dir.path());
    let report = out.path().join("r.tsv");
    let o = run(&[
        dir.path().as_os_str(),
        "--flat".as_ref(),
        "--report".as_ref(),
        report.as_os_str(),
    ]);
    assert_eq!(o.status.code(), Some(0));
    let stdout = text(&o.stdout);
    assert!(stdout.contains("images scanned: 2"), "{stdout}");
    assert!(stdout.contains("groups: 0"), "{stdout}");
}

#[test]
fn min_size_filters_are_reported_as_skips() {
    let dir = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    dup_tree(dir.path());
    let report = out.path().join("r.tsv");
    let o = run(&[
        dir.path().as_os_str(),
        "--min-width".as_ref(),
        "150".as_ref(),
        "--report".as_ref(),
        report.as_os_str(),
    ]);
    assert_eq!(o.status.code(), Some(0));
    let stdout = text(&o.stdout);
    assert!(stdout.contains("1 too small"), "{stdout}");
    assert!(stdout.contains("groups: 0"), "{stdout}");
}
