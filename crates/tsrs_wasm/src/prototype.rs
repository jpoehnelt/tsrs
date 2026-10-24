// Experimental synchronous, transport-free API session in one WASM instance.
// No stdio server, filesystem watcher, background worker, or external code execution.
use std::cell::RefCell;
use std::sync::Arc;
use tsrs_api::wire::{Obj, Params};
use tsrs_api::{ApiError, ApiResult, Handler, Response, Session, SessionOptions};
use tsrs_core::context::Context;
use tsrs_core::json::{self, Value};
use tsrs_lsproto::{self as lsp, Json};
use tsrs_vfs::bundled;

thread_local! {
    static SESSION: RefCell<Option<Arc<Session>>> = const { RefCell::new(None) };
}

fn session() -> ApiResult<Arc<Session>> {
    SESSION.with_borrow(Clone::clone).ok_or_else(|| ApiError::client("session is closed"))
}

/// An embedding consumer can serve native operations without copying compiler data through JS.
pub type RequestExtension = fn(&Session, &str, &Value) -> Option<ApiResult<Value>>;
#[path = "binary.rs"]
pub mod binary;
fn open_session(binary: bool) -> ApiResult<Value> {
    if SESSION.with_borrow(Option::is_some) {
        return Err(ApiError::client("session already open"));
    }
    tsrs_compiler::worker_pool();
    let s = Session::new_with_position_encoding(
        SessionOptions::new(
            "/".into(),
            bundled::lib_path(),
            Arc::new(bundled::wrap_fs(crate::host::HostFs::new(true))),
            binary,
        ),
        lsp::PositionEncodingKind::UTF16,
    );
    let id = s.id().to_string();
    SESSION.with_borrow_mut(|slot| *slot = Some(s));
    Ok(Value::String(id))
}
fn call(request: &Value, extension: Option<RequestExtension>) -> ApiResult<Value> {
    let request = Params(request);
    request.object()?;
    let method = request.str("method")?;
    let params = request.get("params");
    if method == "openSession" {
        if SESSION.with_borrow(Option::is_some) {
            return Err(ApiError::client("session already open"));
        }
        return open_session(false);
    }
    if method == "closeSession" {
        if let Some(s) = SESSION.with_borrow_mut(Option::take) {
            let cleanup =
                extension.and_then(|handler| handler(&s, "closeSession", params)).transpose();
            s.close();
            cleanup?;
        }
        return Ok(Value::Bool(true));
    }
    let s = session()?;
    if let Some(result) = extension.and_then(|handler| handler(&s, method, params)) {
        return result;
    }
    if method == "supportedCodeFixes" {
        return Ok(Value::Array(
            tsrs_ls::supported_code_fix_codes()
                .into_iter()
                .map(|code| Value::Number(f64::from(code)))
                .collect(),
        ));
    }
    if method == "collectIdle" {
        s.collect_idle_checkers();
        return Ok(Value::Bool(true));
    }
    if method == "editor" {
        return editor(&s, params);
    }
    if method == "textDocument/hover" {
        let p = Params(params);
        p.object()?;
        let data = s.snapshot_data(p.u64("snapshot")?)?;
        let project = p.str("project")?;
        let file = p.str("file")?;
        let program = data.get_program(&tsrs_project::ID(project.to_string()))?;
        if program.get_source_file(file).is_none() {
            return Err(ApiError::client("file is not in the snapshot"));
        }
        let host = Arc::clone(&data.snapshot);
        let host: Arc<dyn tsrs_ls::Host> = host;
        let ls = tsrs_ls::new_language_service(
            tsrs_ls::autoimport::ProjectID(project.to_string()),
            program,
            host,
            file,
        );
        let position = lsp::Position {
            line: p
                .u64("line")?
                .try_into()
                .map_err(|_| ApiError::invalid_request("line too large"))?,
            character: p
                .u64("character")?
                .try_into()
                .map_err(|_| ApiError::invalid_request("character too large"))?,
        };
        let mut caps = lsp::ResolvedClientCapabilities::default();
        caps.text_document.hover.content_format = vec![lsp::MarkupKind::MARKDOWN];
        let ctx = lsp::with_client_capabilities(&Context::background(), Arc::new(caps));
        let hover = ls
            .provide_hover(
                &ctx,
                &lsp::HoverParams {
                    text_document: lsp::TextDocumentIdentifier {
                        uri: tsrs_ls::lsconv::file_name_to_document_uri(file),
                    },
                    position,
                    ..Default::default()
                },
            )
            .map_err(|e| ApiError::internal(e.message))?;
        return Ok(hover.to_json());
    }
    let bytes = json::marshal(params).map_err(ApiError::invalid_request)?;
    match tsrs_api::with_compact_diagnostics(|| s.handle_request(method, bytes.as_bytes()))? {
        Response::Json(text) => json::unmarshal(&text).map_err(ApiError::internal),
        Response::Value(value) => Ok(value),
        Response::Binary(_) => Err(ApiError::internal("unexpected binary response")),
    }
}

