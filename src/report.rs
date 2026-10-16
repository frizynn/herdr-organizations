//! What a thread's report says about itself, for a reader who should not
//! have to open the whole report: a verdict from a fixed set, a short
//! summary, and the captures and deliverables in its library. Only the
//! verdict may reach a prompt; the rest is agent-written data for `--json`
//! and the popup.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const MAX_SUMMARY_CHARS: usize = 600;
pub const MAX_CAPTURES: usize = 24;
/// What a report without a `## Captures` list shows: its newest images.
pub const NEWEST_CAPTURES: usize = 12;
pub const MAX_DELIVERABLES: usize = 10;

/// The `Status:` line of a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    Done,
    NeedsDecision,
    Blocked,
    Failed,
}

impl Verdict {
    /// How a prompt or a screen says it.
    pub const fn words(self) -> &'static str {
        match self {
            Verdict::Done => "done",
            Verdict::NeedsDecision => "needs decision",
            Verdict::Blocked => "blocked",
            Verdict::Failed => "failed",
        }
    }
}

/// An image the report shows, with its caption (empty when none was given).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Capture {
    pub path: String,
    pub caption: String,
}

/// A document at the top of the library: `html`, `pdf` or `md`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Deliverable {
    pub path: String,
    pub kind: &'static str,
}

/// The value of the `Status:` line before the report's first heading, when it
/// is exactly one of the verdicts.
pub fn verdict(_report: &str) -> Option<Verdict> {
    None
}

/// The `## Summary` section, trimmed and cut to `MAX_SUMMARY_CHARS`.
pub fn summary(_report: &str) -> Option<String> {
    None
}

/// The images listed under `## Captures`, or the newest images of the
/// library when the report lists none.
pub fn captures(_report: &str, _library: &Path) -> Vec<Capture> {
    Vec::new()
}

/// Regular `html`, `pdf` and `md` files at the top of the library, by name.
pub fn deliverables(_library: &Path) -> Vec<Deliverable> {
    Vec::new()
}

