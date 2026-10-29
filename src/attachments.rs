//! The attachments a session's result refers to in the Markdown Linewright
//! posts (`docs/specs/runs.md`, "Attachments"): finding the references,
//! judging their paths, refusing secrets and rewriting each path to its
//! URL.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};

use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd};

use crate::secrets::Secrets;

/// A session's attachments directory.
#[derive(Clone, Debug)]
pub struct Dir {
    /// Absolute, with no symlinks, so that a reference is judged by its
    /// path alone.
    path: PathBuf,
}

impl Dir {
    /// Makes `record/attachments`, `record` being the session's record
    /// directory.
    pub fn create(record: &Path) -> io::Result<Dir> {
        let path = record.join("attachments");
        fs::create_dir_all(&path)?;
        Ok(Dir {
            path: fs::canonicalize(path)?,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Why a result's references can't be posted, each naming the path as the
/// Markdown writes it.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// A path in the directory that is not a regular file reached through
    /// real directories: missing, a symlink, or behind a symlinked
    /// directory.
    #[error(
        "`{}` is in the attachments directory but is not a regular file there: \
         put the file itself at that path, not a symlink to it, or take the reference out",
        path.display()
    )]
    NotAFile { path: PathBuf },
    /// An absolute path outside the directory where something other than a
    /// directory exists.
    #[error(
        "`{}` is outside the attachments directory: copy the file into the directory \
         and refer to it there",
        path.display()
    )]
    Outside { path: PathBuf },
    /// A referenced file holding one of Linewright's secrets.
    #[error(
        "`{}` holds the secret of `{variable}`: take the secret out of the file, \
         or take the reference out",
        path.display()
    )]
    Secret { variable: String, path: PathBuf },
}

/// A file the Markdown refers to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    path: PathBuf,
    name: String,
    content_type: &'static str,
}

impl File {
    /// Absolute, in the attachments directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The name to upload it under: its file name.
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn content_type(&self) -> &'static str {
        self.content_type
    }
}

/// A result's Markdown whose references were all judged sound.
#[derive(Debug)]
pub struct Resolved {
    files: Vec<File>,
    /// For each text, the splices that put the URLs in, with the path of
    /// the file each puts in.
    splices: Vec<Vec<(Splice, PathBuf)>>,
}

impl Resolved {
    /// Each file the references name, once however many references it has,
    /// in the order the Markdown first refers to them.
    pub fn files(&self) -> &[File] {
        &self.files
    }

    /// Puts each file's URL in `urls` in place of its references in
    /// `markdown`, the texts [`resolve`] was given, changing no other byte.
    /// A reference to a file `urls` lacks is left as written.
    pub fn rewrite(&self, markdown: &mut [&mut String], urls: &BTreeMap<PathBuf, String>) {
        for (text, splices) in markdown.iter_mut().zip(&self.splices) {
            let mut splices: Vec<&(Splice, PathBuf)> = splices.iter().collect();
            splices.sort_by_key(|(splice, _)| std::cmp::Reverse(splice.range.start));
            for (splice, path) in splices {
                let Some(url) = urls.get(path) else {
                    continue;
                };
                let with = match &splice.with {
                    With::Url => url.clone(),
                    With::Delete => String::new(),
                    With::Anchor(label) => {
                        format!("<a href=\"{url}\">{}</a>", label.as_deref().unwrap_or(url))
                    }
                };
                text.replace_range(splice.range.clone(), &with);
            }
        }
    }
}

