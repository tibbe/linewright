//! Finding, judging and rewriting the attachments a result's Markdown
//! refers to, "Attachments" in docs/specs/runs.md, against a real
//! attachments directory.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::PathBuf;

use crate::common;

use common::secrets_unnamed;
use linewright::attachments::{self, BadReference, Dir, Refusal, Resolved};
use linewright::secrets::Secrets;

/// A session's record directory and its attachments directory in it.
struct Record {
    _record: tempfile::TempDir,
    dir: Dir,
}

impl Record {
    fn new() -> Record {
        Record::under("")
    }

    /// A record under a parent named `Application Support`, as in macOS's
    /// default data directory, so the attachments directory's path has a
    /// space in it.
    fn spaced() -> Record {
        Record::under("Application Support")
    }

    /// A record at `parent` in a new temporary directory.
    fn under(parent: &str) -> Record {
        let record = tempfile::tempdir().unwrap();
        let dir = Dir::create(&record.path().join(parent)).unwrap();
        Record {
            _record: record,
            dir,
        }
    }

    /// The attachments directory, as the prompt names it.
    fn path(&self) -> String {
        self.dir.path().display().to_string()
    }

    /// The attachments directory as a file URL, such as
    /// `file:///tmp/x/Application%20Support/attachments`.
    #[track_caller]
    fn file_url(&self) -> String {
        let path = self.path();
        assert!(
            path.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"/._- ".contains(&byte)),
            "{path} needs more than its spaces percent-encoded"
        );
        format!("file://{}", path.replace(' ', "%20"))
    }

    /// Writes `bytes` to `name` in the attachments directory.
    fn put(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.dir.path().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }

    fn resolve(&self, markdown: &str) -> Result<Resolved, Refusal> {
        attachments::resolve(&self.dir, &[markdown], &Secrets::default())
    }

    /// The path of each file `markdown` refers to.
    #[track_caller]
    fn attached(&self, markdown: &str) -> Vec<PathBuf> {
        self.resolve(markdown)
            .unwrap()
            .files()
            .iter()
            .map(|file| file.path().to_path_buf())
            .collect()
    }

    /// `markdown` with each attachment's path replaced by the URL
    /// `https://github.com/user-attachments/assets/<its upload name>`.
    fn rewritten(&self, markdown: &str) -> String {
        let resolved = self.resolve(markdown).unwrap();
        let urls: BTreeMap<PathBuf, String> = resolved
            .files()
            .iter()
            .map(|file| (file.path().to_path_buf(), url(file.name())))
            .collect();
        let mut text = markdown.to_string();
        resolved.rewrite(&mut [&mut text], &urls);
        text
    }
}

fn url(name: &str) -> String {
    format!("https://github.com/user-attachments/assets/{name}")
}

#[test_log::test]
fn an_image_s_path_becomes_its_url() {
    let record = Record::new();
    record.put("before.png", b"png");
    let dir = record.path();
    assert_eq!(
        record.rewritten(&format!(
            "The page before:\n\n![The old page]({dir}/before.png)\n"
        )),
        "The page before:\n\n![The old page](https://github.com/user-attachments/assets/before.png)\n"
    );
}

#[test_log::test]
fn a_file_url_becomes_its_url() {
    let record = Record::spaced();
    record.put("before.png", b"png");
    let dir = record.file_url();
    assert!(dir.contains("%20"));
    assert_eq!(
        record.rewritten(&format!(
            "![Before]({dir}/before.png) and <img src=\"{dir}/before.png\">\n"
        )),
        "![Before](https://github.com/user-attachments/assets/before.png) and \
         <img src=\"https://github.com/user-attachments/assets/before.png\">\n"
    );
}

#[test_log::test]
fn a_file_url_s_scheme_is_read_in_any_letter_case_and_localhost_is_no_host() {
    let record = Record::new();
    let shot = record.put("shot.png", b"png");
    let dir = record.path();
    let files = record.attached(&format!(
        "![a](FILE://{dir}/shot.png) ![b](File://localhost{dir}/shot.png) \
         <img src=\"FILE://{dir}/shot.png\"> <img src=\"file://LOCALHOST{dir}/shot.png\">"
    ));
    assert_eq!(files, [shot]);
}