#[allow(dead_code)]
fn unused(_: PathBuf) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn put(dir: &Path, rel: &str) -> PathBuf {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"x").unwrap();
        path
    }

    fn aged(path: &Path, seconds: u64) {
        let time = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + seconds);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(time)
            .unwrap();
    }

    fn shown(captures: &[Capture], library: &Path) -> Vec<(String, String)> {
        captures
            .iter()
            .map(|c| {
                let rel = Path::new(&c.path).strip_prefix(library).unwrap();
                (rel.display().to_string(), c.caption.clone())
            })
            .collect()
    }

    #[test]
    fn the_verdict_is_one_fixed_value_on_the_status_line() {
        for (value, expected) in [
            ("done", Verdict::Done),
            ("needs-decision", Verdict::NeedsDecision),
            ("blocked", Verdict::Blocked),
            ("failed", Verdict::Failed),
        ] {
            let report = format!("PR: https://github.com/o/r/pull/1\nStatus: {value}\n## Report\n");
            assert_eq!(verdict(&report), Some(expected), "{value}");
            assert_eq!(verdict(&format!("Status:  {value} \n")), Some(expected));
        }
        for report in [
            "Status: Done\n",
            "Status: done, mostly\n",
            "Status: needs decision\n",
            "Status: done | blocked\n",
            "Status:\n",
            "status: done\n",
            "**Status:** done\n",
            "## Report\nStatus: done\n",
            "Nothing here\n",
            "",
        ] {
            assert_eq!(verdict(report), None, "{report:?}");
        }
        // The first Status line decides.
        assert_eq!(verdict("Status: maybe\nStatus: done\n"), None);
        assert_eq!(serde_json::to_value(Verdict::NeedsDecision).unwrap(), "needs-decision");
        assert_eq!(Verdict::NeedsDecision.words(), "needs decision");
    }

    #[test]
    fn the_summary_is_its_section_trimmed_and_cut() {
        let report = "Status: done\n## Summary\n\n  Checkout ships the size order.\nThe 390 px layout is fixed.  \n\n## Report\nnot this\n";
        assert_eq!(
            summary(report).as_deref(),
            Some("Checkout ships the size order.\nThe 390 px layout is fixed.")
        );
        // A line that starts with an issue number is text, not a heading.
        assert_eq!(
            summary("## Summary\n#146 merged.\n### Detail\nnot this\n").as_deref(),
            Some("#146 merged.")
        );
        let long = format!("## Summary\n{}\n## Next\n", "é".repeat(700));
        let cut = summary(&long).unwrap();
        assert_eq!(cut.chars().count(), MAX_SUMMARY_CHARS);
        assert_eq!(
            summary(&format!("## Summary\n{} tail\n", "a".repeat(599))).as_deref(),
            Some("a".repeat(599).as_str()),
            "no trailing space after the cut"
        );
        assert_eq!(summary("## Summary\n\n## Report\nx\n"), None);
        assert_eq!(summary("## Report\nx\n"), None);
    }

    #[test]
    fn listed_captures_keep_their_captions_and_only_safe_images() {
        let home = tempfile::tempdir().unwrap();
        let library = home.path().join("library/t-0001");
        put(&library, "capturas/talles-390.png");
        put(&library, "capturas/carrito.JPG");
        put(&library, "b.webp");
        put(&library, "notes.txt");
        put(&library, "page.html");
        put(home.path(), "library/secret.png");
        put(home.path(), "outside.png");
        std::fs::create_dir_all(library.join("dir.png")).unwrap();
        let outside = home.path().join("outside.png").display().to_string();
        let report = format!(
            "## Captures\n\
             - capturas/talles-390.png \u{2014} Orden de talles a 390 px\n\
             * capturas/carrito.JPG - Carrito vacío - sin cupón\n\
             - `./b.webp`\n\
             - missing.png - Not there\n\
             - notes.txt - Wrong kind\n\
             - page.html - Wrong kind\n\
             - dir.png - A folder\n\
             - ../secret.png - Escapes\n\
             - capturas/../../secret.png - Escapes\n\
             - {outside} - Absolute\n\
             - capturas/talles-390.png - Twice\n\
             loose text\n\
             ## Next\n- b.webp - not a capture\n"
        );
        assert_eq!(
            shown(&captures(&report, &library), &library),
            [
                ("capturas/talles-390.png".into(), "Orden de talles a 390 px".into()),
                ("capturas/carrito.JPG".into(), "Carrito vacío - sin cupón".into()),
                ("b.webp".into(), String::new()),
            ]
        );
        assert!(captures(&report, &library).iter().all(|c| Path::new(&c.path).is_absolute()));
        // A list whose files are all gone shows nothing, not the newest images.
        assert!(captures("## Captures\n- gone.png - x\n", &library).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn captures_never_go_through_a_symbolic_link() {
        use std::os::unix::fs::symlink;
        let home = tempfile::tempdir().unwrap();
        let library = home.path().join("library/t-0001");
        let elsewhere = home.path().join("elsewhere");
        put(&elsewhere, "shot.png");
        put(&library, "real.png");
        symlink(elsewhere.join("shot.png"), library.join("link.png")).unwrap();
        symlink(&elsewhere, library.join("linked")).unwrap();
        let report = "## Captures\n- link.png - a\n- linked/shot.png - b\n- real.png - c\n";
        assert_eq!(
            shown(&captures(report, &library), &library),
            [("real.png".to_string(), "c".to_string())]
        );
        // Nor does the fallback, and a linked library shows nothing at all.
        assert_eq!(
            shown(&captures("", &library), &library),
            [("real.png".to_string(), String::new())]
        );
        let linked_library = home.path().join("library/t-0002");
        symlink(&library, &linked_library).unwrap();
        assert!(captures(report, &linked_library).is_empty());
        assert!(captures("", &linked_library).is_empty());
        assert!(deliverables(&linked_library).is_empty());
    }

    #[test]
    fn at_most_24_listed_captures() {
        let home = tempfile::tempdir().unwrap();
        let library = home.path().join("library/t-0001");
        let mut report = String::from("## Captures\n");
        for n in 0..30 {
            put(&library, &format!("s{n:02}.png"));
            report.push_str(&format!("- s{n:02}.png - shot {n}\n"));
        }
        let listed = captures(&report, &library);
        assert_eq!(listed.len(), MAX_CAPTURES);
        assert_eq!(listed[0].caption, "shot 0");
        assert_eq!(listed[23].caption, "shot 23");
    }

    #[test]
    fn without_a_list_the_twelve_newest_images_show_from_any_subfolder() {
        let home = tempfile::tempdir().unwrap();
        let library = home.path().join("library/t-0001");
        assert!(captures("## Report\n", &library).is_empty(), "no library yet");
        for n in 0..14u64 {
            let rel = if n % 2 == 0 {
                format!("i{n:02}.png")
            } else {
                format!("deep/er/i{n:02}.gif")
            };
            aged(&put(&library, &rel), n);
        }
        aged(&put(&library, "newest.pdf"), 100);
        aged(&put(&library, ".hidden.png"), 100);
        let newest = captures("## Report\nno list\n## Captures\nnone this time\n", &library);
        let names: Vec<String> = shown(&newest, &library)
            .into_iter()
            .map(|(path, caption)| {
                assert!(caption.is_empty());
                path
            })
            .collect();
        assert_eq!(names.len(), NEWEST_CAPTURES);
        assert_eq!(names[0], "deep/er/i13.gif");
        assert_eq!(names[1], "i12.png");
        assert_eq!(names[11], "i02.png");
    }

    #[test]
    fn deliverables_are_top_level_documents_by_name() {
        let home = tempfile::tempdir().unwrap();
        let library = home.path().join("library/t-0001");
        assert!(deliverables(&library).is_empty(), "no library yet");
        put(&library, "b-report.PDF");
        put(&library, "a-page.html");
        put(&library, "c-notes.md");
        put(&library, "d.png");
        put(&library, "e.htm");
        put(&library, ".hidden.md");
        put(&library, "sub/deep.md");
        std::fs::create_dir_all(library.join("folder.md")).unwrap();
        let found: Vec<(String, &str)> = deliverables(&library)
            .into_iter()
            .map(|d| {
                let rel = Path::new(&d.path).strip_prefix(&library).unwrap();
                (rel.display().to_string(), d.kind)
            })
            .collect();
        assert_eq!(
            found,
            [
                ("a-page.html".to_string(), "html"),
                ("b-report.PDF".to_string(), "pdf"),
                ("c-notes.md".to_string(), "md"),
            ]
        );
        for n in 0..12 {
            put(&library, &format!("z{n:02}.md"));
        }
        assert_eq!(deliverables(&library).len(), MAX_DELIVERABLES);
    }
}
