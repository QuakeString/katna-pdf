// SPDX-License-Identifier: GPL-3.0-or-later

//! `katna-pdf-cli`: Katna PDF without a window (docs/PLAN.md, Phase 2,
//! and §"agent-friendly CLI"). Every command can answer in JSON with
//! `--json`, for scripts and agents; errors then come as
//! `{"error": "..."}` too. Exit status: 0 done, 1 failed, 2 usage.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use katna_pdf_cos::{
    Allowed, AnnotKind, Change, Coverage, Document, FieldEdit, FieldSpec, FormDesign, Lock,
    NewField, Object, ObjectChange, PageSource, Placement, SignRequest, SigningInfo, compose,
    text_string,
};
use katna_pdf_sign::{Digest, Revocation, Signer, Trust, common_name, p12, pkcs11};

use katna_pdf_cos::ranges;

const USAGE: &str = "\
usage: katna-pdf-cli COMMAND [--json] ...

  info FILE                       pages, sizes, metadata, outline, form, annotations
  merge -o OUT FILE[:PAGES]...    one PDF of the files given, in order
  split FILE -o DIR [--every N]   files of N pages each (default 1), named FILE-1.pdf...
  pages FILE PAGES -o OUT         the pages listed, in that order
  rotate FILE -o OUT [--by DEG] [--pages PAGES]
                                  turn pages clockwise (90, 180, 270, -90; default 90)
  crop FILE -o OUT (--box L,B,R,T | --margins M | --uncrop) [--pages PAGES]
                                  keep a box of each page, in points from its media
                                  box's corner; or cut margins off what is shown now
                                  (M: all sides, or top,right,bottom,left); or uncrop
  sign FILE... -o OUT (--p12 CERT.pfx | --token LABEL [--cert NAME] [--module LIB])
          (--password-env VAR | --password-file F | --password PW)
          [--page N --rect L,B,R,T] [--reason R] [--location L] [--contact C] [--digest 256|384|512]
          [--tsa URL] [--ltv | --lta] [--certify none|forms|comments] [--field NAME]
          [--picture IMAGE] [--lock all|FIELD,...]
                                  sign with the key in a certificate file, or on a
                                  token (LABEL, or any), unlocked with the password
                                  or PIN (PAdES B-B);
                                  visible in a box on page N when --rect is given;
                                  timestamped by the server at URL with --tsa (PAdES B-T);
                                  in the empty signature field NAME with --field;
                                  showing a PNG or JPEG (a scanned handwritten
                                  signature) beside the text with --picture;
                                  with long-term validation data (PAdES B-LT) with --ltv:
                                  the chain and OCSP answers or CRLs, fetched now;
                                  and a document timestamp after them with --lta (B-LTA);
                                  certified with --certify, allowing no changes after,
                                  form filling and signing (of the file's own empty
                                  signature fields), or comments too;
                                  locking form fields with --lock (all, or those
                                  named), which then may not change;
                                  many files at once, unlocked once, into folder OUT
  insert FILE -o OUT [--page N] [--colour #RRGGBB] (--text TEXT --at X,Y [--font NAME]
         [--size PT] | --image PICTURE --rect L,B,R,T | --shape rectangle|ellipse|line
         --rect L,B,R,T [--fill])
                                  text, a picture or a shape drawn into the page itself;
                                  text in a standard font (Helvetica, Times-Roman,
                                  Courier, -Bold and so on), its first baseline at X,Y;
                                  a line runs from L,B to R,T
  lock FILE -o OUT [--user PW] [--owner PW] [--allow WHAT,…]
                                  a password put on (AES-256): --user opens it (none:
                                  anyone may), with only what --allow lets (print,
                                  print-well, change, copy, comment, fill, assemble;
                                  all without it); --owner opens it fully
  unlock FILE -o OUT --owner PW   the password taken off, given the owner's
                                  (any command opens a locked file with the password
                                  in KATNA_PDF_OPEN_PASSWORD)
  redact FILE -o OUT [--area PAGE:L,B,R,T]... [--find pan,aadhaar,phone,email]
         [--words TEXT]... [--pages PAGES] [--scrub] [--no-fill]
                                  what lies under each box taken out of the file, not
                                  only covered: text (invisible text too), the pixels
                                  of pictures, shapes, annotations; boxes found from
                                  numbers or words on PAGES (all without it); a black
                                  box drawn over each unless --no-fill; with --scrub,
                                  the properties, scripts and attached files go too
  meta FILE -o OUT [--title T] [--author A] [--subject S] [--keywords K]
       [--creator C] [--producer P]
                                  the document's properties changed (`info` shows
                                  them); an empty value takes one out
  stamp FILE -o OUT --text TEXT [--place top-left|top|top-right|bottom-left|bottom|
        bottom-right|middle|across] [--pages LIST] [--font NAME] [--size PT]
        [--colour #RRGGBB] [--opacity 0..1] [--margin PT] [--under]
        [--bates-prefix P] [--bates-suffix S] [--bates-start N] [--bates-digits D]
                                  a header, footer or watermark on every page (or
                                  LIST); {page}, {pages} and {bates} in TEXT become
                                  the page's number, the page count and its Bates
                                  number; across is a large faint watermark corner
                                  to corner; --under draws it beneath the page
  background FILE -o OUT [--colour #RRGGBB] [--image PICTURE [--stretch]]
             [--opacity 0..1] [--pages LIST]
                                  a colour, a picture (fitted inside the page, or
                                  stretched over it), or both, under every page's
                                  own drawing (or LIST's)
  objects FILE [--pages LIST]     the pictures, forms and shapes drawn on pages, numbered
                                  PAGE:OBJECT, with where each is
  object FILE PAGE:OBJECT -o OUT (--rect L,B,R,T | --delete | --turn | --front |
         --back | --opacity 0..1 | --colour #RRGGBB | --image PICTURE)
                                  a picture or shape moved and resized to fill the box,
                                  taken out of the page, turned a quarter right, drawn
                                  over or under everything else, made see-through,
                                  coloured (a shape), or shown as another picture
  form FILE -o OUT [--add KIND:PAGE:L,B,R,T:NAME]... [--tip NAME=TEXT]...
       [--required NAME]... [--optional NAME]... [--read-only NAME]...
       [--rename NAME=NEW]... [--move NAME:L,B,R,T]... [--copy NAME:PAGES]...
       [--remove NAME]... [--order PAGE:NAME,NAME,...]...
       [--total NAME=SUM:FIELD,FIELD,...]... [--format NAME=FORMAT]...
       [--detect PAGES|all]
                                  the form designed: fields added (KIND text, multiline,
                                  password, check, radio=CHOICE, combo=A;B;C,
                                  list=A;B;C, button=CAPTION or signature; radio
                                  buttons with one NAME are one group), their tips and
                                  flags set, renamed, moved, shown on more pages too,
                                  taken out, and the order Tab goes through a page's
                                  fields set; a total (sum, product, average, min or
                                  max of other fields, worked out as the form is
                                  filled) and a format (number:DECIMALS[:CURRENCY],
                                  percent:DECIMALS, date:PATTERN such as dd/mm/yyyy,
                                  phone, or none) given; --detect adds the fields a
                                  flat form's pages ask for (blanks of underscores,
                                  rules to write on, empty boxes, rows of letter
                                  boxes, squares to tick), named from the words
                                  beside them
  form-data FILE [--csv] [-o OUT.json|.csv|.xfdf|.fdf]
                                  each field's value by its full name, as one JSON
                                  object (or CSV: names, then values); with -o saved
                                  as JSON, CSV, XFDF or FDF by OUT's extension
  fill FILE -o OUT [--data DATA.json|.csv|.xfdf|.fdf] [--set NAME=VALUE]...
                                  the form filled in from form data (JSON, CSV, or
                                  Acrobat's XFDF or FDF),
                                  or values given one by one (a check box: yes or
                                  no; a radio group: the button's choice)
  add-field FILE -o OUT --rect L,B,R,T [--page N] [--name NAME]
                                  an empty signature field, for someone to sign later
  ltv FILE -o OUT [--trust PATH] [--tsa URL]
                                  long-term validation data added to a signed file,
                                  and a document timestamp with --tsa (renews B-LTA)
  tokens [--module LIB]           the signing tokens plugged in (ePass2003 and others)
  token-check                     why a token does not show: whether it is plugged
                                  in, the card service runs, and each driver sees it
  verify FILE [--trust PATH] [--online] [--report OUT.pdf]
                                  the signatures, each checked: intact, what it covers,
                                  who signed, and whether their certificate chains to a
                                  root the system trusts, India's CCA roots (built in),
                                  or one in PATH (a certificate file or a folder of them);
                                  with --online, missing issuers are fetched and
                                  revocation is asked of OCSP responders and CRLs;
                                  with --report, the same written as a PDF to keep or send
  text FILE [--pages PAGES] [--blocks]
                                  the text each page draws, in runs (a line's worth
                                  in one font), numbered PAGE:RUN, with font and place;
                                  with --blocks, in paragraphs, numbered PAGE:BLOCK
  edit-text FILE -o OUT (--run PAGE:RUN | --block PAGE:BLOCK | --replace OLD) --to NEW
            [--pages PAGES]       change a run's text, a paragraph's (its words wrapped
                                  to its width, a line break in NEW starting a line),
                                  or OLD to NEW wherever a run holds it; the run's own
                                  font is kept where it has the letters, a standard
                                  font stands in where not
  bookmarks FILE [-o OUT (--from LIST | --add PAGE[@TOP]:TITLE... | --clear)]
                                  the outline, one item a line: two spaces a level in,
                                  the title, a tab, the page (and @ the height at the
                                  top of the view); with -o the outline written anew
                                  from a LIST in that form, items added at the end,
                                  or taken out
  links FILE [--pages PAGES] [-o OUT [--remove PAGE:LINK]... [--add PAGE:L,B,R,T:TO]...]
                                  the links, numbered PAGE:LINK, with their box and
                                  where they go; with -o some taken out and others
                                  added, going to TO: a web or mail address, or a
                                  page (PAGE[@TOP])
  dests FILE [-o OUT [--set NAME=PAGE[@TOP]]... [--remove NAME]...]
                                  the named destinations (places links and other
                                  files go to by name); with -o some set or taken out
  labels FILE [-o OUT (--range FROM:STYLE[:PREFIX[:START]]... | --clear)]
                                  each page's label (the number the document gives
                                  it, such as i, ii, 1, 2); with -o the pages numbered
                                  in ranges from page FROM: STYLE 1, i, I, a, A or
                                  none, after PREFIX, counting from START
  annots FILE                     the annotations, page by page
  xfdf-export FILE -o OUT.xfdf    the annotations as XFDF
  xfdf-import FILE XFDF -o OUT    the annotations of XFDF added to the PDF

PAGES are counted from 1: 1-3,5,8- ; last ; odd ; even ; all.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let json = args.iter().any(|a| a == "--json");
    let args: Vec<String> = args.into_iter().filter(|a| a != "--json").collect();
    let Some((command, rest)) = args.split_first() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let result = match command.as_str() {
        "info" => info(rest, json),
        "merge" => merge(rest, json),
        "split" => split(rest, json),
        "pages" => pages(rest, json),
        "rotate" => rotate(rest, json),
        "crop" => crop(rest, json),
        "sign" => sign(rest, json),
        "verify" => verify(rest, json),
        "add-field" => add_field(rest, json),
        "form" => form(rest, json),
        "form-data" => form_data(rest, json),
        "fill" => fill(rest, json),
        "insert" => insert(rest, json),
        "stamp" => stamp(rest, json),
        "background" => background(rest, json),
        "meta" => meta(rest, json),
        "lock" => lock(rest, json),
        "redact" => redact(rest, json),
        "unlock" => unlock(rest, json),
        "objects" => objects(rest, json),
        "bookmarks" => bookmarks(rest, json),
        "links" => links(rest, json),
        "dests" => dests(rest, json),
        "labels" => labels(rest, json),
        "object" => object(rest, json),
        "ltv" => ltv(rest, json),
        "tokens" => tokens(rest, json),
        "token-check" => token_check(rest, json),
        "text" => text(rest, json),
        "edit-text" => edit_text(rest, json),
        "annots" => annots(rest, json),
        "xfdf-export" => xfdf_export(rest, json),
        "xfdf-import" => xfdf_import(rest, json),
        "help" | "-h" | "--help" => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        _ => Err(Failure::Usage(format!("unknown command {command}"))),
    };
    match result {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(failure) => {
            let (message, code) = match failure {
                Failure::Usage(m) => (m, 2),
                Failure::Failed(m) => (m, 1),
            };
            if json {
                println!("{{\"error\":{}}}", string(&message));
            } else if code == 2 {
                eprintln!("{message}\n\n{USAGE}");
            } else {
                eprintln!("katna-pdf-cli: {message}");
            }
            ExitCode::from(code)
        }
    }
}

enum Failure {
    /// The command was not given as it should be.
    Usage(String),
    /// It was, and it failed.
    Failed(String),
}

type Done = Result<String, Failure>;

fn failed(e: impl std::fmt::Display) -> Failure {
    Failure::Failed(e.to_string())
}

/// The arguments of a command: the words, the values of options and the
/// flags.
struct Args<'a> {
    words: Vec<&'a str>,
    options: Vec<(&'a str, &'a str)>,
    flags: Vec<&'a str>,
}

impl<'a> Args<'a> {
    /// `args`, with `-o`, `--every`, `--by`, `--pages`, `--box`,
    /// `--margins` and `sign`'s options taking values, and `--uncrop` a
    /// flag.
    fn parse(args: &'a [String]) -> Result<Self, Failure> {
        let mut words = Vec::new();
        let mut options = Vec::new();
        let mut flags = Vec::new();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if matches!(
                a.as_str(),
                "-o" | "--every"
                    | "--by"
                    | "--pages"
                    | "--box"
                    | "--margins"
                    | "--p12"
                    | "--password"
                    | "--password-env"
                    | "--password-file"
                    | "--page"
                    | "--rect"
                    | "--text"
                    | "--at"
                    | "--font"
                    | "--size"
                    | "--image"
                    | "--shape"
                    | "--colour"
                    | "--reason"
                    | "--location"
                    | "--contact"
                    | "--digest"
                    | "--tsa"
                    | "--token"
                    | "--module"
                    | "--cert"
                    | "--trust"
                    | "--certify"
                    | "--lock"
                    | "--run"
                    | "--block"
                    | "--replace"
                    | "--to"
                    | "--field"
                    | "--picture"
                    | "--name"
                    | "--report"
                    | "--place"
                    | "--title"
                    | "--user"
                    | "--owner"
                    | "--allow"
                    | "--author"
                    | "--subject"
                    | "--keywords"
                    | "--creator"
                    | "--producer"
                    | "--opacity"
                    | "--margin"
                    | "--bates-prefix"
                    | "--bates-suffix"
                    | "--bates-start"
                    | "--bates-digits"
                    | "--area"
                    | "--find"
                    | "--words"
                    | "--add"
                    | "--tip"
                    | "--required"
                    | "--optional"
                    | "--read-only"
                    | "--rename"
                    | "--move"
                    | "--copy"
                    | "--remove"
                    | "--order"
                    | "--data"
                    | "--set"
                    | "--total"
                    | "--detect"
                    | "--format"
                    | "--from"
                    | "--range"
            ) {
                let Some(value) = it.next() else {
                    return Err(Failure::Usage(format!("{a} needs a value")));
                };
                options.push((a.as_str(), value.as_str()));
            } else if [
                "--uncrop",
                "--online",
                "--ltv",
                "--lta",
                "--blocks",
                "--fill",
                "--delete",
                "--turn",
                "--front",
                "--back",
                "--under",
                "--scrub",
                "--no-fill",
                "--csv",
                "--clear",
                "--stretch",
            ]
            .contains(&a.as_str())
            {
                flags.push(a.as_str());
            } else if a.starts_with("--") {
                return Err(Failure::Usage(format!("unknown option {a}")));
            } else {
                words.push(a.as_str());
            }
        }
        Ok(Args {
            words,
            options,
            flags,
        })
    }

    fn option(&self, name: &str) -> Option<&'a str> {
        self.options
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| *v)
    }

    /// Every value given to option `name`, in order.
    fn all(&self, name: &str) -> Vec<&'a str> {
        self.options
            .iter()
            .filter(|(k, _)| *k == name)
            .map(|(_, v)| *v)
            .collect()
    }

    fn flag(&self, name: &str) -> bool {
        self.flags.contains(&name)
    }

    fn output(&self) -> Result<PathBuf, Failure> {
        self.option("-o")
            .map(PathBuf::from)
            .ok_or_else(|| Failure::Usage("say where to write with -o".into()))
    }

    /// Exactly `N` words.
    fn words<const N: usize>(&self) -> Result<[&'a str; N], Failure> {
        self.words.as_slice().try_into().map_err(|_| {
            Failure::Usage(format!(
                "expected {N} file or page argument{}, got {}",
                if N == 1 { "" } else { "s" },
                self.words.len()
            ))
        })
    }
}