/// Finds the references in `markdown`, the texts a result posts, and judges
/// each: a path in `dir` must be a regular file holding none of `secrets`,
/// and an absolute path outside it must not name a file. Other references
/// are left as written. It reads the files only to look for secrets.
pub fn resolve(dir: &Dir, markdown: &[&str], secrets: &Secrets) -> Result<Resolved, Refusal> {
    let mut files: Vec<File> = Vec::new();
    let mut splices = Vec::new();
    for text in markdown {
        let mut attached: Vec<(Found, File)> = Vec::new();
        for found in find(text) {
            if let Some(file) = judge(dir.path(), &found.written, entry)? {
                attached.push((found, file));
            }
        }
        // A video replaced whole takes what it holds with it.
        let wholes: Vec<Range<usize>> = attached
            .iter()
            .filter_map(|(found, _)| match &found.change {
                Change::Whole(range) => Some(range.clone()),
                Change::Parts(_) => None,
            })
            .collect();
        attached.retain(|(found, _)| !wholes.iter().any(|whole| found.change.lies_within(whole)));
        let mut text_splices = Vec::new();
        for (found, file) in attached {
            if !files.contains(&file) {
                let bytes = fs::read(&file.path).map_err(|_| Refusal::NotAFile {
                    path: PathBuf::from(&found.written),
                })?;
                if let Some(variable) = secrets.held_in(&bytes) {
                    return Err(Refusal::Secret {
                        variable: variable.to_string(),
                        path: PathBuf::from(&found.written),
                    });
                }
                files.push(file.clone());
            }
            text_splices.extend(
                found
                    .change
                    .into_splices()
                    .into_iter()
                    .map(|splice| (splice, file.path.clone())),
            );
        }
        splices.push(text_splices);
    }
    Ok(Resolved { files, splices })
}

/// What the file system holds at a path, not following a symlink there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Entry {
    Nothing,
    Directory,
    File,
    /// A symlink, or anything else that is neither a directory nor a
    /// regular file.
    Other,
}

fn entry(path: &Path) -> Entry {
    match fs::symlink_metadata(path) {
        Err(_) => Entry::Nothing,
        Ok(metadata) if metadata.is_dir() => Entry::Directory,
        Ok(metadata) if metadata.is_file() => Entry::File,
        Ok(_) => Entry::Other,
    }
}

/// The file in `dir` that `written`, a reference's destination, names, or
/// `None` when it names no attachment and is left as written, `entry`
/// telling what the file system holds at a path.
fn judge(
    dir: &Path,
    written: &str,
    entry: impl Fn(&Path) -> Entry,
) -> Result<Option<File>, Refusal> {
    let path = lexically_normal(Path::new(written));
    let Ok(rest) = path.strip_prefix(dir) else {
        return match entry(&path) {
            Entry::Nothing | Entry::Directory => Ok(None),
            Entry::File | Entry::Other => Err(Refusal::Outside {
                path: PathBuf::from(written),
            }),
        };
    };
    let components: Vec<Component> = rest.components().collect();
    let mut at = dir.to_path_buf();
    for (i, component) in components.iter().enumerate() {
        at.push(component);
        let wanted = if i + 1 == components.len() {
            Entry::File
        } else {
            Entry::Directory
        };
        if entry(&at) != wanted {
            break;
        }
        if wanted == Entry::File {
            return Ok(Some(File {
                name: component.as_os_str().to_string_lossy().into_owned(),
                content_type: content_type(&path),
                path,
            }));
        }
    }
    Err(Refusal::NotAFile {
        path: PathBuf::from(written),
    })
}

/// `path` with `.` and `..` resolved without reading the file system.
fn lexically_normal(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normal.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normal.push(component);
            }
        }
    }
    normal
}

/// Whether `written` names a video, by the extensions GitHub plays
/// ([Attaching files](https://docs.github.com/en/get-started/writing-on-github/working-with-advanced-formatting/attaching-files)).
fn is_video(written: &str) -> bool {
    content_type(Path::new(written)).starts_with("video/")
}

fn content_type(path: &Path) -> &'static str {
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "json" => "application/json",
        "csv" => "text/csv",
        "md" => "text/markdown",
        "txt" | "log" => "text/plain",
        _ => "application/octet-stream",
    }
}

/// A reference to an absolute path, found in one text.
#[derive(Debug)]
struct Found {
    /// Its destination, as the parser reads it.
    written: String,
    change: Change,
}

/// What putting a reference's URL in changes.
#[derive(Debug)]
enum Change {
    /// The range a video alone in its paragraph is, replaced whole by the
    /// URL.
    Whole(Range<usize>),
    /// Splices inside the reference, none for a reference-style link, whose
    /// definition holds its destination.
    Parts(Vec<Splice>),
}

