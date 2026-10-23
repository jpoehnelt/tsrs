use std::cmp::Ordering;

use tsrs_ast::{Diagnostic, SourceFile};
use tsrs_compiler::Program;
use tsrs_core::collections::OrderedMap;
use tsrs_core::context::Context;
use tsrs_core::{P, TextRange};
use tsrs_diagnostics as diagnostics;
use tsrs_lsproto as lsproto;

use crate::diagnostics::get_all_diagnostics;
use crate::languageservice::LanguageService;
use crate::lsconv;
use crate::spanmap::Feature;

// codeactions.go:21
// CodeFixProvider represents a provider for a specific type of code fix
pub struct CodeFixProvider {
    pub error_codes: &'static [i32],
    /// Whether this provider reads the prepared package-export/auto-import view.
    pub requires_auto_imports: bool,
    pub get_code_actions:
        fn(ctx: &Context, fix_context: &CodeFixContext) -> Result<Vec<CodeAction>, lsproto::Error>,
    pub fix_ids: &'static [&'static str],
    pub get_all_code_actions: Option<
        fn(
            ctx: &Context,
            fix_context: &CodeFixContext,
        ) -> Result<Option<CombinedCodeActions>, lsproto::Error>,
    >,
}

// codeactions.go:29
// CodeFixContext contains the context needed to generate code fixes
pub struct CodeFixContext<'a> {
    pub source_file: P<SourceFile>,
    pub span: TextRange,
    pub error_code: i32,
    pub fix_id: Option<&'a str>,
    pub program: &'static Program,
    pub ls: &'a LanguageService,
    pub diagnostic: Option<&'a lsproto::Diagnostic>,
    pub params: Option<&'a lsproto::CodeActionParams>,
}

// codeactions.go:40
// CodeAction represents a single code action fix
#[derive(Clone, Debug, Default)]
pub struct CodeAction {
    pub description: String,
    pub changes: Vec<lsproto::TextEdit>,
    pub fix_id: String,
    pub fix_name: Option<String>,
    pub changes_by_file: Vec<(lsproto::DocumentUri, Vec<lsproto::TextEdit>)>,
    pub fix_all_description: String,
}

impl CodeAction {
    pub fn workspace_edit(&self, uri: &lsproto::DocumentUri) -> lsproto::WorkspaceEdit {
        let mut changes = OrderedMap::default();
        if !self.changes.is_empty() {
            changes.insert(uri.clone(), self.changes.clone());
        }
        for (uri, edits) in &self.changes_by_file {
            changes.entry(uri.clone()).or_default().extend(edits.iter().cloned());
        }
        lsproto::WorkspaceEdit { changes: Some(changes), ..Default::default() }
    }

    // codeactions.go:49
    // Compare defines a total ordering for CodeAction values, comparing description
    // then text edits lexicographically. Used with slices.BinarySearchFunc.
    pub fn compare(&self, b: &CodeAction) -> Ordering {
        let c = go_compare(&self.description, &b.description);
        if c != Ordering::Equal {
            return c;
        }
        let c = go_compare(&self.changes.len(), &b.changes.len());
        if c != Ordering::Equal {
            return c;
        }
        if let Some(result) = self.changes.iter().enumerate().find_map(|(i, edit)| {
            let c = edit.compare(&b.changes[i]);
            if c != Ordering::Equal {
                return Some(c);
            }

            None
        }) {
            return result;
        }
        let c = self.changes_by_file.len().cmp(&b.changes_by_file.len());
        if c != Ordering::Equal {
            return c;
        }
        for ((uri, edits), (other_uri, other_edits)) in
            self.changes_by_file.iter().zip(&b.changes_by_file)
        {
            let c = uri.cmp(other_uri).then_with(|| edits.len().cmp(&other_edits.len()));
            if c != Ordering::Equal {
                return c;
            }
            for (edit, other) in edits.iter().zip(other_edits) {
                let c = edit.compare(other);
                if c != Ordering::Equal {
                    return c;
                }
            }
        }
        Ordering::Equal
    }
}

fn go_compare<T: Ord + ?Sized>(a: &T, b: &T) -> Ordering {
    a.cmp(b)
}