/// Opens a file; a locked one with the password in
/// `KATNA_PDF_OPEN_PASSWORD`, when set.
fn open(path: &str) -> Result<Document, Failure> {
    open_with(
        path,
        std::env::var("KATNA_PDF_OPEN_PASSWORD").ok().as_deref(),
    )
}

fn open_with(path: &str, password: Option<&str>) -> Result<Document, Failure> {
    let bytes = std::fs::read(path).map_err(|e| failed(format!("{path}: {e}")))?;
    let mut doc = Document::load(bytes).map_err(|e| failed(format!("{path}: {e}")))?;
    if let Some(password) = password
        && doc.is_encrypted()
    {
        doc.unlock(password)
            .map_err(|e| failed(format!("{path}: {e}")))?;
    }
    Ok(doc)
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), Failure> {
    std::fs::write(path, bytes).map_err(|e| failed(format!("{}: {e}", path.display())))
}

/// What a command that wrote files says: their names and pages.
fn wrote(files: &[(PathBuf, usize)], json: bool) -> String {
    if json {
        let items: Vec<String> = files
            .iter()
            .map(|(p, n)| {
                format!(
                    "{{\"file\":{},\"pages\":{n}}}",
                    string(&p.display().to_string())
                )
            })
            .collect();
        format!("{{\"written\":[{}]}}\n", items.join(","))
    } else {
        files
            .iter()
            .map(|(p, n)| {
                format!(
                    "{} ({n} page{})\n",
                    p.display(),
                    if *n == 1 { "" } else { "s" }
                )
            })
            .collect()
    }
}

fn info(args: &[String], json: bool) -> Done {
    let args = Args::parse(args)?;
    let [file] = args.words()?;
    let doc = open(file)?;
    let count = doc.pages().len();
    let version = version(&doc);
    let meta: Vec<(&str, String)> = [
        "Title",
        "Author",
        "Subject",
        "Keywords",
        "Creator",
        "Producer",
        "CreationDate",
        "ModDate",
    ]
    .into_iter()
    .filter_map(|key| {
        let Object::Dict(info) = doc.resolve(doc.trailer().get("Info")?) else {
            return None;
        };
        match doc.resolve_key(&info, key) {
            Object::String { bytes, .. } => Some((key, text_string(&bytes))),
            _ => None,
        }
    })
    .collect();
    let sizes: Vec<(f32, f32, i64)> = (0..count)
        .filter_map(|p| doc.page_info(p))
        .map(|i| {
            let (w, h) = i.size();
            (w, h, i.rotate)
        })
        .collect();
    let outline = count_outline(&doc.outline());
    let fields = doc.form().len();
    let annots: usize = doc.annotations().iter().map(Vec::len).sum();
    let mut out = String::new();
    if json {
        let _ = write!(
            out,
            "{{\"file\":{},\"version\":{},\"pages\":{count},\"encrypted\":{},\"repaired\":{},\
             \"outline_items\":{outline},\"form_fields\":{fields},\"annotations\":{annots},\"info\":{{",
            string(file),
            string(&version),
            doc.is_encrypted(),
            doc.was_repaired()
        );
        let meta: Vec<String> = meta
            .iter()
            .map(|(k, v)| format!("{}:{}", string(k), string(v)))
            .collect();
        out.push_str(&meta.join(","));
        out.push_str("},\"page_sizes\":[");
        let sizes: Vec<String> = sizes
            .iter()
            .map(|(w, h, r)| {
                format!(
                    "{{\"width\":{},\"height\":{},\"rotate\":{r}}}",
                    num(*w),
                    num(*h)
                )
            })
            .collect();
        out.push_str(&sizes.join(","));
        out.push_str("]}\n");
    } else {
        let _ = writeln!(out, "File:         {file}");
        let _ = writeln!(out, "PDF version:  {version}");
        let _ = writeln!(out, "Pages:        {count}");
        if let Some((w, h, _)) = sizes.first() {
            let same = sizes.iter().all(|s| (s.0, s.1) == (*w, *h));
            let _ = writeln!(
                out,
                "Page size:    {} × {} pt ({} × {} mm){}",
                num(*w),
                num(*h),
                (w / 72.0 * 25.4).round(),
                (h / 72.0 * 25.4).round(),
                if same {
                    ""
                } else {
                    ", first page; others differ"
                }
            );
        }
        let _ = writeln!(out, "Encrypted:    {}", yes(doc.is_encrypted()));
        if doc.was_repaired() {
            let _ = writeln!(
                out,
                "Repaired:     yes (its cross-reference table was broken)"
            );
        }
        let _ = writeln!(out, "Outline:      {outline} items");
        let _ = writeln!(out, "Form fields:  {fields}");
        let _ = writeln!(out, "Annotations:  {annots}");
        for (k, v) in meta {
            let _ = writeln!(out, "{:<14}{v}", format!("{k}:"));
        }
    }
    Ok(out)
}

fn version(doc: &Document) -> String {
    let header = doc
        .bytes()
        .windows(5)
        .take(1024)
        .position(|w| w == b"%PDF-")
        .and_then(|at| doc.bytes().get(at + 5..at + 8))
        .map(|v| String::from_utf8_lossy(v).into_owned())
        .unwrap_or_default();
    // The catalog may name a later version than the header.
    let catalog = doc.catalog().and_then(|c| {
        c.name("Version")
            .map(|v| String::from_utf8_lossy(v).into_owned())
    });
    match catalog {
        Some(v) if v > header => v,
        _ => header,
    }
}

fn count_outline(items: &[katna_pdf_cos::OutlineItem]) -> usize {
    items.iter().map(|i| 1 + count_outline(&i.children)).sum()
}

fn merge(args: &[String], json: bool) -> Done {
    let args = Args::parse(args)?;
    let out = args.output()?;
    if args.words.is_empty() {
        return Err(Failure::Usage("name the files to merge".into()));
    }
    // FILE or FILE:PAGES; a file that exists with a colon in its name
    // is the file.
    let mut docs = Vec::new();
    for word in &args.words {
        let (file, pages) = match word.rsplit_once(':') {
            Some((f, p)) if !Path::new(word).exists() => (f, Some(p)),
            _ => (*word, None),
        };
        let doc = open(file)?;
        let count = doc.pages().len();
        let pages = match pages {
            Some(p) => ranges::parse(p, count).map_err(failed)?,
            None => (0..count).collect(),
        };
        docs.push((doc, pages));
    }
    let sources: Vec<PageSource> = docs
        .iter()
        .flat_map(|(doc, pages)| {
            pages.iter().map(move |&page| PageSource {
                doc,
                page,
                turns: 0,
                crop: None,
            })
        })
        .collect();
    let bytes = compose(Some(&docs[0].0), &sources).map_err(failed)?;
    write(&out, &bytes)?;
    Ok(wrote(&[(out, sources.len())], json))
}

fn split(args: &[String], json: bool) -> Done {
    let args = Args::parse(args)?;
    let [file] = args.words()?;
    let dir = args.output()?;
    let every: usize = match args.option("--every") {
        Some(n) => n
            .parse()
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| Failure::Usage(format!("--every {n}: not a number of pages")))?,
        None => 1,
    };
    let doc = open(file)?;
    std::fs::create_dir_all(&dir).map_err(|e| failed(format!("{}: {e}", dir.display())))?;
    let stem = Path::new(file)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "page".into());
    let count = doc.pages().len();
    let mut files = Vec::new();
    for (i, start) in (0..count).step_by(every).enumerate() {
        let sources: Vec<PageSource> = (start..(start + every).min(count))
            .map(|page| PageSource {
                doc: &doc,
                page,
                turns: 0,
                crop: None,
            })
            .collect();
        let bytes = compose(None, &sources).map_err(failed)?;
        let path = dir.join(format!("{stem}-{}.pdf", i + 1));
        write(&path, &bytes)?;
        files.push((path, sources.len()));
    }
    Ok(wrote(&files, json))
}

fn pages(args: &[String], json: bool) -> Done {
    let args = Args::parse(args)?;
    let [file, spec] = args.words()?;
    let out = args.output()?;
    let doc = open(file)?;
    let picked = ranges::parse(spec, doc.pages().len()).map_err(failed)?;
    let sources: Vec<PageSource> = picked
        .iter()
        .map(|&page| PageSource {
            doc: &doc,
            page,
            turns: 0,
            crop: None,
        })
        .collect();
    let bytes = compose(Some(&doc), &sources).map_err(failed)?;
    write(&out, &bytes)?;
    Ok(wrote(&[(out, sources.len())], json))
}

fn rotate(args: &[String], json: bool) -> Done {
    let args = Args::parse(args)?;
    let [file] = args.words()?;
    let out = args.output()?;
    let by: i32 = match args.option("--by") {
        Some(d) => d
            .parse()
            .ok()
            .filter(|d: &i32| d % 90 == 0)
            .ok_or_else(|| Failure::Usage(format!("--by {d}: turn by 90, 180, 270 or -90")))?,
        None => 90,
    };
    let doc = open(file)?;
    let count = doc.pages().len();
    let turned = match args.option("--pages") {
        Some(spec) => ranges::parse(spec, count).map_err(failed)?,
        None => (0..count).collect(),
    };
    let sources: Vec<PageSource> = (0..count)
        .map(|page| PageSource {
            doc: &doc,
            page,
            turns: if turned.contains(&page) { by / 90 } else { 0 },
            crop: None,
        })
        .collect();
    let bytes = compose(Some(&doc), &sources).map_err(failed)?;
    write(&out, &bytes)?;
    Ok(wrote(&[(out, count)], json))
}

