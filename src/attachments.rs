//! The attachments a session's result refers to in the Markdown Linewright
//! posts (`docs/specs/runs.md`, "Attachments"): finding the references,
//! reading them as URLs, judging their paths, refusing secrets and
//! rewriting each reference to its URL.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::ops::Range;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use percent_encoding::percent_decode_str;
use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd};
use url::Url;

use crate::secrets::Secrets;

/// A session's attachments directory.
#[derive(Clone, Debug)]
pub struct Dir {
    /// Absolute, with no symlinks.
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

/// Why a result's references can't be posted, each naming the reference as
/// its syntax reads it: a Markdown destination as `CommonMark` reads it, an
/// HTML `src` as written.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// A reference to no file Linewright can attach.
    Reference(BadReference),
    /// A referenced file holding the secret of `variable`.
    Secret { variable: String, reference: String },
}

/// A reference to no file Linewright can attach.
#[derive(Debug, PartialEq, Eq)]
pub enum BadReference {
    /// An HTML `src` that is not a valid URL string, or a URL with a host,
    /// a query or a fragment.
    InvalidUrl { reference: String },
    /// A path in the directory that is not a regular file reached through
    /// real directories: missing, a symlink, or behind a symlinked
    /// directory.
    NotAFile { reference: String },
    /// A path outside the directory.
    Outside { reference: String },
    /// A regular file in the directory of a kind Linewright doesn't attach.
    Kind { reference: String },
}

impl From<BadReference> for Refusal {
    fn from(reference: BadReference) -> Refusal {
        Refusal::Reference(reference)
    }
}

/// A file the Markdown refers to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    path: PathBuf,
    name: String,
    content_type: &'static str,
    media: Media,
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

/// Finds the references in `markdown`, the texts a result posts, reads each
/// that refers to an attachment as a URL, and judges its path: a regular
/// file in `dir` of a kind Linewright attaches ([`attachable`]) holding
/// none of `secrets`. Other references are left as written. It reads the
/// files only to look for secrets.
pub fn resolve(dir: &Dir, markdown: &[&str], secrets: &Secrets) -> Result<Resolved, Refusal> {
    let mut files: Vec<File> = Vec::new();
    let mut splices = Vec::new();
    for text in markdown {
        let mut attached: Vec<(String, Change, File)> = Vec::new();
        for found in find(text) {
            let Read::Attachment(path) = read(&found.reference, found.syntax)? else {
                continue;
            };
            let file = judge(dir.path(), &path, &found.reference, entry)?;
            let change = found.changes.of(&file);
            attached.push((found.reference, change, file));
        }
        let replaced_whole: Vec<Range<usize>> = attached
            .iter()
            .filter_map(|(_, change, _)| match change {
                Change::Whole(range) => Some(range.clone()),
                Change::Parts(_) => None,
            })
            .collect();
        attached
            .retain(|(_, change, _)| !replaced_whole.iter().any(|whole| change.lies_within(whole)));
        let mut text_splices = Vec::new();
        for (reference, change, file) in attached {
            if !files.contains(&file) {
                let bytes = fs::read(&file.path).map_err(|_| BadReference::NotAFile {
                    reference: reference.clone(),
                })?;
                if let Some(variable) = secrets.held_in(&bytes) {
                    return Err(Refusal::Secret {
                        variable: variable.to_string(),
                        reference,
                    });
                }
                files.push(file.clone());
            }
            text_splices.extend(
                change
                    .into_splices()
                    .into_iter()
                    .map(|splice| (splice, file.path.clone())),
            );
        }
        splices.push(text_splices);
    }
    Ok(Resolved { files, splices })
}

/// The syntax a reference is written in, which decides how strictly it is
/// read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Syntax {
    /// A Markdown destination, as `CommonMark` reads it.
    Markdown,
    /// An HTML `src`, which must be a valid URL string.
    Html,
}

/// What a reference refers to.
#[derive(Debug, PartialEq, Eq)]
enum Read {
    /// The attachment at this absolute path, lexically normal.
    Attachment(PathBuf),
    /// Something other than an attachment: the reference is left as
    /// written.
    Other,
}

