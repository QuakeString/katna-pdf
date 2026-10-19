// SPDX-License-Identifier: GPL-3.0-or-later

//! Katna PDF's own object layer, "COS" (docs/PLAN.md §3.2): reading a
//! PDF's objects, finding them through cross-reference tables or streams,
//! repairing files whose tables are broken, and appending incremental
//! updates that leave the original bytes untouched, as signing needs.
//!
//! It knows objects, not pages' looks: drawing stays with the renderer.
//! v0 reads encrypted files' structure but cannot decrypt or update them.

mod annots;
mod content;
mod document;
mod dss;
mod edit;
mod filter;
mod fonts;
mod forms;
mod insert;
mod links;
mod mdp;
mod object;
mod outline;
mod pages;
mod parse;
mod report;
mod sign;
mod write;
mod xfdf;

pub use annots::{
    AnnotChanges, AnnotKind, Annotation, MarkupStyle, NOTE_SIZE, pdf_date, stamp_label,
};
pub use content::{Op, ops};
pub use document::{Document, Entry, LoadError, XrefKind};
pub use dss::Dss;
pub use edit::{EditError, TextBlock, TextRun};
pub use filter::{FilterError, decode};
pub use forms::{Field, FieldKind, FieldValue, Widget};
pub use insert::{Image, Insert, ShapeKind};
pub use links::{Link, LinkTarget};
pub use mdp::{Allowed, Change, Lock};
pub use object::{Dict, Name, Object, Ref, Stream};
pub use outline::{Destination, OutlineItem, text_string};
pub use pages::{ComposeError, PageInfo, PageSource, compose};
pub use report::text_pdf;
pub use sign::{
    CONTENTS_LEN, Coverage, Picture, Placement, Prepared, SignError, SignRequest, Signature,
    SigningInfo,
};
pub use write::{Update, UpdateError, write_object};
pub use xfdf::{XfdfAnnotation, XfdfError, from_xfdf, to_xfdf};