fn crop(args: &[String], json: bool) -> Done {
    let args = Args::parse(args)?;
    let [file] = args.words()?;
    let out = args.output()?;
    let numbers = |name: &str, value: &str| -> Result<Vec<f32>, Failure> {
        value
            .split(',')
            .map(|n| n.trim().parse::<f32>().ok().filter(|n| n.is_finite()))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| Failure::Usage(format!("{name} {value}: numbers, comma between")))
    };
    let how = match (
        args.option("--box"),
        args.option("--margins"),
        args.flag("--uncrop"),
    ) {
        (Some(b), None, false) => match numbers("--box", b)?[..] {
            [l, b, r, t] if r > l && t > b => Cut::Box([l, b, r, t]),
            _ => return Err(Failure::Usage(format!("--box {b}: left,bottom,right,top"))),
        },
        (None, Some(m), false) => match numbers("--margins", m)?[..] {
            [a] => Cut::Margins([a; 4]),
            [t, r, b, l] => Cut::Margins([t, r, b, l]),
            _ => {
                return Err(Failure::Usage(format!(
                    "--margins {m}: one number, or top,right,bottom,left"
                )));
            }
        },
        (None, None, true) => Cut::Uncrop,
        _ => {
            return Err(Failure::Usage(
                "crop takes one of --box, --margins and --uncrop".into(),
            ));
        }
    };
    let doc = open(file)?;
    let count = doc.pages().len();
    let cropped = match args.option("--pages") {
        Some(spec) => ranges::parse(spec, count).map_err(failed)?,
        None => (0..count).collect(),
    };
    let sources: Vec<PageSource> = (0..count)
        .map(|page| {
            let info = doc.page_info(page);
            let crop = info
                .filter(|_| cropped.contains(&page))
                .map(|info| match how {
                    Cut::Box([l, b, r, t]) => {
                        let m = info.media;
                        [m[0] + l, m[1] + b, m[0] + r, m[1] + t]
                    }
                    Cut::Margins(cut) => margins(info.crop, info.rotate, cut),
                    Cut::Uncrop => info.media,
                });
            PageSource {
                doc: &doc,
                page,
                turns: 0,
                crop,
            }
        })
        .collect();
    let bytes = compose(Some(&doc), &sources).map_err(failed)?;
    write(&out, &bytes)?;
    Ok(wrote(&[(out, count)], json))
}

/// How `crop` cuts each page.
#[derive(Clone, Copy)]
enum Cut {
    Box([f32; 4]),
    /// Top, right, bottom, left, as the page is shown.
    Margins([f32; 4]),
    Uncrop,
}

/// `crop` (left, bottom, right, top) less margins `cut` (top, right,
/// bottom, left) of the page as shown, turned `rotate` degrees.
fn margins(crop: [f32; 4], rotate: i64, cut: [f32; 4]) -> [f32; 4] {
    // The margins shown as they lie in the page's own space: turning the
    // page clockwise brings its left side to the top.
    let [top, right, bottom, left] = cut;
    let [t, r, b, l] = match rotate {
        90 => [right, bottom, left, top],
        180 => [bottom, left, top, right],
        270 => [left, top, right, bottom],
        _ => [top, right, bottom, left],
    };
    [crop[0] + l, crop[1] + b, crop[2] - r, crop[3] - t]
}

fn sign(args: &[String], json: bool) -> Done {
    let args = Args::parse(args)?;
    if args.words.is_empty() {
        return Err(Failure::Usage("name the file to sign".into()));
    }
    let out = args.output()?;
    // Many files: each is written under its own name into folder `out`.
    let many = args.words.len() > 1;
    let target = |file: &str| -> Result<PathBuf, Failure> {
        if !many {
            return Ok(out.clone());
        }
        let name = Path::new(file)
            .file_name()
            .ok_or_else(|| Failure::Usage(format!("{file}: not a file")))?;
        let target = out.join(name);
        let same = std::fs::canonicalize(file).ok() == std::fs::canonicalize(&target).ok();
        if same {
            return Err(Failure::Usage(format!(
                "-o {}: signing would write over {file}; name another folder",
                out.display()
            )));
        }
        Ok(target)
    };
    if many {
        std::fs::create_dir_all(&out).map_err(|e| failed(format!("{}: {e}", out.display())))?;
        for file in &args.words {
            target(file)?;
        }
    }
    let cert = args.option("--p12");
    let token = args.option("--token");
    if cert.is_some() == token.is_some() {
        return Err(Failure::Usage(
            "sign with a certificate file (--p12) or a token (--token)".into(),
        ));
    }
    let password = match (
        args.option("--password"),
        args.option("--password-env"),
        args.option("--password-file"),
    ) {
        (Some(p), None, None) => p.to_owned(),
        (None, Some(var), None) => {
            std::env::var(var).map_err(|_| failed(format!("the environment has no {var}")))?
        }
        (None, None, Some(path)) => {
            let text = if path == "-" {
                std::io::read_to_string(std::io::stdin())
            } else {
                std::fs::read_to_string(path)
            }
            .map_err(|e| failed(format!("{path}: {e}")))?;
            text.trim_end_matches(['\r', '\n']).to_owned()
        }
        _ => {
            return Err(Failure::Usage(
                "give the password with one of --password-env, --password-file and --password"
                    .into(),
            ));
        }
    };
    let digest = match args.option("--digest") {
        None | Some("256") => Digest::Sha256,
        Some("384") => Digest::Sha384,
        Some("512") => Digest::Sha512,
        Some(d) => return Err(Failure::Usage(format!("--digest {d}: 256, 384 or 512"))),
    };
    let id: Box<dyn Signer> = match (cert, token) {
        (Some(cert), _) => {
            let bytes = std::fs::read(cert).map_err(|e| failed(format!("{cert}: {e}")))?;
            Box::new(p12::open(&bytes, &password).map_err(|e| failed(format!("{cert}: {e}")))?)
        }
        (None, Some(label)) => Box::new(token_identity(
            args.option("--module"),
            label,
            args.option("--cert"),
            &password,
        )?),
        (None, None) => unreachable!("checked above"),
    };
    let id = id.as_ref();
    let reason = args.option("--reason").map(str::to_owned);
    let lta = args.flag("--lta");
    let ltv = lta || args.flag("--ltv");
    if lta && args.option("--tsa").is_none() {
        return Err(Failure::Usage(
            "--lta needs a timestamp server: --tsa URL".into(),
        ));
    }
    let picture = match args.option("--picture") {
        None => None,
        Some(path) => {
            let bytes = std::fs::read(path).map_err(|e| failed(format!("{path}: {e}")))?;
            Some(katna_pdf_sign::picture(&bytes).map_err(|e| failed(format!("{path}: {e}")))?)
        }
    };
    let roots = trusted(&args)?;
    let fetch = katna_pdf_sign::revocation::Http::default();
    let certify = match args.option("--certify") {
        None => None,
        Some("none") => Some(Allowed::Nothing),
        Some("forms") => Some(Allowed::FormFilling),
        Some("comments") => Some(Allowed::Comments),
        Some(other) => {
            return Err(Failure::Usage(format!(
                "--certify {other}: none, forms or comments"
            )));
        }
    };
    let lock = args.option("--lock").map(|l| match l {
        "all" => Lock::All,
        names => Lock::Include(names.split(',').map(|n| n.trim().to_owned()).collect()),
    });
    let server = args.option("--tsa").map(katna_pdf_sign::tsa::Server::new);
    let stamper = server
        .as_ref()
        .map(|s| s as &dyn katna_pdf_sign::tsa::Stamper);
    let placement = |doc: &Document, time| -> Result<Placement, Failure> {
        let lines = || {
            katna_pdf_sign::appearance_lines(
                id,
                &katna_pdf_sign::shown_date(time),
                reason.as_deref(),
            )
        };
        if let Some(name) = args.option("--field") {
            let field = doc
                .form()
                .into_iter()
                .find(|f| f.name == name && f.kind == katna_pdf_cos::FieldKind::Signature)
                .ok_or_else(|| failed(format!("no signature field named {name}")))?;
            return Ok(Placement::Field {
                id: field.id,
                lines: lines(),
            });
        }
        Ok(match (args.option("--page"), args.option("--rect")) {
            (None, None) => Placement::Invisible,
            (page, Some(rect)) => {
                let page = page_number(doc, page)?;
                let rect = parse_rect(rect)?;
                Placement::Visible {
                    page,
                    rect,
                    lines: lines(),
                }
            }
            (Some(_), None) => {
                return Err(Failure::Usage("--page goes with --rect".into()));
            }
        })
    };
    // Each file in turn, with the key unlocked once; one that fails
    // does not stop the rest.
    let mut written = Vec::new();
    let mut failures = Vec::new();
    for file in &args.words {
        let signed = (|| {
            let doc = open(file)?;
            let time = std::time::SystemTime::now();
            let mut request = SignRequest::new(
                placement(&doc, time)?,
                SigningInfo {
                    name: Some(common_name(&id.certificate().tbs_certificate.subject)),
                    reason: reason.clone(),
                    location: args.option("--location").map(str::to_owned),
                    contact: args.option("--contact").map(str::to_owned),
                },
            );
            request.time = time;
            request.certify = certify;
            request.lock = lock.clone();
            request.picture = picture.clone();
            let mut signed = katna_pdf_sign::sign_with(&doc, &request, id, digest, stamper)
                .map_err(|e| failed(format!("{file}: {e}")))?;
            if ltv {
                signed = long_term(
                    file,
                    signed,
                    &roots,
                    &fetch,
                    lta.then_some(stamper).flatten(),
                )?;
            }
            let out = target(file)?;
            write(&out, &signed)?;
            Ok::<_, Failure>((out, doc.pages().len()))
        })();
        match signed {
            Ok(done) => written.push(done),
            Err(e) if !many => return Err(e),
            Err(Failure::Usage(e) | Failure::Failed(e)) => failures.push(e),
        }
    }
    if failures.is_empty() {
        Ok(wrote(&written, json))
    } else {
        let done = wrote(&written, false);
        Err(failed(format!(
            "{done}{} of {} files not signed:\n  {}",
            failures.len(),
            args.words.len(),
            failures.join("\n  ")
        )))
    }
}

/// The modules to look for tokens with: `module`, or those installed.
/// The roots trusted: the standard ones, and those of `--trust`.
fn trusted(args: &Args) -> Result<katna_pdf_sign::Roots, Failure> {
    let mut roots = katna_pdf_sign::Roots::standard();
    if let Some(path) = args.option("--trust") {
        let path = std::path::Path::new(path);
        let added = if path.is_dir() {
            roots.add_folder(path)
        } else {
            let bytes =
                std::fs::read(path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
            roots.add_file(&bytes)
        };
        if added == 0 {
            return Err(failed(format!("{}: no certificates in it", path.display())));
        }
    }
    Ok(roots)
}

/// Signed file `bytes` with its signatures' validation data added
/// (PAdES B-LT), then a document timestamp from `stamper` when given
/// (B-LTA); what could not be told is said on stderr.
fn long_term(
    file: &str,
    bytes: Vec<u8>,
    roots: &katna_pdf_sign::Roots,
    fetch: &dyn katna_pdf_sign::revocation::Fetch,
    stamper: Option<&dyn katna_pdf_sign::tsa::Stamper>,
) -> Result<Vec<u8>, Failure> {
    let fail = |e: &dyn std::fmt::Display| failed(format!("{file}: {e}"));
    let doc = Document::load(bytes).map_err(|e| fail(&e))?;
    let (bytes, untold) =
        katna_pdf_sign::add_validation_data(&doc, roots, fetch).map_err(|e| fail(&e))?;
    for why in untold {
        eprintln!("katna-pdf-cli: {file}: no validation data for {why}");
    }
    let Some(stamper) = stamper else {
        return Ok(bytes);
    };
    let doc = Document::load(bytes).map_err(|e| fail(&e))?;
    katna_pdf_sign::timestamp_document(&doc, stamper, Digest::Sha256).map_err(|e| fail(&e))
}

/// Adds validation data to the signatures of a file signed already.
fn ltv(args: &[String], json: bool) -> Done {
    let args = Args::parse(args)?;
    let [file] = args.words()?;
    let out = args.output()?;
    let doc = open(file)?;
    let roots = trusted(&args)?;
    let fetch = katna_pdf_sign::revocation::Http::default();
    let server = args.option("--tsa").map(katna_pdf_sign::tsa::Server::new);
    let stamper = server
        .as_ref()
        .map(|s| s as &dyn katna_pdf_sign::tsa::Stamper);
    let bytes = long_term(file, doc.bytes().to_vec(), &roots, &fetch, stamper)?;
    write(&out, &bytes)?;
    Ok(wrote(&[(out, doc.pages().len())], json))
}

/// Page `page` (counted from 1; the first when `None`) of `doc`, counted
/// from 0.
fn page_number(doc: &Document, page: Option<&str>) -> Result<usize, Failure> {
    match page {
        None => Ok(0),
        Some(p) => p
            .parse::<usize>()
            .ok()
            .filter(|p| (1..=doc.pages().len()).contains(p))
            .map(|p| p - 1)
            .ok_or_else(|| Failure::Usage(format!("--page {p}: no such page"))),
    }
}

/// A box: `left,bottom,right,top`.
fn parse_rect(rect: &str) -> Result<[f32; 4], Failure> {
    match rect
        .split(',')
        .map(|n| n.trim().parse::<f32>().ok().filter(|n| n.is_finite()))
        .collect::<Option<Vec<_>>>()
        .as_deref()
    {
        Some(&[l, b, r, t]) if r > l && t > b => Ok([l, b, r, t]),
        _ => Err(Failure::Usage(format!(
            "--rect {rect}: left,bottom,right,top"
        ))),
    }
}

/// Draws text, a picture or a shape into a page.
fn insert(args: &[String], json: bool) -> Done {
    let args = Args::parse(args)?;
    let [file] = args.words()?;
    let out = args.output()?;
    let doc = open(file)?;
    let page = page_number(&doc, args.option("--page"))?;
    let colour = colour_option(&args, [0.0; 3])?;
    let numbers = |name: &str, value: &str, n: usize| -> Result<Vec<f32>, Failure> {
        value
            .split(',')
            .map(|v| v.trim().parse::<f32>().ok().filter(|v| v.is_finite()))
            .collect::<Option<Vec<_>>>()
            .filter(|v| v.len() == n)
            .ok_or_else(|| Failure::Usage(format!("{name} {value}: {n} numbers, comma-separated")))
    };
    let item = match (
        args.option("--text"),
        args.option("--image"),
        args.option("--shape"),
    ) {
        (Some(text), None, None) => {
            let at = args
                .option("--at")
                .ok_or_else(|| Failure::Usage("say where with --at X,Y".into()))?;
            let at = numbers("--at", at, 2)?;
            let size = match args.option("--size") {
                Some(s) => s
                    .parse::<f32>()
                    .ok()
                    .filter(|s| *s > 0.0 && s.is_finite())
                    .ok_or_else(|| Failure::Usage(format!("--size {s}: a size in points")))?,
                None => 12.0,
            };
            katna_pdf_cos::Insert::Text {
                at: [at[0], at[1]],
                text: text.replace("\\n", "\n"),
                font: args.option("--font").unwrap_or("Helvetica").to_owned(),
                size,
                colour,
            }
        }
        (None, Some(picture), None) => {
            let rect = parse_rect(
                args.option("--rect")
                    .ok_or_else(|| Failure::Usage("say where with --rect".into()))?,
            )?;
            let bytes = std::fs::read(picture).map_err(|e| failed(format!("{picture}: {e}")))?;
            let image = katna_pdf_sign::page_image(&bytes)
                .map_err(|e| failed(format!("{picture}: {e}")))?;
            katna_pdf_cos::Insert::Image { rect, image }
        }
        (None, None, Some(shape)) => {
            let kind = match shape {
                "rectangle" => katna_pdf_cos::ShapeKind::Rectangle,
                "ellipse" => katna_pdf_cos::ShapeKind::Ellipse,
                "line" => katna_pdf_cos::ShapeKind::Line,
                _ => {
                    return Err(Failure::Usage(format!(
                        "--shape {shape}: rectangle, ellipse or line"
                    )));
                }
            };
            let rect = args
                .option("--rect")
                .ok_or_else(|| Failure::Usage("say where with --rect".into()))?;
            let rect = numbers("--rect", rect, 4)?;
            let fill = args.flag("--fill");
            katna_pdf_cos::Insert::Shape {
                kind,
                rect: [rect[0], rect[1], rect[2], rect[3]],
                stroke: (!fill).then_some(colour),
                fill: fill.then_some(colour),
                width: 2.0,
            }
        }
        _ => {
            return Err(Failure::Usage(
                "insert takes one of --text, --image and --shape".into(),
            ));
        }
    };
    let bytes = doc
        .insert(page, &[item])
        .map_err(|e| failed(format!("{file}: {e}")))?;
    write(&out, &bytes)?;
    Ok(wrote(&[(out, doc.pages().len())], json))
}

/// The colour in `--colour`, or `default`.
fn colour_option(args: &Args, default: [f32; 3]) -> Result<[f32; 3], Failure> {
    Ok(match args.option("--colour") {
        None => default,
        Some(c) => {
            let hex = c.trim_start_matches('#');
            let v = (hex.len() == 6)
                .then(|| u32::from_str_radix(hex, 16).ok())
                .flatten()
                .ok_or_else(|| Failure::Usage(format!("--colour {c}: #RRGGBB")))?;
            [(v >> 16) & 255, (v >> 8) & 255, v & 255].map(|n| n as f32 / 255.0)
        }
    })
}

/// A number in option `name`, or `default`, that `ok` accepts.
fn number_option(
    args: &Args,
    name: &str,
    default: f32,
    ok: impl Fn(f32) -> bool,
) -> Result<f32, Failure> {
    match args.option(name) {
        None => Ok(default),
        Some(v) => v
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite() && ok(*v))
            .ok_or_else(|| Failure::Usage(format!("{name} {v}: not a number it takes"))),
    }
}