// codeactions.go:66
// CombinedCodeActions represents combined code actions for fix-all scenarios
#[derive(Clone, Debug, Default)]
pub struct CombinedCodeActions {
    pub description: String,
    pub changes: Vec<lsproto::TextEdit>,
    pub changes_by_file: Vec<(lsproto::DocumentUri, Vec<lsproto::TextEdit>)>,
}
impl CombinedCodeActions {
    pub fn workspace_edit(&self, uri: &lsproto::DocumentUri) -> lsproto::WorkspaceEdit {
        CodeAction {
            changes: self.changes.clone(),
            changes_by_file: self.changes_by_file.clone(),
            ..Default::default()
        }
        .workspace_edit(uri)
    }
}

// codeactions.go:72
// codeFixProviders is the list of all registered code fix providers
fn code_fix_providers() -> &'static [&'static CodeFixProvider] {
    static PROVIDERS: std::sync::LazyLock<Vec<&'static CodeFixProvider>> = std::sync::LazyLock::new(
        || {
            vec![
            &*crate::codeactions_importfixes::IMPORT_FIX_PROVIDER,
            &*crate::codeactions_fixmissingtypeannotation::ISOLATED_DECLARATIONS_FIX_PROVIDER,
            &*crate::codeactions_fixclassincorrectlyimplementsinterface::FIX_CLASS_INCORRECTLY_IMPLEMENTS_INTERFACE_PROVIDER,
            &crate::codeactions_usage::IMPLICIT_THIS_PROVIDER,
            &crate::codeactions_usage::INFER_USAGE_PROVIDER,
            &crate::codeactions_usage::MISSING_PARAMETER_PROVIDER,
            &crate::codeactions_usage::MISSING_MEMBER_PROVIDER,
            &crate::codeactions_spelling::SPELLING_PROVIDER,
        ]
        },
    );
    &PROVIDERS
}

/// Embedded clients can avoid checking/serializing a file for a code with no registered fix.
pub fn supported_code_fix_codes() -> Vec<i32> {
    let mut codes: Vec<i32> = code_fix_providers()
        .iter()
        .flat_map(|provider| provider.error_codes.iter().copied())
        .collect();
    codes.sort_unstable();
    codes.dedup();
    codes
}

/// Unknown providers retain the conservative prepared-snapshot behavior.
pub fn code_fix_requires_auto_imports(code: i32) -> bool {
    let mut found = false;
    for provider in code_fix_providers() {
        if provider.error_codes.contains(&code) {
            found = true;
            if provider.requires_auto_imports {
                return true;
            }
        }
    }
    !found
}
pub fn combined_code_fix_requires_auto_imports(id: &str) -> bool {
    code_fix_providers()
        .iter()
        .find(|provider| provider.fix_ids.contains(&id))
        .is_none_or(|provider| provider.requires_auto_imports)
}

impl LanguageService {
    /// Preserve compiler fix IDs that the LSP `CodeAction` representation does not carry.
    pub fn embedded_code_fixes(
        &self,
        ctx: &Context,
        params: &lsproto::CodeActionParams,
    ) -> Result<Vec<CodeAction>, lsproto::Error> {
        let (program, file) = self.get_program_and_file(&params.text_document.uri);
        let mut actions = Vec::new();
        for diagnostic in &params.context.diagnostics {
            actions.extend(
                self.code_fixes_for_diagnostic(ctx, program, file, diagnostic, params)?
                    .into_iter()
                    .map(|(_, action)| action),
            );
        }
        Ok(actions)
    }

    pub fn embedded_combined_code_fix(
        &self,
        ctx: &Context,
        uri: &lsproto::DocumentUri,
        fix_id: &str,
    ) -> Result<Option<CombinedCodeActions>, lsproto::Error> {
        let provider = code_fix_providers()
            .iter()
            .find(|provider| provider.fix_ids.contains(&fix_id))
            .ok_or_else(|| lsproto::Error::new(format!("Unknown compiler fix ID: {fix_id}")))?;
        let Some(get_all) = provider.get_all_code_actions else {
            return Ok(None);
        };
        let (program, file) = self.get_program_and_file(uri);
        get_all(
            ctx,
            &CodeFixContext {
                source_file: file,
                span: TextRange::default(),
                error_code: 0,
                fix_id: Some(fix_id),
                program,
                ls: self,
                diagnostic: None,
                params: None,
            },
        )
    }

