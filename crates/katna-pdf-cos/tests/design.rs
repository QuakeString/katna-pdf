// SPDX-License-Identifier: GPL-3.0-or-later

//! Designing forms: fields added, changed, moved, copied, taken out and
//! put in Tab order, and still fillable afterwards.

use katna_pdf_cos::{
    Document, FieldEdit, FieldKind, FieldSpec, FieldValue, FormDesign, FormError, NewField, Object,
    text_pdf,
};

/// A blank file of `pages` pages.
fn blank(pages: usize) -> Document {
    let lines: Vec<String> = (0..pages * 60).map(|i| format!("line {i}")).collect();
    let doc = Document::load(text_pdf("Form", &lines)).unwrap();
    assert!(doc.pages().len() >= pages);
    doc
}

fn spec(page: usize, y: f32, name: &str, kind: NewField) -> FieldSpec {
    FieldSpec {
        page,
        rect: [100.0, y, 250.0, y + 20.0],
        name: name.into(),
        kind,
        tooltip: String::new(),
        required: false,
        read_only: false,
        value: None,
        scripts: katna_pdf_cos::Scripts::default(),
        look: katna_pdf_cos::Look::default(),
        action: None,
    }
}

fn every_kind() -> Vec<FieldSpec> {
    let mut name = spec(
        0,
        700.0,
        "Name",
        NewField::Text {
            multiline: false,
            password: false,
            max_len: Some(40),
            comb: false,
        },
    );
    name.tooltip = "Your full name".into();
    name.required = true;
    let mut agree = spec(0, 660.0, "Agree", NewField::CheckBox);
    agree.rect = [100.0, 660.0, 114.0, 674.0];
    let mut yes = spec(
        0,
        620.0,
        "Answer",
        NewField::Radio {
            choice: "Yes".into(),
        },
    );
    yes.rect = [100.0, 620.0, 114.0, 634.0];
    yes.value = Some(String::new());
    let mut no = spec(
        0,
        620.0,
        "Answer",
        NewField::Radio {
            choice: "No".into(),
        },
    );
    no.rect = [140.0, 620.0, 154.0, 634.0];
    let options = vec!["Delhi".to_string(), "Kolkata".into(), "Pune".into()];
    let mut city = spec(
        0,
        580.0,
        "City",
        NewField::Choice {
            options: options.clone(),
            combo: true,
            editable: false,
        },
    );
    city.value = Some("Pune".into());
    let mut list = spec(
        0,
        480.0,
        "Cities",
        NewField::Choice {
            options,
            combo: false,
            editable: false,
        },
    );
    list.rect = [100.0, 480.0, 250.0, 540.0];
    vec![
        name,
        agree,
        yes,
        no,
        city,
        list,
        spec(
            0,
            440.0,
            "Send",
            NewField::PushButton {
                caption: "Send".into(),
            },
        ),
        spec(0, 380.0, "Signature", NewField::Signature),
    ]
}

fn designed(doc: &Document, design: &FormDesign) -> Document {
    Document::load(doc.design_form(design).unwrap()).unwrap()
}