/// Puts a password on a file.
fn lock(args: &[String], json: bool) -> Done {
    let args = Args::parse(args)?;
    let [file] = args.words()?;
    let out = args.output()?;
    let doc = open(file)?;
    let permissions = match args.option("--allow") {
        None => katna_pdf_cos::Permissions::ALL,
        Some(list) => {
            let mut p = katna_pdf_cos::Permissions {
                print: false,
                print_well: false,
                change: false,
                copy: false,
                comment: false,
                fill_forms: false,
                assemble: false,
            };
            for what in list.split(',').map(str::trim).filter(|w| !w.is_empty()) {
                match what {
                    "print" => p.print = true,
                    "print-well" => {
                        p.print = true;
                        p.print_well = true;
                    }
                    "change" => p.change = true,
                    "copy" => p.copy = true,
                    "comment" => p.comment = true,
                    "fill" => p.fill_forms = true,
                    "assemble" => p.assemble = true,
                    _ => {
                        return Err(Failure::Usage(format!(
                            "--allow {what}: print, print-well, change, copy, comment, \
                             fill or assemble"
                        )));
                    }
                }
            }
            p
        }
    };
    let user = args.option("--user").unwrap_or("");
    let owner = args.option("--owner").unwrap_or("");
    if user.is_empty() && owner.is_empty() && args.option("--allow").is_none() {
        return Err(Failure::Usage(
            "lock needs --user, --owner or --allow".into(),
        ));
    }
    let bytes = doc
        .lock(user, owner, permissions)
        .map_err(|e| failed(format!("{file}: {e}")))?;
    write(&out, &bytes)?;
    Ok(wrote(&[(out, doc.pages().len())], json))
}

/// Takes a file's password off.
fn unlock(args: &[String], json: bool) -> Done {
    let args = Args::parse(args)?;
    let [file] = args.words()?;
    let out = args.output()?;
    let doc = open_with(file, args.option("--owner"))?;
    let bytes = doc
        .without_lock()
        .map_err(|e| failed(format!("{file}: {e}")))?;
    write(&out, &bytes)?;
    Ok(wrote(&[(out, doc.pages().len())], json))
}

/// Takes what lies under boxes out of a file.
fn redact(args: &[String], json: bool) -> Done {
    use katna_pdf_cos::{Pattern, RedactOptions};
    let args = Args::parse(args)?;
    let [file] = args.words()?;
    let out = args.output()?;
    let doc = open(file)?;
    let count = doc.pages().len();
    let mut areas = Vec::new();
    for area in args.all("--area") {
        let (page, rect) = area
            .split_once(':')
            .ok_or_else(|| Failure::Usage(format!("--area {area}: PAGE:L,B,R,T")))?;
        let page: usize = page
            .parse()
            .ok()
            .filter(|p| (1..=count).contains(p))
            .ok_or_else(|| Failure::Usage(format!("--area {area}: no page {page}")))?;
        areas.push((page - 1, parse_rect(rect)?));
    }
    let mut patterns: Vec<Pattern> = args
        .all("--words")
        .into_iter()
        .map(|w| Pattern::Words(w.to_owned()))
        .collect();
    for list in args.all("--find") {
        for what in list.split(',').map(str::trim).filter(|w| !w.is_empty()) {
            patterns.push(match what {
                "pan" => Pattern::Pan,
                "aadhaar" => Pattern::Aadhaar,
                "phone" => Pattern::Phone,
                "email" => Pattern::Email,
                _ => {
                    return Err(Failure::Usage(format!(
                        "--find {what}: pan, aadhaar, phone or email"
                    )));
                }
            });
        }
    }
    if !patterns.is_empty() {
        let pages = match args.option("--pages") {
            Some(spec) => ranges::parse(spec, count).map_err(failed)?,
            None => (0..count).collect(),
        };
        for page in pages {
            for rect in doc.find_redactions(page, &patterns) {
                areas.push((page, rect));
            }
        }
    }
    if areas.is_empty() && !args.flag("--scrub") {
        return Err(failed(format!("{file}: nothing to redact")));
    }
    let options = RedactOptions {
        fill: (!args.flag("--no-fill")).then_some([0.0; 3]),
        scrub: args.flag("--scrub"),
    };
    let bytes = doc
        .redact(&areas, &options)
        .map_err(|e| failed(format!("{file}: {e}")))?;
    write(&out, &bytes)?;
    if json {
        let list: Vec<String> = areas
            .iter()
            .map(|(p, r)| {
                format!(
                    "{{\"page\":{},\"rect\":[{},{},{},{}]}}",
                    p + 1,
                    r[0],
                    r[1],
                    r[2],
                    r[3]
                )
            })
            .collect();
        return Ok(format!(
            "{{\"written\":[{{\"file\":{},\"pages\":{count}}}],\"areas\":[{}]}}\n",
            string(&out.display().to_string()),
            list.join(",")
        ));
    }
    let mut text = String::new();
    for (p, r) in &areas {
        text.push_str(&format!(
            "redacted {}: {:.1},{:.1},{:.1},{:.1}\n",
            p + 1,
            r[0],
            r[1],
            r[2],
            r[3]
        ));
    }
    text.push_str(&wrote(&[(out, count)], false));
    Ok(text)
}

/// Changes the document's properties.
fn meta(args: &[String], json: bool) -> Done {
    let args = Args::parse(args)?;
    let [file] = args.words()?;
    let out = args.output()?;
    let doc = open(file)?;
    let mut meta = doc.metadata();
    for (name, field) in [
        ("--title", &mut meta.title),
        ("--author", &mut meta.author),
        ("--subject", &mut meta.subject),
        ("--keywords", &mut meta.keywords),
        ("--creator", &mut meta.creator),
        ("--producer", &mut meta.producer),
    ] {
        if let Some(value) = args.option(name) {
            *field = Some(value.to_owned());
        }
    }
    let bytes = doc
        .set_metadata(&meta)
        .map_err(|e| failed(format!("{file}: {e}")))?;
    write(&out, &bytes)?;
    Ok(wrote(&[(out, doc.pages().len())], json))
}

/// Draws a header, footer, watermark or Bates number on many pages.
fn stamp(args: &[String], json: bool) -> Done {
    use katna_pdf_cos::StampPlace;
    let args = Args::parse(args)?;
    let [file] = args.words()?;
    let out = args.output()?;
    let doc = open(file)?;
    let count = doc.pages().len();
    let pages = match args.option("--pages") {
        Some(spec) => ranges::parse(spec, count).map_err(failed)?,
        None => (0..count).collect(),
    };
    let text = args
        .option("--text")
        .ok_or_else(|| Failure::Usage("say what with --text".into()))?;
    let place = match args.option("--place").unwrap_or("bottom") {
        "top-left" => StampPlace::TopLeft,
        "top" => StampPlace::TopCentre,
        "top-right" => StampPlace::TopRight,
        "bottom-left" => StampPlace::BottomLeft,
        "bottom" => StampPlace::BottomCentre,
        "bottom-right" => StampPlace::BottomRight,
        "middle" => StampPlace::Middle,
        "across" => StampPlace::Across,
        p => {
            return Err(Failure::Usage(format!(
                "--place {p}: top-left, top, top-right, bottom-left, bottom, \
                 bottom-right, middle or across"
            )));
        }
    };
    let mark = matches!(place, StampPlace::Across | StampPlace::Middle);
    let bates = [
        "--bates-prefix",
        "--bates-suffix",
        "--bates-start",
        "--bates-digits",
    ]
    .iter()
    .any(|o| args.option(o).is_some())
    .then(|| -> Result<_, Failure> {
        let whole = |name: &str, default: u64| match args.option(name) {
            None => Ok(default),
            Some(v) => v
                .parse::<u64>()
                .map_err(|_| Failure::Usage(format!("{name} {v}: a whole number"))),
        };
        Ok(katna_pdf_cos::Bates {
            prefix: args.option("--bates-prefix").unwrap_or("").to_owned(),
            suffix: args.option("--bates-suffix").unwrap_or("").to_owned(),
            start: whole("--bates-start", 1)?,
            digits: whole("--bates-digits", 6)?.min(20) as usize,
        })
    })
    .transpose()?;
    let stamp = katna_pdf_cos::Stamp {
        text: text.replace("\\n", "\n"),
        place,
        font: args.option("--font").unwrap_or("Helvetica").to_owned(),
        size: number_option(&args, "--size", if mark { 60.0 } else { 10.0 }, |v| v > 0.0)?,
        colour: colour_option(&args, if mark { [0.5, 0.5, 0.5] } else { [0.0; 3] })?,
        opacity: number_option(&args, "--opacity", if mark { 0.3 } else { 1.0 }, |v| {
            (0.0..=1.0).contains(&v)
        })?,
        margin: number_option(&args, "--margin", 36.0, |v| v >= 0.0)?,
        under: args.flag("--under"),
        bates,
    };
    let bytes = doc
        .stamp(&pages, &stamp)
        .map_err(|e| failed(format!("{file}: {e}")))?;
    write(&out, &bytes)?;
    Ok(wrote(&[(out, count)], json))
}

/// Lists the pictures, forms and shapes drawn on pages.
fn objects(args: &[String], json: bool) -> Done {
    let args = Args::parse(args)?;
    let [file] = args.words()?;
    let doc = open(file)?;
    let count = doc.pages().len();
    let pages = match args.option("--pages") {
        Some(spec) => ranges::parse(spec, count).map_err(failed)?,
        None => (0..count).collect(),
    };
    let mut out = String::new();
    let mut items = Vec::new();
    for page in pages {
        for (i, object) in doc.page_objects(page).iter().enumerate() {
            let kind = match object.kind {
                katna_pdf_cos::ObjectKind::Picture => "picture",
                katna_pdf_cos::ObjectKind::Form => "form",
                katna_pdf_cos::ObjectKind::Path => "shape",
            };
            let rect = object.rect.map(num);
            if json {
                items.push(format!(
                    "{{\"object\":\"{}:{}\",\"kind\":\"{kind}\",\"rect\":[{}]}}",
                    page + 1,
                    i + 1,
                    rect.join(",")
                ));
            } else {
                let _ = writeln!(
                    out,
                    "{:<8} {kind:<8} {}",
                    format!("{}:{}", page + 1, i + 1),
                    rect.join(",")
                );
            }
        }
    }
    if json {
        out = format!("{{\"objects\":[{}]}}\n", items.join(","));
    }
    Ok(out)
}

