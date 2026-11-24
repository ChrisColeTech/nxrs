//! PORT_MAP.tsv (the repository root) is well formed: every row names its tree, a file, its line count, where it went,
//! an action and a status from plan section 4, once. Completeness against the source trees needs the trees:
//! `py -I scripts/port_map.py --check` (it regenerates and compares when nxnative, Turbo and mkw-recomp are present).

use std::collections::HashSet;

#[test]
fn port_map_rows_are_well_formed() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../PORT_MAP.tsv");
    let text = std::fs::read_to_string(path).expect("PORT_MAP.tsv");
    let mut rows = text.lines().filter(|l| !l.starts_with('#'));
    assert_eq!(
        rows.next(),
        Some("tree\tpath\tlines\tdest\taction\tstatus\tnote")
    );
    const ACTIONS: [&str; 8] = [
        "PORT", "REWRITE", "DONOR", "DATA", "TOOL", "REF", "DROP", "DEFER",
    ];
    const STATUSES: [&str; 6] = [
        "Done",
        "Partial",
        "Not started",
        "Deferred",
        "Dropped",
        "Reference",
    ];
    let mut seen = HashSet::new();
    let mut trees = HashSet::new();
    for (i, row) in rows.enumerate() {
        let c: Vec<&str> = row.split('\t').collect();
        assert_eq!(c.len(), 7, "row {}: {row}", i + 2);
        assert!(
            ["NX", "TB", "MR"].contains(&c[0]),
            "row {}: tree {}",
            i + 2,
            c[0]
        );
        assert!(c[2].parse::<u64>().is_ok(), "row {}: lines {}", i + 2, c[2]);
        assert!(
            !c[3].is_empty() && ACTIONS.contains(&c[4]) && STATUSES.contains(&c[5]),
            "row {}: {row}",
            i + 2
        );
        assert!(seen.insert((c[0], c[1])), "duplicate {} {}", c[0], c[1]);
        trees.insert(c[0]);
    }
    assert_eq!(trees.len(), 3, "rows of all three trees");
    assert!(seen.len() > 5000, "{} rows", seen.len());
}
