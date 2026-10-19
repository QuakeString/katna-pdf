// SPDX-License-Identifier: GPL-3.0-or-later

//! The library: folders scanned, documents found by their text, moved
//! files found again, copies grouped, tags, ratings and collections.

use std::path::Path;

use katna_pdf_library::{Filter, Indexed, Library, Sort};
use katna_pdf_render::Hayro;

fn pdf(title: &str, words: &str) -> Vec<u8> {
    let lines: Vec<String> = words.lines().map(|l| format!("  {l}")).collect();
    katna_pdf_cos::text_pdf(title, &lines)
}

const LEASE: &str = "This lease is made between the landlord and the tenant.\n\
                     The monthly rent is due on the first day of every month.\n\
                     The deposit is returned when the tenant leaves the flat.";
const INVOICE: &str = "Invoice number 4411 for consulting work done in March.\n\
                       Payment within thirty days to the account given below.\n\
                       Thank you for your business and see you again soon.";

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("katna-pdf-library-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn folders_are_scanned_and_searched() {
    let dir = temp("scan");
    write(&dir.join("lease.pdf"), &pdf("Flat lease", LEASE));
    write(&dir.join("bills/invoice.PDF"), &pdf("Invoice", INVOICE));
    write(&dir.join(".hidden/secret.pdf"), &pdf("Hidden", LEASE));
    write(&dir.join("notes.txt"), b"not a pdf");
    write(&dir.join("broken.pdf"), b"%PDF-1.7 nothing here");

    let lib = Library::in_memory().unwrap();
    lib.add_folder(&dir).unwrap();
    let mut seen = 0;
    let report = lib.scan(&Hayro, &mut |_, total, _| seen = total).unwrap();
    assert_eq!(seen, 3);
    assert_eq!(report.added, 3, "{report:?}");
    assert!(report.failed.is_empty(), "{report:?}");

    let docs = lib.docs(&Filter::default()).unwrap();
    let names: Vec<String> = docs.iter().map(|d| d.name()).collect();
    assert_eq!(names, ["broken.pdf", "Flat lease", "Invoice"]);
    assert_eq!(docs[1].pages, 1);
    assert!(!docs[1].scanned && !docs[1].signed && !docs[1].locked);
    assert!(
        lib.cover(docs[1].id)
            .unwrap()
            .is_some_and(|c| c.width == 160)
    );

    let hits = lib.search("rent", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].doc, docs[1].id);
    assert!(hits[0].snippet.contains("[rent]"), "{}", hits[0].snippet);
    assert_eq!(lib.search("consult*", 10).unwrap()[0].doc, docs[2].id);
    assert!(lib.search("rent's (", 10).is_ok());

    // Nothing changed: nothing read again.
    let again = lib.scan(&Hayro, &mut |_, _, _| {}).unwrap();
    assert_eq!(again.unchanged, 3, "{again:?}");

    // Moved: the same document, its tags kept.
    lib.set_tags(docs[1].id, &["home".into(), " rent ".into(), "".into()])
        .unwrap();
    std::fs::rename(dir.join("lease.pdf"), dir.join("bills/lease-2026.pdf")).unwrap();
    let moved = lib.scan(&Hayro, &mut |_, _, _| {}).unwrap();
    assert_eq!(
        (moved.moved, moved.missing, moved.added),
        (1, 0, 0),
        "{moved:?}"
    );
    let lease = lib.doc(docs[1].id).unwrap().unwrap();
    assert!(lease.path.ends_with("bills/lease-2026.pdf"));
    assert_eq!(lease.tags, ["home", "rent"]);

    // Gone: kept as missing, out of the list.
    std::fs::remove_file(dir.join("bills/invoice.PDF")).unwrap();
    let gone = lib.scan(&Hayro, &mut |_, _, _| {}).unwrap();
    assert_eq!(gone.missing, 1);
    assert_eq!(lib.docs(&Filter::default()).unwrap().len(), 2);
    assert_eq!(
        lib.docs(&Filter::parse("missing")).unwrap()[0].name(),
        "Invoice"
    );

    // Changed: read again.
    write(
        &dir.join("bills/lease-2026.pdf"),
        &pdf("Flat lease 2026", LEASE),
    );
    let changed = lib
        .index_file(&Hayro, &dir.join("bills/lease-2026.pdf"))
        .unwrap();
    assert_eq!(changed, Indexed::Updated(lease.id));
    assert_eq!(
        lib.doc(lease.id).unwrap().unwrap().name(),
        "Flat lease 2026"
    );

    lib.remove_folder(&dir).unwrap();
    assert!(lib.docs(&Filter::default()).unwrap().is_empty());
    assert!(lib.search("rent", 10).unwrap().is_empty());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn copies_are_found_by_bytes_and_by_words() {
    let dir = temp("copies");
    let lease = pdf("Flat lease", LEASE);
    write(&dir.join("a.pdf"), &lease);
    write(&dir.join("b/a copy.pdf"), &lease);
    // The same words, a different file.
    write(
        &dir.join("c.pdf"),
        &pdf("Flat lease", &LEASE.to_uppercase()),
    );
    write(&dir.join("d.pdf"), &pdf("Invoice", INVOICE));
    let lib = Library::in_memory().unwrap();
    lib.add_folder(&dir).unwrap();
    lib.scan(&Hayro, &mut |_, _, _| {}).unwrap();
    let groups = lib.duplicates().unwrap();
    assert_eq!(groups.len(), 1);
    let mut names: Vec<String> = groups[0]
        .iter()
        .map(|d| d.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["a copy.pdf", "a.pdf", "c.pdf"]);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn filters_tags_ratings_and_collections() {
    let dir = temp("filters");
    write(&dir.join("x/lease.pdf"), &pdf("Flat lease", LEASE));
    write(&dir.join("y/invoice.pdf"), &pdf("Invoice", INVOICE));
    let lib = Library::in_memory().unwrap();
    lib.add_folder(&dir).unwrap();
    lib.scan(&Hayro, &mut |_, _, _| {}).unwrap();
    let docs = lib.docs(&Filter::default()).unwrap();
    let (lease, invoice) = (docs[0].id, docs[1].id);
    lib.set_tags(invoice, &["Tax".into(), "to pay".into()])
        .unwrap();
    lib.set_rating(lease, 9).unwrap();
    lib.set_note(lease, "Renew in May").unwrap();
    lib.mark_opened(invoice).unwrap();

    let ids = |q: &str| -> Vec<i64> {
        lib.docs(&Filter::parse(q))
            .unwrap()
            .iter()
            .map(|d| d.id)
            .collect()
    };
    assert_eq!(ids("tag:tax"), [invoice]);
    assert_eq!(ids(r#"tag:"to pay" tag:tax"#), [invoice]);
    assert_eq!(ids("rating:4"), [lease]);
    assert_eq!(ids("lease"), [lease]);
    assert_eq!(ids("100%_"), Vec::<i64>::new());
    let canon = std::fs::canonicalize(&dir).unwrap();
    assert_eq!(
        ids(&format!("in:\"{}\"", canon.join("y").display())),
        [invoice]
    );
    assert_eq!(ids("signed:no forms:no sort:opened"), [invoice, lease]);
    assert_eq!(lib.doc(lease).unwrap().unwrap().rating, 5);
    assert_eq!(lib.doc(lease).unwrap().unwrap().note, "Renew in May");
    assert_eq!(
        lib.tags().unwrap(),
        [("Tax".into(), 1), ("to pay".into(), 1)]
    );

    let f = Filter {
        sort: Sort::Rating,
        ..Filter::default()
    };
    assert_eq!(lib.docs(&f).unwrap()[0].id, lease);

    lib.set_collection("Bills", "tag:tax").unwrap();
    assert_eq!(
        lib.collections().unwrap(),
        [("Bills".into(), "tag:tax".into())]
    );
    lib.set_collection("Bills", "").unwrap();
    assert!(lib.collections().unwrap().is_empty());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_catalog_is_kept_in_a_file() {
    let dir = temp("file");
    write(&dir.join("docs/lease.pdf"), &pdf("Flat lease", LEASE));
    let db = dir.join("cat/library.sqlite");
    {
        let lib = Library::open(&db).unwrap();
        lib.add_folder(&dir.join("docs")).unwrap();
        lib.scan(&Hayro, &mut |_, _, _| {}).unwrap();
    }
    // Closed before its folder is taken away, which Windows needs.
    {
        let lib = Library::open(&db).unwrap();
        assert_eq!(lib.folders().unwrap().len(), 1);
        assert_eq!(lib.search("deposit", 5).unwrap().len(), 1);
        let id = lib.doc_at(&dir.join("docs/lease.pdf")).unwrap().unwrap();
        assert_eq!(lib.doc(id).unwrap().unwrap().name(), "Flat lease");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
