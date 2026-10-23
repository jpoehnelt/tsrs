//! TypeScript's fixSpelling provider for property, symbol and namespace suggestions.
//! Suggestions come from the checker, never from localized diagnostic messages.
use crate::codeactions::{CodeAction, CodeFixContext, CodeFixProvider, CombinedCodeActions};
use crate::{astnav, change, lsconv, utilities};
use std::sync::Arc;
use tsrs_ast::{self as ast, Node, NodeFlags, SemanticMeaning, Symbol, SymbolFlags};
use tsrs_core::context::Context;
use tsrs_core::{LanguageVariant, P};
use tsrs_lsproto as lsp;

pub static SPELLING_PROVIDER: CodeFixProvider = CodeFixProvider {
    requires_auto_imports: false,
    error_codes: &[2551, 2568, 2552, 2570, 2833, 2724],
    get_code_actions: spelling,
    fix_ids: &["fixSpelling"],
    get_all_code_actions: Some(spelling_all),
};

fn info(ctx: &Context, fix: &CodeFixContext) -> Option<(P<Node>, P<Symbol>)> {
    let node = astnav::get_token_at_position(
        fix.source_file,
        fix.span.pos().expect_source("diagnostic position"),
    );
    let parent = node.parent()?;
    let mut c = fix.program.get_type_checker_for_file(ctx, fix.source_file);
    let suggestion = if ast::is_property_access_expression(parent) && parent.name() == Some(node) {
        let mut t = c.get_type_at_location(parent.expression()?);
        if parent.flags().intersects(NodeFlags::OptionalChain) {
            t = c.get_non_nullable_type(t);
        }
        c.get_suggested_symbol_for_nonexistent_property(node, t)
    } else if ast::is_qualified_name(parent) && parent.as_qualified_name().right == node {
        let symbol = c.get_symbol_at_location_exported(parent.as_qualified_name().left)?;
        c.get_suggested_symbol_for_nonexistent_module(node, symbol)
    } else if ast::is_identifier(node) && matches!(fix.error_code, 2552 | 2570 | 2833) {
        let meaning = utilities::get_meaning_from_location(node);
        let mut flags = SymbolFlags::None;
        if meaning.intersects(SemanticMeaning::Value) {
            flags |= SymbolFlags::Value;
        }
        if meaning.intersects(SemanticMeaning::Type) {
            flags |= SymbolFlags::Type;
        }
        if meaning.intersects(SemanticMeaning::Namespace) {
            flags |= SymbolFlags::Namespace;
        }
        c.get_suggested_symbol_for_nonexistent_symbol(Some(node), node.text(), flags)
    } else {
        None
    }?;
    Some((node, suggestion))
}
fn tracker(ctx: &Context, fix: &CodeFixContext) -> change::Tracker {
    change::new_tracker(
        ctx,
        &fix.program.options(),
        fix.ls.format_options(),
        Arc::clone(&fix.ls.converters),
    )
}
fn edit(changes: &mut change::Tracker, fix: &CodeFixContext, node: P<Node>, symbol: P<Symbol>) {
    let suggestion = ast::symbol_name(symbol);
    let parent = node.parent().unwrap();
    if !tsrs_scanner::is_identifier_text(suggestion, LanguageVariant::Standard)
        && ast::is_property_access_expression(parent)
        && !symbol
            .value_declaration()
            .and_then(|d| d.name())
            .is_some_and(ast::is_private_identifier)
    {
        let replacement = changes.emit_context.factory.new_element_access_expression(
            parent.expression().unwrap(),
            parent.as_property_access_expression().question_dot_token(),
            changes.emit_context.factory.new_string_literal(suggestion, ast::TokenFlags::None),
            parent.flags(),
        );
        changes.replace_node(fix.source_file, parent, replacement, None);
    } else {
        let replacement = changes.emit_context.factory.new_identifier(suggestion);
        changes.replace_node(fix.source_file, node, replacement, None);
    }
}
fn edits(changes: &mut change::Tracker) -> Vec<(lsp::DocumentUri, Vec<lsp::TextEdit>)> {
    changes
        .get_changes()
        .0
        .into_iter()
        .map(|(file, edits)| (lsconv::file_name_to_document_uri(&file), edits))
        .collect()
}
#[expect(clippy::unnecessary_wraps, reason = "Code-fix provider callback signature")]
fn spelling(ctx: &Context, fix: &CodeFixContext) -> Result<Vec<CodeAction>, lsp::Error> {
    let Some((node, symbol)) = info(ctx, fix) else {
        return Ok(Vec::new());
    };
    let mut changes = tracker(ctx, fix);
    edit(&mut changes, fix, node, symbol);
    Ok(vec![CodeAction {
        description: format!("Change spelling to '{}'", ast::symbol_name(symbol)),
        fix_name: Some("spelling".into()),
        fix_id: "fixSpelling".into(),
        fix_all_description: "Fix all detected spelling errors".into(),
        changes_by_file: edits(&mut changes),
        ..Default::default()
    }])
}
#[expect(clippy::unnecessary_wraps, reason = "Combined code-fix provider callback signature")]
fn spelling_all(
    ctx: &Context,
    fix: &CodeFixContext,
) -> Result<Option<CombinedCodeActions>, lsp::Error> {
    let mut changes = tracker(ctx, fix);
    for diagnostic in crate::diagnostics::get_all_diagnostics(ctx, fix.program, fix.source_file) {
        if !crate::codeactions::is_fixable_diagnostic(diagnostic, SPELLING_PROVIDER.error_codes) {
            continue;
        }
        let Some(span) = diagnostic.loc() else {
            continue;
        };
        let local = CodeFixContext { span, error_code: diagnostic.code(), ..*fix };
        if let Some((node, symbol)) = info(ctx, &local) {
            edit(&mut changes, &local, node, symbol);
        }
    }
    let changes_by_file = edits(&mut changes);
    Ok((!changes_by_file.is_empty()).then(|| CombinedCodeActions {
        description: "Fix all detected spelling errors".into(),
        changes_by_file,
        ..Default::default()
    }))
}
