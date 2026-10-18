// SPDX-License-Identifier: GPL-3.0-or-later

//! Signing and checking, with certificate files made by OpenSSL
//! (`tests/data`: a test root, and "Asha Verma" (RSA) and "Ravi EC"
//! (P-256) under it; password `katna`).

use std::io::Write as _;

use katna_pdf_cos::{
    Allowed, AnnotChanges, AnnotKind, Annotation, Change, Coverage, Document, FieldValue, Lock,
    Object, Placement, SignError, SignRequest, SigningInfo,
};
use katna_pdf_sign::{
    Digest, P12Error, Roots, Signer, SigningError, Trust, check, common_name, p12, sign,
};

fn data(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// A PDF of two pages with a classic xref table.
fn pdf() -> Vec<u8> {
    file(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] >>",
    ])
}

/// A PDF of `objs`, numbered from 1, the first the catalog.
fn file(objs: &[&str]) -> Vec<u8> {
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

#[test]
fn a_picture_shows_in_the_box() {
    let id = p12::open(&data("rsa-aes.p12"), "katna").unwrap();
    // Two pixels, one see-through, as PNG.
    let mut png = Vec::new();
    image::RgbaImage::from_raw(2, 1, vec![0, 0, 255, 255, 0, 0, 0, 0])
        .unwrap()
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let picture = katna_pdf_sign::picture(&png).unwrap();
    assert_eq!((picture.width, picture.height), (2, 1));
    assert_eq!(picture.alpha.as_deref(), Some(&[255, 0][..]));
    let mut request = SignRequest::new(
        Placement::Visible {
            page: 0,
            rect: [20.0, 20.0, 220.0, 80.0],
            lines: vec!["Asha Verma".into()],
        },
        info(),
    );
    request.picture = Some(picture);
    let signed = sign(
        &Document::load(pdf()).unwrap(),
        &request,
        &id,
        Digest::Sha256,
    )
    .unwrap();
    let doc = Document::load(signed.clone()).unwrap();
    assert!(check(&doc, &signed, &Roots::new())[0].intact());
    let images: Vec<_> = doc
        .object_numbers()
        .filter_map(|n| match doc.get(katna_pdf_cos::Ref::new(n, 0)) {
            Some(Object::Stream(s)) if s.dict.name("Subtype") == Some(b"Image") => Some(s),
            _ => None,
        })
        .collect();
    // The picture and its mask.
    assert_eq!(images.len(), 2);
    assert!(images.iter().any(|s| s.dict.get("SMask").is_some()));
}

/// Locked fields (FieldMDP): changing them after signing breaks the
/// signature's promise; other fields, and other signatures, do not.
#[test]
fn locked_fields_stay_as_signed() {
    let id = p12::open(&data("rsa-aes.p12"), "katna").unwrap();
    let form = file(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R] >> >>",
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Annots [4 0 R 5 0 R] >>",
        "<< /FT /Tx /T (Name) /V (Asha) /Type /Annot /Subtype /Widget /Rect [10 300 200 320] /P 3 0 R >>",
        "<< /FT /Tx /T (City) /V (Pune) /Type /Annot /Subtype /Widget /Rect [10 260 200 280] /P 3 0 R >>",
    ]);
    let signed = |lock: Lock| {
        let mut request = SignRequest::new(Placement::Invisible, info());
        request.lock = Some(lock);
        let doc = Document::load(form.clone()).unwrap();
        Document::load(sign(&doc, &request, &id, Digest::Sha256).unwrap()).unwrap()
    };
    let refill = |doc: &Document, name: &str, text: &str| {
        let field = doc.form().into_iter().find(|f| f.name == name).unwrap();
        let bytes = doc
            .fill(&[(field.id, FieldValue::Text(text.into()))])
            .unwrap();
        let doc = Document::load(bytes.clone()).unwrap();
        check(&doc, &bytes, &Roots::new()).remove(0)
    };

    let doc = signed(Lock::Include(vec!["Name".into()]));
    assert_eq!(
        doc.signatures()[0].lock,
        Some(Lock::Include(vec!["Name".into()]))
    );
    // Locked fields are read-only from then on.
    let name = doc.form().into_iter().find(|f| f.name == "Name").unwrap();
    assert!(name.read_only);
    assert!(
        !doc.form()
            .into_iter()
            .find(|f| f.name == "City")
            .unwrap()
            .read_only
    );
    let c = refill(&doc, "City", "Delhi");
    assert!(c.intact() && c.unlocked.is_empty() && c.problem().is_none());
    let c = refill(&doc, "Name", "Ravi");
    assert!(c.intact());
    assert_eq!(c.unlocked, ["Name"]);
    assert!(c.problem().unwrap().contains("Name"));

    let doc = signed(Lock::All);
    assert_eq!(refill(&doc, "City", "Delhi").unlocked, ["City"]);
    // Signature fields stay free to sign.
    let more = doc
        .add_signature_field(0, [10.0, 10.0, 110.0, 40.0], Some("Ravi".into()))
        .unwrap();
    let more = Document::load(more).unwrap();
    let ravi = more.form().into_iter().find(|f| f.name == "Ravi").unwrap();
    let ec = p12::open(&data("ec-p256.p12"), "katna").unwrap();
    let bytes = sign(
        &more,
        &SignRequest::new(
            Placement::Field {
                id: ravi.id,
                lines: Vec::new(),
            },
            SigningInfo::default(),
        ),
        &ec,
        Digest::Sha256,
    )
    .unwrap();
    let both = Document::load(bytes.clone()).unwrap();
    let checks = check(&both, &bytes, &Roots::new());
    assert!(checks.iter().all(|c| c.problem().is_none()), "{checks:?}");

    let doc = signed(Lock::Exclude(vec!["City".into()]));
    assert!(refill(&doc, "City", "Delhi").unlocked.is_empty());
    assert_eq!(refill(&doc, "Name", "Ravi").unlocked, ["Name"]);
}

/// An empty field may say what signing it locks; that is followed.
#[test]
fn a_field_s_own_lock_is_followed() {
    let id = p12::open(&data("rsa-aes.p12"), "katna").unwrap();
    let form = file(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R] >> >>",
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Annots [4 0 R 5 0 R] >>",
        "<< /FT /Tx /T (Name) /V (Asha) /Type /Annot /Subtype /Widget /Rect [10 300 200 320] /P 3 0 R >>",
        "<< /FT /Sig /T (Approver) /Lock << /Type /SigFieldLock /Action /All >> /Type /Annot /Subtype /Widget /Rect [10 10 110 40] /P 3 0 R >>",
    ]);
    let doc = Document::load(form).unwrap();
    let field = doc
        .form()
        .into_iter()
        .find(|f| f.name == "Approver")
        .unwrap();
    let request = SignRequest::new(
        Placement::Field {
            id: field.id,
            lines: Vec::new(),
        },
        info(),
    );
    let signed = Document::load(sign(&doc, &request, &id, Digest::Sha256).unwrap()).unwrap();
    assert_eq!(signed.signatures()[0].lock, Some(Lock::All));
}

/// Only certificates made for signing are offered to sign with: not a
/// certificate authority's.
#[test]
fn signing_certificates_are_told_apart() {
    use katna_pdf_sign::{Certificate, can_sign};
    use x509_cert::der::Decode;
    let asha = Certificate::from_der(&data("asha.der")).unwrap();
    let ca = Certificate::from_der(&data("ca.der")).unwrap();
    assert!(can_sign(&asha));
    assert!(!can_sign(&ca));
}
