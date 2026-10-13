//! Field locks (FieldMDP, ISO 32000-2 §12.8.2.4): signing locks form fields, and changing a
//! locked field afterwards is a change the signature does not permit.

use std::sync::Arc;

use prckopis_cos::{Document, ObjRef, PdfString, SaveOptions, write_incremental};
use prckopis_sign::{FieldLock, Modification, SignOptions, SignatureInfo, Status, TrustStore, kind, pkcs12, signatures};

fn data(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// One page with two text fields, "Name" (object 7) and "Amount" (object 8), and an unsigned
/// signature field "Approval" (object 6) carrying `lock` as its `/Lock` dictionary, if any.
fn fixture(lock: &str) -> Vec<u8> {
    let approval = format!("<< /Type /Annot /Subtype /Widget /FT /Sig /T (Approval) /Rect [150 20 280 70] /P 3 0 R /F 4 {lock} >>");
    let objs: Vec<&[u8]> = vec![
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R 7 0 R 8 0 R] >> >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R 7 0 R 8 0 R] >>",
        b"<< /Length 44 >>\nstream\nBT /F1 14 Tf 20 250 Td (Contract text) Tj ET\nendstream",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        approval.as_bytes(),
        b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (Name) /V (Ada) /Rect [20 200 200 220] /P 3 0 R /F 4 >>",
        b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (Amount) /V (10) /Rect [20 170 200 190] /P 3 0 R /F 4 >>",
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

const NAME: ObjRef = ObjRef { num: 7, generation: 0 };
const AMOUNT: ObjRef = ObjRef { num: 8, generation: 0 };

fn open(b: &[u8]) -> Document {
    Document::open(Arc::new(b.to_vec())).unwrap()
}

fn opts(lock: Option<FieldLock>) -> SignOptions {
    SignOptions { page: 0, rect: Some([20.0, 100.0, 220.0, 150.0]), date: "D:20261002120000+01'00'".into(), lock, ..SignOptions::default() }
}

fn sign(bytes: &[u8], opts: &SignOptions) -> Vec<u8> {
    let id = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    prckopis_sign::sign(&open(bytes), &id, opts).unwrap()
}

/// The first signed signature of `bytes`.
fn signature(bytes: &[u8]) -> SignatureInfo {
    let id = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    signatures(&open(bytes), bytes, &TrustStore { certs: vec![id.certificate.clone()] }).into_iter().find(|s| s.signed).unwrap()
}

/// `bytes` with the text field `field` filled in with `value` in a new revision.
fn fill(bytes: &[u8], field: ObjRef, value: &str) -> Vec<u8> {
    let mut doc = open(bytes);
    doc.update_dict(field, |d| d.set(b"V".to_vec(), PdfString::text(value))).unwrap();
    write_incremental(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap()
}

fn read_only(bytes: &[u8], field: ObjRef) -> bool {
    open(bytes).get(field).as_dict().and_then(|d| d.int(b"Ff")).is_some_and(|f| f & 1 == 1)
}

#[test]
fn signing_locks_the_named_fields_and_a_later_change_to_them_is_not_permitted() {
    let signed = sign(&fixture(""), &opts(Some(FieldLock::Include(vec!["Name".into()]))));
    assert!(read_only(&signed, NAME), "the locked field is read-only");
    assert!(!read_only(&signed, AMOUNT), "the other field is not");
    let s = signature(&signed);
    assert_eq!(s.status, Status::Valid, "{:?}", s.details);
    assert_eq!(s.locked_fields, Some(FieldLock::Include(vec!["Name".into()])));
    assert!(s.details.iter().any(|d| d.message == "The signer locked these form fields: Name."), "{:?}", s.details);
    // Filling in the unlocked field is form fill, which an approval signature permits.
    let s = signature(&fill(&signed, AMOUNT, "20"));
    assert_eq!(s.modification, Modification::Allowed(vec![kind::FORM_FILL]), "{:?}", s.details);
    assert_eq!(s.status, Status::Valid);
    // Changing the locked one is not.
    let s = signature(&fill(&signed, NAME, "Mallory"));
    assert_eq!(s.modification, Modification::Disallowed(vec![kind::LOCKED_FIELDS]), "{:?}", s.details);
    assert_eq!(s.status, Status::Invalid);
}

#[test]
fn all_and_exclude_locks() {
    let signed = sign(&fixture(""), &opts(Some(FieldLock::All)));
    assert!(read_only(&signed, NAME) && read_only(&signed, AMOUNT));
    let s = signature(&signed);
    assert!(s.details.iter().any(|d| d.message == "The signer locked all form fields."), "{:?}", s.details);
    assert_eq!(signature(&fill(&signed, AMOUNT, "20")).modification, Modification::Disallowed(vec![kind::LOCKED_FIELDS]));

    let signed = sign(&fixture(""), &opts(Some(FieldLock::Exclude(vec!["Amount".into()]))));
    assert!(read_only(&signed, NAME) && !read_only(&signed, AMOUNT));
    assert_eq!(signature(&fill(&signed, AMOUNT, "20")).modification, Modification::Allowed(vec![kind::FORM_FILL]));
    assert_eq!(signature(&fill(&signed, NAME, "Mallory")).modification, Modification::Disallowed(vec![kind::LOCKED_FIELDS]));
}

#[test]
fn signing_a_field_applies_the_lock_it_asks_for() {
    let bytes = fixture("/Lock << /Type /SigFieldLock /Action /Include /Fields [(Amount)] >>");
    let signed = sign(&bytes, &SignOptions { field: Some("Approval".into()), ..opts(None) });
    assert!(read_only(&signed, AMOUNT) && !read_only(&signed, NAME));
    let s = signature(&signed);
    assert_eq!(s.locked_fields, Some(FieldLock::Include(vec!["Amount".into()])));
    assert_eq!(signature(&fill(&signed, AMOUNT, "20")).modification, Modification::Disallowed(vec![kind::LOCKED_FIELDS]));
    // The caller's lock wins over the field's.
    let signed = sign(&bytes, &SignOptions { field: Some("Approval".into()), ..opts(Some(FieldLock::Include(vec!["Name".into()]))) });
    assert!(read_only(&signed, NAME) && !read_only(&signed, AMOUNT));
}

#[test]
fn a_lock_without_fields_or_with_an_unknown_action_locks_nothing() {
    for lock in ["/Lock << /Action /Include >>", "/Lock << /Action /Everything /Fields [(Name)] >>", "/Lock 5"] {
        let signed = sign(&fixture(lock), &SignOptions { field: Some("Approval".into()), ..opts(None) });
        assert!(!read_only(&signed, NAME) && !read_only(&signed, AMOUNT), "{lock}");
        assert_eq!(signature(&signed).locked_fields, None, "{lock}");
    }
    // Names that aren't strings are skipped, not fatal.
    let signed =
        sign(&fixture("/Lock << /Action /Include /Fields [42 /Name (Name)] >>"), &SignOptions { field: Some("Approval".into()), ..opts(None) });
    assert_eq!(signature(&signed).locked_fields, Some(FieldLock::Include(vec!["Name".into()])));
}

#[test]
fn naming_a_parent_locks_its_kids() {
    let lock = FieldLock::Include(vec!["Address".into()]);
    assert!(lock.locks("Address") && lock.locks("Address.City"));
    assert!(!lock.locks("Addresses") && !lock.locks("Name"));
    let except = FieldLock::Exclude(vec!["Address".into()]);
    assert!(!except.locks("Address.City") && except.locks("Addresses"));
    assert!(FieldLock::All.locks(""));
}

#[test]
fn a_counter_signature_on_a_new_field_is_still_permitted() {
    let signed = sign(&fixture(""), &opts(Some(FieldLock::Include(vec!["Name".into(), "Amount".into()]))));
    let s = signature(&signed);
    assert_eq!(s.locked_fields, Some(FieldLock::Include(vec!["Name".into(), "Amount".into()])));
    // A later counter-signature on a new field is still a permitted signature change.
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let counter = prckopis_sign::sign(&open(&signed), &id, &SignOptions { new_field_name: Some("Witness".into()), ..opts(None) }).unwrap();
    let s = signatures(&open(&counter), &counter, &TrustStore::default()).into_iter().find(|s| s.field == "Signature1").unwrap();
    assert_eq!(s.modification, Modification::Allowed(vec![kind::SIGNATURE]), "{:?}", s.details);
}