pub fn dispatch(bytes: &[u8]) -> Vec<u8> {
    dispatch_inner(bytes, None)
}
pub fn dispatch_with_extension(bytes: &[u8], extension: RequestExtension) -> Vec<u8> {
    dispatch_inner(bytes, Some(extension))
}
fn dispatch_inner(bytes: &[u8], extension: Option<RequestExtension>) -> Vec<u8> {
    let result = std::str::from_utf8(bytes)
        .map_err(|e| ApiError::invalid_request(e.to_string()))
        .and_then(|text| json::unmarshal(text).map_err(ApiError::invalid_request))
        .and_then(|request| call(&request, extension));
    let envelope = match result {
        Ok(value) => Obj::new().set("result", value).build(),
        Err(error) => Obj::new().set("error", Value::String(error.to_string())).build(),
    };
    json::marshal(&envelope).unwrap().into_bytes()
}

fn decode<T: Json>(value: &Value) -> ApiResult<T> {
    T::from_json(value).map_err(|error| ApiError::invalid_request(error.to_string()))
}
fn editor(s: &Session, params: &Value) -> ApiResult<Value> {
    struct Prepared(Option<Arc<tsrs_project::Snapshot>>);
    impl Drop for Prepared {
        fn drop(&mut self) {
            if let Some(snapshot) = self.0.take() {
                snapshot.deref();
            }
        }
    }
    let p = Params(params);
    p.object()?;
    let data = s.snapshot_data(p.u64("snapshot")?)?;
    let project = p.str("project")?;
    let file = p.str("file")?;
    let operation = p.str("operation")?;
    let request = p.get("request");
    let requires_auto_imports = match operation {
        "codeFixes" => {
            let request: lsp::CodeActionParams = decode(request)?;
            request.context.diagnostics.iter().any(|diagnostic| {
                diagnostic
                    .code
                    .as_ref()
                    .and_then(|code| code.integer)
                    .is_none_or(tsrs_ls::code_fix_requires_auto_imports)
            })
        }
        "combinedCodeFix" => {
            tsrs_ls::combined_code_fix_requires_auto_imports(Params(request).str("fixId")?)
        }
        "codeActions" | "completion" | "completionResolve" => true,
        _ => false,
    };
    let prepared = Prepared(if requires_auto_imports {
        Some(s.embedded_auto_import_snapshot(&data.snapshot, file))
    } else {
        None
    });
    let snapshot = prepared.0.as_ref().unwrap_or(&data.snapshot);
    let project_data = snapshot
        .project_collection
        .get_project(&tsrs_project::ID(project.into()))
        .ok_or_else(|| ApiError::client("missing prepared project"))?;
    let program =
        project_data.get_program().ok_or_else(|| ApiError::client("missing prepared program"))?;
    let source =
        program.get_source_file(file).ok_or_else(|| ApiError::client("file is not in snapshot"))?;
    let host = Arc::clone(snapshot);
    let host: Arc<dyn tsrs_ls::Host> = host;
    let mut ls = tsrs_ls::new_language_service(
        tsrs_ls::autoimport::ProjectID(project.into()),
        program,
        host,
        file,
    );
    ls.set_legacy_code_fix_edits(p.bool("legacyCodeFixEdits")?);
    let mut preferences = tsrs_ls::lsutil::new_default_user_preferences();
    preferences.unmarshal_json_from(p.get("preferences")).map_err(ApiError::invalid_request)?;
    ls.set_user_preferences(preferences);
    if !matches!(p.get("readOnlyFiles"), Value::Null) {
        ls.set_read_only_files(&decode::<Vec<String>>(p.get("readOnlyFiles"))?);
    }
    let mut caps = lsp::ResolvedClientCapabilities::default();
    caps.text_document.hover.content_format = vec![lsp::MarkupKind::MARKDOWN];
    caps.text_document.definition.link_support = true;
    caps.text_document.document_symbol.hierarchical_document_symbol_support = true;
    let ctx = lsp::with_client_capabilities(&Context::background(), Arc::new(caps));
    let uri = tsrs_ls::lsconv::file_name_to_document_uri(file);
    let position = || -> ApiResult<lsp::Position> { decode(Params(request).get("position")) };
    let result: Result<Value, lsp::Error> = match p.str("operation")? {
        "hover" => ls.provide_hover(&ctx, &decode(request)?).map(|v| v.to_json()),
        "quickInfo" => {
            let params: lsp::HoverParams = decode(request)?;
            ls.provide_quick_info(&ctx, &params).map(|v| {
                let mut value = v.to_json();
                if let Value::Object(fields) = &mut value {
                    let (documentation, tags) = ls.quick_info_documentation(&ctx, &params);
                    fields.insert("documentation", Value::String(documentation));
                    fields.insert(
                        "tags",
                        Value::Array(
                            tags.into_iter()
                                .map(|tag| {
                                    Obj::new()
                                        .set("name", Value::String(tag.name))
                                        .set("text", Value::String(tag.text))
                                        .build()
                                })
                                .collect(),
                        ),
                    );
                }
                value
            })
        }
        "completion" => {
            let r: lsp::CompletionParams = decode(request)?;
            ls.provide_completion(&ctx, &uri, r.position, r.context.as_ref()).map(|v| v.to_json())
        }
        "completionResolve" => {
            let item: lsp::CompletionItem = decode(request)?;
            let item_data: lsp::CompletionItemData = decode(Params(request).get("data"))?;
            ls.resolve_completion_item(&ctx, item, Some(&item_data)).map(|v| v.to_json())
        }
        "definition" => ls.provide_definition(&ctx, &uri, position()?).map(|v| v.to_json()),
        "references" => ls.provide_references(&ctx, &decode(request)?, None).map(|v| v.to_json()),
        "rename" => ls.provide_rename(&ctx, &decode(request)?, None).map(|v| v.to_json()),
        "renameInfo" => {
            let info = ls.get_rename_info(&ctx, "__embedded_rename__", &uri, position()?);
            Ok(Obj::new()
                .set("canRename", Value::Bool(info.can_rename))
                .set("localizedErrorMessage", Value::String(info.localized_error_message))
                .set("displayName", Value::String(info.display_name))
                .set("triggerRange", info.trigger_span.to_json())
                .build())
        }
        "signatureHelp" => {
            let position = position()?;
            ls.provide_signature_help(&ctx, &uri, position, None).map(|response| {
                let mut value = response.to_json();
                if let (Value::Object(fields), Some((index, count, range))) =
                    (&mut value, ls.embedded_signature_arguments(&ctx, &uri, position))
                {
                    fields.insert("argumentIndex".to_string(), Value::Number(f64::from(index)));
                    fields.insert("argumentCount".to_string(), Value::Number(f64::from(count)));
                    let map = source.get_position_map();
                    let start =
                        map.utf8_to_utf16(range.pos().expect_source("signature argument span"));
                    let end =
                        map.utf8_to_utf16(range.end().expect_source("signature argument span"));
                    fields.insert(
                        "applicableSpan".to_string(),
                        Obj::new()
                            .set("start", Value::Number(f64::from(start)))
                            .set("length", Value::Number(f64::from(end - start)))
                            .build(),
                    );
                }
                value
            })
        }
        "symbols" => ls.provide_document_symbols(&ctx, &uri).map(|v| v.to_json()),
        "format" => {
            let r: lsp::DocumentRangeFormattingParams = decode(request)?;
            ls.provide_format_document_range(&ctx, &uri, r.options, r.range).map(|v| v.to_json())
        }
        "inlayHints" => ls.provide_inlay_hint(&ctx, &decode(request)?).map(|v| v.to_json()),
        "codeActions" => ls.provide_code_actions(&ctx, &decode(request)?).map(|v| v.to_json()),
        "codeFixes" => ls.embedded_code_fixes(&ctx, &decode(request)?).map(|actions| {
            Value::Array(
                actions
                    .into_iter()
                    .map(|action| {
                        Obj::new()
                            .set("description", Value::String(action.description.clone()))
                            .set(
                                "fixName",
                                Value::String(action.fix_name.clone().unwrap_or_else(|| {
                                    if action.fix_id.is_empty() {
                                        action.description.clone()
                                    } else {
                                        action.fix_id.clone()
                                    }
                                })),
                            )
                            .set_opt(
                                "fixId",
                                (!action.fix_id.is_empty())
                                    .then(|| Value::String(action.fix_id.clone())),
                            )
                            .set_opt(
                                "fixAllDescription",
                                (!action.fix_all_description.is_empty())
                                    .then(|| Value::String(action.fix_all_description.clone())),
                            )
                            .set("edit", action.workspace_edit(&uri).to_json())
                            .build()
                    })
                    .collect(),
            )
        }),
        "combinedCodeFix" => {
            ls.embedded_combined_code_fix(&ctx, &uri, Params(request).str("fixId")?).map(|action| {
                action.map_or_else(
                    || workspace_edit(&uri, Vec::new()),
                    |action| action.workspace_edit(&uri).to_json(),
                )
            })
        }
        "refactors" => {
            let offset = |key| -> ApiResult<u32> {
                Params(request)
                    .u64(key)?
                    .try_into()
                    .map_err(|_| ApiError::invalid_request("refactor offset too large"))
            };
            let start = source.get_position_map().utf16_to_utf8(offset("start")?);
            let end = source.get_position_map().utf16_to_utf8(offset("end")?);
            ls.embedded_refactors(&ctx, &uri, start, end).map(|actions| {
                Value::Array(
                    actions
                        .into_iter()
                        .map(|action| {
                            Obj::new()
                                .set("name", Value::String(action.name.to_string()))
                                .set("description", Value::String(action.description))
                                .set("kind", Value::String(action.kind.to_string()))
                                .set("edit", workspace_edit(&uri, action.changes))
                                .build()
                        })
                        .collect(),
                )
            })
        }
        "docComment" => {
            let offset = Params(request)
                .u64("offset")?
                .try_into()
                .map_err(|_| ApiError::invalid_request("offset too large"))?;
            let internal = source.get_position_map().utf16_to_utf8(offset);
            Ok(ls.doc_comment_template(source, internal).map_or(Value::Null, Value::String))
        }
        _ => return Err(ApiError::invalid_request("unsupported editor operation")),
    };
    result.map_err(|error| ApiError::internal(error.message))
}

