//! The docs: the repository's docs/ folder, served as it is for anyone to
//! read at <domain>/docs. The markdown is the page; the browser renders it
//! (box/docs/web). What the gate adds is the index: every page's title and
//! headings for the contents and the search, and the nav, which is
//! docs/README.md's own list of links when there is one.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::OnceLock;

/// docs/, read once: the files by their path under it, and the index
struct Docs {
    files: HashMap<String, Vec<u8>>,
    index: serde_json::Value,
    tag: String,
}

static DOCS: OnceLock<Docs> = OnceLock::new();

/// what the search reads of a page; a long page past this is found by its
/// title and headings
const TEXT_CAP: usize = 20_000;

fn docs() -> &'static Docs {
    DOCS.get_or_init(|| read(&super::pages::tree().join("docs")))
}

/// The index as json, and its tag
pub fn index() -> (&'static serde_json::Value, &'static str) {
    let d = docs();
    (&d.index, &d.tag)
}

/// One page's markdown, by its path under docs/; only what the index holds
pub fn raw(path: &str) -> Option<&'static [u8]> {
    docs().files.get(path).map(Vec::as_slice)
}

fn read(dir: &Path) -> Docs {
    let mut files = HashMap::new();
    walk(dir, dir, &mut files);
    let mut index = build(&files);
    // the repository on the fleet's forge, for a member's edit and history
    // links (VERIFY_REPO, from dd.repo); unset leaves them off
    index["repo"] = std::env::var("VERIFY_REPO")
        .ok()
        .filter(|r| !r.is_empty())
        .into();
    let tag = super::pages::tag_of(index.to_string().as_bytes());
    Docs { files, index, tag }
}

/// every .md file under `dir`, by its path from `root`; dot files and links
/// are not docs
fn walk(root: &Path, dir: &Path, out: &mut HashMap<String, Vec<u8>>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let Ok(ty) = e.file_type() else { continue };
        let p = e.path();
        if ty.is_dir() {
            walk(root, &p, out);
        } else if ty.is_file()
            && name.ends_with(".md")
            && let Ok(rel) = p.strip_prefix(root)
            && let Some(rel) = rel.to_str()
            && let Ok(b) = std::fs::read(&p)
        {
            out.insert(rel.replace('\\', "/"), b);
        }
    }
}

fn build(files: &HashMap<String, Vec<u8>>) -> serde_json::Value {
    let mut pages = BTreeMap::new();
    for (path, b) in files {
        let src = String::from_utf8_lossy(b);
        pages.insert(path.clone(), page(path, &src));
    }
    let home = files.contains_key("README.md").then_some("README.md");
    let mut groups = home
        .map(|h| nav(&String::from_utf8_lossy(&files[h])))
        .unwrap_or_default();
    // a page the nav does not list is still a page: it goes last, by title
    let listed: std::collections::HashSet<&str> = groups
        .iter()
        .flat_map(|(_, ps)| ps.iter().map(String::as_str))
        .chain(home)
        .collect();
    let mut rest: Vec<&String> = pages
        .keys()
        .filter(|p| !listed.contains(p.as_str()))
        .collect();
    rest.sort_by_key(|p| {
        pages[*p]["title"]
            .as_str()
            .unwrap_or_default()
            .to_lowercase()
    });
    if !rest.is_empty() {
        let name = if groups.is_empty() { "" } else { "More" };
        groups.push((name.to_string(), rest.into_iter().cloned().collect()));
    }
    // links to a page that is not there are left out of the nav
    for (_, ps) in &mut groups {
        ps.retain(|p| pages.contains_key(p));
    }
    groups.retain(|(_, ps)| !ps.is_empty());
    serde_json::json!({
        "home": home,
        "groups": groups.iter().map(|(name, ps)| serde_json::json!({ "name": name, "pages": ps })).collect::<Vec<_>>(),
        "pages": pages,
    })
}

/// A page's title (its first `# `, else its file name), its headings for the
/// contents, and its words for the search
fn page(path: &str, src: &str) -> serde_json::Value {
    let mut title = None;
    let mut headings = Vec::new();
    let mut text = String::new();
    let mut fenced = false;
    for line in src.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if !fenced && let Some((level, h)) = heading(line) {
            let h = plain(h);
            if level == 1 && title.is_none() {
                title = Some(h);
            } else if level <= 3 {
                headings.push(serde_json::json!([level, h]));
            }
            continue;
        }
        if text.len() < TEXT_CAP {
            let words = plain(line);
            if !words.is_empty() {
                text.push_str(&words);
                text.push(' ');
            }
        }
    }
    let title = title.unwrap_or_else(|| {
        let name = path.rsplit('/').next().unwrap_or(path);
        name.trim_end_matches(".md").replace(['-', '_'], " ")
    });
    // cut on a character, not inside one
    let mut end = text.len().min(TEXT_CAP);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    serde_json::json!({ "title": title, "headings": headings, "text": text.trim_end() })
}