#[test_log::test]
fn a_reference_s_path_is_percent_decoded() {
    let record = Record::new();
    let spaced = record.put("a b.png", b"png");
    let nested = record.put("sub/c.png", b"png");
    let dir = record.path();
    let files = record.attached(&format!(
        "![a](file://{dir}/a%20b.png) <img src=\"{dir}/a%20b.png\"> ![c]({dir}/sub%2Fc.png)"
    ));
    assert_eq!(files, [spaced, nested]);
}

#[test_log::test]
fn a_file_url_with_a_host_a_query_or_a_fragment_is_refused() {
    let record = Record::new();
    record.put("shot.png", b"png");
    let dir = record.path();
    for reference in [
        format!("file://example.com{dir}/shot.png"),
        format!("file://{dir}/shot.png?"),
        format!("{dir}/shot.png?size=2"),
        format!("file://{dir}/shot.png#"),
        format!("{dir}/shot.png#top"),
    ] {
        for markdown in [
            format!("![x](<{reference}>)"),
            format!("<img src=\"{reference}\">"),
        ] {
            assert_eq!(
                record.resolve(&markdown).unwrap_err(),
                Refusal::Reference(BadReference::InvalidUrl {
                    reference: reference.clone()
                })
            );
        }
    }
}

#[test_log::test]
fn an_html_src_that_is_not_a_valid_url_string_is_refused() {
    let record = Record::spaced();
    record.put("shot.png", b"png");
    record.put("sub/shot.png", b"png");
    let dir = record.path();
    for reference in [
        format!("{dir}/shot.png"),
        format!("file:{dir}/shot.png"),
        format!("{dir}/sub/./shot.png"),
        format!("{dir}/sub/%2e%2E/shot.png"),
        format!("{dir}/sub/.%2e/shot.png"),
        format!("{dir}\\shot.png"),
        format!("{dir}/%zz.png"),
    ] {
        assert_eq!(
            record
                .resolve(&format!("<img src=\"{reference}\">"))
                .unwrap_err(),
            Refusal::Reference(BadReference::InvalidUrl { reference })
        );
    }
}

#[test_log::test]
fn a_video_html_img_spanning_blockquote_lines_becomes_a_link_keeping_the_quote() {
    let record = Record::new();
    record.put("demo.mp4", b"mp4");
    let dir = record.file_url();
    assert_eq!(
        record.rewritten(&format!(
            "> <div>\n> Watch <img alt=\"the\n> demo\" src=\"{dir}/demo.mp4\">\n> </div>\n"
        )),
        "> <div>\n> Watch <a href=\"https://github.com/user-attachments/assets/demo.mp4\">the\n\
         > demo</a>\n> </div>\n"
    );
}

#[test_log::test]
fn html_with_text_beyond_ascii_is_read() {
    let record = Record::new();
    record.put("café.png", b"png");
    let dir = record.path();
    assert_eq!(
        record.rewritten(&format!(
            "<div>Café</div>\n\n<span title=\"é\">a</span> <img alt=\"é\" src=\"{dir}/caf%C3%A9.png\">\n"
        )),
        "<div>Café</div>\n\n<span title=\"é\">a</span> \
         <img alt=\"é\" src=\"https://github.com/user-attachments/assets/café.png\">\n"
    );
}

#[test_log::test]
fn a_markdown_destination_need_not_be_a_valid_url_string() {
    let record = Record::spaced();
    let shot = record.put("shot.png", b"png");
    let dir = record.path();
    let files = record.attached(&format!(
        "![a](<{dir}/shot.png>) ![b](<file:{dir}/shot.png>) ![c](<{dir}/sub/%2e%2e/shot.png>)"
    ));
    assert_eq!(files, [shot]);
}

#[test_log::test]
fn an_autolink_s_file_url_becomes_its_url() {
    let record = Record::new();
    record.put("demo.mp4", b"mp4");
    let dir = record.path();
    assert_eq!(
        record.rewritten(&format!("The demo: <file://{dir}/demo.mp4>\n")),
        "The demo: <https://github.com/user-attachments/assets/demo.mp4>\n"
    );
}

