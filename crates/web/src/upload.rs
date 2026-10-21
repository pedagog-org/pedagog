//! Submitting from the browser: files picked or dropped (folders included,
//! by their relative paths), checked against the assignment's required files
//! and size before sending, then sent as one multipart upload with progress.
//! The server applies the full rules and has the last word.

use std::rc::Rc;

use pedagog_core::api::{SubmitReply, SubmitRules};
use pedagog_core::recipe::submission::is_junk;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::Closure;

use crate::api::{Problem, message_of};

/// A file to send, by its path within the submission.
#[derive(Clone)]
pub struct Picked {
    pub path: String,
    pub size: u64,
    pub file: Rc<web_sys::File>,
}

/// What the browser can tell before sending.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Check {
    pub total: u64,
    /// Required paths not among the files.
    pub missing: Vec<String>,
    pub over_limit: bool,
    /// One archive to unpack: only the server can look inside.
    pub archive: bool,
}

impl Check {
    pub fn ok(&self) -> bool {
        self.missing.is_empty() && !self.over_limit
    }
}

const ARCHIVES: [&str; 3] = [".zip", ".tar.gz", ".tgz"];

/// The paths as intake will see them: clutter dropped, and one folder
/// wrapping everything stripped unless the required paths already fit.
fn as_submitted(paths: &[&str], rules: &SubmitRules) -> Vec<String> {
    let kept: Vec<&str> = paths.iter().copied().filter(|p| !is_junk(p)).collect();
    let fits =
        !rules.required.is_empty() && rules.required.iter().all(|r| kept.contains(&r.as_str()));
    let wrapper = kept
        .iter()
        .map(|p| p.split_once('/').map(|(first, _)| first))
        .try_fold(None::<&str>, |seen, first| match (seen, first?) {
            (None, f) => Some(Some(f)),
            (Some(s), f) if s == f => Some(Some(s)),
            _ => None,
        })
        .flatten();
    match wrapper {
        Some(w) if rules.strip_wrapper && !fits => {
            kept.iter().map(|p| p[w.len() + 1..].to_owned()).collect()
        }
        _ => kept.iter().map(|p| (*p).to_owned()).collect(),
    }
}

/// Check `(path, size)` files against the rules.
pub fn check(files: &[(String, u64)], rules: &SubmitRules) -> Check {
    let paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
    let total = files
        .iter()
        .filter(|(p, _)| !is_junk(p))
        .map(|(_, s)| s)
        .sum();
    let archive = rules.unpack_archives
        && matches!(paths.as_slice(), [only] if ARCHIVES.iter().any(|s| only.to_ascii_lowercase().ends_with(s)));
    if archive {
        return Check {
            total,
            archive,
            ..Default::default()
        };
    }
    let submitted = as_submitted(&paths, rules);
    Check {
        total,
        missing: rules
            .required
            .iter()
            .filter(|r| !submitted.contains(r))
            .cloned()
            .collect(),
        over_limit: rules.max_bytes.is_some_and(|max| total > max),
        archive,
    }
}

/// Files from an `<input type=file>` (with `webkitdirectory`, by relative path).
pub fn from_input(input: &web_sys::HtmlInputElement) -> Vec<Picked> {
    let Some(list) = input.files() else {
        return Vec::new();
    };
    (0..list.length())
        .filter_map(|i| list.item(i))
        .map(|f| {
            let relative = js_sys::Reflect::get(&f, &"webkitRelativePath".into())
                .ok()
                .and_then(|v| v.as_string())
                .filter(|p| !p.is_empty());
            Picked {
                path: relative.unwrap_or_else(|| f.name()),
                size: f.size() as u64,
                file: Rc::new(f),
            }
        })
        .collect()
}

/// Files dropped on the page, walking any dropped folders.
pub async fn from_drop(event: &web_sys::DragEvent) -> Vec<Picked> {
    let Some(items) = event.data_transfer().map(|d| d.items()) else {
        return Vec::new();
    };
    let entries: Vec<web_sys::FileSystemEntry> = (0..items.length())
        .filter_map(|i| items.get(i))
        .filter_map(|item| item.webkit_get_as_entry().ok().flatten())
        .collect();
    let mut out = Vec::new();
    for e in entries {
        walk(e, String::new(), &mut out).await;
    }
    out
}

async fn walk(entry: web_sys::FileSystemEntry, prefix: String, out: &mut Vec<Picked>) {
    let mut stack = vec![(entry, prefix)];
    while let Some((entry, prefix)) = stack.pop() {
        let path = format!("{prefix}{}", entry.name());
        if entry.is_file() {
            let file_entry: web_sys::FileSystemFileEntry = entry.unchecked_into();
            if let Some(f) = file_of(&file_entry).await {
                out.push(Picked {
                    path,
                    size: f.size() as u64,
                    file: Rc::new(f),
                });
            }
        } else if entry.is_directory() {
            let dir: web_sys::FileSystemDirectoryEntry = entry.unchecked_into();
            let reader = dir.create_reader();
            // Entries come in batches until an empty one.
            loop {
                let batch = entries_of(&reader).await;
                if batch.is_empty() {
                    break;
                }
                for child in batch {
                    stack.push((child, format!("{path}/")));
                }
            }
        }
    }
}

async fn file_of(entry: &web_sys::FileSystemFileEntry) -> Option<web_sys::File> {
    let (done, wait) = futures::channel::oneshot::channel();
    let done = Rc::new(std::cell::Cell::new(Some(done)));
    let ok = {
        let done = done.clone();
        Closure::once(move |f: web_sys::File| {
            if let Some(d) = done.take() {
                let _ = d.send(Some(f));
            }
        })
    };
    let failed = Closure::once(move |_: wasm_bindgen::JsValue| {
        if let Some(d) = done.take() {
            let _ = d.send(None);
        }
    });
    entry.file_with_callback_and_callback(
        ok.as_ref().unchecked_ref(),
        failed.as_ref().unchecked_ref(),
    );
    wait.await.ok().flatten()
}

