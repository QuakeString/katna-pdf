// SPDX-License-Identifier: GPL-3.0-or-later

//! Signing and checking, with certificate files made by OpenSSL
//! (`tests/data`: a test root, and "Asha Verma" (RSA) and "Ravi EC"
//! (P-256) under it; password `katna`).

use std::io::Write as _;

use katna_pdf_cos::{
    Allowed, AnnotChanges, AnnotKind, Annotation, Change, Coverage, Document, Object, Placement,
    SignError, SignRequest, SigningInfo,
};
use katna_pdf_sign::{
    Digest, P12Error, Roots, Signer, SigningError, Trust, check, common_name, p12, sign,
};

fn data(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// A PDF of two pages with a classic xref table.
fn pdf() -> Vec<u8> {
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] >>",
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        write!(out, "{} 0 obj\n{o}\nendobj\n", i + 1).unwrap();
    }
    let xref = out.len();
    write!(out, "xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).unwrap();
    for o in offsets {
        writeln!(out, "{o:010} 00000 n ").unwrap();
    }
    write!(
        out,
        "trailer\n<< /Size {} /Root 1 0 R /ID [<00112233445566778899aabbccddeeff> <00112233445566778899aabbccddeeff>] >>\nstartxref\n{xref}\n%%EOF\n",
        objs.len() + 1
    )
    .unwrap();
    out
}

fn info() -> SigningInfo {
    SigningInfo {
        name: Some("Asha Verma".into()),
        reason: Some("Approved".into()),
        location: Some("Pune".into()),
        contact: None,
    }
}

#[test]
fn certificate_files_open_with_their_password() {
    for (file, name) in [
        ("rsa-aes.p12", "Asha Verma"),
        ("rsa-legacy.p12", "Asha Verma"),
        ("ec-p256.p12", "Ravi EC"),
    ] {
        let id = p12::open(&data(file), "katna").unwrap_or_else(|e| panic!("{file}: {e}"));
        assert_eq!(common_name(&id.certificate().tbs_certificate.subject), name);
        assert_eq!(id.chain().len(), 1, "{file}");
        assert_eq!(
            common_name(&id.chain()[0].tbs_certificate.subject),
            "Katna Test Root"
        );
        assert_eq!(
            p12::open(&data(file), "wrong").err(),
            Some(P12Error::Password),
            "{file}"
        );
    }
    assert_eq!(p12::open(b"not a p12", "x").err(), Some(P12Error::Invalid));
}

#[test]
fn a_signed_file_checks_out_until_changed() {
    let id = p12::open(&data("rsa-aes.p12"), "katna").unwrap();
    let doc = Document::load(pdf()).unwrap();
    let request = SignRequest::new(
        Placement::Visible {
            page: 1,
            rect: [20.0, 20.0, 220.0, 80.0],
            lines: katna_pdf_sign::appearance_lines(&id, "2026-10-18 18:30", Some("Approved")),
        },
        info(),
    );
    let signed = sign(&doc, &request, &id, Digest::Sha256).unwrap();
    let doc = Document::load(signed.clone()).unwrap();
    let checks = check(&doc, &signed, &Roots::new());
    assert_eq!(checks.len(), 1);
    // Nobody trusts the test root yet; once they do, Asha is vouched for.
    assert!(matches!(
        &checks[0].trust,
        Some(Trust::Untrusted { complete: true, .. })
    ));
    let mut roots = Roots::new();
    roots.add_file(&data("ca.pem"));
    assert_eq!(
        check(&doc, &signed, &roots)[0].trust,
        Some(Trust::Trusted {
            root: "Katna Test Root".into()
        })
    );
    let c = &checks[0];
    assert!(c.intact(), "{c:?}");
    assert_eq!(c.coverage, Coverage::Whole);
    assert_eq!(c.signature.page, Some(1));
    assert_eq!(c.signature.sub_filter, "ETSI.CAdES.detached");
    assert_eq!(c.signature.info.reason.as_deref(), Some("Approved"));
    let checked = c.result.as_ref().unwrap();
    assert_eq!(
        common_name(&checked.signer.tbs_certificate.subject),
        "Asha Verma"
    );
    assert_eq!(checked.certificates.len(), 2);
    assert_eq!(checked.certificate_named, Some(true));

    // A change after signing, as an update: still intact, for the
    // revision it signed.
    let note = Annotation::new(0, AnnotKind::Note);
    let changed = doc
        .annotate(&AnnotChanges {
            put: vec![note],
            remove: Vec::new(),
        })
        .unwrap();
    let doc2 = Document::load(changed.clone()).unwrap();
    let c = &check(&doc2, &changed, &Roots::new())[0];
    assert!(c.intact());
    assert_eq!(c.coverage, Coverage::Revision(signed.len()));

    // A byte changed in what was signed: not intact.
    let mut tampered = signed.clone();
    let at = tampered.windows(5).position(|w| w == b"/Page").unwrap();
    tampered[at + 1] = b'p';
    let c = &check(
        &Document::load(tampered.clone()).unwrap(),
        &tampered,
        &Roots::new(),
    )[0];
    assert!(!c.intact());
    assert!(!c.result.as_ref().unwrap().digest_matches);
}