#[test_log::test]
fn a_reference_that_is_neither_a_file_url_nor_an_absolute_path_is_left_as_written() {
    let record = Record::new();
    record.put("shot.png", b"png");
    let dir = record.path();
    let markdown = format!(
        "![a](shot.png) [b](//example.com{dir}/shot.png) <https://example.com{dir}/shot.png> \
         <img src=\"<{dir}/shot.png>\"> <img src=\"//example.com{dir}/shot.png\"> \
         [c](../issues/3)\n"
    );
    assert_eq!(record.rewritten(&markdown), markdown);
}

#[test_log::test]
fn a_path_ending_in_a_slash_is_not_a_file() {
    let record = Record::new();
    record.put("shot.png", b"png");
    let dir = record.file_url();
    let reference = format!("{dir}/shot.png/");
    assert_eq!(
        record.resolve(&format!("![x]({reference})")).unwrap_err(),
        Refusal::Reference(BadReference::NotAFile { reference })
    );
}

#[test_log::test]
fn a_video_is_told_by_its_decoded_path() {
    let record = Record::new();
    record.put("demo.mp4", b"mp4");
    let dir = record.file_url();
    assert_eq!(
        record.rewritten(&format!("The demo:\n\n![The demo]({dir}/demo.%6dp4)\n")),
        "The demo:\n\nhttps://github.com/user-attachments/assets/demo.mp4\n"
    );
}

#[test_log::test]
fn an_html_img_spanning_blockquote_lines_has_its_src_rewritten() {
    let record = Record::new();
    record.put("shot.png", b"png");
    let dir = record.file_url();
    assert_eq!(
        record.rewritten(&format!(
            "> See <img alt=\"The shot\"\n> src=\"{dir}/shot.png\"> here.\n\n\
             > <p>\n> <img alt=\"Again\"\n> src=\"{dir}/shot.png\">\n> </p>\n"
        )),
        "> See <img alt=\"The shot\"\n\
         > src=\"https://github.com/user-attachments/assets/shot.png\"> here.\n\n\
         > <p>\n> <img alt=\"Again\"\n\
         > src=\"https://github.com/user-attachments/assets/shot.png\">\n> </p>\n"
    );
}

#[test_log::test]
fn a_link_s_path_becomes_its_url() {
    let record = Record::new();
    record.put("logs/run.png", b"png");
    let dir = record.path();
    assert_eq!(
        record.rewritten(&format!("See [the run](<{dir}/logs/run.png> \"Run\").")),
        "See [the run](<https://github.com/user-attachments/assets/run.png> \"Run\")."
    );
}

#[test_log::test]
fn a_reference_definition_s_path_becomes_its_url() {
    let record = Record::new();
    record.put("after.png", b"png");
    let dir = record.path();
    assert_eq!(
        record.rewritten(&format!(
            "![After][after]\n\n[after]: {dir}/after.png 'After'\n"
        )),
        "![After][after]\n\n[after]: https://github.com/user-attachments/assets/after.png 'After'\n"
    );
}

#[test_log::test]
fn an_html_img_s_src_becomes_its_url() {
    let record = Record::new();
    record.put("wide.png", b"png");
    let dir = record.path();
    assert_eq!(
        record.rewritten(&format!(
            "<p align=\"center\">\n<img alt=\"Wide\" src=\"{dir}/wide.png\" width=\"600\">\n</p>\n\n\
             Inline <img src='{dir}/wide.png'/> too.\n"
        )),
        "<p align=\"center\">\n<img alt=\"Wide\" \
         src=\"https://github.com/user-attachments/assets/wide.png\" width=\"600\">\n</p>\n\n\
         Inline <img src='https://github.com/user-attachments/assets/wide.png'/> too.\n"
    );
}

#[test_log::test]
fn a_bracketed_path_with_a_space_becomes_its_url() {
    let record = Record::spaced();
    let shot = record.put("home_before.png", b"png");
    let dir = record.path();
    assert!(dir.contains(' '));
    let markdown = format!(
        "![Home before](<{dir}/home_before.png>)\n\n\
         See [the shot](<{dir}/home_before.png> \"Shot\") and [again][shot].\n\n\
         [shot]: <{dir}/home_before.png>\n"
    );
    let files = record.attached(&markdown);
    assert_eq!(files, [shot]);
    assert_eq!(
        record.rewritten(&markdown),
        "![Home before](<https://github.com/user-attachments/assets/home_before.png>)\n\n\
         See [the shot](<https://github.com/user-attachments/assets/home_before.png> \"Shot\") \
         and [again][shot].\n\n\
         [shot]: <https://github.com/user-attachments/assets/home_before.png>\n"
    );
}

