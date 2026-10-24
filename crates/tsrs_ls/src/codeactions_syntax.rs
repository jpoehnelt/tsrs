//! Local syntax repairs from the TypeScript 5.9 code-fix providers. Decisions use
//! compiler nodes and symbols; edits go through the native change tracker.
use crate::codeactions::{CodeAction, CodeFixContext, CodeFixProvider, CombinedCodeActions};
use crate::{astnav, change, lsconv};
use rustc_hash::FxHashSet;
use std::sync::Arc;
use tsrs_ast::{self as ast, Kind, Node, SourceFile};
use tsrs_core::context::Context;
use tsrs_core::{P, TextRange};
use tsrs_lsproto as lsp;

pub static SYNTAX_PROVIDER: CodeFixProvider = CodeFixProvider {
    requires_auto_imports: false,
    error_codes: &[
        1378, 17004, 2610, 2611, 17009, 2515, 2322, 2339, 2345, 2355, 2349, 2564, 2689, 2810, 2794,
        2375, 1337, 2690, 80005, 80003, 80001, 80002, 80006, 1363, 1484, 2459, 1272, 1541, 1542,
        1005, 1064, 1205, 1312, 1340, 1375, 1382, 1381, 1431, 1451, 1497, 2376, 2377, 2348, 2352,
        2612, 2657, 2713, 2774, 2845, 2853, 4111, 4112, 4113, 4114, 4115, 4116, 4117, 4119, 4120,
        4121, 4122, 5084, 5085, 5086, 5087, 6234, 7028, 7051, 80007, 80008, 8024, 80004, 80009,
        17019, 17020, 8020, 2304, 18004, 1329,
    ],
    get_code_actions: actions,
    fix_ids: &[
        "convertFunctionToEs6Class",
        "convertToAsyncFunction",
        "fixPropertyOverrideAccessor",
        "fixClassDoesntImplementInheritedAbstractMember",
        "useDefaultImport",
        "splitTypeOnlyImport",
        "convertToTypeOnlyImport",
        "addMissingResolutionModeImportAttribute",
        "requireInTs",
        "fixImportNonExportedMember",
        "addMissingAwait",
        "addMissingAsync",
        "fixAddReturnStatement",
        "fixRemoveBracesFromArrowFunctionBody",
        "fixWrapTheBlockWithParen",
        "addMissingConstraint",
        "addVoidToPromise",
        "addMissingPropertyUndefinedType",
        "addMissingPropertyDefiniteAssignmentAssertions",
        "extendsInterfaceBecomesImplements",
        "fixConvertToMappedObjectType",
        "convertLiteralTypeToMappedType",
        "addMissingNewOperator",
        "fixNaNEquality",
        "fixPropertyAssignment",
        "fixUnusedLabel",
        "fixMissingCallParentheses",
        "removeAccidentalCallParentheses",
        "removeUnnecessaryAwait",
        "fixExpectedComma",
        "useBigintLiteral",
        "addMissingConst",
        "addConvertToUnknownForNonOverlappingTypes",
        "fixOverrideModifier",
        "fixAddOverrideModifier",
        "fixRemoveOverrideModifier",
        "addNameToNamelessParameter",
        "correctQualifiedNameToIndexedAccessType",
        "fixIncorrectNamedTupleSyntax",
        "fixReturnTypeInAsyncFunction",
        "addMissingDeclareProperty",
        "addEmptyExportDeclaration",
        "fixAddModuleReferTypeMissingTypeof",
        "wrapDecoratorInParentheses",
        "addMissingInvocationForDecorator",
        "convertToTypeOnlyExport",
        "fixNoPropertyAccessFromIndexSignature",
        "fixInvalidJsxCharacters_expression",
        "fixInvalidJsxCharacters_htmlEntity",
        "wrapJsxInFragment",
        "classSuperMustPrecedeThisAccess",
        "constructorForDerivedNeedSuperCall",
        "deleteUnmatchedParameter",
        "renameUnmatchedParameter",
        "annotateWithTypeFromJSDoc",
        "convertTypedefToType",
        "fixJSDocTypes_plain",
        "fixJSDocTypes_nullable",
    ],
    get_all_code_actions: Some(all_actions),
};
pub enum Edit {
    Replace(P<Node>, P<Node>),
    Raw(TextRange, String),
    Delete(P<Node>),
    Before(P<Node>, Kind),
    Jsdoc(P<Node>, Vec<P<Node>>),
    ConstructorStart(P<Node>, P<Node>),
    Type(P<Node>, P<Node>),
    After(P<Node>, P<Node>),
    Member(P<Node>, P<Node>),
    Nodes(P<Node>, Vec<P<Node>>),
    Insert(u32, P<Node>),
}
pub struct Plan {
    pub(crate) name: &'static str,
    pub(crate) id: &'static str,
    pub(crate) description: String,
    pub(crate) edits: Vec<Edit>,
    pub(crate) file: Option<P<SourceFile>>,
}
pub fn plan(name: &'static str, description: impl Into<String>, edits: Vec<Edit>) -> Plan {
    Plan { name, id: name, description: description.into(), edits, file: None }
}
fn tracker(ctx: &Context, fix: &CodeFixContext) -> change::Tracker {
    change::new_tracker(
        ctx,
        &fix.program.options(),
        fix.ls.format_options(),
        Arc::clone(&fix.ls.converters),
    )
}
pub fn ancestor(node: P<Node>, kinds: &[Kind]) -> Option<P<Node>> {
    ast::find_ancestor(node, |n| kinds.contains(&n.kind()))
}
fn start(file: P<SourceFile>, node: P<Node>) -> u32 {
    astnav::get_start_of_node(node, file, false)
}
fn end(node: P<Node>) -> u32 {
    node.end().expect_source("parsed repair node")
}
pub fn range(file: P<SourceFile>, node: P<Node>) -> TextRange {
    TextRange::new(start(file, node), end(node))
}
pub fn text(file: P<SourceFile>, node: P<Node>) -> &'static str {
    &file.text()[start(file, node) as usize..end(node) as usize]
}
fn insert(pos: u32, value: &str) -> Edit {
    Edit::Raw(TextRange::new(pos, pos), value.into())
}
fn plans(ctx: &Context, fix: &CodeFixContext, t: &change::Tracker) -> Vec<Plan> {
    let f = fix.source_file;
    let n = astnav::get_token_at_position(f, fix.span.pos().expect_source("diagnostic span"));
    let make = || -> Option<Plan> {
        Some(match fix.error_code {
            2348 => {
                let call = ancestor(n, &[Kind::CallExpression])?;
                let replacement = t.new_new_expression(
                    call.expression()?,
                    call.type_argument_list(),
                    Some(call.argument_list()?),
                );
                plan(
                    "addMissingNewOperator",
                    "Add missing 'new' operator to call",
                    vec![Edit::Replace(call, replacement)],
                )
            }
            2845 => {
                let binary = ancestor(n, &[Kind::BinaryExpression])?;
                let b = binary.as_binary_expression();
                let (value, other) = if ast::is_identifier(b.left) && b.left.text() == "NaN" {
                    (b.right.get(), b.left)
                } else {
                    (b.left, b.right.get())
                };
                if !ast::is_identifier(other) || other.text() != "NaN" {
                    return None;
                }
                let callee = t.new_property_access_expression(
                    t.new_identifier("Number"),
                    None,
                    t.new_identifier("isNaN"),
                    ast::NodeFlags::None,
                );
                let mut replacement = t.new_call_expression(
                    callee,
                    None,
                    None,
                    t.new_node_list(vec![value]),
                    ast::NodeFlags::None,
                );
                if matches!(
                    b.operator_token.kind(),
                    Kind::ExclamationEqualsToken | Kind::ExclamationEqualsEqualsToken
                ) {
                    replacement =
                        t.new_prefix_unary_expression(Kind::ExclamationToken, replacement);
                }
                plan(
                    "fixNaNEquality",
                    "Use `Number.isNaN(...)`.",
                    vec![Edit::Replace(binary, replacement)],
                )
            }
            1312 => {
                let property = ancestor(n, &[Kind::ShorthandPropertyAssignment])?;
                let value = property
                    .as_shorthand_property_assignment()
                    .object_assignment_initializer
                    .get()?;
                let replacement =
                    t.new_property_assignment(None, property.name()?, None, None, value);
                plan(
                    "fixPropertyAssignment",
                    "Change '=' to ':'",
                    vec![Edit::Replace(property, replacement)],
                )
            }
            7028 => {
                let label = ancestor(n, &[Kind::LabeledStatement])?;
                let statement = label.as_labeled_statement().statement;
                let a = start(f, label);
                let b = start(f, statement);
                let b = if f.text()[a as usize..b as usize].contains('\n') {
                    let colon = astnav::find_child_of_kind(label, Kind::ColonToken, f)?;
                    let mut at = end(colon) as usize;
                    while matches!(f.text().as_bytes().get(at), Some(b' ' | b'\t')) {
                        at += 1;
                    }
                    at as u32
                } else {
                    b
                };
                plan(
                    "fixUnusedLabel",
                    "Remove unused label",
                    vec![Edit::Raw(TextRange::new(a, b), String::new())],
                )
            }
            2774 => {
                let access = ancestor(n, &[Kind::PropertyAccessExpression]).unwrap_or(n);
                let name = access.name().unwrap_or(access);
                if !ast::is_identifier(name) {
                    return None;
                }
                plan(
                    "fixMissingCallParentheses",
                    "Add missing call parentheses",
                    vec![Edit::Raw(range(f, name), format!("{}()", name.text()))],
                )
            }
            6234 => {
                let call = ancestor(n, &[Kind::CallExpression])?;
                plan(
                    "removeAccidentalCallParentheses",
                    "Remove parentheses",
                    vec![Edit::Raw(
                        TextRange::new(end(call.expression()?), end(call)),
                        String::new(),
                    )],
                )
            }
            80007 => {
                let await_node = ancestor(n, &[Kind::AwaitExpression])?;
                let mut target = await_node;
                if let Some(parent) =
                    await_node.parent().filter(|p| p.kind() == Kind::ParenthesizedExpression)
                    && parent.parent().is_none_or(|p| p.kind() != Kind::NewExpression)
                {
                    target = parent;
                }
                plan(
                    "removeUnnecessaryAwait",
                    "Remove unnecessary 'await'",
                    vec![Edit::Replace(target, await_node.expression()?)],
                )
            }
            1005 if n.kind() == Kind::SemicolonToken
                && n.parent().is_some_and(|p| {
                    matches!(p.kind(), Kind::ObjectLiteralExpression | Kind::ArrayLiteralExpression)
                }) =>
            {
                plan(
                    "fixExpectedComma",
                    "Change ';' to ','",
                    vec![Edit::Raw(range(f, n), ",".into())],
                )
            }
            80008 if n.kind() == Kind::NumericLiteral => plan(
                "useBigintLiteral",
                "Convert to a bigint numeric literal",
                vec![Edit::Raw(range(f, n), format!("{}n", text(f, n)))],
            ),
            2304 | 18004 => {
                let parent = n.parent()?;
                let assign = parent.kind() == Kind::BinaryExpression
                    && parent.as_binary_expression().left == n
                    && parent.as_binary_expression().operator_token.kind() == Kind::EqualsToken
                    && parent.parent().is_some_and(ast::is_expression_statement);
                let for_binding =
                    parent.kind() == Kind::ForOfStatement || parent.kind() == Kind::ForInStatement;
                if !(assign || for_binding) {
                    return None;
                }
                let mut c = fix.program.get_type_checker_for_file(ctx, f);
                if c.get_symbol_at_location_exported(n).is_some() {
                    return None;
                }
                plan(
                    "addMissingConst",
                    "Add 'const' to unresolved variable",
                    vec![Edit::Before(n, Kind::ConstKeyword)],
                )
            }
            2352 => {
                let assertion = ancestor(n, &[Kind::AsExpression, Kind::TypeAssertionExpression])?;
                let value = assertion.expression()?;
                let converted =
                    t.new_as_expression(value, t.new_keyword_type_node(Kind::UnknownKeyword));
                plan(
                    "addConvertToUnknownForNonOverlappingTypes",
                    "Add 'unknown' conversion for non-overlapping types",
                    vec![Edit::Replace(value, converted)],
                )
            }
            7051 => {
                let parameter = ancestor(n, &[Kind::Parameter])?;
                let index =
                    parameter.parent()?.parameters().iter().position(|p| *p == parameter)?;
                plan(
                    "addNameToNamelessParameter",
                    "Add parameter name",
                    vec![Edit::Raw(
                        range(f, parameter),
                        format!("arg{index}: {}", text(f, parameter)),
                    )],
                )
            }
            2713 => {
                let qualified = ancestor(n, &[Kind::QualifiedName])?;
                let q = qualified.as_qualified_name();
                let replacement = format!("{}[\"{}\"]", text(f, q.left), q.right.text());
                plan(
                    "correctQualifiedNameToIndexedAccessType",
                    format!("Rewrite as the indexed access type '{replacement}'"),
                    vec![Edit::Raw(range(f, qualified), replacement)],
                )
            }
            5084..=5087 => {
                let tuple = ancestor(n, &[Kind::NamedTupleMember])?;
                let mut typ = tuple.type_node()?;
                let optional = typ.kind() == Kind::OptionalType;
                let rest = typ.kind() == Kind::RestType;
                if optional || rest {
                    typ = typ.type_node()?;
                }
                let m = tuple.as_named_tuple_member();
                let replacement = t.new_named_tuple_member(
                    if rest {
                        Some(t.new_token(Kind::DotDotDotToken))
                    } else {
                        m.dot_dot_dot_token
                    },
                    tuple.name()?,
                    if optional {
                        Some(t.new_token(Kind::QuestionToken))
                    } else {
                        m.question_token
                    },
                    typ,
                );
                plan(
                    "fixIncorrectNamedTupleSyntax",
                    "Move labeled tuple element modifiers to labels",
                    vec![Edit::Replace(tuple, replacement)],
                )
            }
            1064 => {
                let function = ast::find_ancestor(n, ast::is_function_like_declaration)?;
                if ast::is_in_js_file(function) {
                    return None;
                }
                let typ = function.type_node()?;
                let replacement = t.new_type_reference_node(
                    t.new_identifier("Promise"),
                    Some(t.new_node_list(vec![typ])),
                );
                plan(
                    "fixReturnTypeInAsyncFunction",
                    format!("Replace '{}' with 'Promise<{}>'", text(f, typ), text(f, typ)),
                    vec![Edit::Replace(typ, replacement)],
                )
            }
            2612 => {
                let property = ancestor(n, &[Kind::PropertyDeclaration])?;
                plan(
                    "addMissingDeclareProperty",
                    "Prefix with 'declare'",
                    vec![Edit::Before(property, Kind::DeclareKeyword)],
                )
            }
            4112 | 4113 | 4117 | 4119 | 4120 | 4121 | 4122 => {
                let member = ast::find_ancestor(n, ast::is_class_element)?;
                let modifier = member
                    .modifiers()?
                    .nodes()
                    .iter()
                    .copied()
                    .find(|p| p.kind() == Kind::OverrideKeyword)?;
                let mut p = plan(
                    "fixOverrideModifier",
                    "Remove 'override' modifier",
                    vec![Edit::Raw(
                        TextRange::new(start(f, modifier), start(f, member.name()?)),
                        String::new(),
                    )],
                );
                p.id = "fixRemoveOverrideModifier";
                p
            }
            4114..=4116 => {
                let member = ast::find_ancestor(n, ast::is_class_element)?;
                let mut p = plan(
                    "fixOverrideModifier",
                    "Add 'override' modifier",
                    vec![Edit::Before(member, Kind::OverrideKeyword)],
                );
                p.id = "fixAddOverrideModifier";
                p
            }
            1375 | 1431 | 2853 => plan(
                "addEmptyExportDeclaration",
                "Add 'export {}' to make this file into a module",
                vec![insert(f.text().len() as u32, "\n\nexport { };\n")],
            ),
            1340 => {
                let typ = ancestor(n, &[Kind::ImportType])?;
                plan(
                    "fixAddModuleReferTypeMissingTypeof",
                    "Add missing 'typeof'",
                    vec![Edit::Raw(range(f, typ), format!("typeof {}", text(f, typ)))],
                )
            }
            1497 => {
                let decorator = ancestor(n, &[Kind::Decorator])?;
                let value = decorator.expression()?;
                plan(
                    "wrapDecoratorInParentheses",
                    "Wrap in parentheses",
                    vec![Edit::Replace(value, t.new_parenthesized_expression(value))],
                )
            }
            1329 => {
                let decorator = ancestor(n, &[Kind::Decorator])?;
                let value = decorator.expression()?;
                plan(
                    "addMissingInvocationForDecorator",
                    "Call decorator expression",
                    vec![Edit::Replace(
                        value,
                        t.new_call_expression(
                            value,
                            None,
                            None,
                            t.new_node_list(vec![]),
                            ast::NodeFlags::None,
                        ),
                    )],
                )
            }
            1205 => {
                let export = ancestor(n, &[Kind::ExportDeclaration])?;
                let token = astnav::find_child_of_kind(export, Kind::ExportKeyword, f)?;
                plan(
                    "convertToTypeOnlyExport",
                    "Convert to type-only export",
                    vec![insert(end(token) + 1, "type ")],
                )
            }
            4111 => {
                let access = ancestor(n, &[Kind::PropertyAccessExpression])?;
                let name = access.name()?;
                plan(
                    "fixNoPropertyAccessFromIndexSignature",
                    format!("Use element access for '{}'", name.text()),
                    vec![Edit::Raw(
                        range(f, access),
                        format!("{}[\"{}\"]", text(f, access.expression()?), name.text()),
                    )],
                )
            }
            2657 => {
                let a = fix.span.pos().expect_source("JSX start");
                let b = fix.span.end().expect_source("JSX end");
                plan(
                    "wrapJsxInFragment",
                    "Wrap in JSX fragment",
                    vec![Edit::Raw(
                        fix.span,
                        format!("<>{}</>", &f.text()[a as usize..b as usize]),
                    )],
                )
            }
            2377 => {
                let ctr = ancestor(n, &[Kind::Constructor])?;
                let call = t.new_call_expression(
                    t.new_token(Kind::SuperKeyword),
                    None,
                    None,
                    t.new_node_list(vec![]),
                    ast::NodeFlags::None,
                );
                plan(
                    "constructorForDerivedNeedSuperCall",
                    "Add missing 'super()' call",
                    vec![Edit::ConstructorStart(ctr, t.new_expression_statement(call))],
                )
            }
            17009 => {
                let ctr = ancestor(n, &[Kind::Constructor])?;
                let stmt = find_super_call(ctr.body()?)?;
                plan(
                    "classSuperMustPrecedeThisAccess",
                    "Make 'super()' call the first statement in the constructor",
                    vec![Edit::ConstructorStart(ctr, stmt), Edit::Delete(stmt)],
                )
            }
            _ => return None,
        })
    };
    if matches!(fix.error_code, 8024 | 80004 | 80009 | 17019 | 17020 | 8020) {
        return crate::codeactions_jsdoc::plans(ctx, fix, t);
    }
    if matches!(fix.error_code, 1381 | 1382) {
        let at = fix.span.pos().expect_source("JSX character");
        let Some(character) = f.text().get(at as usize..).and_then(|s| s.chars().next()) else {
            return vec![];
        };
        let span = TextRange::new(at, at + character.len_utf8() as u32);
        return vec![
            plan(
                "fixInvalidJsxCharacters_expression",
                "Wrap invalid character in an expression container",
                vec![Edit::Raw(span, format!("{{\"{character}\"}}"))],
            ),
            plan(
                "fixInvalidJsxCharacters_htmlEntity",
                "Convert invalid character to its html entity code",
                vec![Edit::Raw(span, if character == '>' { "&gt;" } else { "&rbrace;" }.into())],
            ),
        ];
    }
    let mut result: Vec<_> = make().into_iter().collect();
    result.extend(crate::codeactions_semantic::plans(ctx, fix, t));
    result.extend(crate::codeactions_modules::plans(ctx, fix, t));
    result.extend(crate::codeactions_conversions::plans(ctx, fix, t));
    result.extend(crate::codeactions_config::plans(ctx, fix, t));
    result
}
fn apply(t: &mut change::Tracker, f: P<SourceFile>, edit: &Edit) {
    match edit {
        Edit::Insert(pos, node) => t.insert_node_at(
            f,
            *pos,
            *node,
            change::NodeOptions { suffix: "\n".into(), ..Default::default() },
        ),
        Edit::Nodes(old, nodes) => t.replace_node_with_nodes(f, *old, nodes.clone(), None),
        Edit::Member(class, member) => t.insert_member_at_start(f, *class, *member),
        Edit::After(old, new) => t.insert_node_after(f, *old, *new),
        Edit::Type(n, typ) => {
            t.try_insert_type_annotation(f, *n, *typ);
        }
        Edit::Replace(old, new) => t.replace_node(f, *old, *new, None),
        Edit::Raw(span, text) => t.replace_text_range_with_text(f, *span, text),
        Edit::Delete(n) => t.delete(f, *n),
        Edit::Before(n, kind) => t.insert_modifier_before(f, *kind, *n),
        Edit::Jsdoc(n, tags) => crate::codeactions_usage::update_jsdoc(t, f, *n, tags, false),
        Edit::ConstructorStart(ctr, stmt) => {
            let body = ctr.body().unwrap();
            if let Some(first) = body.statements().first() {
                let at = t.get_adjusted_start_position(
                    f,
                    *first,
                    change::LeadingTriviaOption::None,
                    false,
                );
                t.insert_node_at(
                    f,
                    at,
                    *stmt,
                    change::NodeOptions {
                        indentation: Some(t.format_settings.indent_size),
                        suffix: "\n".into(),
                        ..Default::default()
                    },
                );
            } else {
                t.replace_node(f, body, t.new_block(t.new_node_list(vec![*stmt]), true), None);
            }
        }
    }
}
fn edits(t: &mut change::Tracker) -> Vec<(lsp::DocumentUri, Vec<lsp::TextEdit>)> {
    t.get_changes()
        .0
        .into_iter()
        .map(|(file, edits)| (lsconv::file_name_to_document_uri(&file), edits))
        .collect()
}
#[expect(clippy::unnecessary_wraps, reason = "Provider callback signature")]
fn actions(ctx: &Context, fix: &CodeFixContext) -> Result<Vec<CodeAction>, lsp::Error> {
    let factory = tracker(ctx, fix);
    Ok(plans(ctx, fix, &factory)
        .into_iter()
        .map(|p| {
            let mut t = tracker(ctx, fix);
            // Synthetic-node metadata belongs to the planner emit context. Share it
            // while keeping every alternative action's edit list independent.
            t.emit_context = factory.emit_context;
            t.node_factory = factory.node_factory.clone();
            for e in &p.edits {
                apply(&mut t, p.file.unwrap_or(fix.source_file), e);
            }
            CodeAction {
                description: p.description,
                fix_name: Some(p.name.into()),
                fix_id: p.id.into(),
                changes_by_file: edits(&mut t),
                ..Default::default()
            }
        })
        .collect())
}
#[expect(clippy::unnecessary_wraps, reason = "Provider callback signature")]
fn all_actions(
    ctx: &Context,
    fix: &CodeFixContext,
) -> Result<Option<CombinedCodeActions>, lsp::Error> {
    let mut t = tracker(ctx, fix);
    let mut seen = FxHashSet::default();
    let mut candidates: Vec<_> =
        crate::diagnostics::get_all_diagnostics(ctx, fix.program, fix.source_file)
            .into_iter()
            .filter(|d| SYNTAX_PROVIDER.error_codes.contains(&d.code()))
            .filter_map(|d| d.loc().map(|span| (span, d.code())))
            .collect();
    // Some TS5 editor suggestions do not have a TS7 diagnostic. Enumerate their
    // syntax eligibility for explicit fix-all requests rather than inventing diagnostics.
    collect_fix_all_candidates(
        fix.source_file.as_node(),
        fix.source_file,
        fix.fix_id.unwrap_or(""),
        &mut candidates,
    );
    for (span, code) in candidates {
        let local = CodeFixContext { span, error_code: code, ..*fix };
        for p in plans(ctx, &local, &t).into_iter().filter(|p| Some(p.id) == fix.fix_id) {
            let file = p.file.unwrap_or(fix.source_file);
            let Some(first) = p.edits.first() else {
                continue;
            };
            let key = match first {
                Edit::Insert(pos, _) => TextRange::new(*pos, *pos),
                Edit::Nodes(n, _)
                | Edit::Member(n, _)
                | Edit::After(n, _)
                | Edit::Type(n, _)
                | Edit::Replace(n, _)
                | Edit::Delete(n)
                | Edit::Before(n, _)
                | Edit::Jsdoc(n, _)
                | Edit::ConstructorStart(n, _) => range(file, *n),
                Edit::Raw(span, _) => *span,
            };
            if seen.insert((file.file_name().to_string(), key.pos(), key.end())) {
                for edit in &p.edits {
                    apply(&mut t, file, edit);
                }
            }
        }
    }
    Ok(Some(CombinedCodeActions { changes_by_file: edits(&mut t), ..Default::default() }))
}

