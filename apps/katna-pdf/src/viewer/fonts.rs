// SPDX-License-Identifier: GPL-3.0-or-later

//! The fonts installed here, for putting a paragraph in a new face: the
//! standard faces every reader has, and the families `fontdb` finds,
//! whose files are put into the PDF.

use std::sync::OnceLock;

use katna_pdf_cos::NewFont;

/// The standard faces, offered first: a reader shows them without the
/// font being in the file.
const STANDARD: [(&str, [&str; 4]); 3] = [
    (
        "Helvetica",
        [
            "Helvetica",
            "Helvetica-Bold",
            "Helvetica-Oblique",
            "Helvetica-BoldOblique",
        ],
    ),
    (
        "Times",
        [
            "Times-Roman",
            "Times-Bold",
            "Times-Italic",
            "Times-BoldItalic",
        ],
    ),
    (
        "Courier",
        [
            "Courier",
            "Courier-Bold",
            "Courier-Oblique",
            "Courier-BoldOblique",
        ],
    ),
];

/// The fonts installed here.
pub(super) struct Installed {
    db: fontdb::Database,
    /// The families to offer: the standard three, then the installed
    /// ones in order.
    families: Vec<String>,
}

static INSTALLED: OnceLock<Installed> = OnceLock::new();

/// The fonts installed here, read once.
pub(super) fn installed() -> &'static Installed {
    INSTALLED.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let mut families: Vec<String> = db
            .faces()
            .filter_map(|f| f.families.first().map(|(name, _)| name.clone()))
            .filter(|name| !STANDARD.iter().any(|(s, _)| s.eq_ignore_ascii_case(name)))
            .collect();
        families.sort_by_key(|f| f.to_lowercase());
        families.dedup();
        let mut all: Vec<String> = STANDARD.iter().map(|(s, _)| s.to_string()).collect();
        all.extend(families);
        Installed { db, families: all }
    })
}

impl Installed {
    /// The families to offer, the standard ones first.
    pub(super) fn families(&self) -> &[String] {
        &self.families
    }

    /// Family `family`'s face of weight and slant `bold` and `italic`,
    /// ready to put in a file. Arial, Times New Roman and Courier New
    /// are the standard faces.
    pub(super) fn new_font(
        &self,
        family: &str,
        bold: bool,
        italic: bool,
    ) -> Result<NewFont, String> {
        let standard = match family.to_ascii_lowercase().as_str() {
            "helvetica" | "arial" => Some(0),
            "times" | "times new roman" => Some(1),
            "courier" | "courier new" => Some(2),
            _ => None,
        };
        if let Some(i) = standard {
            let faces = STANDARD[i].1;
            let face = faces[usize::from(bold) + 2 * usize::from(italic)];
            return Ok(NewFont::Standard(face.to_string()));
        }
        let query = fontdb::Query {
            families: &[fontdb::Family::Name(family)],
            weight: if bold {
                fontdb::Weight::BOLD
            } else {
                fontdb::Weight::NORMAL
            },
            stretch: fontdb::Stretch::Normal,
            style: if italic {
                fontdb::Style::Italic
            } else {
                fontdb::Style::Normal
            },
        };
        let id = self.db.query(&query).ok_or_else(|| family.to_string())?;
        self.db
            .with_face_data(id, |data, index| NewFont::TrueType {
                data: data.to_vec(),
                face: index as usize,
            })
            .ok_or_else(|| family.to_string())
    }
}
