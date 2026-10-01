//! imgdedup - find duplicate images and send the lower-quality copies to the Recycle Bin.

use imgdedup::{
    apply, find_duplicates, group_action, recycle, scan, write_report, Action, Confidence, Group, Outcome, ScanOptions,
    MAX_DECODE_PIXELS,
};
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

const USAGE: &str = "\
usage: imgdedup <ROOT> [--apply] [--flat] [--min-width N] [--min-height N]
                [--phash-threshold 10] [--ssim-threshold 0.90] [--include-low-confidence]
                [--report FILE] [--threads N]

Finds duplicate images under ROOT (jpg, jpeg, png, gif, bmp, tiff, tif, webp). Two images are
duplicates when their pHash differs by at most --phash-threshold bits and their SSIM is at
least --ssim-threshold, or when the files are byte-identical. In each group the copy with the
most pixels is kept; a tie keeps the larger file, then the alphabetically first path.
Symbolic links and junctions are never followed. Images over 50 megapixels are skipped.

  (default)                 dry run: scan, write the report, delete nothing
  --apply                   send the DELETE copies to the Recycle Bin (never a permanent delete)
  --flat                    scan only ROOT itself; default is ROOT and all subfolders
  --min-width N             skip images narrower than N pixels
  --min-height N            skip images shorter than N pixels
  --phash-threshold N       pHash Hamming-distance limit, 0-64; default 10
  --ssim-threshold F        SSIM limit, above 0 and at most 1; default 0.90
  --include-low-confidence  also delete low-confidence copies; default: hold them as REVIEW
  --report FILE             tab-separated report; default imgdedup-report.tsv
  --threads N               worker threads; default the CPU count, at most 8

Confidence: high = byte-identical, or SSIM >= 0.98, or dHash also agrees (distance <= 10).
Low = SSIM between the threshold and 0.98 without dHash agreement.
Before each delete, the copy and its keeper must still have the scanned size and modified
time; if not, the copy stays and the report says so.

exit: 0 ok, 1 usage or I/O error, 3 some deletions failed";

struct Args {
    root: PathBuf,
    apply: bool,
    flat: bool,
    min_width: u32,
    min_height: u32,
    phash_threshold: u32,
    ssim_threshold: f64,
    include_low: bool,
    report: PathBuf,
    threads: usize,
}

fn number<T: std::str::FromStr>(it: &mut impl Iterator<Item = OsString>, flag: &str, what: &str) -> Result<T, String> {
    it.next()
        .ok_or_else(|| format!("{flag} needs {what}"))?
        .to_str()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("{flag} needs {what}"))
}

/// Err(empty string) means `--help`.
fn parse(argv: impl Iterator<Item = OsString>) -> Result<Args, String> {
    let default_threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8);
    let mut a = Args {
        root: PathBuf::new(),
        apply: false,
        flat: false,
        min_width: 0,
        min_height: 0,
        phash_threshold: 10,
        ssim_threshold: 0.90,
        include_low: false,
        report: "imgdedup-report.tsv".into(),
        threads: default_threads,
    };
    let mut root = None;
    let mut it = argv;
    while let Some(arg) = it.next() {
        match arg.to_str() {
            Some("--apply") => a.apply = true,
            Some("--flat") => a.flat = true,
            Some("--include-low-confidence") => a.include_low = true,
            Some("--min-width") => a.min_width = number(&mut it, "--min-width", "a whole number")?,
            Some("--min-height") => a.min_height = number(&mut it, "--min-height", "a whole number")?,
            Some("--phash-threshold") => {
                a.phash_threshold = number(&mut it, "--phash-threshold", "a whole number from 0 to 64")?;
                if a.phash_threshold > 64 {
                    return Err("--phash-threshold needs a whole number from 0 to 64".into());
                }
            }
            Some("--ssim-threshold") => {
                a.ssim_threshold = number(&mut it, "--ssim-threshold", "a number above 0 and at most 1")?;
                if !(a.ssim_threshold > 0.0 && a.ssim_threshold <= 1.0) {
                    return Err("--ssim-threshold needs a number above 0 and at most 1".into());
                }
            }
            Some("--report") => a.report = it.next().ok_or("--report needs a file")?.into(),
            Some("--threads") => {
                a.threads = number(&mut it, "--threads", "a positive number")?;
                if a.threads == 0 {
                    return Err("--threads needs a positive number".into());
                }
            }
            Some("-h" | "--help") => return Err(String::new()),
            Some(s) if s.starts_with("--") => return Err(format!("unknown option {s}")),
            _ if root.is_none() => root = Some(PathBuf::from(arg)),
            _ => return Err("only one ROOT".into()),
        }
    }
    a.root = root.ok_or("missing ROOT")?;
    if !a.root.is_dir() {
        return Err(format!("not a directory: {}", a.root.display()));
    }
    Ok(a)
}

