//! Declaration, qualification, async and unreachable-code editor fixes.
use crate::codeactions::{CodeAction, CodeFixContext, CodeFixProvider, CombinedCodeActions};
use crate::{astnav, change, jsdoc, lsconv};
use rustc_hash::FxHashSet;
use std::sync::Arc;
use tsrs_ast::{self as ast, Kind, Node, SourceFile};
use tsrs_core::context::Context;
use tsrs_core::{P, TextRange};
use tsrs_lsproto as lsp;

pub static CLEANUP_PROVIDER: CodeFixProvider = CodeFixProvider {
    requires_auto_imports: false,
    error_codes: &[
        2588, 1308, 2852, 1103, 2311, 2663, 2662, 1451, 7027, 6133, 6138, 6192, 6196, 6198, 6199,
        6205,
    ],
    get_code_actions: actions,
    fix_ids: &[
        "fixConvertConstToLet",
        "fixAwaitInSyncFunction",
        "forgottenThisPropertyAccess",
        "fixUnreachableCode",
        "unusedIdentifier_prefix",
        "unusedIdentifier_delete",
        "unusedIdentifier_deleteImports",
        "unusedIdentifier_infer",
    ],
    get_all_code_actions: Some(all_actions),
};

enum Edit {
    Const(P<Node>),
    Async { before: P<Node>, return_type: Option<P<Node>> },
    Qualify { node: P<Node>, class_name: Option<P<Node>> },
    Delete(P<Node>),
    DeleteUnused { name: P<Node>, declaration: P<Node> },
    DeleteRange { first: P<Node>, last: P<Node> },
    EmptyBlock(P<Node>),
    Prefix(P<Node>),
    Infer(P<Node>),
    Destructure(P<Node>),
    TypeParameters(P<Node>),
}
struct Plan {
    edit: Edit,
    name: &'static str,
    id: &'static str,
    description: String,
    all_description: &'static str,
}
impl Plan {
    fn key(&self) -> P<Node> {
        match self.edit {
            Edit::Const(n)
            | Edit::Delete(n)
            | Edit::EmptyBlock(n)
            | Edit::Prefix(n)
            | Edit::Infer(n)
            | Edit::Destructure(n)
            | Edit::TypeParameters(n) => n,
            Edit::Async { before, .. } => before,
            Edit::Qualify { node, .. } => node,
            Edit::DeleteRange { first, .. } => first,
            Edit::DeleteUnused { declaration, .. } => declaration,
        }
    }
}
fn plan(
    edit: Edit,
    name: &'static str,
    id: &'static str,
    description: impl Into<String>,
    all_description: &'static str,
) -> Plan {
    Plan { edit, name, id, description: description.into(), all_description }
}
fn tracker(ctx: &Context, fix: &CodeFixContext) -> change::Tracker {
    change::new_tracker(
        ctx,
        &fix.program.options(),
        fix.ls.format_options(),
        Arc::clone(&fix.ls.converters),
    )
}
fn plans(ctx: &Context, fix: &CodeFixContext, all: bool) -> Vec<Plan> {
    let node = astnav::get_token_at_position(
        fix.source_file,
        fix.span.pos().expect_source("diagnostic position"),
    );
    let single = match fix.error_code {
        2588 => {
            let mut c = fix.program.get_type_checker_for_file(ctx, fix.source_file);
            c.get_editor_symbol_at_location(node)
                .and_then(|symbol| symbol.value_declaration())
                .and_then(|d| d.parent())
                .filter(|d| ast::is_variable_declaration_list(*d))
                .and_then(|d| {
                    astnav::find_child_of_kind(
                        d,
                        Kind::ConstKeyword,
                        ast::get_source_file_of_node(d).unwrap(),
                    )
                })
                .map(|n| {
                    plan(
                        Edit::Const(n),
                        "fixConvertConstToLet",
                        "fixConvertConstToLet",
                        "Convert 'const' to 'let'",
                        "Convert all 'const' to 'let'",
                    )
                })
        }
        1308 | 2852 | 1103 | 2311 => async_plan(fix.source_file, node),
        2663 | 2662 | 1451 if ast::is_identifier(node) || ast::is_private_identifier(node) => {
            let class_name =
                (fix.error_code == 2662).then(|| ast::get_containing_class(node)?.name()).flatten();
            if fix.error_code == 2662 && class_name.is_none() {
                return Vec::new();
            }
            Some(plan(
                Edit::Qualify { node, class_name },
                "forgottenThisPropertyAccess",
                "forgottenThisPropertyAccess",
                format!(
                    "Add '{}.' to unresolved variable",
                    class_name.map_or("this", |n| n.text())
                ),
                "Add qualifier to all unresolved variables matching a member name",
            ))
        }
        7027 => unreachable_plan(ctx, fix, node),
        6133 | 6138 | 6192 | 6196 | 6198 | 6199 | 6205 => return unused_plans(ctx, fix, node, all),
        _ => None,
    };
    single.into_iter().collect()
}
fn async_plan(file: P<SourceFile>, node: P<Node>) -> Option<Plan> {
    let function = ast::get_containing_function(node)?;
    let before = match function.kind() {
        Kind::MethodDeclaration => function.name(),
        Kind::FunctionDeclaration | Kind::FunctionExpression => {
            astnav::find_child_of_kind(function, Kind::FunctionKeyword, file)
        }
        Kind::ArrowFunction => astnav::find_child_of_kind(
            function,
            if function.type_parameters().is_empty() {
                Kind::OpenParenToken
            } else {
                Kind::LessThanToken
            },
            file,
        )
        .or_else(|| function.parameters().first().copied()),
        _ => None,
    }?;
    let return_type = function.type_node().or_else(|| {
        let parent = function.parent()?;
        let typ = parent.type_node()?;
        (ast::is_variable_declaration(parent) && typ.kind() == Kind::FunctionType)
            .then(|| typ.type_node())
            .flatten()
    });
    Some(plan(
        Edit::Async { before, return_type },
        "fixAwaitInSyncFunction",
        "fixAwaitInSyncFunction",
        "Add async modifier to containing function",
        "Add all missing async modifiers",
    ))
}
fn unreachable_plan(ctx: &Context, fix: &CodeFixContext, node: P<Node>) -> Option<Plan> {
    if !fix.program.get_syntactic_diagnostics(ctx, Some(fix.source_file)).is_empty() {
        return None;
    }
    let statement = ast::find_ancestor(node, ast::is_statement)?;
    let parent = statement.parent()?;
    let block = ast::is_block(parent);
    let container = if block { parent.parent()? } else { parent };
    let edit = if !block || parent.statements().first() == Some(&statement) {
        match container.kind() {
            Kind::IfStatement if container.as_if_statement().else_statement().is_some() => {
                if block {
                    unreachable_range(fix, statement, parent)
                } else {
                    Edit::EmptyBlock(statement)
                }
            }
            Kind::IfStatement | Kind::WhileStatement | Kind::ForStatement => {
                Edit::Delete(container)
            }
            _ if block => unreachable_range(fix, statement, parent),
            _ => Edit::Delete(statement),
        }
    } else {
        unreachable_range(fix, statement, parent)
    };
    Some(plan(
        edit,
        "fixUnreachableCode",
        "fixUnreachableCode",
        "Remove unreachable code",
        "Remove all unreachable code",
    ))
}
fn unreachable_range(fix: &CodeFixContext, first: P<Node>, block: P<Node>) -> Edit {
    let end = fix.span.end();
    let last = block
        .statements()
        .iter()
        .copied()
        .skip_while(|n| *n != first)
        .take_while(|n| n.pos() < end)
        .last()
        .unwrap_or(first);
    Edit::DeleteRange { first, last }
}
fn unused_delete(edit: Edit, description: impl Into<String>) -> Plan {
    plan(
        edit,
        "unusedIdentifier",
        "unusedIdentifier_delete",
        description,
        "Delete all unused declarations",
    )
}
fn unused_plans(ctx: &Context, fix: &CodeFixContext, node: P<Node>, all: bool) -> Vec<Plan> {
    let Some(parent) = node.parent() else {
        return Vec::new();
    };
    let name = &fix.source_file.text()[astnav::get_start_of_node(node, fix.source_file, false)
        as usize
        ..node.end().expect_source("unused name") as usize];
    if node.kind() == Kind::JSDocTemplateTag {
        return vec![unused_delete(Edit::Delete(node), "Remove @template tag")];
    }
    if node.kind() == Kind::LessThanToken {
        return vec![unused_delete(Edit::TypeParameters(parent), "Remove type parameters")];
    }
    if node.kind() == Kind::ImportKeyword && ast::is_import_declaration(parent) {
        return vec![plan(
            Edit::Delete(parent),
            "unusedIdentifier",
            "unusedIdentifier_deleteImports",
            format!(
                "Remove import from '{}'",
                parent.as_import_declaration().module_specifier().text()
            ),
            "Delete all unused imports",
        )];
    }
    if ast::is_binding_pattern(parent) {
        let declaration = parent.parent().unwrap();
        if ast::is_parameter_declaration(declaration) {
            let elements = parent.as_binding_pattern().elements.nodes();
            let names = elements
                .iter()
                .map(|n| {
                    &fix.source_file.text()[astnav::get_start_of_node(*n, fix.source_file, false)
                        as usize
                        ..n.end().expect_source("binding") as usize]
                })
                .collect::<Vec<_>>()
                .join(", ");
            return vec![unused_delete(
                Edit::Destructure(parent),
                format!(
                    "Remove unused declaration{} for: '{names}'",
                    if elements.len() > 1 { "s" } else { "" }
                ),
            )];
        }
        if all && declaration.initializer().is_some() {
            return Vec::new();
        }
        return vec![unused_delete(
            Edit::Destructure(parent),
            "Remove unused destructuring declaration",
        )];
    }
    if ast::is_variable_declaration_list(parent) {
        return vec![unused_delete(
            Edit::Delete(
                parent.parent().filter(|p| ast::is_variable_statement(*p)).unwrap_or(parent),
            ),
            "Remove variable statement",
        )];
    }
    if node.kind() == Kind::InferKeyword {
        let parameter = parent.as_infer_type_node().type_parameter;
        return vec![plan(
            Edit::Infer(parent),
            "unusedIdentifier",
            "unusedIdentifier_infer",
            format!("Replace 'infer {}' with 'unknown'", parameter.name().unwrap().text()),
            "Replace all unused 'infer' with 'unknown'",
        )];
    }
    let mut result = Vec::new();
    let parameter = ast::is_parameter_declaration(parent);
    if (!parameter
        || can_delete_parameter(ctx, fix, parent, all)
            && parent.initializer().is_none()
            && parameter_not_provided(ctx, fix, parent))
        && !(all
            && parent.kind() == Kind::BindingElement
            && parent.parent().is_some_and(|n| {
                n.kind() == Kind::ArrayBindingPattern
                    && n.parent().is_some_and(|d| d.initializer().is_some())
            }))
    {
        let import = matches!(
            parent.kind(),
            Kind::ImportClause | Kind::ImportSpecifier | Kind::NamespaceImport
        );
        let edit = if parent.kind() == Kind::ImportClause {
            node
        } else if parent.kind() == Kind::ComputedPropertyName {
            parent.parent().unwrap()
        } else {
            parent
        };
        if !all || !ast::is_identifier(node) || symbol_references_in_file(ctx, fix, node).is_empty()
        {
            result.push(if import {
                plan(
                    Edit::Delete(edit),
                    "unusedIdentifier",
                    "unusedIdentifier_deleteImports",
                    format!("Remove unused declaration for: '{name}'"),
                    "Delete all unused imports",
                )
            } else {
                unused_delete(
                    if !all && ast::is_identifier(node) {
                        Edit::DeleteUnused { name: node, declaration: edit }
                    } else {
                        Edit::Delete(edit)
                    },
                    format!("Remove unused declaration for: '{name}'"),
                )
            });
        }
    }
    let can_prefix = parameter
        || ast::is_type_parameter_declaration(parent)
        || ast::is_variable_declaration(parent)
            && parent
                .parent()
                .and_then(|p| p.parent())
                .is_some_and(|p| matches!(p.kind(), Kind::ForInStatement | Kind::ForOfStatement));
    if fix.error_code != 6138 && ast::is_identifier(node) && can_prefix {
        result.push(plan(
            Edit::Prefix(node),
            "unusedIdentifier",
            "unusedIdentifier_prefix",
            format!("Prefix '{name}' with an underscore"),
            "Prefix all unused declarations with '_' where possible",
        ));
    }
    result
}
fn parameter_not_provided(ctx: &Context, fix: &CodeFixContext, parameter: P<Node>) -> bool {
    let function = parameter.parent().unwrap();
    let index = function.parameters().iter().position(|p| *p == parameter).unwrap();
    !fix.ls
        .get_signature_usages(ctx, function)
        .iter()
        .any(|usage| usage.call.is_none_or(|call| call.arguments().len() > index))
}
fn symbol_references_in_file(ctx: &Context, fix: &CodeFixContext, name: P<Node>) -> Vec<P<Node>> {
    fix.ls
        .get_referenced_symbols_for_node_exported(
            ctx,
            astnav::get_start_of_node(name, fix.source_file, false),
            name,
            &[fix.source_file],
        )
        .iter()
        .flat_map(|entry| entry.references().iter().filter_map(|reference| reference.node()))
        .filter(|n| *n != name)
        .collect()
}
fn can_delete_parameter(
    ctx: &Context,
    fix: &CodeFixContext,
    parameter: P<Node>,
    all: bool,
) -> bool {
    let function = parameter.parent().unwrap();
    match function.kind() {
        Kind::SetAccessor => false,
        Kind::FunctionExpression | Kind::ArrowFunction => {
            let index = function.parameters().iter().position(|p| *p == parameter).unwrap();
            if all {
                let c = fix.program.get_type_checker_for_file(ctx, fix.source_file);
                function.parameters()[index + 1..].iter().all(|p| {
                    p.name().is_some_and(|n| {
                        ast::is_identifier(n) && !n.symbol().is_some_and(|s| c.is_referenced(s))
                    })
                })
            } else {
                index + 1 == function.parameters().len()
            }
        }
        Kind::FunctionDeclaration => {
            !fix.ls.get_signature_usages(ctx, function).iter().any(|usage| usage.call.is_none())
                || function.parameters().last() == Some(&parameter)
        }
        _ => true,
    }
}
fn apply(ctx: &Context, fix: &CodeFixContext, changes: &mut change::Tracker, edit: &Edit) {
    let file = fix.source_file;
    match *edit {
        Edit::Const(token) => {
            let target = ast::get_source_file_of_node(token).unwrap();
            changes.replace_node(target, token, changes.new_token(Kind::LetKeyword), None);
        }
        Edit::Async { before, return_type } => {
            if let Some(typ) = return_type {
                let already_promise = typ.kind() == Kind::TypeReference
                    && typ.as_type_reference_node().type_name().kind() == Kind::Identifier
                    && typ.as_type_reference_node().type_name().text() == "Promise";
                if !already_promise {
                    let wrapped = changes.new_type_reference_node(
                        changes.new_identifier("Promise"),
                        Some(changes.new_node_list(vec![typ])),
                    );
                    changes.replace_node(file, typ, wrapped, None);
                }
            }
            changes.insert_modifier_before(file, Kind::AsyncKeyword, before);
        }
        Edit::Qualify { node, class_name } => {
            let receiver = class_name.map_or_else(
                || changes.new_token(Kind::ThisKeyword),
                |n| changes.new_identifier(n.text()),
            );
            let replacement =
                changes.new_property_access_expression(receiver, None, node, ast::NodeFlags::None);
            changes.replace_node(file, node, replacement, None);
        }
        Edit::Delete(n) => delete(changes, file, n),
        Edit::DeleteUnused { name, declaration } => {
            delete(changes, file, declaration);
            for mut reference in symbol_references_in_file(ctx, fix, name) {
                if reference.parent().is_some_and(|p| {
                    ast::is_property_access_expression(p) && p.name() == Some(reference)
                }) {
                    reference = reference.parent().unwrap();
                }
                let Some(parent) = reference.parent() else {
                    continue;
                };
                let assignment = parent.kind() == Kind::BinaryExpression
                    && parent.as_binary_expression().left == reference;
                let increment = match parent.kind() {
                    Kind::PrefixUnaryExpression => {
                        parent.as_prefix_unary_expression().operand == reference
                    }
                    Kind::PostfixUnaryExpression => {
                        parent.as_postfix_unary_expression().operand == reference
                    }
                    _ => false,
                };
                if (assignment || increment)
                    && parent.parent().is_some_and(ast::is_expression_statement)
                {
                    delete(changes, file, parent.parent().unwrap());
                }
            }
        }
        Edit::DeleteRange { first, last } => changes.delete_node_range(
            file,
            first,
            last,
            change::LeadingTriviaOption::IncludeAll,
            change::TrailingTriviaOption::None,
        ),
        Edit::EmptyBlock(n) => changes.replace_node(
            file,
            n,
            changes.new_block(changes.new_node_list(vec![]), false),
            None,
        ),
        Edit::Prefix(n) => {
            let name = format!("_{}", n.text());
            let replace = |changes: &mut change::Tracker, n: P<Node>| {
                changes.replace_text_range_with_text(
                    file,
                    TextRange::new(
                        astnav::get_start_of_node(n, file, false),
                        n.end().expect_source("identifier"),
                    ),
                    &name,
                );
            };
            replace(changes, n);
            if n.parent().is_some_and(ast::is_parameter_declaration) {
                let mut c = fix.program.get_type_checker_for_file(ctx, file);
                if let Some(tag) =
                    jsdoc::get_jsdoc_or_tag(&mut c, n.parent(), &mut FxHashSet::default())
                        .filter(|t| t.kind() == Kind::JSDocParameterTag)
                    && let Some(tag_name) = tag.name().filter(|t| ast::is_identifier(*t))
                {
                    replace(changes, tag_name);
                }
            }
        }
        Edit::Infer(n) => {
            changes.replace_node(
                file,
                n,
                changes.new_keyword_type_node(Kind::UnknownKeyword),
                None,
            );
        }
        Edit::TypeParameters(n) => {
            if let (Some(open), Some(close)) = (
                astnav::find_child_of_kind(n, Kind::LessThanToken, file),
                astnav::find_child_of_kind(n, Kind::GreaterThanToken, file),
            ) {
                changes.delete_node_range(
                    file,
                    open,
                    close,
                    change::LeadingTriviaOption::IncludeAll,
                    change::TrailingTriviaOption::None,
                );
            }
        }
        Edit::Destructure(pattern) => {
            let declaration = pattern.parent().unwrap();
            if ast::is_parameter_declaration(declaration) {
                for &element in pattern.as_binding_pattern().elements.nodes() {
                    changes.delete(file, element);
                }
            } else if declaration.initializer().is_some_and(ast::is_call_like_expression) {
                let list = declaration.parent().unwrap();
                let initializer = declaration.initializer().unwrap();
                if list.as_variable_declaration_list().declarations.nodes().len() > 1 {
                    changes.delete(file, declaration);
                    changes.insert_node_at(
                        file,
                        list.parent().unwrap().end().expect_source("variable statement"),
                        initializer,
                        change::NodeOptions {
                            prefix: "\n".into(),
                            suffix: ";".into(),
                            ..Default::default()
                        },
                    );
                } else {
                    changes.replace_node(file, list, initializer, None);
                }
            } else {
                delete(changes, file, declaration);
            }
        }
    }
}
fn delete(changes: &mut change::Tracker, file: P<SourceFile>, node: P<Node>) {
    if ast::is_property_declaration(node) {
        changes.delete_range(
            file,
            TextRange::new(
                node.pos().expect_source("property trivia"),
                node.end().expect_source("property end"),
            ),
        );
        return;
    }
    if ast::is_parameter_declaration(node)
        && node.parent().is_some_and(|f| {
            f.kind() == Kind::ArrowFunction
                && f.parameters().len() == 1
                && astnav::find_child_of_kind(f, Kind::OpenParenToken, file).is_none()
        })
    {
        changes.replace_text_range_with_text(
            file,
            TextRange::new(
                astnav::get_start_of_node(node, file, false),
                node.end().expect_source("arrow parameter"),
            ),
            "()",
        );
        return;
    }
    let node = if ast::is_variable_declaration(node)
        && node.parent().is_some_and(|p| {
            ast::is_variable_declaration_list(p)
                && p.as_variable_declaration_list().declarations.nodes().len() == 1
                && p.parent().is_some_and(ast::is_variable_statement)
        }) {
        node.parent().unwrap().parent().unwrap()
    } else {
        node
    };
    if ast::is_statement(node) {
        let leading = if matches!(
            node.kind(),
            Kind::VariableStatement | Kind::FunctionDeclaration | Kind::ClassDeclaration
        ) {
            if node.jsdoc(None).is_empty() {
                change::LeadingTriviaOption::StartLine
            } else {
                change::LeadingTriviaOption::JSDoc
            }
        } else {
            change::LeadingTriviaOption::IncludeAll
        };
        changes.delete_node(file, node, leading, change::TrailingTriviaOption::None);
    } else {
        changes.delete(file, node);
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
fn actions(ctx: &Context, fix: &CodeFixContext) -> Result<Vec<CodeAction>, lsp::Error> {
    Ok(plans(ctx, fix, false)
        .into_iter()
        .filter_map(|plan| {
            let mut changes = tracker(ctx, fix);
            apply(ctx, fix, &mut changes, &plan.edit);
            let changes_by_file = edits(&mut changes);
            (!changes_by_file.is_empty()).then(|| CodeAction {
                description: plan.description,
                fix_name: Some(plan.name.into()),
                fix_id: plan.id.into(),
                fix_all_description: plan.all_description.into(),
                changes_by_file,
                ..Default::default()
            })
        })
        .collect())
}
#[expect(clippy::unnecessary_wraps, reason = "Combined code-fix provider callback signature")]
fn all_actions(
    ctx: &Context,
    fix: &CodeFixContext,
) -> Result<Option<CombinedCodeActions>, lsp::Error> {
    let mut changes = tracker(ctx, fix);
    let mut seen = FxHashSet::default();
    let mut description = String::new();
    let mut pending = Vec::new();
    for diagnostic in crate::diagnostics::get_all_diagnostics(ctx, fix.program, fix.source_file) {
        if !crate::codeactions::is_fixable_diagnostic(diagnostic, CLEANUP_PROVIDER.error_codes) {
            continue;
        }
        let Some(span) = diagnostic.loc() else {
            continue;
        };
        let local = CodeFixContext { span, error_code: diagnostic.code(), ..*fix };
        for plan in plans(ctx, &local, true).into_iter().filter(|p| Some(p.id) == fix.fix_id) {
            if seen.insert(plan.key()) {
                pending.push((span, diagnostic.code(), plan));
            }
        }
    }
    if fix.fix_id == Some("unusedIdentifier_delete") {
        fn private_fields(n: P<Node>, out: &mut Vec<P<Node>>) {
            if ast::is_property_declaration(n) && n.name().is_some_and(ast::is_private_identifier) {
                out.push(n);
            }
            n.for_each_child(&mut |child| {
                private_fields(child, out);
                false
            });
        }
        let mut fields = Vec::new();
        private_fields(fix.source_file.as_node(), &mut fields);
        for field in fields {
            let name = field.name().unwrap();
            // An invalid bare private identifier is not a read of the instance field.
            if !symbol_references_in_file(ctx, fix, name).iter().any(|n| {
                n.parent()
                    .is_some_and(|p| ast::is_property_access_expression(p) && p.name() == Some(*n))
            }) {
                let span = TextRange::new(
                    astnav::get_start_of_node(name, fix.source_file, false),
                    name.end().expect_source("private declaration"),
                );
                let local = CodeFixContext { span, error_code: 6133, ..*fix };
                for p in plans(ctx, &local, true)
                    .into_iter()
                    .filter(|p| p.id == "unusedIdentifier_delete")
                {
                    if seen.insert(p.key()) {
                        pending.push((span, 6133, p));
                    }
                }
            }
        }
    }
    for (span, error_code, plan) in &pending {
        // A declaration deletion subsumes its parameters, locals and member diagnostics.
        // Keep the tracker overlap invariant: do not schedule competing edits at all.
        let key = plan.key();
        if pending.iter().any(|(_, _, ancestor)| {
            let (Edit::Delete(root) | Edit::DeleteUnused { declaration: root, .. }) = ancestor.edit
            else {
                return false;
            };
            root != key && ast::find_ancestor(key, |n| n == root).is_some()
        }) {
            continue;
        }
        let local = CodeFixContext { span: *span, error_code: *error_code, ..*fix };
        apply(ctx, &local, &mut changes, &plan.edit);
        description = plan.all_description.into();
    }
    let changes_by_file = edits(&mut changes);
    Ok((!changes_by_file.is_empty()).then(|| CombinedCodeActions {
        description,
        changes_by_file,
        ..Default::default()
    }))
}
