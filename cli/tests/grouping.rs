mod common;

use common::info;
use imgdedup::*;
use std::path::PathBuf;

fn edge(a: usize, b: usize, score: f64, high: bool) -> Edge {
    Edge { a, b, score, high }
}

fn names(g: &Group) -> Vec<String> {
    g.duplicates
        .iter()
        .map(|d| d.info.path.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn union_find_joins_and_separates() {
    let mut uf = UnionFind::new(6);
    for i in 0..6 {
        assert_eq!(uf.find(i), i);
    }
    uf.union(0, 1);
    uf.union(3, 4);
    assert_eq!(uf.find(0), uf.find(1));
    assert_eq!(uf.find(3), uf.find(4));
    assert_ne!(uf.find(0), uf.find(3));
    assert_ne!(uf.find(2), uf.find(0));
    uf.union(1, 4); // joins the two pairs through a chain
    assert_eq!(uf.find(0), uf.find(3));
    assert_eq!(uf.find(1), uf.find(4));
    assert_ne!(uf.find(5), uf.find(0));
    uf.union(0, 3); // already joined: no change
    assert_eq!(uf.find(0), uf.find(4));
}

#[test]
fn union_find_handles_long_chains() {
    let n = 10_000;
    let mut uf = UnionFind::new(n);
    for i in 1..n {
        uf.union(i - 1, i);
    }
    let root = uf.find(0);
    assert!((0..n).all(|i| uf.find(i) == root));
}

#[test]
fn keeper_is_highest_pixel_count() {
    let imgs = vec![
        info("small", 100, 100, 9_000),
        info("big", 400, 300, 1_000),
        info("mid", 200, 200, 5_000),
    ];
    let edges = [edge(0, 1, 0.99, true), edge(1, 2, 0.99, true)];
    let groups = build_groups(&imgs, &edges);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].keeper.path, PathBuf::from("big")); // wins on pixels even with the smallest file
    assert_eq!(names(&groups[0]), ["mid", "small"]);
}

#[test]
fn keeper_tie_breaks_on_larger_file() {
    let imgs = vec![
        info("png", 640, 480, 300_000),
        info("jpg", 640, 480, 150_000),
        info("webp", 640, 480, 450_000),
    ];
    let edges = [edge(0, 1, 0.99, true), edge(1, 2, 0.99, true)];
    let groups = build_groups(&imgs, &edges);
    assert_eq!(groups[0].keeper.path, PathBuf::from("webp"));
    assert_eq!(names(&groups[0]), ["png", "jpg"]);
}

#[test]
fn full_tie_is_decided_by_path_for_a_stable_result() {
    let imgs = vec![info("b.png", 10, 10, 100), info("a.png", 10, 10, 100)];
    for _ in 0..20 {
        let groups = build_groups(&imgs, &[edge(0, 1, 1.0, true)]);
        assert_eq!(groups[0].keeper.path, PathBuf::from("a.png"));
    }
}

#[test]
fn chains_form_one_group_and_singletons_are_dropped() {
    let imgs = vec![
        info("a", 10, 10, 1),
        info("b", 20, 20, 1),
        info("c", 30, 30, 1),
        info("lonely", 99, 99, 1),
        info("d", 5, 5, 1),
        info("e", 6, 6, 1),
    ];
    // a-b and b-c are linked; a-c are not. d-e form a second pair. "lonely" has no edge.
    let edges = [
        edge(0, 1, 0.95, true),
        edge(2, 1, 0.96, true),
        edge(4, 5, 1.0, true),
    ];
    let groups = build_groups(&imgs, &edges);
    assert_eq!(groups.len(), 2);
    let all: Vec<&ImageInfo> = groups
        .iter()
        .flat_map(|g| std::iter::once(&g.keeper).chain(g.duplicates.iter().map(|d| &d.info)))
        .collect();
    assert_eq!(all.len(), 5);
    assert!(all.iter().all(|i| i.path.as_os_str() != "lonely"));
    let chain = groups
        .iter()
        .find(|g| g.keeper.path.as_os_str() == "c")
        .expect("chain group");
    assert_eq!(chain.duplicates.len(), 2);
}

#[test]
fn score_is_zero_when_the_copy_has_no_direct_edge_to_the_keeper() {
    let imgs = vec![
        info("keep", 300, 300, 1),
        info("b", 200, 200, 1),
        info("c", 100, 100, 1),
    ];
    let edges = [edge(0, 1, 0.99, true), edge(1, 2, 0.95, true)];
    let g = &build_groups(&imgs, &edges)[0];
    assert_eq!(g.keeper.path, PathBuf::from("keep"));
    let score = |n: &str| {
        g.duplicates
            .iter()
            .find(|d| d.info.path.as_os_str() == n)
            .unwrap()
            .ssim
    };
    assert_eq!(score("b"), 0.99);
    assert_eq!(score("c"), 0.0);
}

#[test]
fn edge_order_does_not_matter_for_scores() {
    let imgs = vec![info("keep", 300, 300, 1), info("b", 200, 200, 1)];
    let g = &build_groups(&imgs, &[edge(1, 0, 0.93, true)])[0];
    assert_eq!(g.duplicates[0].ssim, 0.93);
}

#[test]
fn one_low_edge_demotes_the_whole_group() {
    let imgs = vec![
        info("a", 30, 30, 1),
        info("b", 20, 20, 1),
        info("c", 10, 10, 1),
    ];
    let all_high = build_groups(&imgs, &[edge(0, 1, 0.99, true), edge(1, 2, 0.99, true)]);
    assert_eq!(all_high[0].confidence, Confidence::High);
    let one_low = build_groups(&imgs, &[edge(0, 1, 0.99, true), edge(1, 2, 0.92, false)]);
    assert_eq!(one_low[0].confidence, Confidence::Low);
}

#[test]
fn groups_are_ordered_by_reclaimable_bytes() {
    let imgs = vec![
        info("k1", 20, 20, 10),
        info("d1", 10, 10, 10),
        info("k2", 20, 20, 10),
        info("d2", 10, 10, 5_000),
    ];
    let groups = build_groups(&imgs, &[edge(0, 1, 1.0, true), edge(2, 3, 1.0, true)]);
    assert_eq!(groups[0].keeper.path, PathBuf::from("k2"));
    assert_eq!(groups[0].reclaimable_bytes(), 5_000);
    assert_eq!(groups[1].reclaimable_bytes(), 10);
}

#[test]
fn no_edges_means_no_groups() {
    let imgs = vec![info("a", 1, 1, 1), info("b", 1, 1, 1)];
    assert!(build_groups(&imgs, &[]).is_empty());
    assert!(build_groups(&[], &[]).is_empty());
}
