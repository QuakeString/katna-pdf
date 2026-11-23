// SPDX-License-Identifier: GPL-3.0-or-later

//! Comments hidden: a test of its own, since the setting is the whole
//! process's and other tests draw at the same time.

use std::sync::Arc;

use katna_pdf_cos::{AnnotChanges, AnnotKind, Annotation, Document as Cos, text_pdf};
use katna_pdf_render::{Bitmap, Engine, Hayro};

#[test]
fn comments_can_be_hidden() {
    let cos = Cos::load(text_pdf("Page", &[])).unwrap();
    let mut square = Annotation::new(
        0,
        AnnotKind::Square {
            fill: Some([1.0, 0.0, 0.0]),
        },
    );
    square.rect = [100.0, 300.0, 500.0, 500.0];
    square.colour = [1.0, 0.0, 0.0];
    let marked = cos
        .annotate(&AnnotChanges {
            put: vec![square],
            remove: vec![],
        })
        .unwrap();
    let doc = Hayro.open(Arc::new(marked), "").unwrap();
    let red = |b: &Bitmap| {
        let i = ((b.height / 2) * b.width + b.width / 2) as usize * 4;
        b.rgba[i] > 200 && b.rgba[i + 1] < 80
    };
    assert!(red(&doc.render(0, 1.0).unwrap()));
    katna_pdf_render::show_comments(false);
    let hidden = doc.render(0, 1.0).unwrap();
    katna_pdf_render::show_comments(true);
    assert!(!red(&hidden));
    assert!(red(&doc.render(0, 1.0).unwrap()));
}