fn workspace_edit(uri: &lsp::DocumentUri, changes: Vec<lsp::TextEdit>) -> Value {
    let mut files = tsrs_core::collections::OrderedMap::default();
    files.insert(uri.clone(), changes);
    lsp::WorkspaceEdit { changes: Some(files), ..Default::default() }.to_json()
}

#[cfg(test)]
mod extension_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CLOSED: AtomicUsize = AtomicUsize::new(0);
    fn extension(session: &Session, method: &str, _: &Value) -> Option<ApiResult<Value>> {
        match method {
            "embedded/test" => Some(Ok(Value::String(session.id().to_string()))),
            "closeSession" => {
                CLOSED.fetch_add(1, Ordering::SeqCst);
                Some(Ok(Value::Null))
            }
            _ => None,
        }
    }
    fn send(method: &str) -> Value {
        let request = format!(r#"{{"method":"{method}","params":{{}}}}"#);
        let bytes = dispatch_with_extension(request.as_bytes(), extension);
        json::unmarshal(std::str::from_utf8(&bytes).unwrap()).unwrap()
    }
    #[test]
    fn a_failing_extension_cleanup_still_closes_the_compiler_session() {
        fn reject_close(_: &Session, method: &str, _: &Value) -> Option<ApiResult<Value>> {
            (method == "closeSession").then(|| Err(ApiError::client("extension cleanup failed")))
        }
        let opened =
            dispatch_with_extension(br#"{"method":"openSession","params":{}}"#, reject_close);
        assert!(std::str::from_utf8(&opened).unwrap().contains("result"));
        let reply =
            dispatch_with_extension(br#"{"method":"closeSession","params":{}}"#, reject_close);
        assert!(std::str::from_utf8(&reply).unwrap().contains("extension cleanup failed"));
        let closed = send("embedded/test");
        assert!(Params(&closed).str("error").unwrap().contains("session is closed"));
    }
    #[test]
    fn extension_borrows_the_live_compiler_session_and_receives_close() {
        let opened = send("openSession");
        assert_eq!(send("embedded/test"), opened);
        assert_eq!(send("collectIdle"), Obj::new().set("result", Value::Bool(true)).build());
        assert_eq!(send("closeSession"), Obj::new().set("result", Value::Bool(true)).build());
        assert_eq!(CLOSED.load(Ordering::SeqCst), 1);
        let closed = send("embedded/test");
        assert!(Params(&closed).str("error").unwrap().contains("session is closed"));
    }
}