#[test_log::test]
fn a_bracketed_video_with_a_space_in_its_path_alone_in_its_paragraph_becomes_its_bare_url() {
    let record = Record::spaced();
    record.put("demo.mp4", b"mp4");
    let dir = record.path();
    assert_eq!(
        record.rewritten(&format!("The demo:\n\n![The demo](<{dir}/demo.mp4>)\n")),
        "The demo:\n\nhttps://github.com/user-attachments/assets/demo.mp4\n"
    );
}

#[test_log::test]
fn a_path_in_code_or_plain_text_is_left_as_written() {
    let record = Record::new();
    record.put("shot.png", b"png");
    let dir = record.path();
    let markdown = format!(
        "The file is {dir}/shot.png, made with `![shot]({dir}/shot.png)`.\n\n\
         ```markdown\n![shot]({dir}/shot.png)\n<img src=\"{dir}/shot.png\">\n```\n\n\
         <!-- <img src=\"{dir}/shot.png\"> -->\n\n\
         \\![shot]\\({dir}/shot.png\\)\n"
    );
    assert_eq!(record.rewritten(&markdown), markdown);
    assert_eq!(record.resolve(&markdown).unwrap().files(), []);
}

#[test_log::test]
fn markdown_with_no_references_comes_back_byte_identical() {
    let record = Record::new();
    let markdown = "# Done\r\n\r\n* one  \n* [GitHub](https://github.com) and [#3](../issues/3)\n\
                    | a | b |\n|---|---|\n| ![x](x.png) | <img src=\"rel.png\"> |\n\n\
                    [^1]: A footnote &amp; more\t\n";
    assert_eq!(record.rewritten(markdown), markdown);
}

#[test_log::test]
fn two_references_to_one_file_list_it_once() {
    let record = Record::new();
    let shot = record.put("shot.png", b"png");
    record.put("other.gif", b"gif");
    let dir = record.path();
    let resolved = attachments::resolve(
        &record.dir,
        &[
            &format!("![a]({dir}/shot.png) [b]({dir}/./sub/../shot.png)"),
            &format!("<img src=\"{dir}/other.gif\"> ![c]({dir}/shot.png)"),
        ],
        &Secrets::default(),
    )
    .unwrap();
    let files: Vec<(PathBuf, &str, &str)> = resolved
        .files()
        .iter()
        .map(|file| (file.path().to_path_buf(), file.name(), file.content_type()))
        .collect();
    assert_eq!(
        files,
        [
            (shot, "shot.png", "image/png"),
            (
                record.dir.path().join("other.gif"),
                "other.gif",
                "image/gif"
            ),
        ]
    );
}

#[test_log::test]
fn each_text_is_rewritten_with_its_own_references() {
    let record = Record::new();
    record.put("shot.png", b"png");
    let dir = record.path();
    let first = format!("![a]({dir}/shot.png)");
    let second = format!("Then [b]({dir}/shot.png).");
    let resolved =
        attachments::resolve(&record.dir, &[&first, &second], &Secrets::default()).unwrap();
    let urls = BTreeMap::from([(record.dir.path().join("shot.png"), url("shot.png"))]);
    let (mut first, mut second) = (first, second);
    resolved.rewrite(&mut [&mut first, &mut second], &urls);
    assert_eq!(
        first,
        "![a](https://github.com/user-attachments/assets/shot.png)"
    );
    assert_eq!(
        second,
        "Then [b](https://github.com/user-attachments/assets/shot.png)."
    );
}

#[test_log::test]
fn a_video_alone_in_its_paragraph_becomes_its_bare_url() {
    let record = Record::new();
    record.put("demo.mp4", b"mp4");
    record.put("demo.webm", b"webm");
    let dir = record.path();
    assert_eq!(
        record.rewritten(&format!(
            "The demo:\n\n![The demo]({dir}/demo.mp4)\n\n<img src=\"{dir}/demo.webm\">\n\n\
             > ![Again]({dir}/demo.mp4)\n\n- ![In a list]({dir}/demo.mp4)\n- 2. ![Nested]({dir}/demo.webm)\n"
        )),
        "The demo:\n\nhttps://github.com/user-attachments/assets/demo.mp4\n\n\
         https://github.com/user-attachments/assets/demo.webm\n\n\
         > https://github.com/user-attachments/assets/demo.mp4\n\n\
         - https://github.com/user-attachments/assets/demo.mp4\n\
         - 2. https://github.com/user-attachments/assets/demo.webm\n"
    );
}

