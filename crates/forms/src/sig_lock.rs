//! A signature field's lock (`/Lock`, ISO 32000-2 §12.7.5.5, Table 236): which form fields become
//! read-only when the field is signed. Acrobat calls it Digital Signature Properties ▸ Signed ▸
//! "Mark as read-only". Signing honours it (`prckopis-sign`); this module authors and reads it.

use prckopis_cos::{Dict, Document, Object, PdfString};

use crate::{Field, FieldKind, FormError};

/// Which fields signing a signature field locks. Names are fully qualified; naming a parent
/// locks its kids.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignatureLock {
    /// Every field in the document.
    All,
    /// Only these fields ("Just these fields").
    Include(Vec<String>),
    /// Every field but these ("All fields except these").
    Exclude(Vec<String>),
}

/// The most field names a lock keeps (the list comes from the file, which is untrusted).
pub const MAX_LOCK_FIELDS: usize = 10_000;

impl SignatureLock {
    /// The field names it lists (none for [`SignatureLock::All`]).
    pub fn fields(&self) -> &[String] {
        match self {
            SignatureLock::All => &[],
            SignatureLock::Include(l) | SignatureLock::Exclude(l) => l,
        }
    }

    /// Read a `/Lock` dictionary. `None` when its `/Action` is missing or unknown, or the
    /// `/Fields` array that `Include` and `Exclude` need is missing (a malformed lock is ignored,
    /// as signing ignores it).
    pub fn from_dict(doc: &Document, d: &Dict) -> Option<SignatureLock> {
        let names = || -> Option<Vec<String>> {
            let list = doc.resolve(d.get(b"Fields")?);
            let list = list.as_array()?;
            Some(
                list.iter()
                    .take(MAX_LOCK_FIELDS)
                    .filter_map(|n| match &*doc.resolve(n) {
                        Object::String(s) => Some(s.to_text()),
                        _ => None,
                    })
                    .collect(),
            )
        };
        match d.name(b"Action")? {
            b"All" => Some(SignatureLock::All),
            b"Include" => Some(SignatureLock::Include(names()?)),
            b"Exclude" => Some(SignatureLock::Exclude(names()?)),
            _ => None,
        }
    }

    /// The lock dictionary: `<< /Type /SigFieldLock /Action … /Fields [...] >>`.
    pub fn to_dict(&self) -> Dict {
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("SigFieldLock"));
        let (action, list) = match self {
            SignatureLock::All => ("All", None),
            SignatureLock::Include(l) => ("Include", Some(l)),
            SignatureLock::Exclude(l) => ("Exclude", Some(l)),
        };
        d.set(b"Action".to_vec(), Object::name(action));
        if let Some(list) = list {
            d.set(b"Fields".to_vec(), Object::Array(list.iter().take(MAX_LOCK_FIELDS).map(|n| Object::String(PdfString::text(n))).collect()));
        }
        d
    }
}

/// The lock a signature field asks for when it is signed (`None` for other fields, fields
/// without a lock, and malformed locks).
pub fn signature_lock(doc: &Document, f: &Field) -> Option<SignatureLock> {
    if f.kind != FieldKind::Signature {
        return None;
    }
    let field = doc.get(f.obj);
    let lock = doc.resolve(field.as_dict()?.get(b"Lock")?);
    SignatureLock::from_dict(doc, lock.as_dict()?)
}

/// Whether `name` names a field of `all` or a parent of one.
fn names_a_field(all: &[Field], name: &str) -> bool {
    all.iter().any(|f| f.name == name || f.name.strip_prefix(name).is_some_and(|rest| rest.starts_with('.')))
}

/// Set (`Some`) or remove (`None`) a signature field's lock. The lock dictionary is written as
/// an indirect object, as Table 236 requires.
pub(crate) fn set_signature_lock(doc: &mut Document, all: &[Field], f: &Field, lock: Option<&SignatureLock>) -> Result<(), FormError> {
    if f.kind != FieldKind::Signature {
        return Err(FormError::Invalid(format!("{:?} is not a signature field; only signature fields lock other fields when signed", f.name)));
    }
    let Some(lock) = lock else {
        doc.update_dict(f.obj, |d| {
            d.remove(b"Lock");
        })?;
        return Ok(());
    };
    let names = lock.fields();
    if names.len() > MAX_LOCK_FIELDS {
        return Err(FormError::Invalid(format!("a lock names at most {MAX_LOCK_FIELDS} fields")));
    }
    if matches!(lock, SignatureLock::Include(l) if l.is_empty()) {
        return Err(FormError::Invalid("choose at least one field to mark as read-only".into()));
    }
    let mut seen = std::collections::HashSet::new();
    for n in names {
        if n.trim().is_empty() {
            return Err(FormError::Invalid("a locked field name can't be empty".into()));
        }
        if !names_a_field(all, n) {
            return Err(FormError::NoSuchField(n.clone()));
        }
        if !seen.insert(n.as_str()) {
            return Err(FormError::Invalid(format!("field {n:?} is listed more than once")));
        }
    }
    // Always a new object: an existing one may be shared with another signature field.
    let r = doc.add(Object::Dict(lock.to_dict()));
    doc.update_dict(f.obj, |d| d.set(b"Lock".to_vec(), Object::Ref(r)))?;
    Ok(())
}