    fn code_fixes_for_diagnostic(
        &self,
        ctx: &Context,
        program: &'static Program,
        file: P<SourceFile>,
        diagnostic: &lsproto::Diagnostic,
        params: &lsproto::CodeActionParams,
    ) -> Result<Vec<(&'static CodeFixProvider, CodeAction)>, lsproto::Error> {
        let Some(error_code) = diagnostic.code.as_ref().and_then(|code| code.integer) else {
            return Ok(Vec::new());
        };
        let mut result = Vec::new();
        for &provider in code_fix_providers() {
            if !code_fix_provider_matches_lsp_diagnostic(provider, diagnostic) {
                continue;
            }
            for mapped in self.converters.from_lsp_range_for_source_file(
                file,
                diagnostic.range,
                Feature::CodeActions,
            ) {
                let context = CodeFixContext {
                    source_file: mapped.script,
                    span: mapped.span,
                    error_code,
                    fix_id: None,
                    program,
                    ls: self,
                    diagnostic: Some(diagnostic),
                    params: Some(params),
                };
                result.extend(
                    (provider.get_code_actions)(ctx, &context)?
                        .into_iter()
                        .map(|action| (provider, action)),
                );
            }
        }
        Ok(result)
    }
    // codeactions.go:80
    // ProvideCodeActions returns code actions for the given range and context
    pub fn provide_code_actions(
        &self,
        ctx: &Context,
        params: &lsproto::CodeActionParams,
    ) -> Result<lsproto::CodeActionResponse, lsproto::Error> {
        let (program, file) = self.get_program_and_file(&params.text_document.uri);

        let mut actions: Vec<lsproto::CommandOrCodeAction> = Vec::new();

        if let Some(only) = &params.context.only {
            for &kind in only {
                let matching_kinds = get_organize_imports_actions_for_kind(kind);
                actions.extend(matching_kinds.into_iter().map(|matching_kind| {
                    self.create_organize_imports_action(ctx, program, file, matching_kind)
                }));

                if is_fix_all_kind(kind) {
                    let fix_all_action =
                        self.create_fix_all_action(ctx, program, file, &params.text_document.uri)?;
                    if let Some(fix_all_action) = fix_all_action {
                        actions.push(fix_all_action);
                    }
                }
            }
        }

        // Go: `params.Context.Diagnostics != nil`; a decoded required array is never nil.
        if wants_quick_fixes(params.context.only.as_deref()) {
            // Go map (random iteration order).
            let mut fix_id_seen: OrderedMap<String, &'static CodeFixProvider> =
                OrderedMap::default();

            let mut seen: Vec<CodeAction> = Vec::new(); // sorted for binary search dedup, dedup across all diagnostics and providers so if multiple diags produce the same codefix, only one is returned

            for diag in &params.context.diagnostics {
                for (provider, action) in
                    self.code_fixes_for_diagnostic(ctx, program, file, diag, params)?
                {
                    let Err(i) = seen.binary_search_by(|probe| probe.compare(&action)) else {
                        continue;
                    };
                    actions.push(convert_to_lsp_code_action(
                        &action,
                        diag,
                        &params.text_document.uri,
                    ));
                    if !action.fix_id.is_empty() {
                        fix_id_seen.insert(action.fix_id.clone(), provider);
                    }
                    seen.insert(i, action);
                }
            }

            let fix_all_actions = self.get_fix_all_quick_fixes(
                ctx,
                program,
                file,
                &params.text_document.uri,
                &fix_id_seen,
            )?;
            actions.extend(fix_all_actions);
        }

        Ok(lsproto::CommandOrCodeActionArrayOrNull { command_or_code_action_array: Some(actions) })
    }