impl Change {
    /// Whether the change is all inside `whole`, another reference's
    /// whole replacement.
    fn lies_within(&self, whole: &Range<usize>) -> bool {
        let inside = |range: &Range<usize>| whole.start <= range.start && range.end <= whole.end;
        match self {
            Change::Whole(range) => range != whole && inside(range),
            Change::Parts(splices) => {
                !splices.is_empty() && splices.iter().all(|splice| inside(&splice.range))
            }
        }
    }

    fn into_splices(self) -> Vec<Splice> {
        match self {
            Change::Whole(range) => vec![Splice {
                range,
                with: With::Url,
            }],
            Change::Parts(splices) => splices,
        }
    }
}

#[derive(Clone, Debug)]
struct Splice {
    range: Range<usize>,
    with: With,
}

#[derive(Clone, Debug)]
enum With {
    Url,
    Delete,
    /// An HTML link to the URL, with this text or else the URL.
    Anchor(Option<String>),
}

/// GitHub's extensions of `CommonMark`
/// ([GitHub Flavored Markdown](https://github.github.com/gfm/)).
fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM
}

/// A tag the parser has opened and not yet closed.
struct Open<'a> {
    tag: Tag<'a>,
    range: Range<usize>,
    /// Where the last of its children so far ends.
    children_end: usize,
}

/// The references to absolute paths in `text`: the destinations of its
/// Markdown images, links and reference definitions, and the `src` of its
/// HTML `img`s.
fn find(text: &str) -> Vec<Found> {
    let parser = Parser::new_ext(text, options()).into_offset_iter();
    let mut found: Vec<Found> = parser
        .reference_definitions()
        .iter()
        .filter(|(_, definition)| definition.dest.starts_with('/'))
        .filter_map(|(_, definition)| {
            Some(Found {
                written: definition.dest.to_string(),
                change: Change::Parts(vec![Splice {
                    range: definition_destination(text, definition.span.clone())?,
                    with: With::Url,
                }]),
            })
        })
        .collect();
    let mut open: Vec<Open> = Vec::new();
    for (event, range) in parser {
        let end = range.end;
        match event {
            Event::Start(tag) => {
                open.push(Open {
                    children_end: range.start,
                    tag,
                    range,
                });
                continue;
            }
            Event::End(TagEnd::Image | TagEnd::Link) => {
                let closed = open.pop().expect("an end follows its start");
                found.extend(link_reference(text, &closed, open.last()));
            }
            Event::End(TagEnd::HtmlBlock) => {
                let closed = open.pop().expect("an end follows its start");
                found.extend(img_tags(text, closed.range.clone(), Some(&closed)));
            }
            Event::End(_) => {
                open.pop();
            }
            Event::InlineHtml(_) => found.extend(img_tags(text, range, open.last())),
            Event::Text(_)
            | Event::Code(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_)
            | Event::Html(_)
            | Event::FootnoteReference(_)
            | Event::SoftBreak
            | Event::HardBreak
            | Event::Rule
            | Event::TaskListMarker(_) => {}
        }
        if let Some(parent) = open.last_mut() {
            parent.children_end = parent.children_end.max(end);
        }
    }
    found
}

/// The reference to `written` that the embed at `range` makes, `parent`
/// being the tag it is in: `plain` when it is not a video, the whole range
/// when it is a video alone in its paragraph, else `as_link`.
fn embed(
    text: &str,
    written: String,
    range: Range<usize>,
    parent: Option<&Open>,
    plain: Vec<Splice>,
    as_link: impl FnOnce() -> Vec<Splice>,
) -> Found {
    let change = if !is_video(&written) {
        Change::Parts(plain)
    } else if alone(text, &range, parent) {
        Change::Whole(range)
    } else {
        Change::Parts(as_link())
    };
    Found { written, change }
}

