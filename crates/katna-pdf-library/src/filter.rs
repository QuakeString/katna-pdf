// SPDX-License-Identifier: GPL-3.0-or-later

//! Which documents to list, and in what order: from the library view's
//! sort and filter choices, or a query typed or saved as a smart
//! collection, such as `tag:tax signed author:rao 2024 sort:added`.

use rusqlite::types::Value;

/// How documents are put in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    /// By title (or file name), A to Z.
    #[default]
    Title,
    Author,
    /// Newest to the library first.
    Added,
    /// Opened most lately first; never opened last.
    Opened,
    /// Changed most lately first.
    Modified,
    /// Biggest first.
    Size,
    /// Most pages first.
    Pages,
    /// Most stars first.
    Rating,
}

impl Sort {
    pub const ALL: [Sort; 8] = [
        Sort::Title,
        Sort::Author,
        Sort::Added,
        Sort::Opened,
        Sort::Modified,
        Sort::Size,
        Sort::Pages,
        Sort::Rating,
    ];

    /// Its name in a query (`sort:added`).
    pub fn name(self) -> &'static str {
        match self {
            Sort::Title => "title",
            Sort::Author => "author",
            Sort::Added => "added",
            Sort::Opened => "opened",
            Sort::Modified => "modified",
            Sort::Size => "size",
            Sort::Pages => "pages",
            Sort::Rating => "rating",
        }
    }

    fn sql(self) -> &'static str {
        match self {
            Sort::Title => "lower(coalesce(nullif(trim(title), ''), path))",
            Sort::Author => "author IS NULL, lower(author), lower(coalesce(title, path))",
            Sort::Added => "added DESC",
            Sort::Opened => "opened IS NULL, opened DESC",
            Sort::Modified => "modified DESC",
            Sort::Size => "size DESC",
            Sort::Pages => "pages DESC",
            Sort::Rating => "rating DESC, lower(coalesce(title, path))",
        }
    }
}

/// What a document must be to be listed. Every condition given must hold.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Filter {
    /// One document only.
    pub id: Option<i64>,
    /// Words each found in the title, author, subject, keywords or path.
    pub words: Vec<String>,
    /// Tags it must all have.
    pub tags: Vec<String>,
    /// Words in the author's name.
    pub author: Option<String>,
    pub signed: Option<bool>,
    pub forms: Option<bool>,
    pub scanned: Option<bool>,
    pub locked: Option<bool>,
    /// Shown only when asked for: `Some(true)` lists only the files no
    /// longer found; `None` leaves them out.
    pub missing: Option<bool>,
    /// At least this many stars.
    pub rating: Option<u8>,
    /// Within this folder (or below it).
    pub folder: Option<String>,
    pub sort: Sort,
}

impl Filter {
    /// A query: words, and `tag:NAME`, `author:NAME`, `signed`, `forms`,
    /// `scanned`, `locked`, `missing` (each also `:no`), `rating:N`,
    /// `in:FOLDER` and `sort:ORDER`. A value with spaces goes in quotes:
    /// `tag:"to read"`.
    pub fn parse(query: &str) -> Self {
        let mut filter = Filter::default();
        for word in split(query) {
            let (key, value) = match word.split_once(':') {
                Some((k, v)) => (k.to_ascii_lowercase(), Some(v.to_owned())),
                None => (word.to_ascii_lowercase(), None),
            };
            let yes = || {
                !matches!(
                    value.as_deref().map(str::to_ascii_lowercase).as_deref(),
                    Some("no" | "false" | "0")
                )
            };
            match key.as_str() {
                "tag" if value.as_deref().is_some_and(|v| !v.is_empty()) => {
                    filter.tags.extend(value);
                }
                "author" if value.is_some() => filter.author = value,
                "in" if value.is_some() => filter.folder = value,
                "rating" => filter.rating = value.and_then(|v| v.parse().ok()),
                "signed" => filter.signed = Some(yes()),
                "forms" | "form" => filter.forms = Some(yes()),
                "scanned" | "scan" => filter.scanned = Some(yes()),
                "locked" => filter.locked = Some(yes()),
                "missing" => filter.missing = Some(yes()),
                "sort" => {
                    if let Some(sort) = Sort::ALL
                        .into_iter()
                        .find(|s| Some(s.name()) == value.as_deref())
                    {
                        filter.sort = sort;
                    }
                }
                _ => filter.words.push(word),
            }
        }
        filter
    }

