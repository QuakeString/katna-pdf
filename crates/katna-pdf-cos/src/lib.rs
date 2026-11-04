// SPDX-License-Identifier: GPL-3.0-or-later

//! Katna PDF's own object layer, "COS" (docs/PLAN.md §3.2): reading a
//! PDF's objects, finding them through cross-reference tables or streams,
//! repairing files whose tables are broken, and appending incremental
//! updates that leave the original bytes untouched, as signing needs.
//!
//! It knows objects, not pages' looks: drawing stays with the renderer.
//! Locked files open with their password (an empty one is tried first);
//! updates to them are locked with the same key.

mod actions;
mod annots;
mod attachments;
mod calc;
mod content;
mod crypt;
mod design;
mod detect;
mod document;
mod dss;
mod edit;
mod export;
mod filter;
mod flatten;
mod fonts;
mod formdata;
mod forms;
mod insert;
mod labels;
mod layers;
mod links;
mod lock;
mod mdp;
mod measure;
mod meta;
mod nav;
mod object;
mod objects;
mod optimize;
mod outline;
mod pages;
mod parse;
mod present;
pub mod ranges;
mod redact;
mod report;
mod scripts;
mod sign;
mod stamp;
mod write;
mod xfdf;

pub use actions::{FieldAction, SubmitFormat};
pub use annots::{
    AnnotChanges, AnnotFlags, AnnotKind, Annotation, DYNAMIC_STAMPS, MarkupStyle, NOTE_SIZE,
    REVIEW_STATES, STAMPS, annotation_status, comments_text, custom_stamp, pdf_date,
    stamp_is_dynamic, stamp_label,
};
pub use attachments::Attachment;
pub use calc::{
    CalcOp, Calculation, Format, Scripts, calculate, number, parse_date, show, write_date,
};
pub use content::{Op, ops};
pub use crypt::{Permissions, UnlockError};
pub use design::{FieldEdit, FieldSpec, FormDesign, FormError, NewField};
pub use document::{Document, Entry, LoadError, XrefKind};
pub use dss::Dss;
pub use edit::{EditError, Layout, NewFont, TextAlign, TextBlock, TextRun};
pub use export::{docx, pictures_pdf, plain_text};
pub use filter::{FilterError, decode};
pub use flatten::Flatten;
pub use formdata::{
    FormDataError, data_from_csv, data_from_fdf, data_from_json, data_from_xfdf, data_to_csv,
    data_to_json, data_to_xfdf, read_data,
};
pub use forms::{Border, Field, FieldKind, FieldValue, Look, Widget};
pub use insert::{Image, Insert, ShapeKind};
pub use labels::{LabelRange, LabelStyle, labels_of};
pub use layers::Layer;
pub use links::{Link, LinkTarget};
pub use lock::LockError;
pub use mdp::{Allowed, Change, Lock};
pub use measure::Scale;
pub use meta::Metadata;
pub use nav::NavError;
pub use object::{Dict, Name, Object, Ref, Stream};
pub use objects::{ObjectChange, ObjectCopy, ObjectKind, PageObject, PictureFile, ShapeStyle};
pub use optimize::{ImageLimit, Optimize, OptimizeError, Optimized};
pub use outline::{Destination, OutlineItem, text_string};
pub use pages::{ComposeError, PageInfo, PageSource, blank_pdf, compose};
pub use present::{PageShow, Transition, TransitionStyle};
pub use redact::{Pattern, RedactError, RedactOptions};
pub use report::text_pdf;
pub use scripts::{ActionPlace, DocAction, DocEvent, FieldEvent};
pub use sign::{
    CONTENTS_LEN, Coverage, Picture, Placement, Prepared, SignError, SignRequest, Signature,
    SigningInfo,
};
pub use stamp::{Background, Bates, MarkWhat, PageMark, Stamp, StampPlace};
pub use write::{Update, UpdateError, write_object};
pub use xfdf::{XfdfAnnotation, XfdfError, from_fdf, from_xfdf, to_fdf, to_xfdf};