fn size(bytes: u64) -> String {
    format!("{:.2} MB", bytes as f64 / 1e6)
}

fn write_results(out: &mut dyn Write, o: &Outcome) -> io::Result<()> {
    writeln!(out, "\nresult\tpath\treason")?;
    for p in &o.deleted {
        writeln!(out, "RECYCLED\t{}\t", p.display())?;
    }
    for (p, why) in &o.left {
        writeln!(out, "LEFT\t{}\t{why}", p.display())?;
    }
    for (p, why) in &o.failed {
        writeln!(out, "FAILED\t{}\t{why}", p.display())?;
    }
    Ok(())
}

fn run(a: Args) -> io::Result<u8> {
    let t = Instant::now();
    // The pool is global; a failure here only means a pool already exists.
    let _ = rayon::ThreadPoolBuilder::new().num_threads(a.threads).build_global();

    println!(
        "imgdedup {} - {}",
        env!("CARGO_PKG_VERSION"),
        if a.apply { "APPLY: DELETE copies go to the Recycle Bin" } else { "DRY RUN: deletes nothing" }
    );
    println!("root: {}{}", a.root.display(), if a.flat { " (flat)" } else { "" });

    let opts = ScanOptions { recursive: !a.flat, min_width: a.min_width, min_height: a.min_height };
    let s = scan(&a.root, &opts)?;
    let scanned_bytes: u64 = s.images.iter().map(|i| i.file_size).sum();
    println!("images scanned: {} ({})", s.images.len(), size(scanned_bytes));

    let found = find_duplicates(&s.images, a.phash_threshold, a.ssim_threshold);
    let groups: &[Group] = &found.groups;

    let unreadable: Vec<_> = s.unreadable.iter().chain(&found.errors).collect();
    println!(
        "skipped: {} too small, {} over {} MP, {} links",
        s.too_small,
        s.too_large,
        MAX_DECODE_PIXELS / 1_000_000,
        s.links
    );
    println!("unreadable: {}", unreadable.len());
    for (p, why) in unreadable.iter().take(20) {
        println!("  unreadable: {} ({why})", p.display());
    }
    if unreadable.len() > 20 {
        println!("  ... {} unreadable in all", unreadable.len());
    }

    let mut to_delete = 0usize;
    let mut reclaim = 0u64;
    let mut review = 0usize;
    for g in groups {
        match group_action(g, a.include_low) {
            Action::Delete => {
                to_delete += g.duplicates.len();
                reclaim += g.reclaimable_bytes();
            }
            Action::Review => review += g.duplicates.len(),
            Action::Keep => {}
        }
    }
    let low_groups = groups.iter().filter(|g| g.confidence == Confidence::Low).count();
    println!("groups: {} ({} low confidence)", groups.len(), low_groups);
    println!("copies to delete: {to_delete} ({} reclaimable)", size(reclaim));
    println!("low-confidence copies held for review: {review}");

    let mut report = io::BufWriter::new(std::fs::File::create(&a.report)?);
    write_report(groups, a.include_low, &mut report)?;

    let mut code = 0;
    if a.apply {
        let o = apply(groups, a.include_low, &mut |p| recycle(p));
        println!(
            "recycled: {}, left (changed since scan): {}, failed: {}",
            o.deleted.len(),
            o.left.len(),
            o.failed.len()
        );
        for (p, why) in o.left.iter().chain(&o.failed).take(20) {
            println!("  not deleted: {} ({why})", p.display());
        }
        write_results(&mut report, &o)?;
        if !o.failed.is_empty() {
            code = 3;
        }
    }
    report.flush()?;
    println!("report: {} ({:.1} s)", std::path::absolute(&a.report)?.display(), t.elapsed().as_secs_f64());
    Ok(code)
}

fn main() -> ExitCode {
    let a = match parse(std::env::args_os().skip(1)) {
        Ok(a) => a,
        Err(msg) if msg.is_empty() => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(msg) => {
            eprintln!("imgdedup: {msg}\n\n{USAGE}");
            return ExitCode::from(1);
        }
    };
    match run(a) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("imgdedup: {e}");
            ExitCode::from(1)
        }
    }
}