#[test]
fn every_kind_of_field_is_added_and_fillable() {
    let doc = designed(
        &blank(1),
        &FormDesign {
            add: every_kind(),
            ..FormDesign::default()
        },
    );
    let form = doc.form();
    let names: Vec<&str> = form.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Name",
            "Agree",
            "Answer",
            "City",
            "Cities",
            "Send",
            "Signature"
        ]
    );
    let name = &form[0];
    assert!(matches!(
        name.kind,
        FieldKind::Text {
            max_len: Some(40),
            ..
        }
    ));
    assert!(name.required);
    assert_eq!(name.tooltip.as_deref(), Some("Your full name"));
    let answer = &form[2];
    assert_eq!(answer.kind, FieldKind::Radio);
    assert_eq!(answer.widgets.len(), 2);
    assert_eq!(answer.value.as_deref(), Some("Yes"));
    let ons: Vec<_> = answer.widgets.iter().map(|w| w.on.clone()).collect();
    assert_eq!(ons, [Some("Yes".into()), Some("No".into())]);
    assert_eq!(form[1].kind, FieldKind::CheckBox);
    assert_eq!(form[1].widgets[0].on.as_deref(), Some("Yes"));
    assert!(matches!(
        form[3].kind,
        FieldKind::Choice { combo: true, .. }
    ));
    assert_eq!(form[3].value.as_deref(), Some("Pune"));
    assert!(matches!(
        form[4].kind,
        FieldKind::Choice { combo: false, .. }
    ));
    assert_eq!(form[5].kind, FieldKind::PushButton);
    assert_eq!(form[6].kind, FieldKind::Signature);
    // Every widget is on the page and has an appearance.
    for f in &form {
        for w in &f.widgets {
            assert_eq!(w.page, 0);
            let Some(Object::Dict(d)) = doc.get(w.id) else {
                panic!()
            };
            assert!(d.get("AP").is_some(), "{}", f.name);
        }
    }
    // Filled like any other form.
    let filled = Document::load(
        doc.fill(&[
            (form[0].id, FieldValue::Text("Asha Rao".into())),
            (form[1].id, FieldValue::State(Some("Yes".into()))),
            (form[2].id, FieldValue::State(Some("No".into()))),
        ])
        .unwrap(),
    )
    .unwrap();
    let form = filled.form();
    assert_eq!(form[0].value.as_deref(), Some("Asha Rao"));
    assert_eq!(form[1].value.as_deref(), Some("Yes"));
    assert_eq!(form[2].value.as_deref(), Some("No"));
}

#[test]
fn fields_are_changed_moved_and_taken_out() {
    let doc = designed(
        &blank(1),
        &FormDesign {
            add: every_kind(),
            ..FormDesign::default()
        },
    );
    let form = doc.form();
    let mut rename = FieldEdit::new(form[0].id);
    rename.name = Some("FullName".into());
    rename.tooltip = Some(String::new());
    rename.required = Some(false);
    rename.read_only = Some(true);
    rename.multiline = Some(true);
    let mut cities = FieldEdit::new(form[4].id);
    cities.options = Some(vec!["Chennai".into()]);
    let changed = designed(
        &doc,
        &FormDesign {
            edit: vec![rename, cities],
            place: vec![(form[5].widgets[0].id, [300.0, 300.0, 400.0, 330.0])],
            remove: vec![form[2].id],
            ..FormDesign::default()
        },
    );
    let after = changed.form();
    assert_eq!(after.len(), form.len() - 1);
    assert!(after.iter().all(|f| f.name != "Answer"));
    let name = &after[0];
    assert_eq!(name.name, "FullName");
    assert!(name.read_only && !name.required);
    assert_eq!(name.tooltip, None);
    assert!(matches!(
        name.kind,
        FieldKind::Text {
            multiline: true,
            ..
        }
    ));
    let list = after.iter().find(|f| f.name == "Cities").unwrap();
    assert_eq!(
        list.kind,
        FieldKind::Choice {
            options: vec!["Chennai".into()],
            combo: false
        }
    );
    let send = after.iter().find(|f| f.name == "Send").unwrap();
    assert_eq!(send.widgets[0].rect, [300.0, 300.0, 400.0, 330.0]);
    // The radio buttons left the page too.
    let page = changed.get(changed.pages()[0]).unwrap();
    let annots = match changed.resolve(page.as_dict().unwrap().get("Annots").unwrap()) {
        Object::Array(a) => a,
        _ => panic!(),
    };
    for w in &form[2].widgets {
        assert!(!annots.contains(&Object::Ref(w.id)));
    }
    // A name another field has is refused.
    let mut clash = FieldEdit::new(after[0].id);
    clash.name = Some("City".into());
    assert_eq!(
        changed.design_form(&FormDesign {
            edit: vec![clash],
            ..FormDesign::default()
        }),
        Err(FormError::NameTaken("City".into()))
    );
    let other = spec(0, 100.0, "City", NewField::CheckBox);
    assert_eq!(
        changed.design_form(&FormDesign {
            add: vec![other],
            ..FormDesign::default()
        }),
        Err(FormError::NameTaken("City".into()))
    );
}

