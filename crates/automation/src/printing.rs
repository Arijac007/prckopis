//! Printing tools: list printers, and print (or save the print-ready PDF) with Acrobat's Print
//! dialog options; find network printers and print to them directly (IPP, LPD, raw socket),
//! and follow, release or cancel their jobs.

use pdfcraft_engine::print::{self, Binding, BookletSubset, Content, Layout, Orientation, PAPERS, PageOrder, SizeMode, Subset, net, spool};
use serde_json::{Value, json};

use crate::{Args, Automation, Result, ToolError, failed, write_atomic};

fn bad(m: impl Into<String>) -> ToolError {
    ToolError::InvalidArgs(m.into())
}

impl Automation {
    pub(crate) fn printers(&self) -> Result<Value> {
        let list: Vec<Value> = spool::printers()
            .into_iter()
            .map(|p| {
                let mut v = json!({ "name": p.name, "default": p.default });
                if let Some(two_sided) = p.two_sided {
                    v["two_sided"] = json!(two_sided);
                }
                v
            })
            .collect();
        Ok(json!({ "count": list.len(), "printers": list }))
    }

    pub(crate) fn printer_options(&self, a: &Args) -> Result<Value> {
        let printer = a.str("printer")?;
        let list: Vec<Value> = spool::printer_options(printer)
            .into_iter()
            .map(|o| {
                let choices: Vec<Value> = o.choices.iter().map(|(value, label)| json!({ "value": value, "label": label })).collect();
                json!({ "key": o.key, "label": o.label, "group": o.group, "default": o.default, "choices": choices })
            })
            .collect();
        Ok(json!({ "printer": printer, "count": list.len(), "options": list }))
    }

