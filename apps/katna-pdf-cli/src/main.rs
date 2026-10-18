// SPDX-License-Identifier: GPL-3.0-or-later

//! `katna-pdf-cli`: Katna PDF without a window (docs/PLAN.md, Phase 2,
//! and §"agent-friendly CLI"). Every command can answer in JSON with
//! `--json`, for scripts and agents; errors then come as
//! `{"error": "..."}` too. Exit status: 0 done, 1 failed, 2 usage.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use katna_pdf_cos::{
    AnnotKind, Coverage, Document, Object, PageSource, Placement, SignRequest, SigningInfo,
    compose, text_string,
};
use katna_pdf_sign::{Digest, Signer, Trust, common_name, p12, pkcs11};

mod ranges;

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
  sign FILE -o OUT (--p12 CERT.pfx | --token LABEL [--cert NAME] [--module LIB])
          (--password-env VAR | --password-file F | --password PW)
          [--page N --rect L,B,R,T] [--reason R] [--location L] [--contact C] [--digest 256|384|512]
          [--tsa URL]
                                  sign with the key in a certificate file, or on a
                                  token (LABEL, or any), unlocked with the password
                                  or PIN (PAdES B-B);
                                  visible in a box on page N when --rect is given;
                                  timestamped by the server at URL with --tsa (PAdES B-T)
  tokens [--module LIB]           the signing tokens plugged in (ePass2003 and others)
  verify FILE [--trust PATH]      the signatures, each checked: intact, what it covers,
                                  who signed, and whether their certificate chains to a
                                  root the system trusts, or one in PATH (a certificate
                                  file or a folder of them, such as India's CCA roots)
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
        "tokens" => tokens(rest, json),
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
                    | "--reason"
                    | "--location"
                    | "--contact"
                    | "--digest"
                    | "--tsa"
                    | "--token"
                    | "--module"
                    | "--cert"
                    | "--trust"
            ) {
                let Some(value) = it.next() else {
                    return Err(Failure::Usage(format!("{a} needs a value")));
                };
                options.push((a.as_str(), value.as_str()));
            } else if a == "--uncrop" {
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

fn open(path: &str) -> Result<Document, Failure> {
    let bytes = std::fs::read(path).map_err(|e| failed(format!("{path}: {e}")))?;
    Document::load(bytes).map_err(|e| failed(format!("{path}: {e}")))
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
    let [file] = args.words()?;
    let out = args.output()?;
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
    let doc = open(file)?;
    let reason = args.option("--reason").map(str::to_owned);
    let time = std::time::SystemTime::now();
    let placement = match (args.option("--page"), args.option("--rect")) {
        (None, None) => Placement::Invisible,
        (page, Some(rect)) => {
            let page = match page {
                None => 0,
                Some(p) => {
                    p.parse::<usize>()
                        .ok()
                        .filter(|p| (1..=doc.pages().len()).contains(p))
                        .ok_or_else(|| Failure::Usage(format!("--page {p}: no such page")))?
                        - 1
                }
            };
            let rect = match rect
                .split(',')
                .map(|n| n.trim().parse::<f32>().ok().filter(|n| n.is_finite()))
                .collect::<Option<Vec<_>>>()
                .as_deref()
            {
                Some(&[l, b, r, t]) if r > l && t > b => [l, b, r, t],
                _ => {
                    return Err(Failure::Usage(format!(
                        "--rect {rect}: left,bottom,right,top"
                    )));
                }
            };
            Placement::Visible {
                page,
                rect,
                lines: katna_pdf_sign::appearance_lines(
                    id,
                    &katna_pdf_sign::shown_date(time),
                    reason.as_deref(),
                ),
            }
        }
        (Some(_), None) => {
            return Err(Failure::Usage("--page goes with --rect".into()));
        }
    };
    let mut request = SignRequest::new(
        placement,
        SigningInfo {
            name: Some(common_name(&id.certificate().tbs_certificate.subject)),
            reason,
            location: args.option("--location").map(str::to_owned),
            contact: args.option("--contact").map(str::to_owned),
        },
    );
    request.time = time;
    let server = args.option("--tsa").map(katna_pdf_sign::tsa::Server::new);
    let stamper = server
        .as_ref()
        .map(|s| s as &dyn katna_pdf_sign::tsa::Stamper);
    let signed = katna_pdf_sign::sign_with(&doc, &request, id, digest, stamper).map_err(failed)?;
    write(&out, &signed)?;
    Ok(wrote(&[(out, doc.pages().len())], json))
}

/// The modules to look for tokens with: `module`, or those installed.
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
            if json {
                items.push(format!(
                    "{{\"label\":{},\"manufacturer\":{},\"model\":{},\"serial\":{},\"module\":{}}}",
                    string(&t.label),
                    string(&t.manufacturer),
                    string(&t.model),
                    string(&t.serial),
                    string(&module)
                ));
            } else {
                let _ = writeln!(
                    out,
                    "{} ({} {}, serial {})\n  driver: {module}",
                    t.label, t.manufacturer, t.model, t.serial
                );
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

fn verify(args: &[String], json: bool) -> Done {
    let args = Args::parse(args)?;
    let [file] = args.words()?;
    let bytes = std::fs::read(file).map_err(|e| failed(format!("{file}: {e}")))?;
    let doc = Document::load(bytes.clone()).map_err(|e| failed(format!("{file}: {e}")))?;
    let mut roots = katna_pdf_sign::Roots::system();
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
    let checks = katna_pdf_sign::check(&doc, &bytes, &roots);
    let mut out = String::new();
    let mut items = Vec::new();
    for c in &checks {
        let s = &c.signature;
        let covers = match c.coverage {
            Coverage::Whole => "whole".to_owned(),
            Coverage::Revision(n) => format!("revision:{n}"),
            Coverage::Broken => "broken".to_owned(),
        };
        let (signer, problem, time) = (c.signer(), c.problem(), c.time());
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
                "{{\"field\":{},\"page\":{},\"intact\":{},\"covers\":{},\"signer\":{},\"time\":{},\"reason\":{},\"location\":{},\"sub_filter\":{},\"timestamp\":{},\"trust\":{{\"state\":{},\"certificate\":{}}},\"problem\":{}}}",
                string(&s.name),
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
                opt(&problem),
            ));
        } else {
            let _ = writeln!(
                out,
                "{}: {}",
                s.name,
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
            line("signed by", &signer);
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
                    Coverage::Revision(n) =>
                        format!("the file as signed (first {n} bytes); changed since"),
                    Coverage::Broken => "a wrong byte range".to_owned(),
                }
            );
        }
    }
    if json {
        Ok(format!("{{\"signatures\":[{}]}}\n", items.join(",")))
    } else if checks.is_empty() {
        Ok("no signatures\n".into())
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
