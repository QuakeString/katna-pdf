// SPDX-License-Identifier: GPL-3.0-or-later

//! Katna PDF's own object layer, "COS" (docs/PLAN.md §3.2): reading a
//! PDF's objects, finding them through cross-reference tables or streams,
//! repairing files whose tables are broken, and appending incremental
//! updates that leave the original bytes untouched, as signing needs.
//!
//! It knows objects, not pages' looks: drawing stays with the renderer.
//! Locked files open with their password (an empty one is tried first);
//! updates to them are locked with the same key.

mod annots;
mod calc;
mod content;
mod crypt;
mod design;
mod detect;
mod document;
mod dss;
mod edit;
mod filter;
mod fonts;
mod formdata;
mod forms;
mod insert;
mod links;
mod lock;
mod mdp;
mod meta;
mod nav;
mod object;
mod objects;
mod outline;
mod pages;
mod parse;
pub mod ranges;
mod redact;
mod report;
mod sign;
mod stamp;
mod write;
mod xfdf;

pub use annots::{
    AnnotChanges, AnnotKind, Annotation, MarkupStyle, NOTE_SIZE, pdf_date, stamp_label,
};
pub use calc::{
    CalcOp, Calculation, Format, Scripts, calculate, number, parse_date, show, write_date,
};
pub use content::{Op, ops};
pub use crypt::{Permissions, UnlockError};
pub use design::{FieldEdit, FieldSpec, FormDesign, FormError, NewField};
pub use document::{Document, Entry, LoadError, XrefKind};
pub use dss::Dss;
pub use edit::{EditError, TextBlock, TextRun};
pub use filter::{FilterError, decode};
pub use formdata::{
    FormDataError, data_from_csv, data_from_fdf, data_from_json, data_from_xfdf, data_to_csv,
    data_to_json, data_to_xfdf, read_data,
};
pub use forms::{Field, FieldKind, FieldValue, Widget};
pub use insert::{Image, Insert, ShapeKind};
pub use links::{Link, LinkTarget};
pub use lock::LockError;
pub use mdp::{Allowed, Change, Lock};
pub use meta::Metadata;
pub use nav::NavError;
pub use object::{Dict, Name, Object, Ref, Stream};
pub use objects::{ObjectChange, ObjectKind, PageObject};
pub use outline::{Destination, OutlineItem, text_string};
pub use pages::{ComposeError, PageInfo, PageSource, compose};
pub use redact::{Pattern, RedactError, RedactOptions};
pub use report::text_pdf;
pub use sign::{
    CONTENTS_LEN, Coverage, Picture, Placement, Prepared, SignError, SignRequest, Signature,
    SigningInfo,
};
pub use stamp::{Bates, Stamp, StampPlace};
pub use write::{Update, UpdateError, write_object};
pub use xfdf::{XfdfAnnotation, XfdfError, from_xfdf, to_xfdf};
