//! Automatically tag an untagged document (ISO 32000-2 §14.7–14.8, PDF/UA-1 §7).
//!
//! Each page's content stream is read once, in content order:
//!
//! - text is grouped into blocks by size and line spacing; a block becomes a paragraph (`P`), or a
//!   heading (`H1`–`H6`) when its text is clearly larger than the body text of the document;
//! - images (image XObjects and inline images) become `Figure`s, and form XObjects become a `P`
//!   when they draw text, a `Figure` when they draw images, and an artifact otherwise;
//! - vector graphics (paths and shadings) become artifacts (`/Artifact BMC … EMC`);
//! - annotations become `Link`, `Form` (widgets) or `Annot` elements through object references,
//!   and pages with annotations tab in structure order.
//!
//! The marked content gets MCIDs (`/P <</MCID n>> BDC … EMC`) that the parent tree maps back to
//! their elements, under one `Document` element. The catalog gets `/StructTreeRoot` and
//! `/MarkInfo <</Marked true>>`. Reading order is content order: the Reading Order and Tags tools
//! correct it afterwards.
//!
//! Marked content is only ever wrapped around whole graphics objects (one text-showing operator,
//! one path object, one `Do`, `sh` or inline image) and the neutral operators between two such
//! objects of one element, so it nests properly with the page's own marked content and never
//! splits a path or text object (§14.6). Content already marked as an artifact, or carrying an
//! MCID, is left as it is. A page whose content can't be read completely is left untouched and
//! reported, never rewritten from a partial reading.

use std::collections::{HashMap, HashSet};

use prckopis_content::{Matrix, Op};
use prckopis_cos::{Dict, Document, ObjRef, Object, Stream};

/// At most this many elements are created; a larger document is refused before anything changes.
pub const MAX_ELEMENTS: usize = 500_000;
/// Existing MCIDs above this keep a page untouched (its parent-tree array would be too large).
const MAX_MCID: i64 = 100_000;
/// Form XObjects nested deeper than this aren't looked into to classify them.
const MAX_FORM_DEPTH: usize = 8;
/// A heading is at least this much larger than the body text.
const HEADING_RATIO: f64 = 1.2;
/// A block with more characters than this is a paragraph, however large its text.
const HEADING_MAX_CHARS: usize = 200;
/// Lines of one paragraph are at most this many font sizes apart.
const LINE_GAP: f64 = 1.7;

/// What [`autotag`] created.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AutotagSummary {
    /// Pages whose content was tagged (pages with nothing to tag are not counted).
    pub pages: usize,
    pub headings: usize,
    pub paragraphs: usize,
    pub figures: usize,
    /// Annotations tagged as `Link`, `Form` or `Annot` elements.
    pub annotations: usize,
    /// Content items (paths, shadings, form XObjects without text or images) marked as artifacts.
    pub artifacts: usize,
    /// Pages (0-based) left untouched because their content couldn't be read completely.
    pub skipped_pages: Vec<usize>,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum AutotagError {
    #[error("the document is already tagged; edit its tags in the Tags panel")]
    AlreadyTagged,
    #[error("the document has no pages")]
    NoPages,
    #[error("the document has nothing to tag")]
    NothingToTag,
    #[error("the document has too much content to tag automatically (more than {MAX_ELEMENTS} tags)")]
    TooLarge,
    #[error(transparent)]
    Cos(#[from] prckopis_cos::CosError),
}

/// What one unit of content becomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// Text of block `n` on this page.
    Text(usize),
    /// A form XObject that draws text: a paragraph of its own.
    FormText,
    Figure,
    Artifact,
}

/// A whole graphics object: operators `start..=end`.
#[derive(Clone, Copy, Debug)]
struct Unit {
    start: usize,
    end: usize,
    kind: Kind,
}

/// A text block on one page.
#[derive(Clone, Debug, Default)]
struct Block {
    /// Characters per rounded font size (in half points).
    sizes: HashMap<i64, usize>,
    chars: usize,
}