#[test_log::test]
fn a_link_to_a_video_alone_in_its_paragraph_stays_a_link() {
    let record = Record::new();
    record.put("demo.mp4", b"mp4");
    let dir = record.path();
    assert_eq!(
        record.rewritten(&format!("[The recording]({dir}/demo.mp4)\n")),
        "[The recording](https://github.com/user-attachments/assets/demo.mp4)\n"
    );
}

#[test_log::test]
fn a_video_beside_other_text_in_a_list_item_becomes_a_link() {
    let record = Record::new();
    record.put("demo.mp4", b"mp4");
    let dir = record.path();
    assert_eq!(
        record.rewritten(&format!(
            "- [ ] ![Check]({dir}/demo.mp4)\n- ![Then]({dir}/demo.mp4) this\n"
        )),
        "- [ ] [Check](https://github.com/user-attachments/assets/demo.mp4)\n\
         - [Then](https://github.com/user-attachments/assets/demo.mp4) this\n"
    );
}

#[test_log::test]
fn a_video_inside_a_sentence_becomes_a_link() {
    let record = Record::new();
    record.put("demo.mov", b"mov");
    let dir = record.path();
    assert_eq!(
        record.rewritten(&format!(
            "Watch ![the demo]({dir}/demo.mov) or ![]({dir}/demo.mov), \
             or <img alt=\"this\" src=\"{dir}/demo.mov\"> or <img alt=\"\" src=\"{dir}/demo.mov\">.\n"
        )),
        "Watch [the demo](https://github.com/user-attachments/assets/demo.mov) or \
         [https://github.com/user-attachments/assets/demo.mov](https://github.com/user-attachments/assets/demo.mov), \
         or <a href=\"https://github.com/user-attachments/assets/demo.mov\">this</a> or \
         <a href=\"https://github.com/user-attachments/assets/demo.mov\">https://github.com/user-attachments/assets/demo.mov</a>.\n"
    );
}

#[test_log::test]
fn a_reference_style_video_inside_a_sentence_becomes_a_link() {
    let record = Record::new();
    record.put("demo.mp4", b"mp4");
    let dir = record.path();
    assert_eq!(
        record.rewritten(&format!(
            "Watch ![the demo][demo].\n\n[demo]: {dir}/demo.mp4\n"
        )),
        "Watch [the demo][demo].\n\n[demo]: https://github.com/user-attachments/assets/demo.mp4\n"
    );
}

#[test_log::test]
fn a_missing_file_is_refused() {
    let record = Record::new();
    let dir = record.path();
    let refusal = record
        .resolve(&format!("![x]({dir}/missing.png)"))
        .unwrap_err();
    assert_eq!(
        refusal,
        Refusal::Reference(BadReference::NotAFile {
            reference: format!("{dir}/missing.png")
        })
    );
}

#[test_log::test]
fn a_file_of_a_kind_linewright_does_not_attach_is_refused() {
    let record = Record::new();
    record.put("tree.txt", b"tree");
    let dir = record.path();
    let refusal = record
        .resolve(&format!("[The tree]({dir}/tree.txt)"))
        .unwrap_err();
    assert_eq!(
        refusal,
        Refusal::Reference(BadReference::Kind {
            reference: format!("{dir}/tree.txt")
        })
    );
}

#[test_log::test]
fn a_kind_is_told_by_its_extension_whatever_its_letter_case() {
    let record = Record::new();
    record.put("SHOT.PNG", b"png");
    let dir = record.path();
    let resolved = record.resolve(&format!("![x]({dir}/SHOT.PNG)")).unwrap();
    assert_eq!(resolved.files()[0].content_type(), "image/png");
}

#[test_log::test]
fn a_missing_file_of_a_kind_linewright_does_not_attach_is_not_a_file() {
    let record = Record::new();
    let dir = record.path();
    assert_eq!(
        record
            .resolve(&format!("[notes]({dir}/notes.txt)"))
            .unwrap_err(),
        Refusal::Reference(BadReference::NotAFile {
            reference: format!("{dir}/notes.txt")
        })
    );
}