/// A place written as `PAGE` or `PAGE@TOP`, the page counted from 1.
fn place(text: &str, count: usize) -> Result<katna_pdf_cos::Destination, Failure> {
    let bad = || Failure::Usage(format!("{text}: PAGE or PAGE@TOP, pages from 1 to {count}"));
    let (page, top) = match text.trim().split_once('@') {
        Some((p, t)) => (p, Some(t.trim().parse::<f32>().map_err(|_| bad())?)),
        None => (text.trim(), None),
    };
    let page = page.trim().parse::<usize>().map_err(|_| bad())?;
    if page == 0 || page > count {
        return Err(bad());
    }
    Ok(katna_pdf_cos::Destination {
        page: page - 1,
        top,
    })
}

fn show_place(dest: &katna_pdf_cos::Destination) -> String {
    match dest.top {
        Some(top) => format!("{}@{}", dest.page + 1, num(top)),
        None => (dest.page + 1).to_string(),
    }
}

fn place_json(dest: &katna_pdf_cos::Destination) -> String {
    match dest.top {
        Some(top) => format!("{{\"page\":{},\"top\":{}}}", dest.page + 1, num(top)),
        None => format!("{{\"page\":{}}}", dest.page + 1),
    }
}

/// Lists the outline, or writes it anew.
fn bookmarks(args: &[String], json: bool) -> Done {
    use katna_pdf_cos::OutlineItem;
    let args = Args::parse(args)?;
    let [file] = args.words()?;
    let doc = open(file)?;
    let count = doc.pages().len();
    let Some(out) = args.option("-o") else {
        fn list(items: &[OutlineItem], depth: usize, out: &mut String) {
            for item in items {
                let to = item.dest.as_ref().map(show_place).unwrap_or_default();
                let _ = writeln!(out, "{}{}\t{to}", "  ".repeat(depth), item.title);
                list(&item.children, depth + 1, out);
            }
        }
        fn list_json(items: &[OutlineItem]) -> String {
            let items: Vec<String> = items
                .iter()
                .map(|i| {
                    format!(
                        "{{\"title\":{},\"to\":{},\"open\":{},\"children\":{}}}",
                        string(&i.title),
                        i.dest.as_ref().map_or("null".into(), place_json),
                        i.open,
                        list_json(&i.children)
                    )
                })
                .collect();
            format!("[{}]", items.join(","))
        }
        let items = doc.outline();
        if json {
            return Ok(format!("{{\"bookmarks\":{}}}\n", list_json(&items)));
        }
        let mut out = String::new();
        list(&items, 0, &mut out);
        return Ok(out);
    };
    let mut items = if args.flag("--clear") {
        Vec::new()
    } else if let Some(from) = args.option("--from") {
        let text = std::fs::read_to_string(from).map_err(|e| failed(format!("{from}: {e}")))?;
        // Each line under the one it is indented beneath.
        let mut items: Vec<OutlineItem> = Vec::new();
        for (n, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let depth = (line.len() - line.trim_start_matches(' ').len()) / 2;
            let (title, to) = line
                .trim_start()
                .split_once('\t')
                .unwrap_or((line.trim(), ""));
            let item = OutlineItem {
                title: title.trim().to_owned(),
                dest: match to.trim() {
                    "" => None,
                    to => Some(place(to, count)?),
                },
                open: false,
                children: Vec::new(),
            };
            let mut list = &mut items;
            for _ in 0..depth {
                if list.is_empty() {
                    return Err(Failure::Usage(format!(
                        "{from}:{}: indented with nothing above it",
                        n + 1
                    )));
                }
                list = &mut list.last_mut().expect("not empty").children;
            }
            list.push(item);
        }
        items
    } else {
        doc.outline()
    };
    for add in args.all("--add") {
        let (to, title) = add
            .split_once(':')
            .ok_or_else(|| Failure::Usage(format!("--add {add}: PAGE[@TOP]:TITLE")))?;
        items.push(OutlineItem {
            title: title.to_owned(),
            dest: Some(place(to, count)?),
            open: false,
            children: Vec::new(),
        });
    }
    let bytes = doc
        .set_outline(&items)
        .map_err(|e| failed(format!("{file}: {e}")))?;
    let out = PathBuf::from(out);
    write(&out, &bytes)?;
    Ok(wrote(&[(out, count)], json))
}

/// Lists the links, or takes some out and adds others.
fn links(args: &[String], json: bool) -> Done {
    use katna_pdf_cos::{Link, LinkTarget};
    let args = Args::parse(args)?;
    let [file] = args.words()?;
    let doc = open(file)?;
    let count = doc.pages().len();
    let Some(out) = args.option("-o") else {
        let pages = match args.option("--pages") {
            Some(spec) => ranges::parse(spec, count).map_err(failed)?,
            None => (0..count).collect(),
        };
        let all = doc.links();
        let mut out = String::new();
        let mut items = Vec::new();
        for page in pages {
            for (i, link) in all[page].iter().enumerate() {
                let rect = link.rect.map(num).join(",");
                let id = format!("{}:{}", page + 1, i + 1);
                match &link.target {
                    LinkTarget::Page(d) if json => items.push(format!(
                        "{{\"link\":\"{id}\",\"rect\":[{rect}],\"to\":{}}}",
                        place_json(d)
                    )),
                    LinkTarget::Uri(u) if json => items.push(format!(
                        "{{\"link\":\"{id}\",\"rect\":[{rect}],\"uri\":{}}}",
                        string(u)
                    )),
                    LinkTarget::Page(d) => {
                        let _ = writeln!(out, "{id:<8} {rect:<24} page {}", show_place(d));
                    }
                    LinkTarget::Uri(u) => {
                        let _ = writeln!(out, "{id:<8} {rect:<24} {u}");
                    }
                }
            }
        }
        if json {
            out = format!("{{\"links\":[{}]}}\n", items.join(","));
        }
        return Ok(out);
    };
    // Changes by page: links out, links in.
    let mut changes: std::collections::BTreeMap<usize, (Vec<usize>, Vec<Link>)> =
        std::collections::BTreeMap::new();
    for id in args.all("--remove") {
        let bad = || Failure::Usage(format!("--remove {id}: PAGE:LINK, as `links` numbers them"));
        let (page, n) = id.split_once(':').ok_or_else(bad)?;
        let page = place(page, count).map_err(|_| bad())?.page;
        let n = n
            .trim()
            .parse::<usize>()
            .ok()
            .filter(|&n| n > 0)
            .ok_or_else(bad)?;
        changes.entry(page).or_default().0.push(n - 1);
    }
    for add in args.all("--add") {
        let bad = || Failure::Usage(format!("--add {add}: PAGE:L,B,R,T:TO"));
        let mut parts = add.splitn(3, ':');
        let (Some(page), Some(rect), Some(to)) = (parts.next(), parts.next(), parts.next()) else {
            return Err(bad());
        };
        let page = place(page, count).map_err(|_| bad())?.page;
        let rect = parse_rect(rect).map_err(|_| bad())?;
        let to = to.trim();
        let target = if to.contains("://") || to.starts_with("mailto:") {
            LinkTarget::Uri(to.to_owned())
        } else {
            LinkTarget::Page(place(to, count)?)
        };
        changes
            .entry(page)
            .or_default()
            .1
            .push(Link { rect, target });
    }
    if changes.is_empty() {
        return Err(Failure::Usage(
            "say what to change with --remove or --add".into(),
        ));
    }
    let mut bytes = std::fs::read(file).map_err(|e| failed(format!("{file}: {e}")))?;
    let mut doc = doc;
    for (page, (remove, add)) in changes {
        bytes = doc
            .change_links(page, &remove, &add)
            .map_err(|e| failed(format!("{file}: {e}")))?;
        doc = Document::load(bytes.clone()).map_err(|e| failed(format!("{file}: {e}")))?;
    }
    let out = PathBuf::from(out);
    write(&out, &bytes)?;
    Ok(wrote(&[(out, count)], json))
}

/// Puts a colour or a picture under the pages.
fn background(args: &[String], json: bool) -> Done {
    let args = Args::parse(args)?;
    let [file] = args.words()?;
    let out = args.output()?;
    let doc = open(file)?;
    let count = doc.pages().len();
    let pages = match args.option("--pages") {
        Some(spec) => ranges::parse(spec, count).map_err(failed)?,
        None => (0..count).collect(),
    };
    let colour = match args.option("--colour") {
        Some(_) => Some(colour_option(&args, [1.0; 3])?),
        None => None,
    };
    let image = match args.option("--image") {
        Some(picture) => {
            let bytes = std::fs::read(picture).map_err(|e| failed(format!("{picture}: {e}")))?;
            Some(
                katna_pdf_sign::page_image(&bytes)
                    .map_err(|e| failed(format!("{picture}: {e}")))?,
            )
        }
        None => None,
    };
    if colour.is_none() && image.is_none() {
        return Err(Failure::Usage(
            "say what to put under with --colour or --image".into(),
        ));
    }
    let background = katna_pdf_cos::Background {
        colour,
        image,
        stretch: args.flag("--stretch"),
        opacity: number_option(&args, "--opacity", 1.0, |v| (0.0..=1.0).contains(&v))?,
    };
    let bytes = doc
        .background(&pages, &background)
        .map_err(|e| failed(format!("{file}: {e}")))?;
    write(&out, &bytes)?;
    Ok(wrote(&[(out, count)], json))
}

/// Lists each page's label, or numbers the pages anew.
fn labels(args: &[String], json: bool) -> Done {
    use katna_pdf_cos::{LabelRange, LabelStyle};
    let args = Args::parse(args)?;
    let [file] = args.words()?;
    let doc = open(file)?;
    let count = doc.pages().len();
    let Some(out) = args.option("-o") else {
        let labels = doc
            .page_labels()
            .unwrap_or_else(|| (1..=count).map(|n| n.to_string()).collect());
        if json {
            let items: Vec<String> = labels.iter().map(|l| string(l)).collect();
            return Ok(format!("{{\"labels\":[{}]}}\n", items.join(",")));
        }
        let mut out = String::new();
        for (i, label) in labels.iter().enumerate() {
            let _ = writeln!(out, "{:<6} {label}", i + 1);
        }
        return Ok(out);
    };
    let mut ranges = Vec::new();
    if !args.flag("--clear") {
        for spec in args.all("--range") {
            let bad = || Failure::Usage(format!("--range {spec}: FROM:STYLE[:PREFIX[:START]]"));
            let mut parts = spec.splitn(4, ':');
            let (Some(from), Some(style)) = (parts.next(), parts.next()) else {
                return Err(bad());
            };
            let start = place(from, count)?.page;
            let style = match style {
                "1" => LabelStyle::Decimal,
                "i" => LabelStyle::LowerRoman,
                "I" => LabelStyle::UpperRoman,
                "a" => LabelStyle::LowerLetters,
                "A" => LabelStyle::UpperLetters,
                "none" => LabelStyle::None,
                _ => return Err(bad()),
            };
            let prefix = parts.next().unwrap_or("").to_owned();
            let first = match parts.next() {
                None | Some("") => 1,
                Some(n) => n.parse::<u32>().ok().filter(|&n| n >= 1).ok_or_else(bad)?,
            };
            ranges.push(LabelRange {
                start,
                style,
                prefix,
                first,
            });
        }
        if ranges.is_empty() {
            return Err(Failure::Usage(
                "say how to number with --range, or --clear".into(),
            ));
        }
    }
    let bytes = doc
        .set_page_labels(&ranges)
        .map_err(|e| failed(format!("{file}: {e}")))?;
    let out = PathBuf::from(out);
    write(&out, &bytes)?;
    Ok(wrote(&[(out, count)], json))
}

/// Lists the named destinations, or sets and takes out some.
fn dests(args: &[String], json: bool) -> Done {
    let args = Args::parse(args)?;
    let [file] = args.words()?;
    let doc = open(file)?;
    let count = doc.pages().len();
    let mut names = doc.named_destinations();
    let Some(out) = args.option("-o") else {
        if json {
            let items: Vec<String> = names
                .iter()
                .map(|(n, d)| format!("{{\"name\":{},\"to\":{}}}", string(n), place_json(d)))
                .collect();
            return Ok(format!("{{\"dests\":[{}]}}\n", items.join(",")));
        }
        let mut out = String::new();
        for (name, dest) in &names {
            let _ = writeln!(out, "{name}\t{}", show_place(dest));
        }
        return Ok(out);
    };
    for name in args.all("--remove") {
        let before = names.len();
        names.retain(|(n, _)| n != name);
        if names.len() == before {
            return Err(failed(format!("{file}: no destination named {name}")));
        }
    }
    for set in args.all("--set") {
        let (name, to) = set
            .rsplit_once('=')
            .ok_or_else(|| Failure::Usage(format!("--set {set}: NAME=PAGE[@TOP]")))?;
        let dest = place(to, count)?;
        match names.iter_mut().find(|(n, _)| n == name) {
            Some(entry) => entry.1 = dest,
            None => names.push((name.to_owned(), dest)),
        }
    }
    let bytes = doc
        .set_named_destinations(&names)
        .map_err(|e| failed(format!("{file}: {e}")))?;
    let out = PathBuf::from(out);
    write(&out, &bytes)?;
    Ok(wrote(&[(out, count)], json))
}