impl Block {
    /// The size most of its characters have, in points.
    fn size(&self) -> f64 {
        self.sizes.iter().max_by_key(|(size, n)| (**n, **size)).map(|(s, _)| *s as f64 / 2.0).unwrap_or(0.0)
    }
}

struct PagePlan {
    index: usize,
    obj: ObjRef,
    ops: Vec<Op>,
    units: Vec<Unit>,
    blocks: Vec<Block>,
    /// The first free MCID on the page.
    first_mcid: i64,
}

/// The decoded page content, or `None` when a stream can't be decoded (so it is not rewritten).
fn page_content(doc: &Document, page: &Dict) -> Option<Vec<u8>> {
    let Some(c) = page.get(b"Contents") else { return Some(Vec::new()) };
    match &*doc.resolve(c) {
        Object::Stream(s) => s.decoded().ok(),
        Object::Array(a) => {
            let mut out = Vec::new();
            for x in a {
                match &*doc.resolve(x) {
                    Object::Stream(s) => {
                        out.extend_from_slice(&s.decoded().ok()?);
                        out.push(b'\n');
                    }
                    Object::Null => {}
                    _ => return None,
                }
            }
            Some(out)
        }
        Object::Null => Some(Vec::new()),
        _ => None,
    }
}

fn sub(doc: &Document, res: &Dict, key: &[u8]) -> Dict {
    res.get(key).map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default()
}

/// What a form XObject draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Draws {
    Text,
    Images,
    Graphics,
}

struct Forms<'a> {
    doc: &'a Document,
    cache: HashMap<ObjRef, Draws>,
}

impl Forms<'_> {
    fn classify(&mut self, r: ObjRef, outer: &Dict, depth: usize, open: &mut HashSet<ObjRef>) -> Draws {
        if let Some(d) = self.cache.get(&r) {
            return *d;
        }
        if depth > MAX_FORM_DEPTH || !open.insert(r) {
            return Draws::Graphics;
        }
        let obj = self.doc.get(r);
        let Object::Stream(s) = &*obj else {
            open.remove(&r);
            return Draws::Graphics;
        };
        let res = s.dict.get(b"Resources").map(|o| self.doc.resolve(o)).and_then(|o| o.as_dict().cloned()).unwrap_or_else(|| outer.clone());
        let xobjects = sub(self.doc, &res, b"XObject");
        let mut draws = Draws::Graphics;
        for op in prckopis_content::parse(&s.decoded().unwrap_or_default()).ops {
            match op.op.as_slice() {
                b"Tj" | b"TJ" | b"'" | b"\"" => {
                    draws = Draws::Text;
                    break;
                }
                b"BI" => draws = Draws::Images,
                b"Do" => {
                    let Some(x) = op.name(0).and_then(|n| xobjects.get(n)).and_then(Object::as_ref) else { continue };
                    let inner = match self.doc.get(x).as_dict().and_then(|d| d.name(b"Subtype").map(<[u8]>::to_vec)).as_deref() {
                        Some(b"Image") => Draws::Images,
                        Some(b"Form") => self.classify(x, &res, depth + 1, open),
                        _ => Draws::Graphics,
                    };
                    if inner == Draws::Text {
                        draws = Draws::Text;
                        break;
                    }
                    if inner == Draws::Images {
                        draws = Draws::Images;
                    }
                }
                _ => {}
            }
        }
        open.remove(&r);
        self.cache.insert(r, draws);
        draws
    }
}

/// Characters a text-showing operator shows (bytes, a proxy good enough to weigh sizes).
fn shown(op: &Op) -> usize {
    op.operands
        .iter()
        .map(|o| match o {
            Object::String(s) => s.bytes.len(),
            Object::Array(a) => a.iter().filter_map(Object::as_string).map(|s| s.bytes.len()).sum(),
            _ => 0,
        })
        .sum()
}