    pub(crate) fn doc_print(&mut self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let (id, count) = (doc.id, doc.info.pages.len());
        let labels: Vec<String> = doc.info.pages.iter().map(|p| p.label.clone()).collect();
        let subset = match a.opt_str("subset")?.unwrap_or("all") {
            "all" => Subset::All,
            "odd" => Subset::Odd,
            "even" => Subset::Even,
            s => return Err(bad(format!("unknown subset {s:?}"))),
        };
        let pages = print::select_pages(count, a.opt_str("pages")?, &labels, subset, a.opt_bool("reverse")?.unwrap_or(false))
            .map_err(|e| bad(e.to_string()))?;
        let layout = match a.opt_str("layout")?.unwrap_or("fit") {
            "fit" => Layout::Size(SizeMode::Fit),
            "actual" => Layout::Size(SizeMode::Actual),
            "shrink" => Layout::Size(SizeMode::Shrink),
            "custom" => Layout::Size(SizeMode::Custom(a.opt_num("scale")?.ok_or_else(|| bad("custom needs scale (percent)"))?)),
            "multiple" => {
                let per = a.opt_int("per_sheet")?.unwrap_or(2).clamp(1, 256) as usize;
                let order = match a.opt_str("order")?.unwrap_or("horizontal") {
                    "horizontal" => PageOrder::Horizontal,
                    "horizontal-reversed" => PageOrder::HorizontalReversed,
                    "vertical" => PageOrder::Vertical,
                    "vertical-reversed" => PageOrder::VerticalReversed,
                    "cut-stack" => PageOrder::CutStack,
                    o => return Err(bad(format!("unknown order {o:?}"))),
                };
                match Layout::multiple(per) {
                    Layout::Multiple { cols, rows, .. } => Layout::Multiple {
                        cols,
                        rows,
                        order,
                        border: a.opt_bool("border")?.unwrap_or(false),
                        auto_rotate: a.opt_bool("auto_rotate")?.unwrap_or(true),
                    },
                    l => l,
                }
            }
            "booklet" => Layout::Booklet {
                subset: match a.opt_str("booklet_subset")?.unwrap_or("both") {
                    "both" => BookletSubset::BothSides,
                    "front" => BookletSubset::FrontOnly,
                    "back" => BookletSubset::BackOnly,
                    s => return Err(bad(format!("unknown booklet_subset {s:?}"))),
                },
                binding: if a.opt_str("binding")? == Some("right") { Binding::Right } else { Binding::Left },
            },
            "poster" => Layout::Poster {
                scale: a.opt_num("scale")?.unwrap_or(200.0),
                overlap: a.opt_num("overlap")?.unwrap_or(18.0),
                cut_marks: a.opt_bool("cut_marks")?.unwrap_or(true),
            },
            l => return Err(bad(format!("unknown layout {l:?}"))),
        };
        if matches!(layout, Layout::Multiple { order: PageOrder::CutStack, .. }) && a.opt_str("duplex")?.unwrap_or("off") != "off" {
            return Err(bad("cut-and-stack printing needs duplex off (single-sided sheets)"));
        }
        if a.opt_str("order")? == Some("cut-stack") && !matches!(layout, Layout::Multiple { .. }) {
            return Err(bad("cut-stack order needs layout multiple"));
        }
        let orientation = match a.opt_str("orientation")?.unwrap_or("auto") {
            "auto" => Orientation::Auto,
            "portrait" => Orientation::Portrait,
            "landscape" => Orientation::Landscape,
            o => return Err(bad(format!("unknown orientation {o:?}"))),
        };
        let content = match a.opt_str("comments_forms")?.unwrap_or("document-and-markups") {
            "document" => Content::Document,
            "document-and-markups" => Content::DocumentAndMarkups,
            "document-and-stamps" => Content::DocumentAndStamps,
            "form-fields-only" => Content::FormFieldsOnly,
            c => return Err(bad(format!("unknown comments_forms {c:?}"))),
        };
        let paper = match a.opt_str("paper")? {
            None => PAPERS[0].1,
            Some(p) => PAPERS
                .iter()
                .find(|(n, _)| {
                    n.eq_ignore_ascii_case(p)
                        || n.replace("US ", "").eq_ignore_ascii_case(p)
                        || n.split(" (").next().is_some_and(|name| name.eq_ignore_ascii_case(p))
                })
                .map(|x| x.1)
                .ok_or_else(|| {
                    bad(format!("unknown paper {p:?}; choose one of {}", PAPERS.iter().map(|(name, _)| *name).collect::<Vec<_>>().join(", ")))
                })?,
        };
        let settings = print::Settings { pages, paper, orientation, layout, content };
        let sizes: Vec<(f64, f64)> = self.doc(a)?.info.pages.iter().map(|p| (p.width as f64, p.height as f64)).collect();
        let sheets = print::layout(&sizes, &settings).map_err(|e| bad(e.to_string()))?.len();
        let bytes = self.session.print_pdf(id, &settings).map_err(failed)?;
        let mut out = json!({ "sheets": sheets, "pages": settings.pages.len(), "bytes": bytes.len() });
        if a.opt_str("uri")?.is_some() && (a.opt_str("path")?.is_some() || a.opt_str("printer")?.is_some()) {
            return Err(bad("pass only one of path, printer and uri"));
        }
        match (a.opt_str("path")?, a.opt_str("printer")?) {
            (Some(p), None) => {
                let path = self.resolve(p, true)?;
                write_atomic(&path, &bytes)?;
                out["path"] = json!(path.to_string_lossy());
            }
            (None, Some(printer)) => {
                let name = self.doc(a)?.name.clone();
                let job = spool::Job {
                    printer: if printer == "default" { None } else { Some(printer.to_string()) },
                    copies: a.opt_int("copies")?.unwrap_or(1).clamp(1, 999) as u32,
                    collate: a.opt_bool("collate")?.unwrap_or(true),
                    duplex: Some(match a.opt_str("duplex")?.unwrap_or("off") {
                        "off" => spool::Duplex::Off,
                        "long-edge" => spool::Duplex::LongEdge,
                        "short-edge" => spool::Duplex::ShortEdge,
                        d => return Err(bad(format!("unknown duplex {d:?}"))),
                    }),
                    scaling: spool::Scaling::Exact,
                    grayscale: a.opt_bool("grayscale")?.unwrap_or(false),
                    title: name,
                    options: match a.get("options") {
                        None => Vec::new(),
                        Some(Value::Object(m)) => m
                            .iter()
                            .map(|(k, v)| match v.as_str() {
                                Some(v) => Ok((k.clone(), v.to_string())),
                                None => Err(bad(format!("options.{k} must be a string (a choice from printer_options)"))),
                            })
                            .collect::<Result<Vec<_>>>()?,
                        Some(_) => return Err(bad("options must be an object of option keys and choices from printer_options")),
                    },
                };
                out["job"] = json!(spool::submit(&bytes, &job).map_err(|e| failed(e.to_string()))?);
            }
            (None, None) if a.opt_str("uri")?.is_some() => {
                let target = target(a)?;
                let job = net_job(a, self.doc(a)?.name.clone())?;
                let r = net::submit(&target, &bytes, &job).map_err(|e| failed(e.to_string()))?;
                out["uri"] = json!(target.uri());
                out["job"] = receipt(&r);
            }
            _ => {
                return Err(bad(
                    "pass one of path (save the print-ready PDF), printer (a name from printers, or \"default\") or uri (a network printer from net_printers, e.g. ipp://10.0.0.5/ipp/print)",
                ));
            }
        }
        Ok(out)
    }

    pub(crate) fn net_printers(&self, a: &Args) -> Result<Value> {
        let seconds = a.opt_num("seconds")?.unwrap_or(2.0);
        if !(0.2..=30.0).contains(&seconds) {
            return Err(bad("seconds must be between 0.2 and 30"));
        }
        let found = net::discover_with(std::time::Duration::from_secs_f64(seconds)).map_err(|e| failed(e.to_string()))?;
        let list: Vec<Value> = found
            .iter()
            .map(|f| {
                json!({
                    "name": f.name,
                    "uri": f.uri(),
                    "protocol": f.protocol.scheme(),
                    "host": f.host,
                    "address": f.address.map(|a| a.to_string()),
                    "port": f.port,
                    "make_and_model": f.make_and_model,
                    "location": f.location,
                    "pdf": f.pdf,
                    "color": f.color,
                    "duplex": f.duplex,
                    "direct_printing": f.protocol != net::Protocol::Ipps,
                })
            })
            .collect();
        Ok(json!({ "count": list.len(), "printers": list }))
    }