/// Moves and resizes a picture or shape, or takes it out of its page.
fn object(args: &[String], json: bool) -> Done {
    let args = Args::parse(args)?;
    let [file, id] = args.words()?;
    let out = args.output()?;
    let doc = open(file)?;
    let bad = || Failure::Usage(format!("{id}: PAGE:OBJECT, as `objects` numbers them"));
    let (page, index) = id.split_once(':').ok_or_else(bad)?;
    let page = page
        .parse::<usize>()
        .ok()
        .filter(|p| (1..=doc.pages().len()).contains(p))
        .ok_or_else(bad)?
        - 1;
    let index = index
        .parse::<usize>()
        .ok()
        .and_then(|i| i.checked_sub(1))
        .ok_or_else(bad)?;
    let object = doc
        .page_objects(page)
        .into_iter()
        .nth(index)
        .ok_or_else(|| failed(format!("{id}: no such object")))?;
    let picked = [
        args.option("--rect").is_some(),
        args.flag("--delete"),
        args.flag("--turn"),
        args.flag("--front"),
        args.flag("--back"),
        args.option("--opacity").is_some(),
        args.option("--colour").is_some(),
        args.option("--image").is_some(),
    ];
    if picked.iter().filter(|p| **p).count() != 1 {
        return Err(Failure::Usage(
            "object takes one of --rect, --delete, --turn, --front, --back, --opacity, --colour and --image".into(),
        ));
    }
    let [l, b, r, t] = object.rect;
    let change = |change: ObjectChange| doc.change_object(page, index, &change);
    let bytes = if let Some(rect) = args.option("--rect") {
        let [nl, nb, nr, nt] = parse_rect(rect)?;
        let sx = if r > l { (nr - nl) / (r - l) } else { 1.0 };
        let sy = if t > b { (nt - nb) / (t - b) } else { 1.0 };
        doc.transform_object(page, index, [sx, 0.0, 0.0, sy, nl - l * sx, nb - b * sy])
    } else if args.flag("--delete") {
        doc.delete_object(page, index)
    } else if args.flag("--turn") {
        let (mx, my) = ((l + r) / 2.0, (b + t) / 2.0);
        doc.transform_object(page, index, [0.0, -1.0, 1.0, 0.0, mx - my, mx + my])
    } else if args.flag("--front") {
        change(ObjectChange::Front)
    } else if args.flag("--back") {
        change(ObjectChange::Back)
    } else if args.option("--opacity").is_some() {
        let opacity = number_option(&args, "--opacity", 1.0, |v| (0.0..=1.0).contains(&v))?;
        change(ObjectChange::Opacity(opacity))
    } else if let Some(path) = args.option("--image") {
        let bytes = std::fs::read(path).map_err(|e| failed(format!("{path}: {e}")))?;
        let image =
            katna_pdf_sign::page_image(&bytes).map_err(|e| failed(format!("{path}: {e}")))?;
        change(ObjectChange::Picture(image))
    } else {
        let ink = colour_option(&args, [0.0; 3])?;
        change(ObjectChange::Colours {
            fill: Some(ink),
            stroke: Some(ink),
        })
    }
    .map_err(|e| failed(format!("{file}: {e}")))?;
    write(&out, &bytes)?;
    Ok(wrote(&[(out, doc.pages().len())], json))
}

/// Designs the form: adds, changes, moves, copies, removes and orders
/// fields.
fn form(args: &[String], json: bool) -> Done {
    let args = Args::parse(args)?;
    let [file] = args.words()?;
    let out = args.output()?;
    let doc = open(file)?;
    let count = doc.pages().len();
    let fields = doc.form();
    let page_of = |p: &str| {
        p.parse::<usize>()
            .ok()
            .filter(|p| (1..=count).contains(p))
            .map(|p| p - 1)
            .ok_or_else(|| Failure::Usage(format!("{p}: no such page")))
    };
    let mut design = FormDesign::default();
    for add in args.all("--add") {
        let bad = || Failure::Usage(format!("--add {add}: KIND:PAGE:L,B,R,T:NAME"));
        let mut parts = add.splitn(4, ':');
        let (Some(kind), Some(page), Some(rect), Some(name)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(bad());
        };
        let (kind, arg) = kind.split_once('=').unwrap_or((kind, ""));
        let options = || arg.split(';').map(str::to_owned).collect::<Vec<_>>();
        let text = |multiline, password| NewField::Text {
            multiline,
            password,
            max_len: None,
        };
        let kind = match kind {
            "text" => text(false, false),
            "multiline" => text(true, false),
            "password" => text(false, true),
            "check" => NewField::CheckBox,
            "radio" if !arg.is_empty() => NewField::Radio {
                choice: arg.to_owned(),
            },
            "combo" | "list" if !arg.is_empty() => NewField::Choice {
                options: options(),
                combo: kind == "combo",
                editable: false,
            },
            "button" => NewField::PushButton {
                caption: arg.to_owned(),
            },
            "signature" => NewField::Signature,
            _ => return Err(bad()),
        };
        design.add.push(FieldSpec {
            page: page_of(page)?,
            rect: parse_rect(rect)?,
            name: name.to_owned(),
            kind,
            tooltip: String::new(),
            required: false,
            read_only: false,
            value: None,
            scripts: katna_pdf_cos::Scripts::default(),
        });
    }
    if let Some(spec) = args.option("--detect") {
        let pages = match spec {
            "all" => (0..count).collect(),
            spec => ranges::parse(spec, count).map_err(failed)?,
        };
        let mut taken: Vec<String> = fields.iter().map(|f| f.name.clone()).collect();
        taken.extend(design.add.iter().map(|s| s.name.clone()));
        for page in pages {
            let found = doc.detect_fields(page, &taken);
            taken.extend(found.iter().map(|s| s.name.clone()));
            design.add.extend(found);
        }
    }
    let field = |name: &str| {
        fields
            .iter()
            .find(|f| f.name == name)
            .ok_or_else(|| failed(format!("{file}: no field named {name}")))
    };
    let mut edits: Vec<FieldEdit> = Vec::new();
    // A change to a field added here goes into what is added.
    let mut change =
        |name: &str, how: &dyn Fn(&mut FieldSpec, &mut FieldEdit)| -> Result<(), Failure> {
            let mut scratch = FieldEdit::new(katna_pdf_cos::Ref::new(0, 0));
            let mut added = false;
            for spec in design.add.iter_mut().filter(|s| s.name == name) {
                how(spec, &mut scratch);
                added = true;
            }
            if added {
                return Ok(());
            }
            let id = field(name)?.id;
            let at = match edits.iter().position(|e| e.field == id) {
                Some(i) => i,
                None => {
                    edits.push(FieldEdit::new(id));
                    edits.len() - 1
                }
            };
            let mut spec = FieldSpec {
                page: 0,
                rect: [0.0; 4],
                name: String::new(),
                kind: NewField::Signature,
                tooltip: String::new(),
                required: false,
                read_only: false,
                value: None,
                scripts: katna_pdf_cos::Scripts::default(),
            };
            how(&mut spec, &mut edits[at]);
            Ok(())
        };
    for tip in args.all("--tip") {
        let (name, text) = tip
            .split_once('=')
            .ok_or_else(|| Failure::Usage(format!("--tip {tip}: NAME=TEXT")))?;
        change(name, &|s, e| {
            s.tooltip = text.to_owned();
            e.tooltip = Some(text.to_owned());
        })?;
    }
    for name in args.all("--required") {
        change(name, &|s, e| {
            s.required = true;
            e.required = Some(true);
        })?;
    }
    for name in args.all("--optional") {
        change(name, &|s, e| {
            s.required = false;
            e.required = Some(false);
        })?;
    }
    for name in args.all("--read-only") {
        change(name, &|s, e| {
            s.read_only = true;
            e.read_only = Some(true);
        })?;
    }
    let current = doc.form_scripts(&fields);
    let scripts_of = |e: &FieldEdit| {
        e.scripts
            .clone()
            .unwrap_or_else(|| current.get(&e.field).cloned().unwrap_or_default())
    };
    for item in args.all("--total") {
        let bad = || Failure::Usage(format!("--total {item}: NAME=SUM:FIELD,FIELD,..."));
        let (name, how) = item.split_once('=').ok_or_else(bad)?;
        let (op, from) = how.split_once(':').ok_or_else(bad)?;
        let op = match op.to_ascii_lowercase().as_str() {
            "sum" => katna_pdf_cos::CalcOp::Sum,
            "product" | "prd" => katna_pdf_cos::CalcOp::Product,
            "average" | "avg" => katna_pdf_cos::CalcOp::Average,
            "min" => katna_pdf_cos::CalcOp::Min,
            "max" => katna_pdf_cos::CalcOp::Max,
            _ => return Err(bad()),
        };
        let calc = katna_pdf_cos::Calculation::Simple {
            op,
            fields: from.split(',').map(|f| f.trim().to_owned()).collect(),
        };
        change(name, &|s, e| {
            s.scripts.calculate = Some(calc.clone());
            let mut scripts = scripts_of(e);
            scripts.calculate = Some(calc.clone());
            e.scripts = Some(scripts);
        })?;
    }
    for item in args.all("--format") {
        let bad = || {
            Failure::Usage(format!(
                "--format {item}: NAME=number:DECIMALS[:CURRENCY], percent:DECIMALS, date:PATTERN, phone or none"
            ))
        };
        let (name, how) = item.split_once('=').ok_or_else(bad)?;
        let mut parts = how.splitn(3, ':');
        let kind = parts.next().unwrap_or_default();
        let decimals = |p: Option<&str>| p.and_then(|d| d.parse::<usize>().ok()).unwrap_or(2);
        let format = match kind {
            "number" => Some(katna_pdf_cos::Format::Number {
                decimals: decimals(parts.next()),
                separators: 0,
                negative: 0,
                currency: parts.next().unwrap_or_default().to_owned(),
                currency_first: true,
            }),
            "percent" => Some(katna_pdf_cos::Format::Percent {
                decimals: decimals(parts.next()),
                separators: 0,
            }),
            "date" => Some(katna_pdf_cos::Format::Date(
                parts
                    .next()
                    .filter(|p| !p.is_empty())
                    .ok_or_else(bad)?
                    .to_owned(),
            )),
            "phone" => Some(katna_pdf_cos::Format::Special(2)),
            "none" => None,
            _ => return Err(bad()),
        };
        change(name, &|s, e| {
            s.scripts.format = format.clone();
            let mut scripts = scripts_of(e);
            scripts.format = format.clone();
            e.scripts = Some(scripts);
        })?;
    }
    for rename in args.all("--rename") {
        let (name, new) = rename
            .split_once('=')
            .ok_or_else(|| Failure::Usage(format!("--rename {rename}: NAME=NEW")))?;
        change(name, &|s, e| {
            s.name = new.to_owned();
            e.name = Some(new.to_owned());
        })?;
    }
    design.edit = edits;
    for item in args.all("--move") {
        let (name, rect) = item
            .rsplit_once(':')
            .ok_or_else(|| Failure::Usage(format!("--move {item}: NAME:L,B,R,T")))?;
        let f = field(name)?;
        let widget = f
            .widgets
            .first()
            .ok_or_else(|| failed(format!("{name} is not shown")))?;
        design.place.push((widget.id, parse_rect(rect)?));
    }
    for item in args.all("--copy") {
        let (name, pages) = item
            .rsplit_once(':')
            .ok_or_else(|| Failure::Usage(format!("--copy {item}: NAME:PAGES")))?;
        let pages = ranges::parse(pages, count).map_err(failed)?;
        design.copy.push((field(name)?.id, pages));
    }
    for name in args.all("--remove") {
        design.remove.push(field(name)?.id);
    }
    for item in args.all("--order") {
        let (page, names) = item
            .split_once(':')
            .ok_or_else(|| Failure::Usage(format!("--order {item}: PAGE:NAME,NAME,...")))?;
        let page = page_of(page)?;
        let mut widgets = Vec::new();
        for name in names.split(',') {
            widgets.extend(
                field(name)?
                    .widgets
                    .iter()
                    .filter(|w| w.page == page)
                    .map(|w| w.id),
            );
        }
        design.order.push((page, widgets));
    }
    let bytes = doc
        .design_form(&design)
        .map_err(|e| failed(format!("{file}: {e}")))?;
    write(&out, &bytes)?;
    Ok(wrote(&[(out, count)], json))
}

/// Prints the form's data as JSON or CSV, or saves it as any kind
/// form data comes in (by `-o`'s extension).
fn form_data(args: &[String], json: bool) -> Done {
    let args = Args::parse(args)?;
    let [file] = args.words()?;
    let doc = open(file)?;
    if args.option("-o").is_some() {
        let out = args.output()?;
        let pdf = Path::new(file)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        write(&out, &doc.write_data(&out.to_string_lossy(), &pdf))?;
        let count = doc.form_data().len();
        return Ok(if json {
            format!(
                "{{\"written\":[{{\"file\":{},\"fields\":{count}}}]}}\n",
                string(&out.display().to_string())
            )
        } else {
            format!("{} ({count} fields)\n", out.display())
        });
    }
    let data = doc.form_data();
    Ok(if args.flag("--csv") {
        katna_pdf_cos::data_to_csv(&data)
    } else {
        katna_pdf_cos::data_to_json(&data)
    })
}