/// Read one page: its units of content and text blocks. `None`: leave the page alone.
fn plan_page(doc: &Document, forms: &mut Forms, index: usize, obj: ObjRef, page: &Dict) -> Option<PagePlan> {
    let data = page_content(doc, page)?;
    let parsed = prckopis_content::parse(&data);
    if parsed.skipped > 0 {
        return None;
    }
    let ops = parsed.ops;
    let res = page.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let (xobjects, props) = (sub(doc, &res, b"XObject"), sub(doc, &res, b"Properties"));
    let mut units: Vec<Unit> = Vec::new();
    let mut blocks: Vec<Block> = Vec::new();
    // Graphics state: CTM stack; text state: matrices, size and leading.
    let mut ctm = vec![Matrix::IDENTITY];
    let (mut tm, mut tlm) = (Matrix::IDENTITY, Matrix::IDENTITY);
    let (mut font_size, mut leading) = (0.0_f64, 0.0_f64);
    // Marked-content stack: whether each level is already tagged or an artifact.
    let mut marks: Vec<bool> = Vec::new();
    let mut max_mcid: Option<i64> = None;
    let mut path_start: Option<usize> = None;
    // The last text shown: its baseline height, size and block.
    let mut last: Option<(f64, f64, usize)> = None;
    for (i, op) in ops.iter().enumerate() {
        let covered = marks.last().copied().unwrap_or(false);
        let top = ctm.last().copied().unwrap_or_default();
        let mut unit = |kind: Kind, start: usize| {
            if !covered {
                units.push(Unit { start, end: i, kind });
            }
        };
        match op.op.as_slice() {
            b"q" => {
                path_start = None;
                ctm.push(top);
            }
            b"Q" => {
                path_start = None;
                if ctm.len() > 1 {
                    ctm.pop();
                }
            }
            b"cm" => {
                if let Some(m) = Matrix::from_operands(&op.operands)
                    && let Some(t) = ctm.last_mut()
                {
                    *t = m.then(&top);
                }
            }
            b"BMC" | b"BDC" => {
                path_start = None;
                let mcid = match op.operands.get(1) {
                    Some(Object::Dict(d)) => d.get(b"MCID").and_then(Object::as_int),
                    Some(Object::Name(n)) => props.get(n).map(|x| doc.resolve(x)).and_then(|x| x.as_dict().and_then(|d| d.int(b"MCID"))),
                    _ => None,
                };
                if let Some(m) = mcid {
                    max_mcid = Some(max_mcid.map_or(m, |x: i64| x.max(m)));
                }
                marks.push(covered || op.name(0) == Some(b"Artifact") || mcid.is_some());
            }
            b"EMC" => {
                path_start = None;
                marks.pop();
            }
            b"BT" => {
                path_start = None;
                (tm, tlm) = (Matrix::IDENTITY, Matrix::IDENTITY);
            }
            b"ET" => path_start = None,
            b"Tf" => {
                if let Some(s) = op.num(1).filter(|s| s.is_finite()) {
                    font_size = s;
                }
            }
            b"TL" => {
                if let Some(l) = op.num(0).filter(|l| l.is_finite()) {
                    leading = l;
                }
            }
            b"Td" | b"TD" => {
                if let Some([x, y]) = op.nums::<2>() {
                    if op.op == b"TD" {
                        leading = -y;
                    }
                    tlm = Matrix::translate(x, y).then(&tlm);
                    tm = tlm;
                }
            }
            b"Tm" => {
                if let Some(m) = Matrix::from_operands(&op.operands) {
                    (tm, tlm) = (m, m);
                }
            }
            b"T*" => {
                tlm = Matrix::translate(0.0, -leading).then(&tlm);
                tm = tlm;
            }
            b"Tj" | b"TJ" | b"'" | b"\"" => {
                if matches!(op.op.as_slice(), b"'" | b"\"") {
                    tlm = Matrix::translate(0.0, -leading).then(&tlm);
                    tm = tlm;
                }
                let m = tm.then(&top);
                let [_, _, c, d, _, _] = m.0;
                let size = font_size.abs() * c.hypot(d);
                let y = m.apply(0.0, 0.0).1;
                let size = if size.is_finite() { size.min(10_000.0) } else { 0.0 };
                let y = if y.is_finite() { y } else { 0.0 };
                let same = last.filter(|&(ly, ls, _)| {
                    let big = ls.max(size).max(0.1);
                    let similar = (ls - size).abs() <= 0.15 * big;
                    let same_line = (ly - y).abs() <= 0.3 * big;
                    let next_line = ly - y > 0.0 && ly - y <= LINE_GAP * big;
                    similar && (same_line || next_line)
                });
                let block = match same {
                    Some((.., b)) => b,
                    None => {
                        blocks.push(Block::default());
                        blocks.len() - 1
                    }
                };
                if !covered {
                    if let Some(b) = blocks.get_mut(block) {
                        let n = shown(op);
                        *b.sizes.entry((size * 2.0).round() as i64).or_default() += n;
                        b.chars = b.chars.saturating_add(n);
                    }
                    last = Some((y, size, block));
                }
                unit(Kind::Text(block), i);
            }
            b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re" => {
                path_start.get_or_insert(i);
            }
            b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"S" | b"s" => {
                unit(Kind::Artifact, path_start.take().unwrap_or(i));
            }
            b"n" => path_start = None,
            b"sh" => unit(Kind::Artifact, i),
            b"BI" => {
                unit(Kind::Figure, i);
                last = None;
            }
            b"Do" => {
                let Some(r) = op.name(0).and_then(|n| xobjects.get(n)).and_then(Object::as_ref) else { continue };
                let subtype = doc.get(r).as_dict().and_then(|d| d.name(b"Subtype").map(<[u8]>::to_vec));
                let kind = match subtype.as_deref() {
                    Some(b"Image") => Kind::Figure,
                    Some(b"Form") => match forms.classify(r, &res, 0, &mut HashSet::new()) {
                        Draws::Text => Kind::FormText,
                        Draws::Images => Kind::Figure,
                        Draws::Graphics => Kind::Artifact,
                    },
                    _ => continue,
                };
                if kind != Kind::Artifact {
                    last = None;
                }
                unit(kind, i);
            }
            _ => {}
        }
    }
    let first_mcid = match max_mcid {
        Some(m) if !(0..MAX_MCID).contains(&m) => return None,
        Some(m) => m + 1,
        None => 0,
    };
    Some(PagePlan { index, obj, ops, units, blocks, first_mcid })
}