/// The reference a closed image or link makes, if it is to an absolute
/// path, `parent` being the tag it is in.
fn link_reference(text: &str, closed: &Open, parent: Option<&Open>) -> Option<Found> {
    let (Tag::Image {
        link_type,
        dest_url,
        ..
    }
    | Tag::Link {
        link_type,
        dest_url,
        ..
    }) = &closed.tag
    else {
        return None;
    };
    if !dest_url.starts_with('/') {
        return None;
    }
    let range = closed.range.clone();
    let image = matches!(closed.tag, Tag::Image { .. });
    let text_start = range.start + if image { 2 } else { 1 };
    let text_end = closed.children_end.max(text_start);
    let mut splices = Vec::new();
    match link_type {
        LinkType::Inline => {
            let bracket = text_end + text[text_end..range.end].find("](")?;
            splices.push(Splice {
                range: destination(text, bracket + 2)?,
                with: With::Url,
            });
        }
        // The definition holds the destination.
        LinkType::Reference | LinkType::Collapsed | LinkType::Shortcut => {}
        // A reference to no definition is plain text, and an autolink is
        // never a path.
        LinkType::ReferenceUnknown
        | LinkType::CollapsedUnknown
        | LinkType::ShortcutUnknown
        | LinkType::Autolink
        | LinkType::Email
        | LinkType::WikiLink { .. } => return None,
    }
    let written = dest_url.to_string();
    // A link to a video stays a link: only an embedded one plays.
    if !image {
        return Some(Found {
            written,
            change: Change::Parts(splices),
        });
    }
    let link_start = range.start;
    let as_link = {
        let splices = splices.clone();
        move || {
            let mut splices = splices;
            splices.push(Splice {
                range: link_start..link_start + 1,
                with: With::Delete,
            });
            // A link with no text would not show; a shortcut's or collapsed
            // reference's text is its label, which must stay.
            if text_end == text_start && matches!(link_type, LinkType::Inline | LinkType::Reference)
            {
                splices.push(Splice {
                    range: text_start..text_start,
                    with: With::Url,
                });
            }
            splices
        }
    };
    Some(embed(text, written, range, parent, splices, as_link))
}

/// Whether `range` is all its paragraph or HTML block holds, `parent`
/// being the tag it is in. A tight list item's paragraph is the item, less
/// its marker.
fn alone(text: &str, range: &Range<usize>, parent: Option<&Open>) -> bool {
    let Some(parent) = parent else {
        return false;
    };
    let before = text[parent.range.start..range.start].trim();
    let after = text[range.end..parent.range.end].trim();
    let only_marker_before = if matches!(parent.tag, Tag::Paragraph | Tag::HtmlBlock) {
        before.is_empty()
    } else if matches!(parent.tag, Tag::Item) {
        is_list_marker(before)
    } else {
        false
    };
    only_marker_before && after.is_empty()
}

/// Whether `text` is a `CommonMark`
/// [list marker](https://spec.commonmark.org/0.31.2/#list-marker).
fn is_list_marker(text: &str) -> bool {
    match text {
        "-" | "+" | "*" => true,
        _ => text
            .strip_suffix(['.', ')'])
            .is_some_and(|number| !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())),
    }
}

/// Where the ASCII whitespace at `from` in `bytes` ends.
fn past_whitespace(bytes: &[u8], from: usize) -> usize {
    let mut i = from;
    while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    i
}

/// The range of the destination of the definition at `span`.
fn definition_destination(text: &str, span: Range<usize>) -> Option<Range<usize>> {
    let bytes = text.as_bytes();
    let mut i = span.start + text[span.clone()].find('[')? + 1;
    while i < span.end {
        match bytes[i] {
            b'\\' => i += 2,
            b']' => break,
            _ => i += 1,
        }
    }
    if bytes.get(i + 1) != Some(&b':') {
        return None;
    }
    destination(text, i + 2)
}

/// The range of the link destination at or after `from`, past any
/// whitespace, inside its `<` and `>` if it has them, as `CommonMark` reads a
/// [link destination](https://spec.commonmark.org/0.31.2/#link-destination).
fn destination(text: &str, from: usize) -> Option<Range<usize>> {
    let bytes = text.as_bytes();
    let start = past_whitespace(bytes, from);
    if bytes.get(start) == Some(&b'<') {
        let mut i = start + 1;
        while i < bytes.len() {
            match bytes[i] {
                b'\\' => i += 2,
                b'>' => return Some(start + 1..i),
                b'\n' | b'<' => return None,
                _ => i += 1,
            }
        }
        return None;
    }
    let mut depth = 0usize;
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' if depth == 0 => break,
            b')' => {
                depth -= 1;
                i += 1;
            }
            byte if byte.is_ascii_whitespace() || byte.is_ascii_control() => break,
            _ => i += 1,
        }
    }
    let i = i.min(bytes.len());
    (i > start).then_some(start..i)
}