#[test_log::test]
fn a_file_of_a_kind_linewright_does_not_attach_is_refused_for_its_kind_before_its_secrets() {
    let record = Record::new();
    record.put("env.txt", b"GITHUB_TOKEN=ghp_s3cr3tt0k3n");
    let dir = record.path();
    let secrets = secrets_unnamed(&[("GITHUB_TOKEN", "ghp_s3cr3tt0k3n")]);
    assert_eq!(
        attachments::resolve(&record.dir, &[&format!("[env]({dir}/env.txt)")], &secrets)
            .unwrap_err(),
        Refusal::Reference(BadReference::Kind {
            reference: format!("{dir}/env.txt")
        })
    );
}

#[test_log::test]
fn a_symlink_is_refused() {
    let record = Record::new();
    let target = record.put("real.png", b"png");
    symlink(&target, record.dir.path().join("link.png")).unwrap();
    let dir = record.path();
    assert_eq!(
        record
            .resolve(&format!("![x]({dir}/link.png)"))
            .unwrap_err(),
        Refusal::Reference(BadReference::NotAFile {
            reference: format!("{dir}/link.png")
        })
    );
}

#[test_log::test]
fn a_file_behind_a_symlinked_directory_is_refused() {
    let record = Record::new();
    record.put("real/shot.png", b"png");
    symlink(
        record.dir.path().join("real"),
        record.dir.path().join("linked"),
    )
    .unwrap();
    let dir = record.path();
    assert_eq!(
        record
            .resolve(&format!("<img src=\"{dir}/linked/shot.png\">"))
            .unwrap_err(),
        Refusal::Reference(BadReference::NotAFile {
            reference: format!("{dir}/linked/shot.png")
        })
    );
}

#[test_log::test]
fn a_dot_dot_escape_from_the_directory_is_refused() {
    let record = Record::new();
    let dir = record.path();
    fs::write(record.dir.path().join("../notes.txt"), "notes").unwrap();
    let refusal = record
        .resolve(&format!("[notes]({dir}/../notes.txt)"))
        .unwrap_err();
    assert_eq!(
        refusal,
        Refusal::Reference(BadReference::Outside {
            reference: format!("{dir}/../notes.txt")
        })
    );
}

#[test_log::test]
fn a_file_outside_the_directory_is_refused() {
    let record = Record::new();
    let elsewhere = tempfile::tempdir().unwrap();
    let file = elsewhere.path().join("shot.png");
    fs::write(&file, "png").unwrap();
    assert_eq!(
        record
            .resolve(&format!("![x]({})", file.display()))
            .unwrap_err(),
        Refusal::Reference(BadReference::Outside {
            reference: file.display().to_string()
        })
    );
}

#[test_log::test]
fn an_absolute_path_outside_the_directory_naming_no_file_is_refused() {
    let record = Record::new();
    let elsewhere = tempfile::tempdir().unwrap();
    let gone = format!("{}/gone.png", elsewhere.path().display());
    assert_eq!(
        record.resolve(&format!("[gone]({gone})")).unwrap_err(),
        Refusal::Reference(BadReference::Outside { reference: gone })
    );
}

#[test_log::test]
fn a_secret_in_a_binary_file_is_refused() {
    let record = Record::new();
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x00, 0xff];
    bytes.extend_from_slice(b"ghp_s3cr3tt0k3n");
    record.put("shot.png", &bytes);
    let dir = record.path();
    let secrets = secrets_unnamed(&[("GITHUB_TOKEN", "ghp_s3cr3tt0k3n")]);
    let refusal = attachments::resolve(&record.dir, &[&format!("![x]({dir}/shot.png)")], &secrets)
        .unwrap_err();
    assert_eq!(
        refusal,
        Refusal::Secret {
            variable: "GITHUB_TOKEN".to_string(),
            reference: format!("{dir}/shot.png")
        }
    );
}

#[test_log::test]
fn dir_create_makes_the_attachments_directory_in_the_record() {
    let record = tempfile::tempdir().unwrap();
    let dir = Dir::create(record.path()).unwrap();
    fs::write(dir.path().join("shot.png"), "png").unwrap();
    assert_eq!(
        fs::read_to_string(record.path().join("attachments/shot.png")).unwrap(),
        "png"
    );
}