/// What `reference`, written in `syntax`, refers to (`docs/specs/runs.md`,
/// "References" and "Reading a reference"): an attachment when it is a file
/// URL or an absolute path, read with the URL Standard's parser against
/// `file:///`, its path the URL's path percent-decoded.
fn read(reference: &str, syntax: Syntax) -> Result<Read, BadReference> {
    let is_file_url = reference
        .get(..5)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("file:"));
    let is_absolute_path = reference.starts_with('/') && !reference.starts_with("//");
    if !is_file_url && !is_absolute_path {
        return Ok(Read::Other);
    }
    let invalid = || BadReference::InvalidUrl {
        reference: reference.to_string(),
    };
    let base = Url::parse("file:///").expect("`file:///` is a URL");
    let violated = Cell::new(false);
    let violation = |_| violated.set(true);
    let url = Url::options()
        .base_url(Some(&base))
        .syntax_violation_callback(Some(&violation))
        .parse(reference)
        .map_err(|_| invalid())?;
    let valid_url_string = !violated.get() && !has_dot_segment(reference);
    if syntax == Syntax::Html && !valid_url_string {
        return Err(invalid());
    }
    let has_host = url.host_str().is_some_and(|host| !host.is_empty());
    if has_host || url.query().is_some() || url.fragment().is_some() {
        return Err(invalid());
    }
    let path: Vec<u8> = percent_decode_str(url.path()).collect();
    let names_a_directory = path.ends_with(b"/");
    if names_a_directory {
        return Err(BadReference::NotAFile {
            reference: reference.to_string(),
        });
    }
    Ok(Read::Attachment(lexically_normal(Path::new(
        OsStr::from_bytes(&path),
    ))))
}