/// The references the HTML `img` tags at `range` make to absolute paths,
/// `parent` being the tag the HTML is in. HTML comments are skipped.
fn img_tags(text: &str, range: Range<usize>, parent: Option<&Open>) -> Vec<Found> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut i = range.start;
    while i < range.end {
        if text[i..range.end].starts_with("<!--") {
            i = text[i + 4..range.end]
                .find("-->")
                .map_or(range.end, |at| i + 4 + at + 3);
            continue;
        }
        let is_img = bytes[i] == b'<'
            && text
                .get(i + 1..i + 4)
                .is_some_and(|name| name.eq_ignore_ascii_case("img"))
            && bytes
                .get(i + 4)
                .is_some_and(|byte| byte.is_ascii_whitespace() || matches!(byte, b'/' | b'>'));
        if !is_img {
            i += 1;
            continue;
        }
        let Some(tag) = img_tag(text, i, range.end) else {
            break;
        };
        let tag_range = i..tag.end;
        i = tag.end;
        let Some(src) = tag.src else {
            continue;
        };
        let written = text[src.clone()].to_string();
        if !written.starts_with('/') {
            continue;
        }
        let plain = vec![Splice {
            range: src,
            with: With::Url,
        }];
        // An `alt=""` would make a link that does not show.
        let label = tag
            .alt
            .map(|alt| text[alt].to_string())
            .filter(|alt| !alt.is_empty());
        let as_link = {
            let tag_range = tag_range.clone();
            || {
                vec![Splice {
                    range: tag_range,
                    with: With::Anchor(label),
                }]
            }
        };
        found.push(embed(text, written, tag_range, parent, plain, as_link));
    }
    found
}

/// An HTML `img` tag's attributes that matter here, and where it ends.
struct ImgTag {
    end: usize,
    src: Option<Range<usize>>,
    alt: Option<Range<usize>>,
}

/// The `img` tag at `start`, which must end before `limit`.
fn img_tag(text: &str, start: usize, limit: usize) -> Option<ImgTag> {
    let bytes = &text.as_bytes()[..limit];
    let mut src = None;
    let mut alt = None;
    let mut i = start + 4;
    loop {
        i = past_whitespace(bytes, i);
        let end = match bytes.get(i)? {
            b'>' => Some(i + 1),
            b'/' if bytes.get(i + 1) == Some(&b'>') => Some(i + 2),
            _ => None,
        };
        if let Some(end) = end {
            return Some(ImgTag { end, src, alt });
        }
        let name_start = i;
        while bytes
            .get(i)
            .is_some_and(|byte| !byte.is_ascii_whitespace() && !matches!(byte, b'/' | b'>' | b'='))
        {
            i += 1;
        }
        if i == name_start {
            // A `/` not closing the tag.
            i += 1;
            continue;
        }
        let name = &text[name_start..i];
        i = past_whitespace(bytes, i);
        if bytes.get(i) != Some(&b'=') {
            continue;
        }
        i = past_whitespace(bytes, i + 1);
        let value = match bytes.get(i)? {
            quote @ (b'"' | b'\'') => {
                let value_start = i + 1;
                let value_end = value_start + text[value_start..limit].find(*quote as char)?;
                i = value_end + 1;
                value_start..value_end
            }
            _ => {
                let value_start = i;
                while bytes
                    .get(i)
                    .is_some_and(|byte| !byte.is_ascii_whitespace() && *byte != b'>')
                {
                    i += 1;
                }
                value_start..i
            }
        };
        if name.eq_ignore_ascii_case("src") {
            src = Some(value);
        } else if name.eq_ignore_ascii_case("alt") {
            alt = Some(value);
        }
    }
}