/// Fills the form in from a JSON or CSV file and values given.
fn fill(args: &[String], json: bool) -> Done {
    let args = Args::parse(args)?;
    let [file] = args.words()?;
    let out = args.output()?;
    let doc = open(file)?;
    let mut data = Vec::new();
    if let Some(path) = args.option("--data") {
        let bytes = std::fs::read(path).map_err(|e| failed(format!("{path}: {e}")))?;
        let read = if args.flag("--csv") {
            katna_pdf_cos::data_from_csv(&String::from_utf8_lossy(&bytes))
        } else {
            katna_pdf_cos::read_data(path, &bytes)
        };
        data = read.map_err(|e| failed(format!("{path}: {e}")))?;
    }
    for set in args.all("--set") {
        let (name, value) = set
            .split_once('=')
            .ok_or_else(|| Failure::Usage(format!("--set {set}: NAME=VALUE")))?;
        data.push((name.to_owned(), value.to_owned()));
    }
    if data.is_empty() {
        return Err(Failure::Usage("give values with --data or --set".into()));
    }
    let bytes = doc
        .fill_data(&data)
        .map_err(|e| failed(format!("{file}: {e}")))?;
    write(&out, &bytes)?;
    Ok(wrote(&[(out, doc.pages().len())], json))
}

/// Adds an empty signature field, for someone to sign later.
fn add_field(args: &[String], json: bool) -> Done {
    let args = Args::parse(args)?;
    let [file] = args.words()?;
    let out = args.output()?;
    let doc = open(file)?;
    let page = page_number(&doc, args.option("--page"))?;
    let rect = parse_rect(
        args.option("--rect")
            .ok_or_else(|| Failure::Usage("say where with --rect".into()))?,
    )?;
    let bytes = doc
        .add_signature_field(page, rect, args.option("--name").map(str::to_owned))
        .map_err(|e| failed(format!("{file}: {e}")))?;
    write(&out, &bytes)?;
    Ok(wrote(&[(out, doc.pages().len())], json))
}

fn modules(module: Option<&str>) -> Result<Vec<pkcs11::Module>, Failure> {
    let paths = match module {
        Some(m) => vec![PathBuf::from(m)],
        None => pkcs11::known_modules(),
    };
    if paths.is_empty() {
        return Err(failed(
            "no token driver is installed: install OpenSC, or name the token's PKCS#11 module with --module",
        ));
    }
    let mut out = Vec::new();
    for path in paths {
        match pkcs11::Module::load(&path) {
            Ok(m) => out.push(m),
            Err(e) if module.is_some() => return Err(failed(format!("{}: {e}", path.display()))),
            Err(_) => {}
        }
    }
    Ok(out)
}

/// The signing key on the token labelled `label` (`any` for the first
/// one plugged in), for the certificate named `name` when given.
fn token_identity(
    module: Option<&str>,
    label: &str,
    name: Option<&str>,
    pin: &str,
) -> Result<pkcs11::TokenIdentity, Failure> {
    for m in modules(module)? {
        let tokens = m.tokens().map_err(failed)?;
        let Some(token) = tokens.iter().find(|t| label == "any" || t.label == label) else {
            continue;
        };
        let ids = m
            .open(token, pin)
            .map_err(|e| failed(format!("{}: {e}", token.label)))?;
        return ids
            .into_iter()
            .find(|id| name.is_none_or(|n| id.name() == n))
            .ok_or_else(|| {
                failed(format!(
                    "{}: no certificate named {}",
                    token.label,
                    name.unwrap_or("")
                ))
            });
    }
    Err(failed(if label == "any" {
        "no token is plugged in".to_owned()
    } else {
        format!("no token labelled {label} is plugged in")
    }))
}

fn tokens(args: &[String], json: bool) -> Done {
    let args = Args::parse(args)?;
    let [] = args.words()?;
    let mut out = String::new();
    let mut items = Vec::new();
    for m in modules(args.option("--module"))? {
        for t in m.tokens().map_err(failed)? {
            let module = m.path().display().to_string();
            // Readable without the PIN on most tokens.
            let certs: Vec<(String, String, String)> = m
                .certificates(&t)
                .unwrap_or_default()
                .iter()
                .map(|c| {
                    let tbs = &c.tbs_certificate;
                    (
                        common_name(&tbs.subject),
                        common_name(&tbs.issuer),
                        katna_pdf_sign::shown_date(tbs.validity.not_after.to_system_time()),
                    )
                })
                .collect();
            if json {
                items.push(format!(
                    "{{\"label\":{},\"manufacturer\":{},\"model\":{},\"serial\":{},\"module\":{},\"certificates\":[{}]}}",
                    string(&t.label),
                    string(&t.manufacturer),
                    string(&t.model),
                    string(&t.serial),
                    string(&module),
                    certs
                        .iter()
                        .map(|(name, issuer, until)| format!(
                            "{{\"name\":{},\"issuer\":{},\"valid_until\":{}}}",
                            string(name),
                            string(issuer),
                            string(until)
                        ))
                        .collect::<Vec<_>>()
                        .join(",")
                ));
            } else {
                let _ = writeln!(
                    out,
                    "{} ({} {}, serial {})\n  driver: {module}",
                    t.label, t.manufacturer, t.model, t.serial
                );
                for (name, issuer, until) in &certs {
                    let _ = writeln!(
                        out,
                        "  certificate: {name}, issued by {issuer}, valid until {until}"
                    );
                }
            }
        }
    }
    if json {
        Ok(format!("{{\"tokens\":[{}]}}\n", items.join(",")))
    } else if out.is_empty() {
        Ok("no tokens plugged in\n".into())
    } else {
        Ok(out)
    }
}

fn token_check(args: &[String], json: bool) -> Done {
    let args = Args::parse(args)?;
    let [] = args.words()?;
    let mut lines = Vec::new();
    let mut items = Vec::new();
    for f in pkcs11::diagnose() {
        match f {
            pkcs11::Finding::Device { name, id } => {
                lines.push(format!("ok      plugged in: {name} (USB {id})"));
                items.push(format!(
                    "{{\"step\":\"device\",\"ok\":true,\"name\":{},\"id\":{}}}",
                    string(&name),
                    string(&id)
                ));
            }
            pkcs11::Finding::NoDevice => {
                lines.push("problem no smart card device is plugged in".into());
                items.push("{\"step\":\"device\",\"ok\":false}".into());
            }
            pkcs11::Finding::Service { running } => {
                lines.push(if running {
                    "ok      the PC/SC service (pcscd) is running".into()
                } else {
                    "problem the PC/SC service (pcscd) is not running: sudo systemctl enable --now pcscd.socket".into()
                });
                items.push(format!("{{\"step\":\"service\",\"ok\":{running}}}"));
            }
            pkcs11::Finding::NoModule => {
                lines.push(
                    "problem no token driver is installed: install OpenSC (Arch: sudo pacman -S opensc)"
                        .into(),
                );
                items.push("{\"step\":\"driver\",\"ok\":false}".into());
            }
            pkcs11::Finding::Module { path, tokens } => {
                let path = path.display().to_string();
                match &tokens {
                    Ok(t) if t.is_empty() => {
                        lines.push(format!("problem driver {path} sees no token"));
                    }
                    Ok(t) => {
                        for (label, certs) in t {
                            lines.push(format!(
                                "ok      driver {path} sees {label}, with {certs} signing certificate{}",
                                if *certs == 1 { "" } else { "s" }
                            ));
                        }
                    }
                    Err(e) => lines.push(format!("problem driver {path} did not load: {e}")),
                }
                items.push(format!(
                    "{{\"step\":\"driver\",\"path\":{},\"ok\":{},{}}}",
                    string(&path),
                    tokens.as_ref().is_ok_and(|t| !t.is_empty()),
                    match &tokens {
                        Ok(t) => format!(
                            "\"tokens\":[{}]",
                            t.iter()
                                .map(|(l, c)| format!(
                                    "{{\"label\":{},\"certificates\":{c}}}",
                                    string(l)
                                ))
                                .collect::<Vec<_>>()
                                .join(",")
                        ),
                        Err(e) => format!("\"error\":{}", string(e)),
                    }
                ));
            }
        }
    }
    if json {
        Ok(format!("{{\"steps\":[{}]}}\n", items.join(",")))
    } else {
        Ok(lines.join("\n") + "\n")
    }
}