    // codeactions.go:154
    // getFixAllQuickFixes returns per-provider "Fix all in file" quickfix entries for providers
    // that matched at least 2 diagnostics in the full file.
    fn get_fix_all_quick_fixes(
        &self,
        ctx: &Context,
        program: &'static Program,
        file: P<SourceFile>,
        uri: &lsproto::DocumentUri,
        fix_id_seen: &OrderedMap<String, &'static CodeFixProvider>,
    ) -> Result<Vec<lsproto::CommandOrCodeAction>, lsproto::Error> {
        let mut actions = Vec::new();

        for (fix_id, &provider) in fix_id_seen {
            let Some(get_all_code_actions) = provider.get_all_code_actions else {
                continue;
            };

            if !has_multiple_fixable_diagnostics(ctx, program, file, provider.error_codes) {
                continue;
            }

            let fix_context = CodeFixContext {
                source_file: file,
                span: TextRange::default(),
                error_code: 0,
                fix_id: Some(fix_id),
                program,
                ls: self,
                diagnostic: None,
                params: None,
            };
            let combined = get_all_code_actions(ctx, &fix_context)?;
            if let Some(combined) = combined {
                if !combined.changes.is_empty() || !combined.changes_by_file.is_empty() {
                    let kind = lsproto::CodeActionKind::QuickFix;
                    let edit = combined.workspace_edit(uri);
                    actions.push(lsproto::CommandOrCodeAction {
                        code_action: Some(lsproto::CodeAction {
                            title: combined.description,
                            kind: Some(kind),
                            edit: Some(edit),
                            ..Default::default()
                        }),
                        ..Default::default()
                    });
                }
            }
        }

        Ok(actions)
    }