#[test]
fn fields_are_copied_to_other_pages_and_ordered() {
    let doc = blank(2);
    let text = NewField::Text {
        multiline: false,
        password: false,
        max_len: None,
        comb: false,
    };
    let doc = designed(
        &doc,
        &FormDesign {
            add: vec![
                spec(0, 700.0, "Ref", text.clone()),
                spec(0, 600.0, "Second", text.clone()),
                spec(0, 500.0, "Third", text.clone()),
            ],
            ..FormDesign::default()
        },
    );
    let form = doc.form();
    let copied = designed(
        &doc,
        &FormDesign {
            copy: vec![(form[0].id, vec![1])],
            order: vec![(
                0,
                vec![
                    form[2].widgets[0].id,
                    form[0].widgets[0].id,
                    form[1].widgets[0].id,
                ],
            )],
            ..FormDesign::default()
        },
    );
    let after = copied.form();
    let field = after.iter().find(|f| f.name == "Ref").unwrap();
    let pages: Vec<usize> = field.widgets.iter().map(|w| w.page).collect();
    assert_eq!(pages, [0, 1]);
    assert_eq!(field.widgets[0].rect, field.widgets[1].rect);
    // One value, shown on both pages.
    let filled = Document::load(
        copied
            .fill(&[(field.id, FieldValue::Text("K-12".into()))])
            .unwrap(),
    )
    .unwrap();
    let field = filled.form().into_iter().find(|f| f.name == "Ref").unwrap();
    assert_eq!(field.value.as_deref(), Some("K-12"));
    // Tab goes Third, then Ref (its widget now a kid), then Second.
    let page = copied.get(copied.pages()[0]).unwrap();
    let annots: Vec<Object> = match copied.resolve(page.as_dict().unwrap().get("Annots").unwrap()) {
        Object::Array(a) => a,
        _ => panic!(),
    };
    let ids: Vec<Object> = [
        after.iter().find(|f| f.name == "Third").unwrap().widgets[0].id,
        after.iter().find(|f| f.name == "Ref").unwrap().widgets[0].id,
        after.iter().find(|f| f.name == "Second").unwrap().widgets[0].id,
    ]
    .map(Object::Ref)
    .to_vec();
    assert_eq!(annots, ids);
}

#[test]
fn form_data_goes_out_and_comes_back() {
    let doc = designed(
        &blank(1),
        &FormDesign {
            add: every_kind(),
            ..FormDesign::default()
        },
    );
    let before = doc.form_data();
    let names: Vec<&str> = before.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["Name", "Agree", "Answer", "City", "Cities"]);
    let json = r#"{"Name": "Asha Rao\nPune", "Agree": true, "Answer": "No", "Cities": "Kolkata"}"#;
    let data = katna_pdf_cos::data_from_json(json).unwrap();
    let filled = Document::load(doc.fill_data(&data).unwrap()).unwrap();
    let after = filled.form_data();
    let value = |name: &str| after.iter().find(|(n, _)| n == name).unwrap().1.clone();
    assert_eq!(value("Name"), "Asha Rao\nPune");
    assert_eq!(value("Agree"), "Yes");
    assert_eq!(value("Answer"), "No");
    assert_eq!(value("City"), "Pune");
    assert_eq!(value("Cities"), "Kolkata");
    // Through CSV and back, nothing changes.
    let csv = katna_pdf_cos::data_to_csv(&after);
    let again = Document::load(
        filled
            .fill_data(&katna_pdf_cos::data_from_csv(&csv).unwrap())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(again.form_data(), after);
    // Off again; and what cannot be is refused.
    let off = Document::load(
        filled
            .fill_data(&[
                ("Agree".into(), "no".into()),
                ("Answer".into(), "Off".into()),
            ])
            .unwrap(),
    )
    .unwrap();
    assert_eq!(off.form_data()[1].1, "Off");
    assert_eq!(off.form_data()[2].1, "Off");
    assert!(doc.fill_data(&[("Answer".into(), "Maybe".into())]).is_err());
    assert!(doc.fill_data(&[("Cities".into(), "Agra".into())]).is_err());
    assert!(doc.fill_data(&[("Nobody".into(), "x".into())]).is_err());
}

