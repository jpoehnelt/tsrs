//! Generic editor fixes ported from TypeScript 5.9 inferFromUsage, fixImplicitThis,
//! fixAddMissingParam and fixMissingMember. These providers operate on compiler nodes/types.
use crate::codeactions::{CodeAction, CodeFixContext, CodeFixProvider};
use crate::{astnav, change, lsconv};
use std::sync::Arc;
use tsrs_ast::{self as ast, Kind, Node, SourceFile};
use tsrs_checker::{self as checker, Checker, Flags, InternalFlags};
use tsrs_core::context::Context;
use tsrs_core::{P, TextRange};
use tsrs_lsproto as lsp;

pub static INFER_USAGE_PROVIDER: CodeFixProvider = CodeFixProvider {
    requires_auto_imports: false,
    error_codes: &[7006, 7019, 7044, 2683],
    get_code_actions: infer_usage,
    fix_ids: &["inferFromUsage"],
    get_all_code_actions: Some(infer_usage_all),
};
pub static IMPLICIT_THIS_PROVIDER: CodeFixProvider = CodeFixProvider {
    requires_auto_imports: false,
    error_codes: &[2683],
    get_code_actions: implicit_this,
    fix_ids: &[],
    get_all_code_actions: None,
};
pub static MISSING_PARAMETER_PROVIDER: CodeFixProvider = CodeFixProvider {
    requires_auto_imports: false,
    error_codes: &[2554],
    get_code_actions: missing_parameter,
    fix_ids: &["addMissingParam", "addOptionalParam"],
    get_all_code_actions: Some(missing_parameter_all),
};
pub static MISSING_MEMBER_PROVIDER: CodeFixProvider = CodeFixProvider {
    requires_auto_imports: false,
    error_codes: &[2339],
    get_code_actions: missing_member,
    fix_ids: &[],
    get_all_code_actions: None,
};
fn tracker(ctx: &Context, fix: &CodeFixContext) -> change::Tracker {
    change::new_tracker(
        ctx,
        &fix.program.options(),
        fix.ls.format_options(),
        Arc::clone(&fix.ls.converters),
    )
}
fn action(tracker: &mut change::Tracker, name: &str, description: String) -> Vec<CodeAction> {
    let (changes, _) = tracker.get_changes();
    if changes.is_empty() {
        return Vec::new();
    }
    vec![CodeAction {
        description,
        fix_name: Some(name.to_string()),
        changes_by_file: changes
            .into_iter()
            .map(|(file, edits)| (lsconv::file_name_to_document_uri(&file), edits))
            .collect(),
        ..Default::default()
    }]
}
fn token(fix: &CodeFixContext) -> P<Node> {
    astnav::get_token_at_position(
        fix.source_file,
        fix.span.pos().expect_source("compiler diagnostic span"),
    )
}
fn bounds(file: P<SourceFile>, node: P<Node>) -> (u32, u32) {
    (astnav::get_start_of_node(node, file, false), node.end().expect_source("parsed editor node"))
}
fn type_node(
    c: &mut Checker,
    changes: &change::Tracker,
    t: P<checker::Type>,
    node: P<Node>,
) -> Option<P<Node>> {
    let builder = checker::new_node_builder(c, changes.emit_context);
    builder.type_to_type_node(
        c,
        t,
        Some(node),
        Flags::NoTruncation
            | Flags::UseAliasDefinedOutsideCurrentScope
            | Flags::UseSingleQuotesForStringLiteralType,
        InternalFlags::None,
        None,
    )
}
fn add_jsdoc(
    changes: &mut change::Tracker,
    file: P<SourceFile>,
    node: P<Node>,
    new_tags: Vec<P<Node>>,
) {
    let docs = node.jsdoc(None);
    let mut tags = Vec::new();
    let mut comment = Vec::new();
    let mut factory = changes.node_factory.clone();
    let ranges = tsrs_parser::get_jsdoc_comment_ranges(&mut factory, &[], node, file.text());
    for (&doc, range) in docs.iter().zip(ranges) {
        comment.extend_from_slice(doc.as_jsdoc().comment.nodes());
        if let Some(old) = doc.as_jsdoc().tags {
            tags.extend_from_slice(old.nodes());
        }
        let (start, end) = (range.pos(), range.end());
        let start = file.text()[..start as usize].rfind('\n').map_or(0, |i| i + 1);
        let end = if file.text().get(end as usize..).is_some_and(|s| s.starts_with("\r\n")) {
            end + 2
        } else if file.text().as_bytes().get(end as usize) == Some(&b'\n') {
            end + 1
        } else {
            end
        };
        changes.delete_range(file, TextRange::new(start as u32, end));
    }
    for tag in new_tags {
        if let Some(existing) = tags.iter_mut().find(|old| {
            old.kind() == tag.kind()
                && (old.kind() != Kind::JSDocParameterTag
                    || old.name().map(|n| n.text()) == tag.name().map(|n| n.text()))
        }) {
            *existing = tag;
        } else {
            tags.push(tag);
        }
    }
    let doc = changes.new_jsdoc(changes.new_node_list(comment), Some(changes.new_node_list(tags)));
    let (start, _) = bounds(file, node);
    let before = &file.text()[..start as usize];
    let whitespace =
        before.rfind(|c: char| c != ' ' && c != '\t').map_or(before, |i| &before[i + 1..]);
    changes.insert_node_at(
        file,
        start,
        doc,
        change::NodeOptions { suffix: format!("\n{whitespace}"), ..Default::default() },
    );
}
fn infer_function(
    ctx: &Context,
    fix: &CodeFixContext,
    changes: &mut change::Tracker,
    include_this: bool,
    include_parameters: bool,
) {
    let Some(function) = ast::find_ancestor(token(fix), ast::is_function_like_declaration) else {
        return;
    };
    let mut c = fix.program.get_type_checker_for_file(ctx, fix.source_file);
    let mut tags = Vec::new();
    if include_this {
        let t = c.infer_this_from_usage(function, fix.program.get_source_files());
        if let Some(typ) = type_node(&mut c, changes, t, function) {
            tags.push(changes.new_jsdoc_this_tag(
                changes.new_identifier("this"),
                changes.new_jsdoc_type_expression(typ),
                None,
            ));
        }
    }
    if include_parameters {
        let inferences = c.infer_parameters_from_usage(function, fix.program.get_source_files());
        let bare_arrow = ast::is_arrow_function(function)
            && function.parameters().len() == 1
            && !fix.source_file.text()[bounds(fix.source_file, function).0 as usize
                ..bounds(fix.source_file, function.parameters()[0]).0 as usize]
                .contains('(');
        let mut inserted_arrow = false;
        for inference in inferences {
            let param = inference.declaration;
            if param.initializer().is_some()
                || param.type_node().is_some()
                || !param.name().is_some_and(ast::is_identifier)
            {
                continue;
            }
            if !param.jsdoc(None).is_empty()
                || function.jsdoc(None).iter().any(|doc| {
                    doc.as_jsdoc().tags.is_some_and(|tags| {
                        tags.nodes().iter().any(|tag| {
                            tag.kind() == Kind::JSDocParameterTag
                                && tag.name().map(|n| n.text()) == param.name().map(|n| n.text())
                                && tag.type_expression().is_some()
                        })
                    })
                })
            {
                continue;
            }
            let Some(typ) = type_node(&mut c, changes, inference.type_, param) else {
                continue;
            };
            if ast::is_in_js_file(function) {
                let expression = changes.new_jsdoc_type_expression(typ);
                if ast::is_arrow_function(function) || ast::is_function_expression(function) {
                    let tag = changes.new_jsdoc_type_tag(
                        changes.new_identifier("type"),
                        expression,
                        None,
                    );
                    let doc = changes.new_jsdoc(
                        changes.new_node_list(Vec::new()),
                        Some(changes.new_node_list(vec![tag])),
                    );
                    let (start, end) = bounds(fix.source_file, param);
                    if bare_arrow && !inserted_arrow {
                        changes.replace_text_range_with_text(
                            fix.source_file,
                            TextRange::new(start, start),
                            "(",
                        );
                        changes.replace_text_range_with_text(
                            fix.source_file,
                            TextRange::new(end, end),
                            ")",
                        );
                        inserted_arrow = true;
                    }
                    changes.insert_node_at(
                        fix.source_file,
                        start,
                        doc,
                        change::NodeOptions { suffix: " ".to_string(), ..Default::default() },
                    );
                } else {
                    tags.push(changes.new_jsdoc_parameter_or_property_tag(
                        Kind::JSDocParameterTag,
                        changes.new_identifier("param"),
                        changes.new_identifier(param.name().unwrap().text()),
                        inference.optional,
                        Some(expression),
                        false,
                        None,
                    ));
                }
            } else {
                changes.try_insert_type_annotation(fix.source_file, param, typ);
            }
        }
    }
    if !tags.is_empty() {
        add_jsdoc(changes, fix.source_file, function, tags);
    }
}
#[expect(
    clippy::unnecessary_wraps,
    reason = "The code-fix provider interface has a fallible callback signature."
)]
fn infer_usage(ctx: &Context, fix: &CodeFixContext) -> Result<Vec<CodeAction>, lsp::Error> {
    let Some(function) = ast::find_ancestor(token(fix), ast::is_function_like_declaration) else {
        return Ok(Vec::new());
    };
    let mut changes = tracker(ctx, fix);
    infer_function(ctx, fix, &mut changes, fix.error_code == 2683, fix.error_code != 2683);
    let description = if fix.error_code == 2683 {
        format!("Infer 'this' type of '{}' from usage", function.name().map_or("", |n| n.text()))
    } else {
        "Infer parameter types from usage".to_string()
    };
    let mut result = action(&mut changes, "inferFromUsage", description);
    for action in &mut result {
        action.fix_id = "inferFromUsage".to_string();
        action.fix_all_description = "Infer all types from usage".to_string();
    }
    Ok(result)
}
#[expect(
    clippy::unnecessary_wraps,
    reason = "The code-fix provider interface has a fallible callback signature."
)]
fn infer_usage_all(
    ctx: &Context,
    fix: &CodeFixContext,
) -> Result<Option<crate::codeactions::CombinedCodeActions>, lsp::Error> {
    let mut functions: Vec<(P<Node>, TextRange, bool, bool)> = Vec::new();
    for diag in crate::diagnostics::get_all_diagnostics(ctx, fix.program, fix.source_file) {
        if !INFER_USAGE_PROVIDER.error_codes.contains(&diag.code()) {
            continue;
        }
        let Some(span) = diag.loc() else {
            continue;
        };
        let node = astnav::get_token_at_position(
            fix.source_file,
            span.pos().expect_source("diagnostic span"),
        );
        let Some(function) = ast::find_ancestor(node, ast::is_function_like_declaration) else {
            continue;
        };
        if let Some((_, _, this, params)) = functions.iter_mut().find(|(n, _, _, _)| *n == function)
        {
            *this |= diag.code() == 2683;
            *params |= diag.code() != 2683;
        } else {
            functions.push((function, span, diag.code() == 2683, diag.code() != 2683));
        }
    }
    let mut changes = tracker(ctx, fix);
    for (_, span, this, params) in functions {
        let local = CodeFixContext { span, ..*fix };
        infer_function(ctx, &local, &mut changes, this, params);
    }
    let mut result =
        action(&mut changes, "inferFromUsage", "Infer all types from usage".to_string());
    Ok(result.pop().map(|a| crate::codeactions::CombinedCodeActions {
        description: a.description,
        changes: a.changes,
        changes_by_file: a.changes_by_file,
    }))
}
#[expect(
    clippy::unnecessary_wraps,
    reason = "The code-fix provider interface has a fallible callback signature."
)]
fn implicit_this(ctx: &Context, fix: &CodeFixContext) -> Result<Vec<CodeAction>, lsp::Error> {
    let Some(function) = ast::find_ancestor(token(fix), ast::is_function_like_declaration)
        .filter(|f| ast::is_function_declaration(*f) && f.name().is_some())
    else {
        return Ok(Vec::new());
    };
    let mut changes = tracker(ctx, fix);
    let (start, _) = bounds(fix.source_file, function);
    let name = function.name().unwrap();
    let (_, name_end) = bounds(fix.source_file, name);
    let end = function.parameter_list().unwrap().end().expect_source("parsed parameter list");
    let close_token = astnav::get_token_at_position(fix.source_file, end);
    if close_token.kind() != Kind::CloseParenToken {
        return Ok(Vec::new());
    }
    let close = close_token.end().expect_source("parsed closing parenthesis");
    changes.replace_text_range_with_text(
        fix.source_file,
        TextRange::new(start, start + 8),
        "const",
    );
    changes.replace_text_range_with_text(
        fix.source_file,
        TextRange::new(name_end, name_end),
        " = ",
    );
    changes.replace_text_range_with_text(fix.source_file, TextRange::new(close, close), " =>");
    Ok(action(
        &mut changes,
        "fixImplicitThis",
        format!("Convert function declaration '{}' to arrow function", name.text()),
    ))
}
#[expect(
    clippy::unnecessary_wraps,
    reason = "The code-fix provider interface has a fallible callback signature."
)]
fn missing_parameter(ctx: &Context, fix: &CodeFixContext) -> Result<Vec<CodeAction>, lsp::Error> {
    let Some(call) = ast::find_ancestor(token(fix), ast::is_call_expression) else {
        return Ok(Vec::new());
    };
    let mut c = fix.program.get_type_checker_for_file(ctx, fix.source_file);
    let typ = c.get_type_at_location(call.expression().unwrap());
    let function = c
        .get_resolved_signature(call, None, checker::CheckMode::Normal)
        .declaration()
        .filter(|n| n.body().is_some())
        .or_else(|| {
            let s = typ.symbol()?;
            s.declarations()
                .iter()
                .rev()
                .copied()
                .find(|n| ast::is_function_like_declaration(*n) && n.body().is_some())
        });
    let Some(function) = function else {
        return Ok(Vec::new());
    };
    let file = ast::get_source_file_of_node(function).unwrap();
    if file.is_declaration_file() || function.parameters().len() >= call.arguments().len() {
        return Ok(Vec::new());
    }
    let name = function.name().or_else(|| {
        let p = function.parent()?;
        if ast::is_binary_expression(p) { Some(p.as_binary_expression().left) } else { p.name() }
    });
    let name =
        name.map(|n| if ast::is_property_access_expression(n) { n.name().unwrap() } else { n });
    let Some(name) = name else {
        return Ok(Vec::new());
    };
    let mut new_parameters = Vec::new();
    let mut existing = 0;
    let mut unnamed = 0;
    for (i, &arg) in call.arguments().iter().enumerate() {
        let t = c.get_type_at_location(arg);
        let t = c.get_base_type_of_literal_type(t);
        let t = c.get_widened_type(t);
        if let Some(&parameter) = function.parameters().get(existing) {
            let parameter_type = c.get_type_at_location(parameter);
            if c.is_type_assignable_to(t, parameter_type) {
                existing += 1;
                continue;
            }
        }
        let arg_name =
            if ast::is_property_access_expression(arg) { arg.name().unwrap() } else { arg };
        let parameter_name = if ast::is_identifier(arg_name) {
            arg_name.text().to_string()
        } else {
            let n = format!("p{unnamed}");
            unnamed += 1;
            n
        };
        new_parameters.push((i, existing >= function.parameters().len(), parameter_name, t));
    }
    let mut result = Vec::new();
    for optional in [false, true] {
        let selected: Vec<_> =
            new_parameters.iter().filter(|(_, allowed, _, _)| !optional || *allowed).collect();
        if selected.is_empty() {
            continue;
        }
        let mut changes = tracker(ctx, fix);
        let mut parameters = function
            .parameters()
            .iter()
            .map(|p| {
                changes.new_parameter_declaration(
                    p.modifiers(),
                    p.as_parameter_declaration().dot_dot_dot_token(),
                    changes.deep_clone_node(p.name()).unwrap(),
                    p.question_token(),
                    changes.deep_clone_node(p.type_node()),
                    changes.deep_clone_node(p.initializer()),
                )
            })
            .collect::<Vec<_>>();
        for &&(index, _, ref parameter_name, t) in &selected {
            let typ = type_node(&mut c, &changes, t, function);
            let question = (optional
                || parameters
                    .get(index.wrapping_sub(1))
                    .is_some_and(|p| p.question_token().is_some()))
            .then(|| changes.new_token(Kind::QuestionToken));
            let parameter = changes.new_parameter_declaration(
                None,
                None,
                changes.new_identifier(changes.alloc_text(parameter_name)),
                question,
                typ,
                None,
            );
            parameters.insert(index.min(parameters.len()), parameter);
        }
        let options = change::NodeOptions {
            joiner: ", ".to_string(),
            indentation: Some(0),
            leading_trivia_option: change::LeadingTriviaOption::IncludeAll,
            trailing_trivia_option: change::TrailingTriviaOption::Include,
            ..Default::default()
        };
        if let (Some(first), Some(last)) =
            (function.parameters().first(), function.parameters().last())
        {
            let start = first.pos().expect_source("parameter range");
            let end = last.end().expect_source("parameter range");
            changes.replace_range_with_nodes(file, TextRange::new(start, end), parameters, options);
        } else {
            let list = function.parameter_list().unwrap();
            let pos = list.end().expect_source("parameter list");
            for (index, parameter) in parameters.into_iter().enumerate() {
                changes.insert_node_at(
                    file,
                    pos,
                    parameter,
                    change::NodeOptions {
                        prefix: if index == 0 { String::new() } else { ", ".to_string() },
                        ..Default::default()
                    },
                );
            }
        }
        let kind = if optional { "optional" } else { "missing" };
        let plural = if selected.len() == 1 { "parameter" } else { "parameters" };
        let id = if optional { "addOptionalParam" } else { "addMissingParam" };
        let mut actions =
            action(&mut changes, id, format!("Add {kind} {plural} to '{}'", name.text()));
        for action in &mut actions {
            action.fix_id = id.to_string();
            action.fix_all_description = format!("Add all {kind} parameters");
        }
        result.extend(actions);
    }
    Ok(result)
}
#[expect(
    clippy::unnecessary_wraps,
    reason = "The code-fix provider interface has a fallible callback signature."
)]
fn missing_member(ctx: &Context, fix: &CodeFixContext) -> Result<Vec<CodeAction>, lsp::Error> {
    let token = token(fix);
    let Some(access) = token
        .parent()
        .filter(|n| ast::is_property_access_expression(*n) && n.name() == Some(token))
    else {
        return Ok(Vec::new());
    };
    let mut c = fix.program.get_type_checker_for_file(ctx, fix.source_file);
    let typ = c.get_type_at_location(access.expression().unwrap());
    let Some(declaration) = typ
        .symbol()
        .and_then(|s| s.declarations().iter().copied().find(|n| ast::is_interface_declaration(*n)))
    else {
        return Ok(Vec::new());
    };
    let file = ast::get_source_file_of_node(declaration).unwrap();
    let name = token.text();
    let mut result = Vec::new();
    for kind in 0..3 {
        let mut changes = tracker(ctx, fix);
        let typ = changes.new_keyword_type_node(if kind == 0 {
            Kind::UnknownKeyword
        } else {
            Kind::AnyKeyword
        });
        let mut parameters = Vec::new();
        if kind == 0 {
            if let Some(call) = access
                .parent()
                .filter(|n| ast::is_call_expression(*n) && n.expression() == Some(access))
            {
                for (index, &argument) in call.arguments().iter().enumerate() {
                    let t = c.get_type_at_location(argument);
                    let t = c.get_base_type_of_literal_type(t);
                    let t = c.get_widened_type(t);
                    let parameter_type = type_node(&mut c, &changes, t, declaration);
                    let name = if ast::is_identifier(argument) {
                        argument.text().to_string()
                    } else {
                        format!("arg{index}")
                    };
                    parameters.push(changes.new_parameter_declaration(
                        None,
                        None,
                        changes.new_identifier(changes.alloc_text(&name)),
                        None,
                        parameter_type,
                        None,
                    ));
                }
            }
        }
        let member = match kind {
            0 => changes.new_method_signature_declaration(
                None,
                changes.new_identifier(name),
                None,
                None,
                Some(changes.new_node_list(parameters)),
                Some(typ),
            ),
            1 => changes.new_property_signature_declaration(
                None,
                changes.new_identifier(name),
                None,
                Some(typ),
                None,
            ),
            _ => {
                let param = changes.new_parameter_declaration(
                    None,
                    None,
                    changes.new_identifier("x"),
                    None,
                    Some(changes.new_keyword_type_node(Kind::StringKeyword)),
                    None,
                );
                changes.new_index_signature_declaration(
                    None,
                    Some(changes.new_node_list(vec![param])),
                    Some(typ),
                )
            }
        };
        changes.insert_member_at_start(file, declaration, member);
        let description = match kind {
            0 => format!("Declare method '{name}'"),
            1 => format!("Declare property '{name}'"),
            _ => format!("Add index signature for property '{name}'"),
        };
        result.extend(action(&mut changes, "fixMissingMember", description));
    }
    Ok(result)
}