    // codeactions.go:250
    // createFixAllAction creates a source.fixAll code action that applies all auto-fixable
    // code fixes across the file.
    fn create_fix_all_action(
        &self,
        ctx: &Context,
        program: &'static Program,
        file: P<SourceFile>,
        uri: &lsproto::DocumentUri,
    ) -> Result<Option<lsproto::CommandOrCodeAction>, lsproto::Error> {
        let kind = lsproto::CodeActionKind::SourceFixAllTs;
        let mut lsp_changes: OrderedMap<lsproto::DocumentUri, Vec<lsproto::TextEdit>> =
            OrderedMap::default();

        for &provider in code_fix_providers() {
            let Some(get_all_code_actions) = provider.get_all_code_actions else {
                continue;
            };

            let fix_context = CodeFixContext {
                source_file: file,
                span: TextRange::default(),
                error_code: 0,
                fix_id: None,
                program,
                ls: self,
                diagnostic: None,
                params: None,
            };

            let combined = get_all_code_actions(ctx, &fix_context)?;
            if let Some(combined) = combined {
                if let Some(changes) = combined.workspace_edit(uri).changes {
                    for (uri, edits) in changes {
                        lsp_changes.entry(uri).or_default().extend(edits);
                    }
                }
            }
        }

        if lsp_changes.is_empty() {
            return Ok(None);
        }

        Ok(Some(lsproto::CommandOrCodeAction {
            code_action: Some(lsproto::CodeAction {
                title: diagnostics::Fix_All.localize(&[]),
                kind: Some(kind),
                edit: Some(lsproto::WorkspaceEdit {
                    changes: Some(lsp_changes),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        }))
    }

    // codeactions.go:340
    // createOrganizeImportsAction creates the organize imports code action
    fn create_organize_imports_action(
        &self,
        ctx: &Context,
        program: &'static Program,
        file: P<SourceFile>,
        kind: lsproto::CodeActionKind,
    ) -> lsproto::CommandOrCodeAction {
        let title = get_organize_imports_action_title(kind);
        let changes = self.organize_imports(ctx, file, program, kind);
        if changes.is_empty() {
            return lsproto::CommandOrCodeAction {
                code_action: Some(lsproto::CodeAction {
                    title,
                    kind: Some(kind),
                    edit: Some(lsproto::WorkspaceEdit {
                        changes: Some(OrderedMap::default()),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            };
        }

        let mut lsp_changes: OrderedMap<lsproto::DocumentUri, Vec<lsproto::TextEdit>> =
            OrderedMap::default();
        lsp_changes.extend(changes.into_iter().map(|(file_name, edits)| {
            let file_uri = lsconv::file_name_to_document_uri(&file_name);
            (file_uri, edits)
        }));

        lsproto::CommandOrCodeAction {
            code_action: Some(lsproto::CodeAction {
                title,
                kind: Some(kind),
                edit: Some(lsproto::WorkspaceEdit {
                    changes: Some(lsp_changes),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }
}

// codeactions.go:200
// hasMultipleFixableDiagnostics returns true if the file has at least 2 diagnostics
// matching the given error codes. Checks all diagnostic sources (semantic,
// syntactic, suggestion, declaration) to match ProvideDiagnostics.
fn has_multiple_fixable_diagnostics(
    ctx: &Context,
    program: &'static Program,
    file: P<SourceFile>,
    error_codes: &[i32],
) -> bool {
    let all_diags = get_all_diagnostics(ctx, program, file);
    all_diags.into_iter().filter(|d| is_fixable_diagnostic(*d, error_codes)).nth(1).is_some()
}

// codeactions.go:213
fn code_fix_provider_matches_lsp_diagnostic(
    provider: &CodeFixProvider,
    diagnostic: &lsproto::Diagnostic,
) -> bool {
    if let Some(source) = &diagnostic.source {
        if source != "ts" {
            return false;
        }
    }
    diagnostic
        .code
        .as_ref()
        .and_then(|c| c.integer)
        .is_some_and(|code| contains_error_code(provider.error_codes, code))
}

// codeactions.go:220
pub(crate) fn is_fixable_diagnostic(diagnostic: P<Diagnostic>, error_codes: &[i32]) -> bool {
    diagnostic.source().is_empty() && contains_error_code(error_codes, diagnostic.code())
}

// codeactions.go:225
// isFixAllKind returns true if the requested kind matches source.fixAll
fn is_fix_all_kind(kind: lsproto::CodeActionKind) -> bool {
    kind.contains(lsproto::CodeActionKind::SourceFixAllTs)
}

// codeactions.go:231
// wantsQuickFixes returns true if the Only filter is nil/empty (meaning all kinds are wanted)
// or explicitly includes the quickfix kind.
fn wants_quick_fixes(only: Option<&[lsproto::CodeActionKind]>) -> bool {
    let Some(only) = only else {
        return true;
    };
    if only.is_empty() {
        return true;
    }
    only.iter().any(|&kind| kind.contains(lsproto::CodeActionKind::QuickFix))
}

// codeactions.go:299
// getOrganizeImportsActionTitle returns the appropriate title for the given organize imports kind
fn get_organize_imports_action_title(kind: lsproto::CodeActionKind) -> String {
    match kind {
        lsproto::CodeActionKind::SourceRemoveUnusedImportsTs => {
            diagnostics::Remove_Unused_Imports.localize(&[])
        }
        lsproto::CodeActionKind::SourceSortImportsTs => diagnostics::Sort_Imports.localize(&[]),
        _ => diagnostics::Organize_Imports.localize(&[]),
    }
}

// codeactions.go:313
// getOrganizeImportsActionsForKind returns the organize imports code action kinds that should be
// returned for the given requested kind.
fn get_organize_imports_actions_for_kind(
    requested_kind: lsproto::CodeActionKind,
) -> Vec<lsproto::CodeActionKind> {
    let organize_imports_kinds = [
        lsproto::CodeActionKind::SourceOrganizeImportsTs,
        lsproto::CodeActionKind::SourceRemoveUnusedImportsTs,
        lsproto::CodeActionKind::SourceSortImportsTs,
    ];

    let mut result = Vec::new();
    result.extend(
        organize_imports_kinds
            .into_iter()
            .filter(|&organize_kind| requested_kind.contains(organize_kind)),
    );

    if result.contains(&requested_kind) {
        return vec![requested_kind];
    }

    result
}

// codeactions.go:378
// containsErrorCode checks if the error code is in the list
pub(crate) fn contains_error_code(codes: &[i32], code: i32) -> bool {
    codes.contains(&code)
}

// codeactions.go:383
// convertToLSPCodeAction converts an internal CodeAction to an LSP CodeAction
fn convert_to_lsp_code_action(
    action: &CodeAction,
    diag: &lsproto::Diagnostic,
    uri: &lsproto::DocumentUri,
) -> lsproto::CommandOrCodeAction {
    let kind = lsproto::CodeActionKind::QuickFix;
    let diagnostics = vec![diag.clone()];

    lsproto::CommandOrCodeAction {
        code_action: Some(lsproto::CodeAction {
            title: action.description.clone(),
            kind: Some(kind),
            edit: Some(action.workspace_edit(uri)),
            diagnostics: Some(diagnostics),
            ..Default::default()
        }),
        ..Default::default()
    }
}