    /// The query this filter is, as [`Filter::parse`] reads it.
    pub fn query(&self) -> String {
        let quote = |v: &str| {
            if v.contains(char::is_whitespace) {
                format!("\"{v}\"")
            } else {
                v.to_owned()
            }
        };
        let mut out: Vec<String> = self.words.iter().map(|w| quote(w)).collect();
        out.extend(self.tags.iter().map(|t| format!("tag:{}", quote(t))));
        if let Some(a) = &self.author {
            out.push(format!("author:{}", quote(a)));
        }
        for (name, value) in [
            ("signed", self.signed),
            ("forms", self.forms),
            ("scanned", self.scanned),
            ("locked", self.locked),
            ("missing", self.missing),
        ] {
            match value {
                Some(true) => out.push(name.to_owned()),
                Some(false) => out.push(format!("{name}:no")),
                None => {}
            }
        }
        if let Some(r) = self.rating {
            out.push(format!("rating:{r}"));
        }
        if let Some(f) = &self.folder {
            out.push(format!("in:{}", quote(f)));
        }
        if self.sort != Sort::default() {
            out.push(format!("sort:{}", self.sort.name()));
        }
        out.join(" ")
    }

    /// The SQL listing the documents, and its arguments.
    pub(crate) fn sql(&self) -> (String, Vec<Value>) {
        let mut wheres = Vec::new();
        let mut args: Vec<Value> = Vec::new();
        let arg = |v: Value, args: &mut Vec<Value>| {
            args.push(v);
            format!("?{}", args.len())
        };
        if let Some(id) = self.id {
            wheres.push(format!("id = {}", arg(Value::Integer(id), &mut args)));
        }
        for word in &self.words {
            let like = arg(Value::Text(like(word)), &mut args);
            wheres.push(format!(
                "(title LIKE {like} ESCAPE '\\' OR author LIKE {like} ESCAPE '\\' \
                 OR subject LIKE {like} ESCAPE '\\' OR keywords LIKE {like} ESCAPE '\\' \
                 OR path LIKE {like} ESCAPE '\\')"
            ));
        }
        for tag in &self.tags {
            let tag = arg(Value::Text(tag.clone()), &mut args);
            wheres.push(format!(
                "EXISTS (SELECT 1 FROM tags WHERE tags.doc = docs.id \
                 AND tags.tag = {tag} COLLATE NOCASE)"
            ));
        }
        if let Some(author) = &self.author {
            let like = arg(Value::Text(like(author)), &mut args);
            wheres.push(format!("author LIKE {like} ESCAPE '\\'"));
        }
        for (column, value) in [
            ("signed", self.signed),
            ("forms", self.forms),
            ("scanned", self.scanned),
            ("locked", self.locked),
        ] {
            if let Some(v) = value {
                wheres.push(format!("{column} = {}", i32::from(v)));
            }
        }
        if self.id.is_none() {
            wheres.push(format!(
                "missing = {}",
                i32::from(self.missing.unwrap_or(false))
            ));
        }
        if let Some(r) = self.rating {
            wheres.push(format!("rating >= {}", r.min(5)));
        }
        if let Some(folder) = &self.folder {
            // Paths are kept as the system writes them.
            let sep = std::path::MAIN_SEPARATOR;
            let folder = folder.replace('/', &sep.to_string());
            let folder = folder.trim_end_matches(sep);
            let exact = arg(Value::Text(folder.to_owned()), &mut args);
            let below = arg(Value::Text(format!("{}{sep}%", escape(folder))), &mut args);
            wheres.push(format!("(path = {exact} OR path LIKE {below} ESCAPE '\\')"));
        }
        let mut sql = String::from(
            "SELECT id, path, size, modified, hash, title, author, subject, keywords, \
             pages, signed, forms, scanned, locked, added, opened, missing, rating, note \
             FROM docs",
        );
        if !wheres.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&wheres.join(" AND "));
        }
        sql.push_str(" ORDER BY ");
        sql.push_str(self.sort.sql());
        (sql, args)
    }
}

/// `text` for LIKE, its `%`, `_` and `\` taken as themselves.
fn escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn like(text: &str) -> String {
    format!("%{}%", escape(text))
}

/// The words of a query, a quoted value kept whole: `tag:"to read"` is
/// one word, `tag:to read`.
fn split(query: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    for c in query.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !word.is_empty() {
                    out.push(std::mem::take(&mut word));
                }
            }
            c => word.push(c),
        }
    }
    if !word.is_empty() {
        out.push(word);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_are_read_and_written_back() {
        let f = Filter::parse(
            r#"rent tag:"to read" tag:tax signed forms:no author:Rao rating:3 sort:added"#,
        );
        assert_eq!(f.words, ["rent"]);
        assert_eq!(f.tags, ["to read", "tax"]);
        assert_eq!(f.signed, Some(true));
        assert_eq!(f.forms, Some(false));
        assert_eq!(f.author.as_deref(), Some("Rao"));
        assert_eq!(f.rating, Some(3));
        assert_eq!(f.sort, Sort::Added);
        assert_eq!(Filter::parse(&f.query()), f);
        // Unknown keys are words.
        assert_eq!(Filter::parse("http://x").words, ["http://x"]);
    }
}
