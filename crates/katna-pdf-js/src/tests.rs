// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

/// A PDF of `objects` (numbered from 1; 1 is the catalog, 2 the pages,
/// 3 the page) with an xref table worked out.
fn pdf(objects: &[String]) -> Vec<u8> {
    let mut out = String::from("%PDF-1.7\n");
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.push_str(&format!("{} 0 obj\n{o}\nendobj\n", i + 1));
    }
    let xref = out.len();
    out.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        objects.len() + 1
    ));
    for o in offsets {
        out.push_str(&format!("{o:010} 00000 n \n"));
    }
    out.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    ));
    out.into_bytes()
}

fn js(code: &str) -> String {
    format!(
        "<< /S /JavaScript /JS ({}) >>",
        code.replace('(', "\\(").replace(')', "\\)")
    )
}

/// A form: Qty and Price typed, Total = Qty × Price by a script of its
/// own, Price checked to be under 1000, a Note hidden unless Qty > 10,
/// a button that resets Qty, and a document script with a helper.
fn order_form() -> Document {
    let widget = |name: &str, rect: &str, extra: &str| {
        format!(
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T ({name}) /Rect [{rect}] /P 3 0 R /DA (/Helv 10 Tf 0 g) {extra} >>"
        )
    };
    let objects = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R 6 0 R 7 0 R 8 0 R] /CO [6 0 R] >> /Names << /JavaScript 9 0 R >> >>".to_string(),
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Annots [4 0 R 5 0 R 6 0 R 7 0 R 8 0 R] >>".to_string(),
        widget(
            "Qty",
            "10 700 110 720",
            &format!(
                "/AA << /K {} /V {} >>",
                js("AFNumber_Keystroke(0, 0, 0, 0, \"\", true);"),
                js("var n = this.getField(\"Note\"); n.display = event.value > 10 ? display.visible : display.hidden;")
            ),
        ),
        widget(
            "Price",
            "10 650 110 670",
            &format!(
                "/AA << /V {} /F {} >>",
                js("AFRange_Validate(true, 0, true, 1000);"),
                js("AFNumber_Format(2, 0, 0, 0, \"Rs \", true);")
            ),
        ),
        widget(
            "Total",
            "10 600 110 620",
            &format!(
                "/Ff 1 /AA << /C {} /F {} >>",
                js("event.value = times(this.getField(\"Qty\").value, this.getField(\"Price\").value);"),
                js("AFNumber_Format(2, 0, 0, 0, \"\", true);")
            ),
        ),
        widget("Note", "10 550 300 570", "/F 6 /V (Bulk order)"),
        format!(
            "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (Clear) /Rect [400 700 500 720] /P 3 0 R /A {} >>",
            js("event.target.doc.resetForm([\"Qty\"]); app.alert(\"Cleared\"); app.launchURL(\"https://example.com\");")
        ),
        "<< /Names [(helpers) 10 0 R] >>".to_string(),
        js("function times(a, b) { return (Number(a) || 0) * (Number(b) || 0); }"),
    ];
    Document::load(pdf(&objects)).unwrap()
}

#[test]
fn calculations_run_in_the_forms_order() {
    let doc = order_form();
    let fields = doc.form();
    let mut form = Form::new(&doc, &fields, "order.pdf").unwrap();
    assert!(form.errors.is_empty(), "{:?}", form.errors);
    let out = form.commit("Qty", "3");
    assert!(out.accepted);
    assert!(out.values.contains(&("Qty".into(), "3".into())));
    let out = form.commit("Price", "250.5");
    assert!(out.accepted, "{out:?}");
    assert!(
        out.values.contains(&("Total".into(), "751.5".into())),
        "{out:?}"
    );
    let shown = form.shown();
    assert_eq!(shown["Price"], "Rs 250.50");
    assert_eq!(shown["Total"], "751.50");
}

#[test]
fn keystroke_and_validate_turn_values_down() {
    let doc = order_form();
    let fields = doc.form();
    let mut form = Form::new(&doc, &fields, "order.pdf").unwrap();
    let out = form.commit("Qty", "three");
    assert!(!out.accepted);
    assert_eq!(out.alerts.len(), 1, "{out:?}");
    assert!(out.alerts[0].contains("Qty"));
    assert_eq!(form.value("Qty"), "");
    let out = form.commit("Price", "5000");
    assert!(!out.accepted);
    assert!(
        out.alerts[0].contains("less than or equal to 1000"),
        "{out:?}"
    );
}

#[test]
fn scripts_show_and_hide_fields_and_buttons_reset() {
    let doc = order_form();
    let fields = doc.form();
    let mut form = Form::new(&doc, &fields, "order.pdf").unwrap();
    let out = form.commit("Qty", "12");
    assert!(
        out.display.contains(&("Note".into(), Display::Visible)),
        "{out:?}"
    );
    let out = form.commit("Qty", "2");
    assert!(out.display.contains(&("Note".into(), Display::Hidden)));
    let out = form.click("Clear");
    assert_eq!(out.alerts, vec!["Cleared".to_string()]);
    assert_eq!(
        out.requests,
        vec![Request::Url("https://example.com".into())]
    );
    assert!(
        out.values.contains(&("Qty".into(), String::new())),
        "{out:?}"
    );
}