/// Operators that may sit inside one element's marked content between two of its objects.
fn neutral(op: &Op) -> bool {
    !matches!(op.op.as_slice(), b"BT" | b"ET" | b"q" | b"Q" | b"BMC" | b"BDC" | b"EMC" | b"BX" | b"EX")
}

/// Units merged into marked-content sequences: (start, end, kind).
fn sequences(plan: &PagePlan) -> Vec<Unit> {
    let mut out: Vec<Unit> = Vec::new();
    for u in &plan.units {
        if let Some(prev) = out.last_mut()
            && prev.kind == u.kind
            && matches!(u.kind, Kind::Text(_) | Kind::Artifact)
            && plan.ops.get(prev.end + 1..u.start).is_some_and(|between| between.iter().all(neutral))
        {
            prev.end = u.end;
            continue;
        }
        out.push(*u);
    }
    out
}

/// Heading levels of all blocks (`None`: a paragraph), in document order.
fn heading_levels(plans: &[PagePlan]) -> Vec<Vec<Option<u8>>> {
    let mut by_size: HashMap<i64, usize> = HashMap::new();
    for b in plans.iter().flat_map(|p| &p.blocks) {
        for (s, n) in &b.sizes {
            *by_size.entry(*s).or_default() += n;
        }
    }
    let body = by_size.iter().max_by_key(|(s, n)| (**n, -**s)).map(|(s, _)| *s as f64 / 2.0).unwrap_or(0.0);
    let is_heading = |b: &Block| body > 0.0 && b.chars > 0 && b.chars <= HEADING_MAX_CHARS && b.size() >= body * HEADING_RATIO;
    let mut sizes: Vec<i64> = plans.iter().flat_map(|p| &p.blocks).filter(|b| is_heading(b)).map(|b| (b.size() * 2.0).round() as i64).collect();
    sizes.sort_unstable_by(|a, b| b.cmp(a));
    sizes.dedup();
    // Levels never skip one going down (PDF/UA-1 §7.4.2): an H3 after an H1 is an H2.
    let mut previous = 0u8;
    plans
        .iter()
        .map(|p| {
            p.blocks
                .iter()
                .map(|b| {
                    if !is_heading(b) {
                        return None;
                    }
                    let rank = sizes.iter().position(|s| *s == (b.size() * 2.0).round() as i64).unwrap_or(0);
                    let level = u8::try_from(rank + 1).unwrap_or(6).min(6).min(previous.saturating_add(1)).max(1);
                    previous = level;
                    Some(level)
                })
                .collect()
        })
        .collect()
}