/// The text a widget's appearance draws, decoded.
fn appearance(doc: &Document, widget: katna_pdf_cos::Ref) -> String {
    let Some(Object::Dict(d)) = doc.get(widget) else {
        panic!()
    };
    let Object::Dict(ap) = doc.resolve(d.get("AP").unwrap()) else {
        panic!()
    };
    let Object::Stream(s) = doc.resolve(ap.get("N").unwrap()) else {
        panic!()
    };
    String::from_utf8_lossy(&katna_pdf_cos::decode(&s.dict, &s.data).unwrap()).into_owned()
}

#[test]
fn totals_add_up_and_formats_show() {
    use katna_pdf_cos::{CalcOp, Calculation, Format};
    let text = NewField::Text {
        multiline: false,
        password: false,
        max_len: None,
        comb: false,
    };
    let amount = Format::Number {
        decimals: 2,
        separators: 0,
        negative: 0,
        currency: "Rs. ".into(),
        currency_first: true,
    };
    let mut fee = spec(0, 700.0, "Fee", text.clone());
    fee.scripts.format = Some(amount.clone());
    let mut tax = spec(0, 660.0, "Tax", text.clone());
    tax.scripts.format = Some(amount.clone());
    let mut total = spec(0, 620.0, "Total", text.clone());
    total.scripts.calculate = Some(Calculation::Simple {
        op: CalcOp::Sum,
        fields: vec!["Fee".into(), "Tax".into()],
    });
    total.scripts.format = Some(amount);
    let mut date = spec(0, 580.0, "Date", text);
    date.scripts.format = Some(Format::Date("dd/mm/yyyy".into()));
    let doc = designed(
        &blank(1),
        &FormDesign {
            add: vec![fee, tax, total, date],
            ..FormDesign::default()
        },
    );
    let form = doc.form();
    let scripts = doc.form_scripts(&form);
    assert_eq!(scripts.len(), 4);
    assert!(scripts[&form[2].id].calculate.is_some());
    let filled = Document::load(
        doc.fill_data(&[
            ("Fee".into(), "1200".into()),
            ("Tax".into(), "34.5".into()),
            ("Date".into(), "5/1/2026".into()),
        ])
        .unwrap(),
    )
    .unwrap();
    let data = filled.form_data();
    let value = |name: &str| data.iter().find(|(n, _)| n == name).unwrap().1.clone();
    // The value stays a plain number; only what is shown is formatted.
    assert_eq!(value("Fee"), "1200");
    assert_eq!(value("Total"), "1234.5");
    let after = filled.form();
    let shown = |i: usize| appearance(&filled, after[i].widgets[0].id);
    assert!(shown(0).contains("Rs. 1,200.00"), "{}", shown(0));
    assert!(shown(2).contains("Rs. 1,234.50"), "{}", shown(2));
    assert!(shown(3).contains("05/01/2026"), "{}", shown(3));
    // Changing one part changes the total again.
    let again = Document::load(filled.fill_data(&[("Tax".into(), "0".into())]).unwrap()).unwrap();
    let total = again.form_data();
    assert_eq!(total.iter().find(|(n, _)| n == "Total").unwrap().1, "1200");
}