/// Whether a segment of `reference`'s path is `.` or `..`, in any
/// percent-encoded spelling such as `%2e`, which no
/// [valid URL string](https://url.spec.whatwg.org/#valid-url-string) has.
/// The parser reports no
/// [violation](https://docs.rs/url/2.5.8/url/enum.SyntaxViolation.html) for
/// them.
fn has_dot_segment(reference: &str) -> bool {
    let path = reference
        .split(['?', '#'])
        .next()
        .expect("a split yields at least one part");
    path.split('/').any(|segment| {
        let decoded: Vec<u8> = percent_decode_str(segment).collect();
        decoded == b"." || decoded == b".."
    })
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

/// The file in `dir` at `path`, an attachment's lexically normal absolute
/// path, `entry` telling what the file system holds at a path. A refusal
/// names `reference`, the reference `path` was read from.
fn judge(
    dir: &Path,
    path: &Path,
    reference: &str,
    entry: impl Fn(&Path) -> Entry,
) -> Result<File, BadReference> {
    let Ok(rest) = path.strip_prefix(dir) else {
        return Err(BadReference::Outside {
            reference: reference.to_string(),
        });
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
            let Some(kind) = kind(path) else {
                return Err(BadReference::Kind {
                    reference: reference.to_string(),
                });
            };
            return Ok(File {
                name: component.as_os_str().to_string_lossy().into_owned(),
                content_type: kind.content_type,
                media: kind.media,
                path: path.to_path_buf(),
            });
        }
    }
    Err(BadReference::NotAFile {
        reference: reference.to_string(),
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

/// A kind of file Linewright attaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Kind {
    /// Lower case, without the dot.
    extension: &'static str,
    content_type: &'static str,
    media: Media,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Media {
    Image,
    /// GitHub plays it
    /// ([Attaching files](https://docs.github.com/en/get-started/writing-on-github/working-with-advanced-formatting/attaching-files)).
    Video,
}

/// The only files Linewright attaches: the images and videos GitHub's
/// attachment upload takes
/// ([Attaching files](https://docs.github.com/en/get-started/writing-on-github/working-with-advanced-formatting/attaching-files)).
const KINDS: [Kind; 9] = [
    Kind {
        extension: "png",
        content_type: "image/png",
        media: Media::Image,
    },
    Kind {
        extension: "jpg",
        content_type: "image/jpeg",
        media: Media::Image,
    },
    Kind {
        extension: "jpeg",
        content_type: "image/jpeg",
        media: Media::Image,
    },
    Kind {
        extension: "gif",
        content_type: "image/gif",
        media: Media::Image,
    },
    Kind {
        extension: "webp",
        content_type: "image/webp",
        media: Media::Image,
    },
    Kind {
        extension: "svg",
        content_type: "image/svg+xml",
        media: Media::Image,
    },
    Kind {
        extension: "mp4",
        content_type: "video/mp4",
        media: Media::Video,
    },
    Kind {
        extension: "mov",
        content_type: "video/quicktime",
        media: Media::Video,
    },
    Kind {
        extension: "webm",
        content_type: "video/webm",
        media: Media::Video,
    },
];

/// The extension of every kind of file Linewright attaches, such as `png`:
/// the first and the rest.
pub fn attachable() -> (&'static str, impl Iterator<Item = &'static str>) {
    let [first, rest @ ..] = KINDS;
    (first.extension, rest.into_iter().map(|kind| kind.extension))
}

/// The extension of every kind of video Linewright attaches, such as `mp4`:
/// the first and the rest.
pub fn videos() -> (&'static str, impl Iterator<Item = &'static str>) {
    let mut videos = KINDS
        .into_iter()
        .filter(|kind| kind.media == Media::Video)
        .map(|kind| kind.extension);
    let first = videos.next().expect("`KINDS` holds videos");
    (first, videos)
}

/// The kind of the file at `path`, told by its extension whatever its
/// letter case, or `None` when Linewright doesn't attach it.
fn kind(path: &Path) -> Option<Kind> {
    let extension = path.extension()?.to_str()?;
    KINDS
        .into_iter()
        .find(|kind| kind.extension.eq_ignore_ascii_case(extension))
}

/// A reference found in one text.
#[derive(Debug)]
struct Found {
    /// A Markdown destination as `CommonMark` reads it, or an HTML `src` as
    /// written.
    reference: String,
    syntax: Syntax,
    changes: Changes,
}

/// What putting a reference's URL in changes, for each kind of file it can
/// turn out to name.
#[derive(Debug)]
struct Changes {
    image: Change,
    video: Change,
}

impl Changes {
    /// The same change whatever the file.
    fn any(change: Change) -> Changes {
        Changes {
            image: change.clone(),
            video: change,
        }
    }

    fn of(self, file: &File) -> Change {
        match file.media {
            Media::Image => self.image,
            Media::Video => self.video,
        }
    }
}

/// What putting a reference's URL in changes.
#[derive(Clone, Debug)]
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

/// The references in `text`: the destinations of its Markdown images,
/// links (autolinks included) and reference definitions, and the `src` of
/// its HTML `img`s.
fn find(text: &str) -> Vec<Found> {
    let parser = Parser::new_ext(text, options()).into_offset_iter();
    let mut found: Vec<Found> = parser
        .reference_definitions()
        .iter()
        .filter_map(|(_, definition)| {
            Some(Found {
                reference: definition.dest.to_string(),
                syntax: Syntax::Markdown,
                changes: Changes::any(Change::Parts(vec![Splice {
                    range: definition_destination(text, definition.span.clone())?,
                    with: With::Url,
                }])),
            })
        })
        .collect();
    let mut open: Vec<Open> = Vec::new();
    let mut html_block = Html::default();
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
                let html = std::mem::take(&mut html_block);
                found.extend(img_tags(text, &html, Some(&closed)));
            }
            Event::End(_) => {
                open.pop();
            }
            Event::Html(_) => html_block.push_line(text, range),
            Event::InlineHtml(html) => {
                found.extend(img_tags(
                    text,
                    &Html::inline(text, &html, range),
                    open.last(),
                ));
            }
            Event::Text(_)
            | Event::Code(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_)
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

/// The changes the embed at `range` makes, `parent` being the tag it is
/// in: `plain` for an image, and for a video the whole range when it is
/// alone in its paragraph, else `as_link`.
fn embed(
    text: &str,
    range: Range<usize>,
    parent: Option<&Open>,
    plain: Vec<Splice>,
    as_link: Vec<Splice>,
) -> Changes {
    let video = if alone(text, &range, parent) {
        Change::Whole(range)
    } else {
        Change::Parts(as_link)
    };
    Changes {
        image: Change::Parts(plain),
        video,
    }
}

/// The reference a closed image or link makes, `parent` being the tag it is
/// in.
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
        // An autolink's destination is all between its `<` and `>`
        // (https://spec.commonmark.org/0.31.2/#autolinks).
        LinkType::Autolink => splices.push(Splice {
            range: range.start + 1..range.end - 1,
            with: With::Url,
        }),
        LinkType::Reference | LinkType::Collapsed | LinkType::Shortcut => {}
        // A reference to no definition is plain text
        // (https://spec.commonmark.org/0.31.2/#reference-link).
        LinkType::ReferenceUnknown
        | LinkType::CollapsedUnknown
        | LinkType::ShortcutUnknown
        | LinkType::Email
        | LinkType::WikiLink { .. } => return None,
    }
    let reference = dest_url.to_string();
    // Only an embedded video plays: https://docs.github.com/en/get-started/writing-on-github/working-with-advanced-formatting/attaching-files
    if !image {
        return Some(Found {
            reference,
            syntax: Syntax::Markdown,
            changes: Changes::any(Change::Parts(splices)),
        });
    }
    let mut as_link = splices.clone();
    as_link.push(Splice {
        range: range.start..range.start + 1,
        with: With::Delete,
    });
    let shows_no_text =
        text_end == text_start && matches!(link_type, LinkType::Inline | LinkType::Reference);
    if shows_no_text {
        as_link.push(Splice {
            range: text_start..text_start,
            with: With::Url,
        });
    }
    Some(Found {
        reference,
        syntax: Syntax::Markdown,
        changes: embed(text, range, parent, splices, as_link),
    })
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

/// HTML as the parser reads it, without the markers of the containers it
/// is in, such as a block quote's `>` at the start of each line, and where
/// each of its lines is in the source.
#[derive(Default)]
struct Html {
    text: String,
    lines: Vec<LineStart>,
}

/// Where a line of [`Html`] starts, in its text and in the source.
#[derive(Clone, Copy)]
struct LineStart {
    in_text: usize,
    in_source: usize,
}

impl Html {
    /// Inline HTML that the parser reads as `read`, at `range` in `source`.
    /// Each line of `read` after the first is the end of its line in the
    /// source, the markers before it taken out. When `read` is not that,
    /// it is the source as written.
    fn inline(source: &str, read: &str, range: Range<usize>) -> Html {
        let written = &source[range.clone()];
        let mut html = Html {
            text: read.to_string(),
            lines: Vec::new(),
        };
        let mut at = LineStart {
            in_text: 0,
            in_source: range.start,
        };
        for (read_line, written_line) in read
            .split_inclusive('\n')
            .zip(written.split_inclusive('\n'))
        {
            if !written_line.ends_with(read_line) {
                return Html {
                    text: written.to_string(),
                    lines: vec![LineStart {
                        in_text: 0,
                        in_source: range.start,
                    }],
                };
            }
            let markers = written_line.len() - read_line.len();
            html.lines.push(LineStart {
                in_text: at.in_text,
                in_source: at.in_source + markers,
            });
            at = LineStart {
                in_text: at.in_text + read_line.len(),
                in_source: at.in_source + written_line.len(),
            };
        }
        html
    }

    /// Adds a line of an HTML block, at `range` in `source`.
    fn push_line(&mut self, source: &str, range: Range<usize>) {
        self.lines.push(LineStart {
            in_text: self.text.len(),
            in_source: range.start,
        });
        self.text.push_str(&source[range]);
    }

    /// Where `range`, in `text`, is in the source.
    fn to_source(&self, range: Range<usize>) -> Range<usize> {
        let at = |offset: usize| {
            let line = self.lines.partition_point(|line| line.in_text <= offset) - 1;
            let start = self.lines[line];
            start.in_source + offset - start.in_text
        };
        let start = at(range.start);
        let end = if range.is_empty() {
            start
        } else {
            at(range.end - 1) + 1
        };
        start..end
    }
}

/// The references the HTML `img` tags in `html` make, in `source`, `parent`
/// being the tag the HTML is in. HTML comments are skipped.
fn img_tags(source: &str, html: &Html, parent: Option<&Open>) -> Vec<Found> {
    let text = html.text.as_str();
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut i = 0;
    while i < text.len() {
        if bytes[i..].starts_with(b"<!--") {
            i = text[i + 4..]
                .find("-->")
                .map_or(text.len(), |at| i + 4 + at + 3);
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
        let Some(tag) = img_tag(text, i, text.len()) else {
            break;
        };
        let tag_range = html.to_source(i..tag.end);
        i = tag.end;
        let Some(src) = tag.src else {
            continue;
        };
        let reference = text[src.clone()].to_string();
        let plain = vec![Splice {
            range: html.to_source(src),
            with: With::Url,
        }];
        let non_empty_alt = tag
            .alt
            .map(|alt| source[html.to_source(alt)].to_string())
            .filter(|alt| !alt.is_empty());
        let as_link = vec![Splice {
            range: tag_range.clone(),
            with: With::Anchor(non_empty_alt),
        }];
        found.push(Found {
            reference,
            syntax: Syntax::Html,
            changes: embed(source, tag_range, parent, plain, as_link),
        });
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
        let at_a_slash_not_closing_the_tag = i == name_start;
        if at_a_slash_not_closing_the_tag {
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
