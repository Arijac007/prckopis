//! Preflight: run a profile over a document, group what it finds, fix what can be fixed, and write
//! a report.
//!
//! A profile is a standard's checks: PDF/A-2b and PDF/A-3b ([`crate::verify`], ISO 19005-2/-3) and
//! PDF/X-4 ([`crate::pdfx`], ISO 15930-7). Its results are the [`Issue`]s, each citing its clause;
//! [`Results::groups`] folds the same problem on several pages into one row with the pages listed,
//! as a results tree does. [`fix`] applies the profile's fixups (none of which changes what a page
//! looks like) and runs the profile again.

use prckopis_cos::{CosError, Document};

use crate::{Issue, Level, Words, pdfx};

/// A Preflight profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Profile {
    PdfA2b,
    PdfA3b,
    PdfX4,
}

/// How a profile is described: "Verify compliance with PDF/X-4", the standard's name in `{}`.
pub const PROFILE_WORDING: &str = "Verify compliance with {}";

impl Profile {
    pub const ALL: [Profile; 3] = [Profile::PdfA2b, Profile::PdfA3b, Profile::PdfX4];

    /// The stable id automation uses: `pdfa-2b`, `pdfa-3b`, `pdfx-4`.
    pub fn id(self) -> &'static str {
        match self {
            Profile::PdfA2b => "pdfa-2b",
            Profile::PdfA3b => "pdfa-3b",
            Profile::PdfX4 => "pdfx-4",
        }
    }

    /// The standard's own name, the same in every language.
    pub fn label(self) -> &'static str {
        match self {
            Profile::PdfA2b => "PDF/A-2b",
            Profile::PdfA3b => "PDF/A-3b",
            Profile::PdfX4 => pdfx::VERSION,
        }
    }

    /// The standard its checks come from.
    pub fn standard(self) -> &'static str {
        match self {
            Profile::PdfA2b => Level::A2b.standard(),
            Profile::PdfA3b => Level::A3b.standard(),
            Profile::PdfX4 => pdfx::STANDARD,
        }
    }

    /// "Verify compliance with PDF/X-4", in the language `words` translates into.
    pub fn described(self, words: Words) -> String {
        words(PROFILE_WORDING).replacen("{}", self.label(), 1)
    }

    /// A profile from its id, or from the standard's name written loosely (`PDF/X-4`, `x4`, `2b`).
    pub fn from_id(id: &str) -> Option<Profile> {
        match id.to_ascii_lowercase().replace(['/', '-', ' ', '_'], "").as_str() {
            "pdfa2b" | "a2b" | "2b" => Some(Profile::PdfA2b),
            "pdfa3b" | "a3b" | "3b" => Some(Profile::PdfA3b),
            "pdfx4" | "x4" => Some(Profile::PdfX4),
            _ => None,
        }
    }
}

/// What a profile found.
#[derive(Clone, Debug, PartialEq)]
pub struct Results {
    pub profile: Profile,
    pub issues: Vec<Issue>,
}

/// One problem, with every page it is on.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    /// The first issue of the kind: its wording, clause and whether it can be fixed.
    pub issue: Issue,
    /// How many times it was found (once per page, or once for the document).
    pub count: usize,
    /// 0-based pages, in order, without repeats.
    pub pages: Vec<usize>,
}

impl Results {
    /// Whether the document passed: nothing found.
    pub fn passed(&self) -> bool {
        self.issues.is_empty()
    }

    /// How many problems [`fix`] can fix.
    pub fn fixable(&self) -> usize {
        self.issues.iter().filter(|i| i.fixable).count()
    }

    /// The problems, the same problem on several pages folded into one, in the order found.
    pub fn groups(&self) -> Vec<Group> {
        let mut out: Vec<Group> = Vec::new();
        for i in &self.issues {
            let same = |g: &&mut Group| {
                g.issue.standard == i.standard && g.issue.clause == i.clause && g.issue.pattern == i.pattern && g.issue.value == i.value
            };
            match out.iter_mut().find(|g| same(g)) {
                Some(g) => {
                    g.count += 1;
                    if let Some(p) = i.page
                        && !g.pages.contains(&p)
                    {
                        g.pages.push(p);
                    }
                }
                None => out.push(Group { issue: i.clone(), count: 1, pages: i.page.into_iter().collect() }),
            }
        }
        for g in &mut out {
            g.pages.sort_unstable();
        }
        out
    }
}

/// Run `profile` over `doc`.
pub fn run(doc: &Document, profile: Profile) -> Results {
    let issues = match profile {
        Profile::PdfA2b => crate::verify(doc, Level::A2b),
        Profile::PdfA3b => crate::verify(doc, Level::A3b),
        Profile::PdfX4 => pdfx::verify(doc),
    };
    Results { profile, issues }
}

/// What [`fix`] did, and the results of running the profile again.
#[derive(Clone, Debug, PartialEq)]
pub struct Fixed {
    /// What was done, in English (for scripts).
    pub fixed: Vec<String>,
    pub results: Results,
}

/// Apply `profile`'s fixups to `doc`, then run it again. `now` is a PDF date for the dates a
/// PDF/X file must have and lacks.
pub fn fix(doc: &mut Document, profile: Profile, now: Option<&str>) -> Result<Fixed, CosError> {
    let fixed = match profile {
        Profile::PdfA2b => crate::convert(doc, Level::A2b)?.fixed,
        Profile::PdfA3b => crate::convert(doc, Level::A3b)?.fixed,
        Profile::PdfX4 => pdfx::fix(doc, now)?,
    };
    Ok(Fixed { fixed, results: run(doc, profile) })
}

/// Every wording of a [`report_text`], in English, with `{}` where the report's own data goes.
pub const REPORT_WORDS: &[&str] = &[
    "Preflight report",
    "Profile: {}",
    "File: {}",
    "Date: {}",
    "No problems found.",
    "Problems found: {}",
    "Pages: {}",
    "Can be fixed",
    PROFILE_WORDING,
];

/// The results as a plain-text report: the profile, the file and the date, then each problem with
/// its pages, the clause it breaks and whether it can be fixed, worded in the language `words`
/// translates into (`crate::in_english` keeps it in English).
pub fn report_text(results: &Results, file_name: &str, date: &str, words: Words) -> String {
    let said = |pattern: &str, value: &str| words(pattern).replacen("{}", value, 1);
    let mut out = String::new();
    out.push_str(&words("Preflight report"));
    out.push('\n');
    out.push_str(&said("Profile: {}", &results.profile.described(words)));
    out.push('\n');
    out.push_str(&said("File: {}", file_name));
    out.push('\n');
    out.push_str(&said("Date: {}", date));
    out.push_str("\n\n");
    if results.passed() {
        out.push_str(&words("No problems found."));
        out.push('\n');
        return out;
    }
    out.push_str(&said("Problems found: {}", &results.issues.len().to_string()));
    out.push('\n');
    for g in results.groups() {
        out.push_str("\n- ");
        out.push_str(&g.issue.said(words));
        if g.count > 1 {
            out.push_str(&format!(" (×{})", g.count));
        }
        out.push('\n');
        if !g.pages.is_empty() {
            let pages: Vec<String> = g.pages.iter().map(|p| (p + 1).to_string()).collect();
            out.push_str("  ");
            out.push_str(&said("Pages: {}", &pages.join(", ")));
            out.push('\n');
        }
        out.push_str("  ");
        out.push_str(&g.issue.citation());
        if g.issue.fixable {
            out.push_str(" · ");
            out.push_str(&words("Can be fixed"));
        }
        out.push('\n');
    }
    out
}