#[test]
fn form_data_goes_through_xfdf_and_fdf() {
    let doc = designed(
        &blank(1),
        &FormDesign {
            add: every_kind(),
            ..FormDesign::default()
        },
    );
    let data = [
        ("Name".to_string(), "आशा राव\n(Pune)".to_string()),
        ("Agree".into(), "Yes".into()),
        ("Answer".into(), "No".into()),
        ("Cities".into(), "Kolkata".into()),
    ];
    let filled = Document::load(doc.fill_data(&data).unwrap()).unwrap();
    let want = filled.form_data();
    for name in ["out.xfdf", "out.fdf", "out.csv", "out.json"] {
        let bytes = filled.write_data(name, "form.pdf");
        let back = katna_pdf_cos::read_data(name, &bytes).unwrap();
        assert_eq!(back, want, "{name}");
        let again = Document::load(doc.fill_data(&back).unwrap()).unwrap();
        assert_eq!(again.form_data(), want, "{name}");
    }
    // Check boxes and radio groups are names in FDF.
    let fdf = String::from_utf8_lossy(&filled.write_data("x.fdf", "form.pdf")).into_owned();
    assert!(fdf.contains("/V /No"), "{fdf}");

    // Nested fields, as Acrobat writes them.
    let xfdf = r#"<?xml version="1.0" encoding="UTF-8"?>
<xfdf xmlns="http://ns.adobe.com/xfdf/"><fields>
<field name="Applicant"><field name="Name"><value>Asha &amp; Ravi</value></field>
<field name="City"><value>Pune</value></field></field>
<field name="Empty"/>
</fields></xfdf>"#;
    assert_eq!(
        katna_pdf_cos::data_from_xfdf(xfdf).unwrap(),
        [
            ("Applicant.Name".to_string(), "Asha & Ravi".to_string()),
            ("Applicant.City".into(), "Pune".into())
        ]
    );
    let nested = katna_pdf_cos::data_to_xfdf(
        &[
            ("Applicant.Name".into(), "A".into()),
            ("Applicant.City".into(), "B".into()),
        ],
        "f.pdf",
    );
    assert_eq!(nested.matches("<field name=\"Applicant\">").count(), 1);
    let fdf = b"%FDF-1.2\n1 0 obj\n<< /FDF << /Fields [ << /T (Applicant) /Kids [ << /T (Name) /V (Asha) >> 2 0 R ] >> ] >> >>\nendobj\n2 0 obj\n<< /T (City) /V <FEFF092A0941> >>\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n";
    assert_eq!(
        katna_pdf_cos::data_from_fdf(fdf).unwrap(),
        [
            ("Applicant.Name".to_string(), "Asha".to_string()),
            ("Applicant.City".into(), "पु".into())
        ]
    );
}

#[test]
fn looks_comb_and_limits_are_written_and_drawn() {
    use katna_pdf_cos::{Border, Look};
    let mut boxed = spec(
        0,
        700.0,
        "PAN",
        NewField::Text {
            multiline: false,
            password: false,
            max_len: Some(10),
            comb: true,
        },
    );
    boxed.look = Look {
        border: Border::Dashed,
        width: 2.0,
        border_colour: Some([0.0, 0.0, 1.0]),
        fill: Some([1.0, 1.0, 0.8]),
        text_colour: [0.5, 0.0, 0.0],
        text_size: 11.0,
    };
    boxed.value = Some("ABCDE1234F".into());
    let mut agree = spec(0, 660.0, "Agree", NewField::CheckBox);
    agree.rect = [100.0, 660.0, 114.0, 674.0];
    agree.look.border = Border::Beveled;
    let doc = designed(
        &blank(1),
        &FormDesign {
            add: vec![
                boxed,
                agree,
                spec(
                    0,
                    620.0,
                    "Secret",
                    NewField::Text {
                        multiline: false,
                        password: false,
                        max_len: None,
                        comb: false,
                    },
                ),
            ],
            ..FormDesign::default()
        },
    );
    let form = doc.form();
    let pan = &form[0];
    assert_eq!(
        pan.kind,
        FieldKind::Text {
            multiline: false,
            password: false,
            max_len: Some(10),
            comb: true
        }
    );
    let look = doc.field_look(pan);
    assert_eq!(look.border, Border::Dashed);
    assert_eq!(look.width, 2.0);
    assert_eq!(look.border_colour, Some([0.0, 0.0, 1.0]));
    assert_eq!(look.fill, Some([1.0, 1.0, 0.8]));
    assert_eq!(look.text_colour, [0.5, 0.0, 0.0]);
    assert_eq!(look.text_size, 11.0);
    let drawn = appearance(&doc, pan.widgets[0].id);
    // Dashed, blue, with the cells' lines and a letter each.
    assert!(drawn.contains("[3] 0 d"), "{drawn}");
    assert!(drawn.contains("0.000 0.000 1.000 RG"), "{drawn}");
    assert_eq!(drawn.matches(" Tj").count(), 10, "{drawn}");
    assert!(drawn.contains("(A) Tj"), "{drawn}");
    assert_eq!(doc.field_look(&form[1]).border, Border::Beveled);
    let Some(Object::Dict(w)) = doc.get(form[1].widgets[0].id) else {
        panic!()
    };
    let Object::Dict(ap) = doc.resolve(w.get("AP").unwrap()) else {
        panic!()
    };
    let Object::Dict(states) = doc.resolve(ap.get("N").unwrap()) else {
        panic!()
    };
    let Object::Stream(off) = doc.resolve(states.get("Off").unwrap()) else {
        panic!()
    };
    let off =
        String::from_utf8_lossy(&katna_pdf_cos::decode(&off.dict, &off.data).unwrap()).into_owned();
    assert!(off.contains("1 g"), "{off}");
    // A look given later lands on the widget and is drawn; a text field
    // becomes a password with a limit, and a comb gives up its cells.
    let secret = &form[2];
    let mut edit = FieldEdit::new(secret.id);
    edit.password = Some(true);
    edit.max_len = Some(Some(6));
    edit.look = Some(Look {
        border: Border::Underline,
        border_colour: Some([0.0; 3]),
        ..Look::default()
    });
    let mut plain = FieldEdit::new(pan.id);
    plain.comb = Some(false);
    plain.max_len = Some(None);
    let doc = designed(
        &doc,
        &FormDesign {
            edit: vec![edit, plain],
            ..FormDesign::default()
        },
    );
    let form = doc.form();
    assert_eq!(
        form[2].kind,
        FieldKind::Text {
            multiline: false,
            password: true,
            max_len: Some(6),
            comb: false
        }
    );
    assert_eq!(doc.field_look(&form[2]).border, Border::Underline);
    let drawn = appearance(&doc, form[2].widgets[0].id);
    assert!(drawn.contains(" m ") && drawn.contains(" l S Q"), "{drawn}");
    assert!(!drawn.contains(" re S"), "{drawn}");
    assert_eq!(
        form[0].kind,
        FieldKind::Text {
            multiline: false,
            password: false,
            max_len: None,
            comb: false
        }
    );
    let drawn = appearance(&doc, form[0].widgets[0].id);
    assert!(drawn.contains("(ABCDE1234F) Tj"), "{drawn}");
}