#[test]
fn the_filled_file_shows_what_the_scripts_say() {
    let doc = order_form();
    let fields = doc.form();
    let mut form = Form::new(&doc, &fields, "order.pdf").unwrap();
    let a = form.commit("Qty", "12");
    let b = form.commit("Price", "100");
    let mut changes = form.changes(&a, &fields);
    for c in form.changes(&b, &fields) {
        changes.retain(|(r, _)| *r != c.0);
        changes.push(c);
    }
    let extras = form.extras(&[a, b]);
    let filled = Document::load(doc.fill_with(&changes, &extras).unwrap()).unwrap();
    let after = filled.form();
    let total = after.iter().find(|f| f.name == "Total").unwrap();
    assert_eq!(total.value.as_deref(), Some("1200"));
    let note = after.iter().find(|f| f.name == "Note").unwrap();
    assert!(!note.widgets[0].hidden);
    // The total's appearance holds its formatted text.
    let id = total.widgets[0].id;
    let Some(katna_pdf_cos::Object::Dict(w)) = filled.get(id) else {
        panic!()
    };
    let ap = format!("{:?}", filled.resolve_key(&w, "AP"));
    assert!(!ap.is_empty());
}

#[test]
fn runaway_scripts_are_stopped() {
    let objects = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R] >> >>".to_string(),
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Annots [4 0 R] >>".to_string(),
        format!(
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (A) /Rect [0 0 10 10] /P 3 0 R /AA << /V {} >> >>",
            js("while (true) {}")
        ),
    ];
    let doc = Document::load(pdf(&objects)).unwrap();
    let fields = doc.form();
    let mut form = Form::new(&doc, &fields, "loop.pdf").unwrap();
    let out = form.commit("A", "1");
    assert_eq!(out.errors.len(), 1);
    assert!(out.accepted);
}

#[test]
fn util_formats_like_acrobat() {
    let doc = order_form();
    let fields = doc.form();
    let mut form = Form::new(&doc, &fields, "order.pdf").unwrap();
    let eval = |form: &mut Form, code: &str| {
        form.ctx
            .eval(Source::from_bytes(code))
            .unwrap()
            .to_string(&mut form.ctx)
            .unwrap()
            .to_std_string_escaped()
    };
    assert_eq!(
        eval(&mut form, "util.printf('%,0.2f', 1234567.891)"),
        "1,234,567.89"
    );
    assert_eq!(
        eval(&mut form, "util.printf('%05d|%s', 42, 'x')"),
        "00042|x"
    );
    assert_eq!(
        eval(
            &mut form,
            "util.printd('dd/mm/yyyy HH:MM', new Date(2026, 10, 23, 7, 5))"
        ),
        "23/11/2026 07:05"
    );
    assert_eq!(
        eval(
            &mut form,
            "util.printd('mmmm d', util.scand('dd/mm/yyyy', '05/01/2026'))"
        ),
        "January 5"
    );
    assert_eq!(
        eval(&mut form, "util.printx('(999) 999-9999', '2025551234')"),
        "(202) 555-1234"
    );
    assert_eq!(eval(&mut form, "AFMakeNumber('₹ 1,250.50')"), "1250.5");
    assert_eq!(eval(&mut form, "AFMakeNumber('12,5')"), "12.5");
    assert_eq!(
        eval(&mut form, "typeof this.getField('Qty').value"),
        "string"
    );
}

#[test]
fn simplified_field_notation_runs() {
    let scripts = katna_pdf_cos::Scripts {
        calculate: Some(katna_pdf_cos::Calculation::Expr("Qty * (Price + 5)".into())),
        ..Default::default()
    };
    let [Some((_, sum)), ..] = katna_pdf_cos::script_text(&scripts) else {
        panic!()
    };
    let widget = |name: &str, rect: &str, extra: &str| {
        format!(
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T ({name}) /Rect [{rect}] /P 3 0 R /DA (/Helv 10 Tf 0 g) {extra} >>"
        )
    };
    let objects = vec![
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R 6 0 R] /CO [6 0 R] >> >>"
            .to_string(),
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Annots [4 0 R 5 0 R 6 0 R] >>"
            .to_string(),
        widget("Qty", "10 700 110 720", ""),
        widget("Price", "10 650 110 670", ""),
        widget(
            "Total",
            "10 600 110 620",
            &format!("/AA << /C {} >>", js(&sum)),
        ),
    ];
    let doc = Document::load(pdf(&objects)).unwrap();
    let fields = doc.form();
    let mut form = Form::new(&doc, &fields, "sum.pdf").unwrap();
    assert!(form.errors.is_empty(), "{:?}", form.errors);
    form.commit("Qty", "3");
    let out = form.commit("Price", "10");
    assert!(
        out.values.contains(&("Total".into(), "45".into())),
        "{out:?}"
    );
}
