//! Redaction code sets (Acrobat's Redaction Properties ▸ Redaction Code): the exemption codes
//! that justify a redaction, used as its overlay text. The built-in sets are the exemptions of
//! the U.S. Freedom of Information Act (5 U.S.C. 552(b)) and the U.S. Privacy Act
//! (5 U.S.C. 552a(d), (j) and (k)).
//!
//! Users define their own sets too ([`CodeSets`], the Redaction Code Editor): each has a name
//! and a list of codes, each code with a description. Sets are exchanged as a small JSON file of
//! our own design ([`export`] / [`import`]):
//!
//! ```json
//! {
//!   "format": "prckopis-redaction-codes",
//!   "version": 1,
//!   "sets": [
//!     { "name": "GDPR", "codes": [ { "code": "Art. 9", "description": "Special categories" } ] }
//!   ]
//! }
//! ```
//!
//! `format` must be present and say `prckopis-redaction-codes`; `version` is 1 (missing means 1);
//! a code's `description` is optional; unknown keys are ignored. Imported files are untrusted:
//! they are at most [`MAX_FILE_BYTES`] long, and sets and codes that are empty, repeated or too
//! long are skipped and counted ([`Imported::skipped`]) rather than trusted.

use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CodeSet {
    pub id: &'static str,
    pub name: &'static str,
    pub codes: &'static [&'static str],
}

pub const CODE_SETS: [CodeSet; 2] = [
    CodeSet {
        id: "foia",
        name: "U.S. FOIA",
        codes: &[
            "(b)(1)(A)",
            "(b)(1)(B)",
            "(b)(2)",
            "(b)(3)(A)",
            "(b)(3)(B)",
            "(b)(4)",
            "(b)(5)",
            "(b)(6)",
            "(b)(7)(A)",
            "(b)(7)(B)",
            "(b)(7)(C)",
            "(b)(7)(D)",
            "(b)(7)(E)",
            "(b)(7)(F)",
            "(b)(8)",
            "(b)(9)",
        ],
    },
    CodeSet {
        id: "privacy-act",
        name: "U.S. Privacy Act",
        codes: &["(d)(5)", "(j)(1)", "(j)(2)", "(k)(1)", "(k)(2)", "(k)(3)", "(k)(4)", "(k)(5)", "(k)(6)", "(k)(7)"],
    },
];

/// The overlay text for `picked` codes of a set whose codes are `set`: in the set's order, once
/// each, joined by ", ". Picked codes the set doesn't have are left out.
pub fn overlay_in<S: AsRef<str>, P: AsRef<str>>(set: &[S], picked: &[P]) -> String {
    set.iter().map(AsRef::as_ref).filter(|c| picked.iter().any(|p| p.as_ref() == *c)).collect::<Vec<_>>().join(", ")
}

impl CodeSet {
    pub fn from_id(id: &str) -> Option<Self> {
        CODE_SETS.into_iter().find(|s| s.id == id)
    }

    /// The overlay text for the picked codes: in the set's order, once each, joined by ", ".
    /// Fails with the first picked code that isn't in the set.
    pub fn overlay<'a>(&self, picked: &[&'a str]) -> Result<String, &'a str> {
        if let Some(bad) = picked.iter().find(|p| !self.codes.contains(p)) {
            return Err(bad);
        }
        Ok(overlay_in(self.codes, picked))
    }
}

/// The identifier a code-set file starts with.
pub const FORMAT: &str = "prckopis-redaction-codes";
/// The newest file version this build reads and the one it writes.
pub const VERSION: u64 = 1;
/// The largest code-set file [`import`] reads.
pub const MAX_FILE_BYTES: usize = 1 << 20;
/// The most user-defined sets kept.
pub const MAX_SETS: usize = 100;
/// The most codes a set holds.
pub const MAX_CODES: usize = 500;
/// The longest set name, in characters.
pub const MAX_NAME_CHARS: usize = 100;
/// The longest code, in characters (it becomes overlay text).
pub const MAX_CODE_CHARS: usize = 100;
/// The longest code description, in characters.
pub const MAX_DESCRIPTION_CHARS: usize = 1000;

/// One code of a user-defined set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Code {
    pub code: String,
    pub description: String,
}

/// A user-defined code set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CustomCodeSet {
    pub name: String,
    pub codes: Vec<Code>,
}

impl CustomCodeSet {
    /// The overlay text for the picked codes, as [`overlay_in`].
    pub fn overlay<P: AsRef<str>>(&self, picked: &[P]) -> String {
        let codes: Vec<&str> = self.codes.iter().map(|c| c.code.as_str()).collect();
        overlay_in(&codes, picked)
    }