#[test]
fn files_are_signed_again_and_with_every_key() {
    let doc = Document::load(pdf()).unwrap();
    let rsa = p12::open(&data("rsa-legacy.p12"), "katna").unwrap();
    let ec = p12::open(&data("ec-p256.p12"), "katna").unwrap();
    let once = sign(
        &doc,
        &SignRequest::new(Placement::Invisible, info()),
        &rsa,
        Digest::Sha384,
    )
    .unwrap();
    let doc = Document::load(once.clone()).unwrap();
    let twice = sign(
        &doc,
        &SignRequest::new(Placement::Invisible, SigningInfo::default()),
        &ec,
        Digest::Sha256,
    )
    .unwrap();
    let doc = Document::load(twice.clone()).unwrap();
    let checks = check(&doc, &twice, &Roots::new());
    assert_eq!(checks.len(), 2);
    assert!(checks.iter().all(|c| c.intact()), "{checks:?}");
    assert_eq!(checks[0].coverage, Coverage::Revision(once.len()));
    assert_eq!(checks[1].coverage, Coverage::Whole);
    assert_eq!(checks[0].signature.name, "Signature1");
    assert_eq!(checks[1].signature.name, "Signature2");
}

#[test]
fn certified_files_allow_only_the_changes_they_say() {
    let id = p12::open(&data("rsa-aes.p12"), "katna").unwrap();
    let ec = p12::open(&data("ec-p256.p12"), "katna").unwrap();
    let certify = |allowed| {
        let mut request = SignRequest::new(Placement::Invisible, info());
        request.certify = Some(allowed);
        // With a field for another signer.
        let form = Document::load(pdf())
            .unwrap()
            .add_signature_field(0, [10.0, 10.0, 110.0, 40.0], Some("Ravi".into()))
            .unwrap();
        let signed = sign(
            &Document::load(form).unwrap(),
            &request,
            &id,
            Digest::Sha256,
        )
        .unwrap();
        let doc = Document::load(signed).unwrap();
        assert_eq!(doc.certification(), Some(allowed));
        doc
    };
    let forbidden = |file: Vec<u8>| {
        let doc = Document::load(file.clone()).unwrap();
        let c = check(&doc, &file, &Roots::new()).remove(0);
        assert!(c.intact());
        c.forbidden
    };
    let annotated = |doc: &Document| {
        doc.annotate(&AnnotChanges {
            put: vec![Annotation::new(0, AnnotKind::Note)],
            remove: Vec::new(),
        })
        .unwrap()
    };
    let signed_again = |doc: &Document, placement| {
        sign(
            doc,
            &SignRequest::new(placement, SigningInfo::default()),
            &ec,
            Digest::Sha256,
        )
    };
    let field = |doc: &Document| Placement::Field {
        id: doc.form().iter().find(|f| f.name == "Ravi").unwrap().id,
        lines: vec!["Ravi".into()],
    };
    let rotated = |doc: &Document| {
        let page = doc.pages()[0];
        let Some(Object::Dict(mut dict)) = doc.get(page) else {
            panic!()
        };
        dict.set("Rotate", 90);
        let mut update = doc.update().unwrap();
        update.replace(page, Object::Dict(dict));
        update.finish()
    };

    // No changes: nothing at all, not even another signature.
    let locked = certify(Allowed::Nothing);
    assert_eq!(forbidden(annotated(&locked)), [Change::Commented]);
    assert!(matches!(
        signed_again(&locked, field(&locked)),
        Err(SigningError::Pdf(SignError::Certified))
    ));
    // Form filling: others may sign the fields it has, but not add
    // their own, comment or turn pages.
    let forms = certify(Allowed::FormFilling);
    assert_eq!(forbidden(signed_again(&forms, field(&forms)).unwrap()), []);
    assert!(matches!(
        signed_again(&forms, Placement::Invisible),
        Err(SigningError::Pdf(SignError::CertifiedFieldsOnly))
    ));
    let added = forms
        .add_signature_field(1, [10.0, 10.0, 110.0, 40.0], None)
        .unwrap();
    assert_eq!(forbidden(added), [Change::NewField]);
    assert_eq!(forbidden(annotated(&forms)), [Change::Commented]);
    assert_eq!(forbidden(rotated(&forms)), [Change::Pages]);
    // Comments too.
    let comments = certify(Allowed::Comments);
    assert_eq!(forbidden(annotated(&comments)), []);
    assert_eq!(forbidden(rotated(&comments)), [Change::Pages]);
    // Only the first signature may certify.
    let mut request = SignRequest::new(Placement::Invisible, info());
    request.certify = Some(Allowed::Comments);
    assert!(matches!(
        sign(&comments, &request, &id, Digest::Sha256),
        Err(SigningError::Pdf(SignError::AlreadySigned))
    ));
}