#[test]
fn a_form_is_reset_to_its_defaults() {
    let mut name = spec(
        0,
        700.0,
        "Name",
        NewField::Text {
            multiline: false,
            password: false,
            max_len: None,
            comb: false,
        },
    );
    name.value = Some("Mozammel".into());
    let mut agree = spec(0, 660.0, "Agree", NewField::CheckBox);
    agree.rect = [100.0, 660.0, 114.0, 674.0];
    let doc = designed(
        &blank(1),
        &FormDesign {
            add: vec![name, agree],
            ..FormDesign::default()
        },
    );
    let form = doc.form();
    let filled = Document::load(
        doc.fill(&[
            (form[0].id, FieldValue::Text("Someone".into())),
            (form[1].id, FieldValue::State(Some("Yes".into()))),
        ])
        .unwrap(),
    )
    .unwrap();
    let form = filled.form();
    assert_eq!(form[0].value.as_deref(), Some("Someone"));
    assert_eq!(form[1].value.as_deref(), Some("Yes"));
    let reset = Document::load(filled.fill(&filled.reset_values()).unwrap()).unwrap();
    let form = reset.form();
    // New fields have no default, so the text goes; the box is off.
    assert_eq!(form[0].value, None);
    assert_eq!(form[1].value, None);
}

#[test]
fn button_actions_are_written_read_and_done() {
    use katna_pdf_cos::{Destination, FieldAction, SubmitFormat};
    let text = NewField::Text {
        multiline: false,
        password: false,
        max_len: None,
        comb: false,
    };
    let mut name = spec(0, 700.0, "Name", text.clone());
    name.value = Some("Mozammel".into());
    let mut city = spec(0, 660.0, "City", text);
    city.value = Some("Kolkata".into());
    let button = |y, name: &str, action| {
        let mut b = spec(
            1,
            y,
            name,
            NewField::PushButton {
                caption: name.into(),
            },
        );
        b.action = Some(action);
        b
    };
    let doc = designed(
        &blank(2),
        &FormDesign {
            add: vec![
                name,
                city,
                button(
                    700.0,
                    "Go",
                    FieldAction::GoTo(Destination {
                        page: 0,
                        top: Some(700.0),
                    }),
                ),
                button(
                    660.0,
                    "Web",
                    FieldAction::Uri("https://katna.invenia.in".into()),
                ),
                button(
                    620.0,
                    "Clear",
                    FieldAction::Reset {
                        fields: vec!["Name".into()],
                        others: false,
                    },
                ),
                button(
                    580.0,
                    "Tidy",
                    FieldAction::Hide {
                        fields: vec!["City".into()],
                        hide: true,
                    },
                ),
                button(
                    540.0,
                    "Send",
                    FieldAction::Submit {
                        url: "https://example.org/in".into(),
                        format: SubmitFormat::Xfdf,
                    },
                ),
                button(500.0, "Next", FieldAction::Named("NextPage".into())),
            ],
            ..FormDesign::default()
        },
    );
    let form = doc.form();
    let action = |name: &str| doc.field_action(form.iter().find(|f| f.name == name).unwrap());
    assert_eq!(
        action("Go"),
        Some(FieldAction::GoTo(Destination {
            page: 0,
            top: Some(700.0)
        }))
    );
    assert_eq!(
        action("Web"),
        Some(FieldAction::Uri("https://katna.invenia.in".into()))
    );
    assert_eq!(
        action("Clear"),
        Some(FieldAction::Reset {
            fields: vec!["Name".into()],
            others: false
        })
    );
    assert_eq!(
        action("Send"),
        Some(FieldAction::Submit {
            url: "https://example.org/in".into(),
            format: SubmitFormat::Xfdf
        })
    );
    assert_eq!(action("Next"), Some(FieldAction::Named("NextPage".into())));
    assert_eq!(action("Name"), None);
    // Clearing Name alone leaves City.
    let reset = Document::load(
        doc.fill(&doc.reset_values_of(&["Name".into()], false))
            .unwrap(),
    )
    .unwrap();
    let values = reset.form_data();
    assert_eq!(values[0], ("Name".into(), String::new()));
    assert_eq!(values[1], ("City".into(), "Kolkata".into()));
    // Clearing all but Name leaves Name.
    let reset = Document::load(
        doc.fill(&doc.reset_values_of(&["Name".into()], true))
            .unwrap(),
    )
    .unwrap();
    let values = reset.form_data();
    assert_eq!(values[0], ("Name".into(), "Mozammel".into()));
    assert_eq!(values[1], ("City".into(), String::new()));
    // Hiding City sets its widget's flag; the submit body carries the data.
    let hidden = Document::load(doc.hide_fields(&["City".into()], true).unwrap()).unwrap();
    let city = hidden
        .form()
        .into_iter()
        .find(|f| f.name == "City")
        .unwrap();
    let Some(Object::Dict(w)) = hidden.get(city.widgets[0].id) else {
        panic!()
    };
    assert_eq!(hidden.resolve(w.get("F").unwrap()).as_int().unwrap() & 2, 2);
    let (body, kind) = doc.submit_body(SubmitFormat::Html, "form.pdf");
    assert_eq!(kind, "application/x-www-form-urlencoded");
    assert_eq!(
        String::from_utf8(body).unwrap(),
        "Name=Mozammel&City=Kolkata"
    );
    // The action comes off, and another goes on, by an edit.
    let web = form.iter().find(|f| f.name == "Web").unwrap();
    let mut off = FieldEdit::new(web.id);
    off.action = Some(None);
    let go = form.iter().find(|f| f.name == "Go").unwrap();
    let mut other = FieldEdit::new(go.id);
    other.action = Some(Some(FieldAction::Named("LastPage".into())));
    let doc = designed(
        &doc,
        &FormDesign {
            edit: vec![off, other],
            ..FormDesign::default()
        },
    );
    let form = doc.form();
    let action = |name: &str| doc.field_action(form.iter().find(|f| f.name == name).unwrap());
    assert_eq!(action("Web"), None);
    assert_eq!(action("Go"), Some(FieldAction::Named("LastPage".into())));
}
