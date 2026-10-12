//! data/the-joke, a section at a time (scripts/the-joke wrote it; its
//! README says where it comes from and its licence): the few sections that
//! best match some words, for Monty in the corner of each page to answer from.

use std::sync::OnceLock;

#[derive(serde::Deserialize, serde::Serialize, Clone)]
pub struct Part {
    /// the article
    t: String,
    /// the section's heading, "" before the first
    s: String,
    /// its text
    x: String,
}

static PARTS: OnceLock<Vec<Part>> = OnceLock::new();

const WIKI: &str = "https://montypython.fandom.com/wiki/";
/// what one answer is given
const FEW: usize = 3;

fn parts() -> &'static [Part] {
    PARTS.get_or_init(|| read(&super::pages::tree().join("data/the-joke")))
}

fn read(dir: &std::path::Path) -> Vec<Part> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    files.retain(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("part-") && n.ends_with(".json"))
    });
    files.sort();
    files
        .iter()
        .filter_map(|p| std::fs::read(p).ok())
        .filter_map(|b| serde_json::from_slice::<Vec<Part>>(&b).ok())
        .flatten()
        .collect()
}

const STOP: &[&str] = &[
    "the", "a", "an", "and", "or", "of", "in", "on", "to", "is", "was", "who", "what", "which",
    "where", "when", "why", "how", "did", "does", "do", "it", "that", "this", "for", "with",
    "monty", "python", "pythons", "me", "tell", "about", "are", "were", "by", "from", "his", "her",
    "their", "be", "as", "at",
];

fn words(q: &str) -> Vec<String> {
    q.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .map(|w| w.trim_matches('\''))
        .filter(|w| w.len() > 1 && !STOP.contains(w))
        .map(str::to_string)
        .collect()
}

/// The sections whose words fit best: an article's title counts most, then
/// the heading, then the text
pub fn find(q: &str) -> Vec<&'static Part> {
    let ws = words(q);
    if ws.is_empty() {
        return vec![];
    }
    let mut scored: Vec<(f32, &Part)> = parts()
        .iter()
        .filter_map(|p| {
            let (t, s, x) = (p.t.to_lowercase(), p.s.to_lowercase(), p.x.to_lowercase());
            let mut n = 0.0;
            for w in &ws {
                if t.contains(w.as_str()) {
                    n += 3.0;
                }
                if s.contains(w.as_str()) {
                    n += 1.5;
                }
                if x.contains(w.as_str()) {
                    n += 1.0;
                }
            }
            // the article itself before its parts, when they fit alike
            if p.s.is_empty() {
                n += 0.25;
            }
            (n >= 2.0).then_some((n, p))
        })
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored.into_iter().take(FEW).map(|(_, p)| p).collect()
}

/// What Monty is given: the sections, each with its article's address on
/// the wiki, and where they come from
pub fn answer(q: &str) -> serde_json::Value {
    let found: Vec<_> = find(q)
        .into_iter()
        .map(|p| {
            serde_json::json!({
                "title": p.t,
                "section": p.s,
                "text": p.x,
                "url": format!(
                    "{WIKI}{}",
                    url::form_urlencoded::byte_serialize(p.t.replace(' ', "_").as_bytes())
                        .collect::<String>()
                ),
            })
        })
        .collect();
    serde_json::json!({
        "found": found,
        "from": {
            "name": "the Monty Python Wiki at Fandom",
            "license": "CC BY-SA 3.0",
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_parts_are_there_and_found_by_their_words() {
        assert!(parts().len() > 1000, "{}", parts().len());
        let f = find("who played the black knight?");
        assert!(
            f.iter().any(|p| p.t.contains("Black Knight")),
            "{:?}",
            f.iter().map(|p| &p.t).collect::<Vec<_>>()
        );
        assert!(
            find("dead parrot sketch")
                .iter()
                .any(|p| p.t.to_lowercase().contains("parrot"))
        );
        assert!(find("the of a").is_empty());
        let a = answer("spanish inquisition");
        assert_eq!(a["from"]["license"], "CC BY-SA 3.0");
        let url = a["found"][0]["url"].as_str().unwrap();
        assert!(url.starts_with(WIKI) && !url.contains(' '), "{url}");
    }

    #[test]
    fn the_folder_says_where_it_comes_from() {
        let readme =
            std::fs::read_to_string(super::super::pages::tree().join("data/the-joke/README.md"))
                .unwrap();
        assert!(readme.contains("Do not read this in full."));
        assert!(readme.contains("creativecommons.org/licenses/by-sa/3.0"));
        assert!(readme.contains("montypython.fandom.com"));
    }
}