fn element(kind: &str, parent: ObjRef, page: ObjRef, k: Object) -> Object {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("StructElem"));
    d.set(b"S".to_vec(), Object::name(kind));
    d.set(b"P".to_vec(), Object::Ref(parent));
    d.set(b"Pg".to_vec(), Object::Ref(page));
    d.set(b"K".to_vec(), k);
    Object::Dict(d)
}

fn mcid_kids(ids: &[i64]) -> Object {
    match ids {
        [one] => Object::Int(*one),
        _ => Object::Array(ids.iter().map(|n| Object::Int(*n)).collect()),
    }
}

/// Tag an untagged document; see the module documentation.
pub fn autotag(doc: &mut Document) -> Result<AutotagSummary, AutotagError> {
    let catalog_ref = doc.root().ok_or(AutotagError::NoPages)?;
    // A structure tree with any kids is kept: replacing tags someone made is not ours to do.
    let kids = crate::alt::tree_root(doc).and_then(|r| r.get(b"K").map(|k| doc.resolve(k)));
    if kids.is_some_and(|k| !matches!(&*k, Object::Null) && k.as_array().is_none_or(|a| !a.is_empty())) {
        return Err(AutotagError::AlreadyTagged);
    }
    let pages = prckopis_model::pages(doc);
    if pages.is_empty() {
        return Err(AutotagError::NoPages);
    }
    let mut summary = AutotagSummary::default();
    let mut forms = Forms { doc, cache: HashMap::new() };
    let mut plans = Vec::new();
    for (index, page) in pages.iter().enumerate() {
        match plan_page(doc, &mut forms, index, page.obj, &page.dict) {
            Some(p) => plans.push(p),
            None => summary.skipped_pages.push(index),
        }
    }
    // Annotations to tag, per page: (annotation, structure type).
    let mut annots: Vec<(usize, ObjRef, &'static str)> = Vec::new();
    let mut seen = HashSet::new();
    for (index, page) in pages.iter().enumerate() {
        let list = page.dict.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default();
        for r in list.iter().filter_map(Object::as_ref) {
            let Some(a) = doc.get(r).as_dict().cloned() else { continue };
            let kind = match a.name(b"Subtype") {
                Some(b"Popup" | b"PrinterMark") | None => continue,
                Some(b"Widget") => "Form",
                Some(b"Link") => "Link",
                Some(_) => "Annot",
            };
            if seen.insert(r) {
                annots.push((index, r, kind));
            }
        }
    }
    let sequences: Vec<Vec<Unit>> = plans.iter().map(sequences).collect();
    let elements = plans.iter().zip(&sequences).map(|(p, s)| p.blocks.len() + s.iter().filter(|u| u.kind != Kind::Artifact).count()).sum::<usize>();
    if elements.saturating_add(annots.len()) > MAX_ELEMENTS {
        return Err(AutotagError::TooLarge);
    }
    if sequences.iter().all(Vec::is_empty) && annots.is_empty() {
        return Err(AutotagError::NothingToTag);
    }
    let levels = heading_levels(&plans);

    let root = doc.add(Object::Null);
    let document = doc.add(Object::Null);
    let mut kids: Vec<Object> = Vec::new();
    // Parent tree: key → (page MCIDs → element) or the annotation's element.
    let mut parent_tree: Vec<(i64, Object)> = Vec::new();
    let mut next_key: i64 = 0;
    let mut annot_iter = annots.iter().peekable();
    let mut tabs: HashSet<ObjRef> = HashSet::new();
    let mut plan_iter = plans.iter().zip(&sequences).zip(&levels).peekable();
    for (index, page) in pages.iter().enumerate() {
        if let Some(((plan, seqs), levels)) = plan_iter.next_if(|((p, _), _)| p.index == index) {
            let mut mcid = plan.first_mcid;
            let mut owners: Vec<Option<ObjRef>> = Vec::new();
            // Elements of this page, by text block and in order of appearance.
            let mut block_elems: HashMap<usize, (ObjRef, Vec<i64>)> = HashMap::new();
            let mut order: Vec<ObjRef> = Vec::new();
            // Operator → what to insert before (opening) and after (closing) it.
            let mut open: HashMap<usize, Op> = HashMap::new();
            let mut close: HashMap<usize, usize> = HashMap::new();
            for s in seqs {
                let tag = match s.kind {
                    Kind::Artifact => {
                        open.insert(s.start, Op::new("BMC", vec![Object::name("Artifact")]));
                        *close.entry(s.end).or_default() += 1;
                        summary.artifacts += 1;
                        continue;
                    }
                    Kind::Text(b) => match levels.get(b).copied().flatten() {
                        Some(level) => format!("H{level}"),
                        None => "P".to_owned(),
                    },
                    Kind::FormText => "P".to_owned(),
                    Kind::Figure => "Figure".to_owned(),
                };
                let id = mcid;
                mcid = mcid.saturating_add(1);
                let mut props = Dict::new();
                props.set(b"MCID".to_vec(), Object::Int(id));
                open.insert(s.start, Op::new("BDC", vec![Object::name(&tag), Object::Dict(props)]));
                *close.entry(s.end).or_default() += 1;
                let elem = match s.kind {
                    Kind::Text(b) => match block_elems.get_mut(&b) {
                        Some((r, ids)) => {
                            ids.push(id);
                            *r
                        }
                        None => {
                            let r = doc.add(Object::Null);
                            block_elems.insert(b, (r, vec![id]));
                            order.push(r);
                            if tag.starts_with('H') {
                                summary.headings += 1;
                            } else {
                                summary.paragraphs += 1;
                            }
                            r
                        }
                    },
                    _ => {
                        let r = doc.add(element(&tag, document, plan.obj, Object::Int(id)));
                        order.push(r);
                        if s.kind == Kind::Figure {
                            summary.figures += 1;
                        } else {
                            summary.paragraphs += 1;
                        }
                        r
                    }
                };
                let slot = usize::try_from(id).unwrap_or(0);
                if owners.len() <= slot {
                    owners.resize(slot + 1, None);
                }
                if let Some(o) = owners.get_mut(slot) {
                    *o = Some(elem);
                }
            }
            // Text block elements, now that their MCIDs are known.
            for (b, (r, ids)) in &block_elems {
                let tag = match levels.get(*b).copied().flatten() {
                    Some(level) => format!("H{level}"),
                    None => "P".to_owned(),
                };
                doc.set(*r, element(&tag, document, plan.obj, mcid_kids(ids)));
            }
            if !seqs.is_empty() {
                let mut ops = Vec::with_capacity(plan.ops.len() + seqs.len() * 2);
                for (i, op) in plan.ops.iter().enumerate() {
                    if let Some(o) = open.get(&i) {
                        ops.push(o.clone());
                    }
                    ops.push(op.clone());
                    for _ in 0..close.get(&i).copied().unwrap_or(0) {
                        ops.push(Op::new("EMC", Vec::new()));
                    }
                }
                let content = doc.add(Object::Stream(Stream::flate(Dict::new(), &prckopis_content::serialize_ops(&ops))));
                let key = (!owners.is_empty()).then_some(next_key);
                doc.update_dict(plan.obj, |p| {
                    p.set(b"Contents".to_vec(), Object::Ref(content));
                    if let Some(k) = key {
                        p.set(b"StructParents".to_vec(), Object::Int(k));
                    }
                })?;
                if let Some(k) = key {
                    parent_tree.push((k, Object::Array(owners.iter().map(|o| o.map_or(Object::Null, Object::Ref)).collect())));
                    next_key += 1;
                }
                summary.pages += 1;
            }
            kids.extend(order.into_iter().map(Object::Ref));
        }
        // The page's annotations, after its content.
        while let Some((_, r, kind)) = annot_iter.next_if(|(p, ..)| *p == index) {
            let mut objr = Dict::new();
            objr.set(b"Type".to_vec(), Object::name("OBJR"));
            objr.set(b"Obj".to_vec(), Object::Ref(*r));
            objr.set(b"Pg".to_vec(), Object::Ref(page.obj));
            let elem = doc.add(element(kind, document, page.obj, Object::Dict(objr)));
            let key = next_key;
            next_key += 1;
            doc.update_dict(*r, |a| a.set(b"StructParent".to_vec(), Object::Int(key)))?;
            parent_tree.push((key, Object::Ref(elem)));
            kids.push(Object::Ref(elem));
            tabs.insert(page.obj);
            summary.annotations += 1;
        }
    }
    for p in tabs {
        doc.update_dict(p, |d| d.set(b"Tabs".to_vec(), Object::name("S")))?;
    }
    let mut doc_elem = Dict::new();
    doc_elem.set(b"Type".to_vec(), Object::name("StructElem"));
    doc_elem.set(b"S".to_vec(), Object::name("Document"));
    doc_elem.set(b"P".to_vec(), Object::Ref(root));
    doc_elem.set(b"K".to_vec(), Object::Array(kids));
    doc.set(document, Object::Dict(doc_elem));
    parent_tree.sort_by_key(|(k, _)| *k);
    let mut nums = Vec::with_capacity(parent_tree.len() * 2);
    for (k, v) in parent_tree {
        nums.push(Object::Int(k));
        nums.push(v);
    }
    let mut tree = Dict::new();
    tree.set(b"Nums".to_vec(), Object::Array(nums));
    let tree = doc.add(Object::Dict(tree));
    let mut root_dict = Dict::new();
    root_dict.set(b"Type".to_vec(), Object::name("StructTreeRoot"));
    root_dict.set(b"K".to_vec(), Object::Ref(document));
    root_dict.set(b"ParentTree".to_vec(), Object::Ref(tree));
    root_dict.set(b"ParentTreeNextKey".to_vec(), Object::Int(next_key));
    doc.set(root, Object::Dict(root_dict));
    // MarkInfo: kept (with its other entries) and marked.
    let catalog = doc.get(catalog_ref).as_dict().cloned().unwrap_or_default();
    match catalog.get(b"MarkInfo") {
        Some(Object::Ref(m)) if doc.get(*m).as_dict().is_some() => doc.update_dict(*m, |d| d.set(b"Marked".to_vec(), Object::Bool(true)))?,
        other => {
            let mut info = other.and_then(Object::as_dict).cloned().unwrap_or_default();
            info.set(b"Marked".to_vec(), Object::Bool(true));
            doc.update_dict(catalog_ref, |c| c.set(b"MarkInfo".to_vec(), Object::Dict(info)))?;
        }
    }
    doc.update_dict(catalog_ref, |c| c.set(b"StructTreeRoot".to_vec(), Object::Ref(root)))?;
    if !summary.skipped_pages.is_empty() {
        let list: Vec<String> = summary.skipped_pages.iter().map(|p| (p + 1).to_string()).collect();
        doc.note_repair(format!("Autotag left page(s) {} untagged: their content could not be read completely", list.join(", ")));
    }
    Ok(summary)
}