fn verify(args: &[String], json: bool) -> Done {
    let args = Args::parse(args)?;
    let [file] = args.words()?;
    let bytes = std::fs::read(file).map_err(|e| failed(format!("{file}: {e}")))?;
    let doc = Document::load(bytes.clone()).map_err(|e| failed(format!("{file}: {e}")))?;
    let roots = trusted(&args)?;
    let mut checks = katna_pdf_sign::check(&doc, &bytes, &roots);
    if args.flag("--online") {
        let fetch = katna_pdf_sign::revocation::Http::default();
        for c in &mut checks {
            katna_pdf_sign::check_online(c, &roots, &fetch);
        }
    }
    let mut out = String::new();
    let mut items = Vec::new();
    for c in &checks {
        let s = &c.signature;
        let covers = match c.coverage {
            Coverage::Whole => "whole".to_owned(),
            Coverage::Revision(n) => format!("revision:{n}"),
            Coverage::Broken => "broken".to_owned(),
        };
        let (signer, time) = (c.signer(), c.time());
        let revoked = match &c.revocation {
            Some(Revocation::Revoked { name, when }) => Some(format!(
                "{name}'s certificate was revoked ({})",
                when.as_deref().unwrap_or("?")
            )),
            _ => None,
        };
        let problem = c.problem().or(revoked);
        let revocation = match &c.revocation {
            None => None,
            Some(Revocation::Good) => Some(("good", None)),
            Some(Revocation::Revoked { name, when }) => Some((
                "revoked",
                Some(format!("{name} ({})", when.as_deref().unwrap_or("?"))),
            )),
            Some(Revocation::Unknown { name, why }) => {
                Some(("unknown", Some(format!("{name}: {why}"))))
            }
        };
        let stamp = c.result.as_ref().ok().and_then(|r| r.timestamp.clone());
        let (trust, root) = match &c.trust {
            None => ("unknown", None),
            Some(Trust::Trusted { root }) => ("trusted", Some(root.clone())),
            Some(Trust::Untrusted { root, complete }) => (
                if *complete {
                    "untrusted-root"
                } else {
                    "incomplete"
                },
                Some(common_name(&root.tbs_certificate.subject)),
            ),
            Some(Trust::Expired { name }) => ("expired", Some(name.clone())),
            Some(Trust::Forged { name }) => ("forged", Some(name.clone())),
        };
        if json {
            let opt = |v: &Option<String>| v.as_deref().map_or("null".into(), string);
            items.push(format!(
                "{{\"field\":{},\"kind\":{},\"page\":{},\"intact\":{},\"covers\":{},\"signer\":{},\"time\":{},\"reason\":{},\"location\":{},\"sub_filter\":{},\"timestamp\":{},\"trust\":{{\"state\":{},\"certificate\":{}}},\"revocation\":{},\"certifies\":{},\"locks\":{},\"problem\":{}}}",
                string(&s.name),
                string(if c.is_document_timestamp() {
                    "document-timestamp"
                } else {
                    "signature"
                }),
                s.page.map_or("null".into(), |p| (p + 1).to_string()),
                c.intact(),
                string(&covers),
                opt(&signer),
                opt(&time),
                opt(&s.info.reason),
                opt(&s.info.location),
                string(&s.sub_filter),
                stamp.as_ref().map_or("null".into(), |t| format!(
                    "{{\"valid\":{},\"time\":{},\"authority\":{}}}",
                    t.valid,
                    opt(&t.time),
                    opt(&t.authority)
                )),
                string(trust),
                opt(&root),
                revocation.as_ref().map_or("null".into(), |(state, detail)| format!(
                    "{{\"state\":{},\"detail\":{}}}",
                    string(state),
                    opt(detail)
                )),
                s.certify.map_or("null".into(), |a| format!(
                    "{{\"allows\":{},\"forbidden_changes\":[{}]}}",
                    string(match a {
                        Allowed::Nothing => "none",
                        Allowed::FormFilling => "forms",
                        Allowed::Comments => "comments",
                    }),
                    c.forbidden
                        .iter()
                        .map(|f| string(katna_pdf_sign::change_text(*f)))
                        .collect::<Vec<_>>()
                        .join(",")
                )),
                s.lock.as_ref().map_or("null".into(), |l| {
                    let names = |n: &[String]| n.iter().map(|n| string(n)).collect::<Vec<_>>().join(",");
                    let (action, fields) = match l {
                        Lock::All => ("all", String::new()),
                        Lock::Include(n) => ("include", names(n)),
                        Lock::Exclude(n) => ("exclude", names(n)),
                    };
                    format!(
                        "{{\"action\":{},\"fields\":[{fields}],\"changed\":[{}]}}",
                        string(action),
                        c.unlocked.iter().map(|n| string(n)).collect::<Vec<_>>().join(",")
                    )
                }),
                opt(&problem),
            ));
        }
        // The text, for the terminal or the report.
        {
            let _ = writeln!(
                out,
                "{}{}: {}",
                s.name,
                if c.is_document_timestamp() {
                    " (document timestamp)"
                } else {
                    ""
                },
                match &problem {
                    None => "intact".to_owned(),
                    Some(p) => format!("NOT VALID: {p}"),
                }
            );
            let mut line = |k: &str, v: &Option<String>| {
                if let Some(v) = v {
                    let _ = writeln!(out, "  {k}: {v}");
                }
            };
            line(
                if c.is_document_timestamp() {
                    "timestamped by"
                } else {
                    "signed by"
                },
                &signer,
            );
            line(
                "identity",
                &Some(match (trust, &root) {
                    ("trusted", Some(r)) => format!("vouched for by {r}"),
                    ("untrusted-root", Some(r)) => {
                        format!("NOT VERIFIED: {r} is not a trusted root")
                    }
                    ("incomplete", Some(r)) => {
                        format!("NOT VERIFIED: the issuer of {r} was not sent with the signature")
                    }
                    ("expired", Some(r)) => format!("NOT VERIFIED: {r} was not valid at the time"),
                    ("forged", Some(r)) => {
                        format!("NOT VERIFIED: {r} was not issued by its issuer")
                    }
                    _ => "unknown".to_owned(),
                }),
            );
            line(
                "revocation",
                &revocation.as_ref().map(|(state, detail)| match detail {
                    Some(d) => format!("{state}: {d}"),
                    None => (*state).to_owned(),
                }),
            );
            line(
                "certifies",
                &s.certify
                    .map(|a| format!("the file, allowing {}", katna_pdf_sign::allowed_text(a))),
            );
            line("locks", &s.lock.as_ref().map(katna_pdf_sign::lock_text));
            line("time", &time);
            line("reason", &s.info.reason);
            line("location", &s.info.location);
            line(
                "timestamp",
                &stamp.as_ref().map(|t| {
                    format!(
                        "{}{}{}",
                        t.time.as_deref().unwrap_or("?"),
                        t.authority
                            .as_deref()
                            .map(|a| format!(" by {a}"))
                            .unwrap_or_default(),
                        if t.valid { "" } else { " (NOT VALID)" }
                    )
                }),
            );
            line("page", &s.page.map(|p| (p + 1).to_string()));
            let _ = writeln!(
                out,
                "  covers: {}",
                match c.coverage {
                    Coverage::Whole => "the whole file".to_owned(),
                    Coverage::Revision(n) if c.later.iter().all(|l| *l == Change::Dss) => format!(
                        "the file as signed (first {n} bytes); only validation data added since"
                    ),
                    Coverage::Revision(n) => format!(
                        "the file as signed (first {n} bytes); changed since ({})",
                        c.later
                            .iter()
                            .map(|l| katna_pdf_sign::change_text(*l))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    Coverage::Broken => "a wrong byte range".to_owned(),
                }
            );
        }
    }
    if checks.is_empty() {
        out = "no signatures\n".into();
    }
    if let Some(report) = args.option("--report") {
        let name = Path::new(file)
            .file_name()
            .map_or(file.into(), |n| n.to_string_lossy());
        let mut lines = vec![
            format!("File: {name}"),
            format!(
                "  checked {}",
                katna_pdf_sign::shown_date(std::time::SystemTime::now())
            ),
            format!(
                "  revocation {}",
                if args.flag("--online") {
                    "checked online, and from the file's own validation data"
                } else {
                    "checked from the file's own validation data only"
                }
            ),
            String::new(),
        ];
        lines.extend(out.lines().map(str::to_owned));
        write(
            Path::new(report),
            &katna_pdf_cos::text_pdf("Signature report", &lines),
        )?;
        if !json {
            let _ = writeln!(out, "report: {report}");
        }
    }
    if json {
        Ok(format!("{{\"signatures\":[{}]}}\n", items.join(",")))
    } else {
        Ok(out)
    }
}

fn annots(args: &[String], json: bool) -> Done {
    let args = Args::parse(args)?;
    let [file] = args.words()?;
    let doc = open(file)?;
    let mut out = String::new();
    let mut items = Vec::new();
    for a in doc.annotations().iter().flatten() {
        let kind = match &a.kind {
            AnnotKind::Markup(style, _) => format!("{style:?}"),
            AnnotKind::Note => "Note".into(),
            AnnotKind::FreeText { .. } => "FreeText".into(),
            AnnotKind::Ink(_) => "Ink".into(),
            AnnotKind::Square { .. } => "Square".into(),
            AnnotKind::Circle { .. } => "Circle".into(),
            AnnotKind::Line { arrow: true, .. } => "Arrow".into(),
            AnnotKind::Line { .. } => "Line".into(),
            AnnotKind::Polygon(_) => "Polygon".into(),
            AnnotKind::PolyLine(_) => "PolyLine".into(),
            AnnotKind::Stamp(name) => format!("Stamp {name}"),
            AnnotKind::Other(subtype) => subtype.clone(),
        };
        if json {
            items.push(format!(
                "{{\"page\":{},\"kind\":{},\"rect\":[{}],\"author\":{},\"contents\":{},\"modified\":{}}}",
                a.page + 1,
                string(&kind),
                a.rect.iter().map(|v| num(*v)).collect::<Vec<_>>().join(","),
                string(&a.author),
                string(&a.contents),
                a.modified.as_deref().map_or("null".into(), string),
            ));
        } else {
            let _ = write!(out, "page {:<4} {kind}", a.page + 1);
            if !a.author.is_empty() {
                let _ = write!(out, " by {}", a.author);
            }
            if !a.contents.is_empty() {
                let _ = write!(out, ": {}", a.contents.replace('\n', " / "));
            }
            out.push('\n');
        }
    }
    if json {
        out = format!("{{\"annotations\":[{}]}}\n", items.join(","));
    }
    Ok(out)
}

fn text(args: &[String], json: bool) -> Done {
    let args = Args::parse(args)?;
    let [file] = args.words()?;
    let doc = open(file)?;
    let count = doc.pages().len();
    let pages = match args.option("--pages") {
        Some(spec) => ranges::parse(spec, count).map_err(failed)?,
        None => (0..count).collect(),
    };
    let mut out = String::new();
    let mut items = Vec::new();
    if args.flag("--blocks") {
        for page in pages {
            for (i, block) in doc.text_blocks(page).iter().enumerate() {
                if json {
                    items.push(format!(
                        "{{\"block\":\"{}:{}\",\"runs\":[{}],\"font\":{},\"size\":{},\"text\":{}}}",
                        page + 1,
                        i + 1,
                        block
                            .runs
                            .iter()
                            .map(|r| format!("\"{}:{}\"", page + 1, r + 1))
                            .collect::<Vec<_>>()
                            .join(","),
                        string(&block.font),
                        num(block.size),
                        string(&block.text),
                    ));
                } else {
                    let _ = writeln!(
                        out,
                        "{:<8} {} {}pt, {} lines: {}",
                        format!("{}:{}", page + 1, i + 1),
                        block.font,
                        num(block.size),
                        block.runs.len(),
                        block.text.replace('\n', " / ")
                    );
                }
            }
        }
        if json {
            out = format!("{{\"blocks\":[{}]}}\n", items.join(","));
        }
        return Ok(out);
    }
    for page in pages {
        for (i, run) in doc.text_runs(page).iter().enumerate() {
            if json {
                items.push(format!(
                    "{{\"run\":\"{}:{}\",\"font\":{},\"size\":{},\"rect\":[{}],\"text\":{}}}",
                    page + 1,
                    i + 1,
                    string(&run.font),
                    num(run.size),
                    run.rect
                        .iter()
                        .map(|v| num(*v))
                        .collect::<Vec<_>>()
                        .join(","),
                    string(&run.text),
                ));
            } else {
                let _ = writeln!(
                    out,
                    "{:<8} {} {}pt: {}",
                    format!("{}:{}", page + 1, i + 1),
                    run.font,
                    num(run.size),
                    run.text
                );
            }
        }
    }
    if json {
        out = format!("{{\"runs\":[{}]}}\n", items.join(","));
    }
    Ok(out)
}

fn edit_text(args: &[String], json: bool) -> Done {
    let args = Args::parse(args)?;
    let [file] = args.words()?;
    let out = args.output()?;
    let new = args
        .option("--to")
        .ok_or_else(|| Failure::Usage("edit-text needs --to NEW".into()))?;
    let doc = open(file)?;
    let count = doc.pages().len();
    // Each page's changes: (run, new text).
    let mut edits: Vec<(usize, Vec<(usize, String)>)> = Vec::new();
    if let Some(id) = args.option("--block") {
        let bad = || {
            Failure::Usage(format!(
                "--block {id}: PAGE:BLOCK, as `text --blocks` numbers them"
            ))
        };
        let (page, block) = id.split_once(':').ok_or_else(bad)?;
        let page = page
            .parse::<usize>()
            .ok()
            .filter(|p| (1..=count).contains(p))
            .ok_or_else(bad)?;
        let block = block
            .parse::<usize>()
            .ok()
            .and_then(|b| doc.text_blocks(page - 1).into_iter().nth(b.checked_sub(1)?))
            .ok_or_else(bad)?;
        let bytes = doc.edit_block(page - 1, &block.runs, new).map_err(failed)?;
        write(&out, &bytes)?;
        return Ok(if json {
            format!(
                "{{\"written\":[{{\"file\":{},\"pages\":{count}}}],\"runs_changed\":{}}}\n",
                string(&out.display().to_string()),
                block.runs.len()
            )
        } else {
            format!(
                "{} (a paragraph of {} lines changed)\n",
                out.display(),
                block.runs.len()
            )
        });
    }
    match (args.option("--run"), args.option("--replace")) {
        (Some(id), None) => {
            let bad = || Failure::Usage(format!("--run {id}: PAGE:RUN, as `text` numbers them"));
            let (page, run) = id.split_once(':').ok_or_else(bad)?;
            let page = page
                .parse::<usize>()
                .ok()
                .filter(|p| (1..=count).contains(p))
                .ok_or_else(bad)?;
            let run = run
                .parse::<usize>()
                .ok()
                .filter(|r| *r >= 1)
                .ok_or_else(bad)?;
            if doc.text_runs(page - 1).len() < run {
                return Err(bad());
            }
            edits.push((page - 1, vec![(run - 1, new.to_owned())]));
        }
        (None, Some(old)) if !old.is_empty() => {
            let pages = match args.option("--pages") {
                Some(spec) => ranges::parse(spec, count).map_err(failed)?,
                None => (0..count).collect(),
            };
            for page in pages {
                let changed: Vec<(usize, String)> = doc
                    .text_runs(page)
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.text.contains(old))
                    .map(|(i, r)| (i, r.text.replace(old, new)))
                    .collect();
                if !changed.is_empty() {
                    edits.push((page, changed));
                }
            }
            if edits.is_empty() {
                return Err(Failure::Failed(format!("no text holds “{old}”")));
            }
        }
        _ => {
            return Err(Failure::Usage(
                "edit-text takes one of --run PAGE:RUN and --replace OLD".into(),
            ));
        }
    }
    let mut bytes = doc.bytes().to_vec();
    let mut changed = 0;
    for (page, page_edits) in edits {
        let doc = Document::load(bytes).map_err(failed)?;
        changed += page_edits.len();
        bytes = doc.edit_text(page, &page_edits).map_err(failed)?;
    }
    write(&out, &bytes)?;
    Ok(if json {
        format!(
            "{{\"written\":[{{\"file\":{},\"pages\":{count}}}],\"runs_changed\":{changed}}}\n",
            string(&out.display().to_string())
        )
    } else {
        format!(
            "{} ({changed} run{} changed)\n",
            out.display(),
            if changed == 1 { "" } else { "s" }
        )
    })
}

fn xfdf_export(args: &[String], json: bool) -> Done {
    let args = Args::parse(args)?;
    let [file] = args.words()?;
    let out = args.output()?;
    let doc = open(file)?;
    let annots = doc.annotations();
    let name = Path::new(file)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    write(&out, katna_pdf_cos::to_xfdf(&annots, &name).as_bytes())?;
    let count: usize = annots.iter().map(Vec::len).sum();
    Ok(if json {
        format!(
            "{{\"written\":[{{\"file\":{},\"annotations\":{count}}}]}}\n",
            string(&out.display().to_string())
        )
    } else {
        format!("{} ({count} annotations)\n", out.display())
    })
}

fn xfdf_import(args: &[String], json: bool) -> Done {
    let args = Args::parse(args)?;
    let [file, xfdf] = args.words()?;
    let out = args.output()?;
    let doc = open(file)?;
    let xml = std::fs::read(xfdf).map_err(|e| failed(format!("{xfdf}: {e}")))?;
    let changes = doc
        .xfdf_changes(&String::from_utf8_lossy(&xml))
        .map_err(|e| failed(format!("{xfdf}: {e}")))?;
    let added = changes.put.len();
    let bytes = doc.annotate(&changes).map_err(failed)?;
    write(&out, &bytes)?;
    Ok(if json {
        format!(
            "{{\"written\":[{{\"file\":{},\"annotations_added\":{added}}}]}}\n",
            string(&out.display().to_string())
        )
    } else {
        format!(
            "{} ({added} annotations added or replaced)\n",
            out.display()
        )
    })
}

fn yes(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

fn num(v: f32) -> String {
    let v = (f64::from(v) * 100.0).round() / 100.0;
    format!("{v}")
}

/// `s` as a JSON string.
fn string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_strings_are_escaped() {
        assert_eq!(string("a\"b\\\n\u{1}é"), "\"a\\\"b\\\\\\n\\u0001é\"");
    }

    #[test]
    fn margins_follow_the_page_as_shown() {
        let crop = [0.0, 0.0, 100.0, 200.0];
        // Top 1, right 2, bottom 3, left 4.
        let cut = [1.0, 2.0, 3.0, 4.0];
        assert_eq!(margins(crop, 0, cut), [4.0, 3.0, 98.0, 199.0]);
        // Turned a quarter clockwise, the page's left side is at the top.
        assert_eq!(margins(crop, 90, cut), [1.0, 4.0, 97.0, 198.0]);
        assert_eq!(margins(crop, 180, cut), [2.0, 1.0, 96.0, 197.0]);
        assert_eq!(margins(crop, 270, cut), [3.0, 2.0, 99.0, 196.0]);
    }
}