async fn entries_of(reader: &web_sys::FileSystemDirectoryReader) -> Vec<web_sys::FileSystemEntry> {
    let (done, wait) = futures::channel::oneshot::channel();
    let done = Rc::new(std::cell::Cell::new(Some(done)));
    let ok = {
        let done = done.clone();
        Closure::once(move |list: js_sys::Array| {
            if let Some(d) = done.take() {
                let _ = d.send(list.iter().map(|e| e.unchecked_into()).collect());
            }
        })
    };
    let failed = Closure::once(move |_: wasm_bindgen::JsValue| {
        if let Some(d) = done.take() {
            let _ = d.send(Vec::new());
        }
    });
    let _ = reader.read_entries_with_callback_and_callback(
        ok.as_ref().unchecked_ref(),
        failed.as_ref().unchecked_ref(),
    );
    wait.await.unwrap_or_default()
}

/// Send the files to `path` (under `/api`), reporting the share uploaded
/// (0.0–1.0) as it goes.
pub async fn send(
    path: &str,
    files: &[Picked],
    progress: impl Fn(f64) + 'static,
) -> Result<SubmitReply, Problem> {
    let unreachable = |_| Problem::unreachable();
    let form = web_sys::FormData::new().map_err(unreachable)?;
    for f in files {
        form.append_with_blob_and_filename("file", &f.file, &f.path)
            .map_err(unreachable)?;
    }
    let xhr = web_sys::XmlHttpRequest::new().map_err(unreachable)?;
    xhr.open("POST", &crate::api::url(path))
        .map_err(unreachable)?;
    xhr.set_request_header("X-Pedagog", "web")
        .map_err(unreachable)?;

    let (done, wait) = futures::channel::oneshot::channel::<bool>();
    let done = Rc::new(std::cell::Cell::new(Some(done)));
    let finished = |ok: bool| {
        let done = done.clone();
        Closure::<dyn FnMut()>::new(move || {
            if let Some(d) = done.take() {
                let _ = d.send(ok);
            }
        })
    };
    let on_load = finished(true);
    let on_error = finished(false);
    let on_progress =
        Closure::<dyn FnMut(web_sys::ProgressEvent)>::new(move |e: web_sys::ProgressEvent| {
            if e.length_computable() && e.total() > 0.0 {
                progress(e.loaded() / e.total());
            }
        });
    xhr.set_onload(Some(on_load.as_ref().unchecked_ref()));
    xhr.set_onerror(Some(on_error.as_ref().unchecked_ref()));
    if let Ok(upload) = xhr.upload() {
        upload.set_onprogress(Some(on_progress.as_ref().unchecked_ref()));
    }
    xhr.send_with_opt_form_data(Some(&form))
        .map_err(unreachable)?;
    let loaded = wait.await.unwrap_or(false);
    if !loaded {
        return Err(Problem::unreachable());
    }
    let status = xhr.status().unwrap_or(0);
    let body = xhr.response_text().ok().flatten().unwrap_or_default();
    if (200..300).contains(&status) {
        serde_json::from_str(&body).map_err(|e| Problem {
            status,
            message: format!("an unexpected reply from the server ({e})"),
        })
    } else {
        let problem = Problem {
            status,
            message: message_of(status, &body),
        };
        if problem.is_unauthorized() {
            crate::session::expired();
        }
        Err(problem)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(required: &[&str]) -> SubmitRules {
        SubmitRules {
            required: required.iter().map(|s| (*s).to_owned()).collect(),
            max_bytes: Some(10_000),
            unpack_archives: true,
            strip_wrapper: true,
        }
    }

    fn files(list: &[(&str, u64)]) -> Vec<(String, u64)> {
        list.iter().map(|(p, s)| ((*p).to_owned(), *s)).collect()
    }

    #[test]
    fn required_files_are_checked_as_intake_will_see_them() {
        let r = rules(&["linked_list.py"]);
        let c = check(&files(&[("linkedlist.py", 6_200), ("node.py", 1_100)]), &r);
        assert_eq!(
            (c.missing, c.total),
            (vec!["linked_list.py".to_owned()], 7_300)
        );

        // A wrapping folder (as picked with "Choose folder") is stripped.
        let c = check(
            &files(&[
                ("hw3/linked_list.py", 10),
                ("hw3/node.py", 10),
                ("hw3/.DS_Store", 99),
            ]),
            &r,
        );
        assert!(c.ok(), "{c:?}");
        assert_eq!(c.total, 20, "clutter doesn't count");

        // ... unless the required paths need it.
        let nested = rules(&["src/main.c"]);
        assert!(check(&files(&[("src/main.c", 10)]), &nested).ok());
        let off = SubmitRules {
            strip_wrapper: false,
            ..r.clone()
        };
        assert!(!check(&files(&[("hw3/linked_list.py", 10)]), &off).ok());
    }

    #[test]
    fn sizes_over_the_limit_and_archives_are_said() {
        let r = rules(&["a.py"]);
        assert!(check(&files(&[("a.py", 20_000)]), &r).over_limit);
        let zipped = check(&files(&[("hw3.ZIP", 500)]), &r);
        assert!(zipped.archive && zipped.ok(), "the server looks inside");
        let kept = SubmitRules {
            unpack_archives: false,
            ..r
        };
        assert!(!check(&files(&[("hw3.zip", 500)]), &kept).archive);
    }
}