    pub(crate) fn net_printer_status(&self, a: &Args) -> Result<Value> {
        let target = target(a)?;
        let s = net::status(&target).map_err(|e| failed(e.to_string()))?;
        let supplies: Vec<Value> =
            s.supplies.iter().map(|(name, level)| json!({ "name": name, "percent": (*level >= 0).then_some(*level) })).collect();
        Ok(json!({
            "uri": target.uri(),
            "name": s.name,
            "make_and_model": s.make_and_model,
            "location": s.location,
            "state": s.state,
            "state_reasons": s.state_reasons,
            "state_message": s.state_message,
            "accepting_jobs": s.accepting_jobs,
            "queued_jobs": s.queued_jobs,
            "document_formats": s.document_formats,
            "takes_pdf": s.takes_pdf(),
            "sides": s.sides,
            "color": s.color,
            "supplies": supplies,
        }))
    }

    pub(crate) fn net_jobs(&self, a: &Args) -> Result<Value> {
        let target = target(a)?;
        let completed = match a.opt_str("which")?.unwrap_or("not-completed") {
            "not-completed" => false,
            "completed" => true,
            w => return Err(bad(format!("unknown which {w:?}; use not-completed or completed"))),
        };
        let list = net::jobs(&target, completed, a.opt_str("user")?).map_err(|e| failed(e.to_string()))?;
        let list: Vec<Value> = list.iter().map(job_json).collect();
        Ok(json!({ "uri": target.uri(), "count": list.len(), "jobs": list }))
    }

    pub(crate) fn net_job_control(&self, a: &Args, release: bool) -> Result<Value> {
        let target = target(a)?;
        let id = a.int("job_id")?;
        let id = i32::try_from(id).ok().filter(|n| *n > 0).ok_or_else(|| bad("job_id must be a positive job number from net_jobs"))?;
        let user = a.opt_str("user")?.map(str::to_string).unwrap_or_else(net::default_user);
        if release { net::release_job(&target, id, &user) } else { net::cancel_job(&target, id, &user) }.map_err(|e| failed(e.to_string()))?;
        Ok(json!({ "uri": target.uri(), "job_id": id, "done": if release { "released" } else { "canceled" } }))
    }
}

fn target(a: &Args) -> Result<net::Target> {
    net::Target::parse(a.str("uri")?).map_err(|e| bad(e.to_string()))
}

fn job_json(j: &net::JobStatus) -> Value {
    json!({ "job_id": j.job_id, "name": j.name, "user": j.user, "state": j.state, "state_reasons": j.state_reasons, "sheets_completed": j.sheets_completed })
}

fn receipt(r: &net::Receipt) -> Value {
    json!({ "protocol": r.protocol.scheme(), "job_id": r.job_id, "job_uri": r.job_uri, "state": r.state, "ignored": r.ignored, "bytes": r.bytes })
}

/// The network job asked for by doc_print's arguments.
fn net_job(a: &Args, title: String) -> Result<net::NetJob> {
    let finishings = match a.get("finishings") {
        None => Vec::new(),
        Some(Value::Array(list)) => list
            .iter()
            .map(|v| v.as_str().and_then(net::Finishing::parse).ok_or_else(|| bad("finishings are \"staple\", \"punch\" or \"fold\"")))
            .collect::<Result<Vec<_>>>()?,
        Some(_) => return Err(bad("finishings must be a list such as [\"staple\"]")),
    };
    let job = net::NetJob {
        copies: a.opt_int("copies")?.unwrap_or(1).clamp(1, 999) as u32,
        collate: a.opt_bool("collate")?.unwrap_or(true),
        duplex: match a.opt_str("duplex")? {
            None => None,
            Some("off") => Some(spool::Duplex::Off),
            Some("long-edge") => Some(spool::Duplex::LongEdge),
            Some("short-edge") => Some(spool::Duplex::ShortEdge),
            Some(d) => return Err(bad(format!("unknown duplex {d:?}"))),
        },
        color: match (a.opt_str("color")?, a.opt_bool("grayscale")?) {
            (Some("color"), _) => net::ColorMode::Color,
            (Some("monochrome"), _) | (None, Some(true)) => net::ColorMode::Monochrome,
            (Some("auto") | None, _) => net::ColorMode::Auto,
            (Some(c), _) => return Err(bad(format!("unknown color {c:?}; use auto, color or monochrome"))),
        },
        title,
        user: a.opt_str("user")?.map(str::to_string).unwrap_or_else(net::default_user),
        hold: a.opt_bool("hold")?.unwrap_or(false),
        pin: a.opt_str("pin")?.map(str::to_string),
        account: a.opt_str("account")?.map(str::to_string),
        finishings,
        pjl: a.opt_bool("pjl")?.unwrap_or(true),
    };
    job.validate().map_err(|e| bad(e.to_string()))?;
    Ok(job)
}