    pub fn has(&self, code: &str) -> bool {
        self.codes.iter().any(|c| c.code == code)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CodeSetError {
    #[error("the name is empty")]
    EmptyName,
    #[error("the name is longer than {MAX_NAME_CHARS} characters")]
    NameTooLong,
    #[error("a code set named {0:?} already exists")]
    DuplicateSet(String),
    #[error("there is no code set named {0:?}")]
    NoSuchSet(String),
    #[error("the code is empty")]
    EmptyCode,
    #[error("the code is longer than {MAX_CODE_CHARS} characters")]
    CodeTooLong,
    #[error("the description is longer than {MAX_DESCRIPTION_CHARS} characters")]
    DescriptionTooLong,
    #[error("the set already has the code {0:?}")]
    DuplicateCode(String),
    #[error("the set has no code {0:?}")]
    NoSuchCode(String),
    #[error("there can be at most {MAX_SETS} code sets")]
    TooManySets,
    #[error("a set can hold at most {MAX_CODES} codes")]
    TooManyCodes,
    #[error("the file is larger than {MAX_FILE_BYTES} bytes")]
    TooLarge,
    #[error("not a redaction code set file: {0}")]
    Malformed(String),
}

fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// A checked, trimmed set name.
fn set_name(name: &str) -> Result<String, CodeSetError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(CodeSetError::EmptyName);
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(CodeSetError::NameTooLong);
    }
    Ok(name.to_string())
}

/// A checked code: trimmed, on one line.
fn code_entry(code: &str, description: &str) -> Result<Code, CodeSetError> {
    let code = code.trim();
    if code.is_empty() {
        return Err(CodeSetError::EmptyCode);
    }
    if code.chars().count() > MAX_CODE_CHARS {
        return Err(CodeSetError::CodeTooLong);
    }
    let description = description.trim();
    if description.chars().count() > MAX_DESCRIPTION_CHARS {
        return Err(CodeSetError::DescriptionTooLong);
    }
    // Overlay text is one run of text: line breaks become spaces.
    let one_line = |s: &str| s.split(['\r', '\n']).filter(|l| !l.trim().is_empty()).map(str::trim).collect::<Vec<_>>().join(" ");
    Ok(Code { code: one_line(code), description: one_line(description) })
}

/// The user's own redaction code sets (the Redaction Code Editor).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CodeSets {
    sets: Vec<CustomCodeSet>,
}

impl CodeSets {
    pub fn sets(&self) -> &[CustomCodeSet] {
        &self.sets
    }

    pub fn is_empty(&self) -> bool {
        self.sets.is_empty()
    }

    /// The set called `name` (letter case and surrounding spaces don't matter).
    pub fn get(&self, name: &str) -> Option<&CustomCodeSet> {
        self.sets.iter().find(|s| same_name(&s.name, name))
    }

    fn get_mut(&mut self, name: &str) -> Result<&mut CustomCodeSet, CodeSetError> {
        self.sets.iter_mut().find(|s| same_name(&s.name, name)).ok_or_else(|| CodeSetError::NoSuchSet(name.to_string()))
    }

    /// Whether `name` is free: no user set and no built-in set (by name or id) uses it.
    fn free(&self, name: &str, except: Option<&str>) -> bool {
        let builtin = CODE_SETS.iter().any(|s| same_name(s.name, name) || same_name(s.id, name));
        let taken = self.sets.iter().any(|s| same_name(&s.name, name) && !except.is_some_and(|e| same_name(e, &s.name)));
        !builtin && !taken
    }

    /// `base`, or `base (2)`, `base (3)`, … : the first name no set uses.
    pub fn unique_name(&self, base: &str) -> String {
        let base = base.trim();
        if !base.is_empty() && self.free(base, None) {
            return base.to_string();
        }
        // Room for the " (n)" suffix within the name limit.
        let base: String = base.chars().take(MAX_NAME_CHARS.saturating_sub(8)).collect();
        (2..=MAX_SETS + CODE_SETS.len() + 2).map(|n| format!("{base} ({n})")).find(|n| self.free(n, None)).unwrap_or(base)
    }

    /// Add an empty set; returns its (trimmed) name.
    pub fn add_set(&mut self, name: &str) -> Result<String, CodeSetError> {
        let name = set_name(name)?;
        if self.sets.len() >= MAX_SETS {
            return Err(CodeSetError::TooManySets);
        }
        if !self.free(&name, None) {
            return Err(CodeSetError::DuplicateSet(name));
        }
        self.sets.push(CustomCodeSet { name: name.clone(), codes: Vec::new() });
        Ok(name)
    }