fn missing_parameter_all(
    ctx: &Context,
    fix: &CodeFixContext,
) -> Result<Option<crate::codeactions::CombinedCodeActions>, lsp::Error> {
    let id = fix.fix_id.unwrap_or("addMissingParam");
    let mut all =
        tsrs_core::collections::OrderedMap::<lsp::DocumentUri, Vec<lsp::TextEdit>>::default();
    for diag in crate::diagnostics::get_all_diagnostics(ctx, fix.program, fix.source_file) {
        if diag.code() != 2554 {
            continue;
        }
        let Some(span) = diag.loc() else {
            continue;
        };
        let local = CodeFixContext { span, error_code: 2554, ..*fix };
        for action in missing_parameter(ctx, &local)?.into_iter().filter(|a| a.fix_id == id) {
            for (uri, edits) in action.changes_by_file {
                let existing = all.entry(uri).or_default();
                // Multiple failed calls can point to the same parameter list.
                if !edits.iter().any(|e| existing.iter().any(|old| old.range == e.range)) {
                    existing.extend(edits);
                }
            }
        }
    }
    let description = if id == "addOptionalParam" {
        "Add all optional parameters"
    } else {
        "Add all missing parameters"
    };
    Ok((!all.is_empty()).then(|| crate::codeactions::CombinedCodeActions {
        description: description.to_string(),
        changes: Vec::new(),
        changes_by_file: all.into_iter().collect(),
    }))
}