fn find_super_call(n: P<Node>) -> Option<P<Node>> {
    if ast::is_expression_statement(n)
        && n.expression().is_some_and(|e| {
            ast::is_call_expression(e)
                && e.expression().is_some_and(|x| x.kind() == Kind::SuperKeyword)
        })
    {
        return Some(n);
    }
    if ast::is_function_like(n) && n.kind() != Kind::Block {
        return None;
    }
    let mut result = None;
    n.for_each_child(&mut |child| {
        result = find_super_call(child);
        result.is_some()
    });
    result
}

fn collect_fix_all_candidates(
    n: P<Node>,
    file: P<SourceFile>,
    id: &str,
    out: &mut Vec<(TextRange, i32)>,
) {
    let code = match id {
        "annotateWithTypeFromJSDoc"
            if ast::is_function_like_declaration(n) && !n.jsdoc(None).is_empty() =>
        {
            Some(80004)
        }
        "requireInTs"
            if ast::is_variable_declaration(n)
                && n.initializer().is_some_and(|v| {
                    ast::is_call_expression(v)
                        && v.expression()
                            .is_some_and(|e| ast::is_identifier(e) && e.text() == "require")
                }) =>
        {
            Some(80005)
        }
        "useDefaultImport" if n.kind() == Kind::NamespaceImport => Some(80003),
        "convertFunctionToEs6Class" if n.kind() == Kind::FunctionDeclaration => Some(80002),
        "convertToAsyncFunction" if ast::is_function_like_declaration(n) => Some(80006),
        _ => None,
    };
    if let Some(code) = code {
        out.push((range(file, n.name().unwrap_or(n)), code));
    }
    for &doc in n.jsdoc(None) {
        if let Some(tags) = doc.as_jsdoc().tags {
            for &tag in tags.nodes() {
                if id == "convertTypedefToType" && tag.kind() == Kind::JSDocTypedefTag {
                    out.push((range(file, tag), 80009));
                }
            }
        }
    }
    n.for_each_child(&mut |child| {
        collect_fix_all_candidates(child, file, id, out);
        false
    });
}