    /// Rename a set; returns the new (trimmed) name.
    pub fn rename_set(&mut self, old: &str, new: &str) -> Result<String, CodeSetError> {
        let new = set_name(new)?;
        let current = self.get(old).map(|s| s.name.clone()).ok_or_else(|| CodeSetError::NoSuchSet(old.to_string()))?;
        if !self.free(&new, Some(&current)) {
            return Err(CodeSetError::DuplicateSet(new));
        }
        self.get_mut(old)?.name = new.clone();
        Ok(new)
    }

    pub fn remove_set(&mut self, name: &str) -> Result<CustomCodeSet, CodeSetError> {
        let i = self.sets.iter().position(|s| same_name(&s.name, name)).ok_or_else(|| CodeSetError::NoSuchSet(name.to_string()))?;
        Ok(self.sets.remove(i))
    }

    /// Add a code at the end of a set.
    pub fn add_code(&mut self, set: &str, code: &str, description: &str) -> Result<(), CodeSetError> {
        let entry = code_entry(code, description)?;
        let s = self.get_mut(set)?;
        if s.codes.len() >= MAX_CODES {
            return Err(CodeSetError::TooManyCodes);
        }
        if s.has(&entry.code) {
            return Err(CodeSetError::DuplicateCode(entry.code));
        }
        s.codes.push(entry);
        Ok(())
    }

    /// Change a code's text and description, in place.
    pub fn edit_code(&mut self, set: &str, old: &str, code: &str, description: &str) -> Result<(), CodeSetError> {
        let entry = code_entry(code, description)?;
        let s = self.get_mut(set)?;
        let i = s.codes.iter().position(|c| c.code == old).ok_or_else(|| CodeSetError::NoSuchCode(old.to_string()))?;
        if entry.code != old && s.has(&entry.code) {
            return Err(CodeSetError::DuplicateCode(entry.code));
        }
        if let Some(slot) = s.codes.get_mut(i) {
            *slot = entry;
        }
        Ok(())
    }

    pub fn remove_code(&mut self, set: &str, code: &str) -> Result<(), CodeSetError> {
        let s = self.get_mut(set)?;
        let i = s.codes.iter().position(|c| c.code == code).ok_or_else(|| CodeSetError::NoSuchCode(code.to_string()))?;
        s.codes.remove(i);
        Ok(())
    }

    /// Add imported sets, renaming any whose name is taken ("Name (2)"). Returns the names
    /// they were added under; sets beyond [`MAX_SETS`] are left out (fewer names than sets).
    pub fn merge(&mut self, imported: Vec<CustomCodeSet>) -> Vec<String> {
        let mut added = Vec::new();
        for mut set in imported {
            if self.sets.len() >= MAX_SETS {
                break;
            }
            set.name = self.unique_name(&set.name);
            if set.name.trim().is_empty() {
                continue;
            }
            added.push(set.name.clone());
            self.sets.push(set);
        }
        added
    }

    /// The sets as a code-set file's JSON value (also how preferences keep them).
    pub fn to_json(&self) -> Value {
        to_json(self.sets.iter())
    }

    /// The sets from preferences: lenient, what can't be read is left out.
    pub fn from_json(v: &Value) -> Self {
        let mut sets = Self::default();
        if let Ok(imported) = import_value(v) {
            sets.merge(imported.sets);
        }
        sets
    }
}

fn to_json<'a>(sets: impl Iterator<Item = &'a CustomCodeSet>) -> Value {
    let sets: Vec<Value> = sets
        .map(|s| {
            let codes: Vec<Value> = s.codes.iter().map(|c| json!({ "code": c.code, "description": c.description })).collect();
            json!({ "name": s.name, "codes": codes })
        })
        .collect();
    json!({ "format": FORMAT, "version": VERSION, "sets": sets })
}

/// A code-set file holding `sets`, as text.
pub fn export<'a>(sets: impl IntoIterator<Item = &'a CustomCodeSet>) -> String {
    let v = to_json(sets.into_iter());
    // Serializing a `Value` (string keys, no non-finite numbers) can't fail.
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

/// The sets read from a code-set file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Imported {
    pub sets: Vec<CustomCodeSet>,
    /// Sets and codes left out: unnamed, repeated, too long or too many.
    pub skipped: usize,
}