/// `## Words` as (2, "Words"); not a heading inside an indented code block
fn heading(line: &str) -> Option<(usize, &str)> {
    if line.starts_with("    ") || line.starts_with('\t') {
        return None;
    }
    let t = line.trim_start();
    let level = t.bytes().take_while(|b| *b == b'#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &t[level..];
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    Some((level, rest.trim().trim_end_matches('#').trim()))
}

/// Markdown's marks taken off a line: what a reader sees of it, for the
/// titles and the search
fn plain(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.trim().chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // [words](where): the words
            ']' if chars.peek() == Some(&'(') => {
                for c in chars.by_ref() {
                    if c == ')' {
                        break;
                    }
                }
            }
            '*' | '`' | '[' | ']' | '|' | '>' | '#' => out.push(' '),
            '_' if out.ends_with(' ') || out.is_empty() => {}
            c => out.push(c),
        }
    }
    let words: Vec<&str> = out
        .split_whitespace()
        .filter(|w| !w.chars().all(|c| matches!(c, '-' | ':' | '=')))
        .collect();
    words.join(" ")
}

/// docs/README.md as the nav: each `## Name` starts a group, and each
/// `- [Title](path.md)` under it is a page, by its path under docs/
fn nav(readme: &str) -> Vec<(String, Vec<String>)> {
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    let mut fenced = false;
    for line in readme.lines() {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        if let Some((2, h)) = heading(line) {
            groups.push((plain(h), Vec::new()));
            continue;
        }
        let Some(item) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) else {
            continue;
        };
        let Some(group) = groups.last_mut() else {
            continue;
        };
        if let Some(at) = item.find("](")
            && item.starts_with('[')
            && let Some(end) = item[at + 2..].find(')')
        {
            let href = &item[at + 2..at + 2 + end];
            if let Some(p) = resolve(href)
                && !group.1.contains(&p)
            {
                group.1.push(p);
            }
        }
    }
    groups
}

/// a link in README.md as a path under docs/: relative, .md, and never
/// above docs/ itself
fn resolve(href: &str) -> Option<String> {
    let href = href.split('#').next()?;
    if href.contains(':') || href.starts_with('/') || !href.ends_with(".md") {
        return None;
    }
    let mut out: Vec<&str> = Vec::new();
    for seg in href.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            s => out.push(s),
        }
    }
    Some(out.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(list: &[(&str, &str)]) -> HashMap<String, Vec<u8>> {
        list.iter()
            .map(|(p, s)| (p.to_string(), s.as_bytes().to_vec()))
            .collect()
    }

    #[test]
    fn the_readme_is_the_nav_and_every_other_page_still_shows() {
        let f = files(&[
            (
                "README.md",
                "# Docs\n\nIntro.\n\n## Fleet\n\n- [Boxes](fleet/boxes.md)\n- [Gone](fleet/gone.md)\n\n## Design\n\n* [Control](./design/control-plane.md#shape)\n",
            ),
            ("fleet/boxes.md", "# Boxes and roles\n"),
            ("design/control-plane.md", "# The control plane\n## Shape\n"),
            ("loose.md", "# A loose page\n"),
        ]);
        let i = build(&f);
        assert_eq!(i["home"], "README.md");
        assert_eq!(
            i["groups"],
            serde_json::json!([
                { "name": "Fleet", "pages": ["fleet/boxes.md"] },
                { "name": "Design", "pages": ["design/control-plane.md"] },
                { "name": "More", "pages": ["loose.md"] },
            ])
        );
    }

    #[test]
    fn without_a_readme_every_page_is_listed_by_title() {
        let f = files(&[
            ("b.md", "# Zebra\n"),
            ("a/c.md", "# Apple\n"),
            ("d.md", "no title"),
        ]);
        let i = build(&f);
        assert!(i["home"].is_null());
        assert_eq!(
            i["groups"],
            serde_json::json!([{ "name": "", "pages": ["a/c.md", "d.md", "b.md"] }])
        );
        assert_eq!(i["pages"]["d.md"]["title"], "d");
    }

    #[test]
    fn titles_headings_and_words_leave_code_and_marks_out() {
        let p = page(
            "x.md",
            "# The `control` plane\n\nSome **bold** [link](a.md) text.\n\n```\n## not a heading\n```\n\n    # nor this\n\n## What is *voted* on\n### Time\n#### deep\n|---|---|\n",
        );
        assert_eq!(p["title"], "The control plane");
        assert_eq!(
            p["headings"],
            serde_json::json!([[2, "What is voted on"], [3, "Time"]])
        );
        let text = p["text"].as_str().unwrap();
        assert!(text.contains("Some bold link text."), "{text}");
        assert!(text.contains("not a heading"), "code is searchable: {text}");
        assert!(!text.contains("---"), "{text}");
    }

    #[test]
    fn readme_links_stay_inside_docs() {
        assert_eq!(resolve("a/../b.md").as_deref(), Some("b.md"));
        assert_eq!(resolve("../secrets/x.md"), None);
        assert_eq!(resolve("https://x.org/a.md"), None);
        assert_eq!(resolve("/etc/a.md"), None);
        assert_eq!(resolve("a.txt"), None);
    }

    #[test]
    fn only_markdown_under_the_folder_is_read() {
        let dir = std::env::temp_dir().join(format!("dd-docs-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        std::fs::write(dir.join("sub/a.md"), "# A").unwrap();
        std::fs::write(dir.join("b.txt"), "no").unwrap();
        std::fs::write(dir.join(".hidden/c.md"), "# C").unwrap();
        let d = read(&dir);
        let mut keys: Vec<_> = d.files.keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["sub/a.md"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    // CI's test tree has no docs/: the index is still whole, and empty
    #[test]
    fn the_index_answers_however_much_is_there() {
        let (i, tag) = index();
        assert!(!tag.is_empty());
        assert!(i["pages"].is_object() && i["groups"].is_array());
    }
}