/// Read a code-set file (untrusted).
pub fn import(bytes: &[u8]) -> Result<Imported, CodeSetError> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(CodeSetError::TooLarge);
    }
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let v: Value = serde_json::from_slice(bytes).map_err(|e| CodeSetError::Malformed(e.to_string()))?;
    import_value(&v)
}

fn import_value(v: &Value) -> Result<Imported, CodeSetError> {
    let malformed = |m: &str| CodeSetError::Malformed(m.to_string());
    if v.get("format").and_then(Value::as_str) != Some(FORMAT) {
        return Err(malformed("\"format\" is not \"prckopis-redaction-codes\""));
    }
    match v.get("version") {
        None => {}
        Some(n) => match n.as_u64() {
            Some(n) if (1..=VERSION).contains(&n) => {}
            Some(n) => return Err(CodeSetError::Malformed(format!("version {n} is newer than this app reads ({VERSION})"))),
            None => return Err(malformed("\"version\" is not a number")),
        },
    }
    let list = v.get("sets").and_then(Value::as_array).ok_or_else(|| malformed("\"sets\" is not a list"))?;
    let mut out = Imported::default();
    for s in list {
        let name = s.get("name").and_then(Value::as_str).map(set_name);
        let Some(Ok(name)) = name else {
            out.skipped += 1;
            continue;
        };
        if out.sets.len() >= MAX_SETS || out.sets.iter().any(|o| same_name(&o.name, &name)) {
            out.skipped += 1;
            continue;
        }
        let mut set = CustomCodeSet { name, codes: Vec::new() };
        for c in s.get("codes").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default() {
            let code = c.get("code").and_then(Value::as_str).unwrap_or_default();
            let description = match c.get("description") {
                None | Some(Value::Null) => Some(""),
                Some(d) => d.as_str(),
            };
            match description.map(|d| code_entry(code, d)) {
                Some(Ok(entry)) if set.codes.len() < MAX_CODES && !set.has(&entry.code) => set.codes.push(entry),
                _ => out.skipped += 1,
            }
        }
        out.sets.push(set);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sets() -> CodeSets {
        let mut s = CodeSets::default();
        s.add_set("  GDPR ").unwrap();
        s.add_code("gdpr", "Art. 9", "Special categories of personal data").unwrap();
        s.add_code("GDPR", "Art. 10", "").unwrap();
        s
    }

    #[test]
    fn custom_sets_are_created_renamed_edited_and_removed() {
        let mut s = sets();
        assert_eq!(s.sets()[0].name, "GDPR");
        assert_eq!(s.add_set("gdpr"), Err(CodeSetError::DuplicateSet("gdpr".into())));
        assert_eq!(s.add_set("U.S. FOIA"), Err(CodeSetError::DuplicateSet("U.S. FOIA".into())), "built-in names are taken");
        assert_eq!(s.add_set("foia"), Err(CodeSetError::DuplicateSet("foia".into())), "built-in ids too");
        assert_eq!(s.add_set("   "), Err(CodeSetError::EmptyName));
        assert_eq!(s.add_set(&"x".repeat(MAX_NAME_CHARS + 1)), Err(CodeSetError::NameTooLong));
        assert_eq!(s.add_code("GDPR", "Art. 9", "again"), Err(CodeSetError::DuplicateCode("Art. 9".into())));
        assert_eq!(s.add_code("GDPR", " ", ""), Err(CodeSetError::EmptyCode));
        assert_eq!(s.add_code("Nope", "A", ""), Err(CodeSetError::NoSuchSet("Nope".into())));
        s.add_code("GDPR", "Art.\n6", "two\r\nlines").unwrap();
        assert_eq!(s.get("gdpr").unwrap().codes[2], Code { code: "Art. 6".into(), description: "two lines".into() });

        assert_eq!(s.rename_set("gdpr", "EU GDPR"), Ok("EU GDPR".into()));
        assert_eq!(s.rename_set("EU GDPR", "eu gdpr"), Ok("eu gdpr".into()), "a set may change its own letter case");
        s.add_set("Other").unwrap();
        assert_eq!(s.rename_set("other", "EU GDPR"), Err(CodeSetError::DuplicateSet("EU GDPR".into())));

        s.edit_code("eu gdpr", "Art. 10", "Art. 10(1)", "Criminal convictions").unwrap();
        assert_eq!(s.edit_code("eu gdpr", "Art. 10(1)", "Art. 9", ""), Err(CodeSetError::DuplicateCode("Art. 9".into())));
        assert_eq!(s.edit_code("eu gdpr", "Art. 99", "X", ""), Err(CodeSetError::NoSuchCode("Art. 99".into())));
        s.remove_code("eu gdpr", "Art. 9").unwrap();
        let codes: Vec<&str> = s.get("EU GDPR").unwrap().codes.iter().map(|c| c.code.as_str()).collect();
        assert_eq!(codes, ["Art. 10(1)", "Art. 6"]);
        assert_eq!(s.get("EU GDPR").unwrap().overlay(&["Art. 6", "Art. 10(1)", "gone"]), "Art. 10(1), Art. 6");

        assert_eq!(s.remove_set("other").map(|r| r.name), Ok("Other".into()));
        assert!(s.remove_set("other").is_err());
        assert_eq!(s.unique_name("EU GDPR"), "EU GDPR (2)");
        assert_eq!(s.unique_name("New code set"), "New code set");
    }

    #[test]
    fn set_and_code_counts_are_capped() {
        let mut s = CodeSets::default();
        for i in 0..MAX_SETS {
            s.add_set(&format!("S{i}")).unwrap();
        }
        assert_eq!(s.add_set("one more"), Err(CodeSetError::TooManySets));
        for i in 0..MAX_CODES {
            s.add_code("S0", &format!("c{i}"), "").unwrap();
        }
        assert_eq!(s.add_code("S0", "extra", ""), Err(CodeSetError::TooManyCodes));
        assert_eq!(s.merge(vec![CustomCodeSet { name: "late".into(), codes: Vec::new() }]), Vec::<String>::new());
    }

    #[test]
    fn code_set_files_round_trip() {
        let s = sets();
        let text = export(s.sets());
        assert!(text.contains("\"format\": \"prckopis-redaction-codes\""));
        let back = import(text.as_bytes()).unwrap();
        assert_eq!((back.sets.as_slice(), back.skipped), (s.sets(), 0));
        // A byte-order mark, a missing version and a missing description are fine.
        let lenient = "\u{feff}{\"format\":\"prckopis-redaction-codes\",\"sets\":[{\"name\":\"Č\",\"codes\":[{\"code\":\"§ 1\"}],\"extra\":1}]}";
        let back = import(lenient.as_bytes()).unwrap();
        assert_eq!(back.sets[0].codes[0], Code { code: "§ 1".into(), description: String::new() });
        // Importing next to sets with the same names renames them.
        let mut mine = sets();
        assert_eq!(mine.merge(import(text.as_bytes()).unwrap().sets), ["GDPR (2)"]);
        assert_eq!(CodeSets::from_json(&mine.to_json()), mine, "preferences keep the sets as they are");
    }

    #[test]
    fn malformed_code_set_files_are_refused_or_trimmed() {
        for bad in [
            &b""[..],
            b"not json",
            b"[]",
            b"{\"sets\":[]}",
            b"{\"format\":\"other\",\"sets\":[]}",
            b"{\"format\":\"prckopis-redaction-codes\"}",
            b"{\"format\":\"prckopis-redaction-codes\",\"version\":2,\"sets\":[]}",
            b"{\"format\":\"prckopis-redaction-codes\",\"version\":\"1\",\"sets\":[]}",
            b"{\"format\":\"prckopis-redaction-codes\",\"sets\":{}}",
        ] {
            assert!(matches!(import(bad), Err(CodeSetError::Malformed(_))), "{}", String::from_utf8_lossy(bad));
        }
        assert_eq!(import(&vec![b' '; MAX_FILE_BYTES + 1]), Err(CodeSetError::TooLarge));
        // Deep nesting is refused by the parser, not by the stack.
        let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
        assert!(import(deep.as_bytes()).is_err());
        // Bad entries are skipped and counted; the rest is kept.
        let long = "x".repeat(MAX_CODE_CHARS + 1);
        let mixed = json!({
            "format": FORMAT,
            "sets": [
                { "name": "", "codes": [] },
                { "name": 7 },
                { "name": "A", "codes": [ { "code": "1" }, { "code": "1" }, { "code": "" }, { "code": long }, { "code": "2", "description": 5 }, "x", { "code": "3", "description": null } ] },
                { "name": "a" },
                { "name": "B", "codes": "none" },
            ]
        });
        let got = import(mixed.to_string().as_bytes()).unwrap();
        let names: Vec<&str> = got.sets.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["A", "B"]);
        let codes: Vec<&str> = got.sets[0].codes.iter().map(|c| c.code.as_str()).collect();
        assert_eq!(codes, ["1", "3"]);
        assert_eq!(got.skipped, 3 + 5);
    }
}
