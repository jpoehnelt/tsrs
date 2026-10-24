//! Semantic repairs share the live compiler checker and the native edit planner.
use crate::codeactions::CodeFixContext;
use crate::codeactions_syntax::{Edit, Plan, ancestor, plan, range, text};
use crate::{astnav, change};
use tsrs_ast::{self as ast, Kind};
use tsrs_checker::{ContextFlags, SignatureKind};
use tsrs_core::TextRange;
use tsrs_core::context::Context;

pub fn plans(ctx: &Context, fix: &CodeFixContext, t: &change::Tracker) -> Vec<Plan> {
    let f = fix.source_file;
    let n = astnav::get_token_at_position(f, fix.span.pos().expect_source("repair diagnostic"));
    let mut out = Vec::new();
    let mut c = fix.program.get_type_checker_for_file(ctx, f);
    if matches!(fix.error_code, 2339 | 2345 | 2349) {
        let expr = if fix.error_code == 2339 {
            n.parent()
                .filter(|p| ast::is_property_access_expression(*p))
                .and_then(|p| p.expression())
        } else {
            ancestor(n, &[Kind::CallExpression]).or(Some(n))
        };
        if let Some(expr) = expr {
            let typ = c.get_type_at_location(expr);
            if c.get_promised_type_of_promise(typ).is_some()
                && ast::find_ancestor(expr, |p| {
                    ast::is_function_like(p)
                        && ast::has_syntactic_modifier(p, ast::ModifierFlags::Async)
                })
                .is_some()
            {
                let mut initializer = None;
                if ast::is_identifier(expr) {
                    initializer = c
                        .get_symbol_at_location_exported(expr)
                        .and_then(|s| s.value_declaration())
                        .filter(|d| ast::is_variable_declaration(*d) && d.type_node().is_none())
                        .and_then(|d| d.initializer().map(|v| (d, v)));
                }
                if let Some((d, value)) = initializer {
                    let mut p = plan(
                        "addMissingAwaitToInitializer",
                        format!("Add 'await' to initializer for '{}'", d.name().unwrap().text()),
                        vec![Edit::Replace(value, t.new_await_expression(value))],
                    );
                    p.id = if fix.fix_id.is_some() { "addMissingAwait" } else { "" };
                    out.push(p);
                }
                if fix.fix_id.is_none() || initializer.is_none() {
                    let mut value = t.new_await_expression(expr);
                    if expr.parent().is_some_and(|p| {
                        ast::is_property_access_expression(p)
                            || ast::is_element_access_expression(p)
                            || ast::is_call_expression(p) && p.expression() == Some(expr)
                    }) {
                        value = t.new_parenthesized_expression(value);
                    }
                    out.push(plan(
                        "addMissingAwait",
                        "Add 'await'",
                        vec![Edit::Replace(expr, value)],
                    ));
                }
            }
        }
    }
    let function = ast::find_ancestor(n, ast::is_function_like_declaration).or_else(|| {
        n.parent()
            .filter(|p| {
                ast::is_variable_declaration(*p)
                    || ast::is_property_declaration(*p)
                    || ast::is_parameter_declaration(*p)
            })
            .and_then(|p| p.initializer())
            .filter(|p| ast::is_function_like(*p))
    });
    if matches!(fix.error_code, 2322 | 2345 | 2355)
        && !(fix.error_code == 2355 && ast::is_in_js_file(n))
        && let Some(fun) = function
    {
        let expected = if fix.error_code == 2355 {
            fun.type_node().map(|ty| c.get_type_from_type_node(ty))
        } else {
            c.get_contextual_type(fun, ContextFlags::None)
                .and_then(|ty| {
                    c.get_signatures_of_type_exported(ty, SignatureKind::Call).first().copied()
                })
                .map(|sig| c.get_return_type_of_signature(sig))
        };
        if let Some(expected) = expected {
            if c.get_promised_type_of_promise(expected).is_some()
                && !ast::has_syntactic_modifier(fun, ast::ModifierFlags::Async)
            {
                let replacement = fun.clone_node(&t.node_factory);
                let mut mods = fun.modifiers().map(|m| m.nodes().to_vec()).unwrap_or_default();
                mods.push(t.new_token(Kind::AsyncKeyword));
                replacement.set_modifiers(Some(t.new_modifier_list(mods)));
                out.push(plan(
                    "addMissingAsync",
                    "Add async modifier to containing function",
                    vec![Edit::Replace(fun, replacement)],
                ));
            }
            if let Some(body) =
                fun.body().filter(|b| b.kind() == Kind::Block && b.statements().len() == 1)
            {
                let stmt = body.statements()[0];
                if ast::is_expression_statement(stmt) {
                    let expr = stmt.expression().unwrap();
                    let actual = c.get_type_at_location(expr);
                    if c.is_type_assignable_to_exported(actual, expected) {
                        let mut p = plan(
                            "returnValueCorrect",
                            "Add a return statement",
                            vec![Edit::Replace(stmt, t.new_return_statement(Some(expr)))],
                        );
                        p.id = "fixAddReturnStatement";
                        out.push(p);
                        if ast::is_arrow_function(fun) {
                            let mut p = plan(
                                "returnValueCorrect",
                                "Remove braces from arrow function body",
                                vec![Edit::Replace(body, expr)],
                            );
                            p.id = "fixRemoveBracesFromArrowFunctionBody";
                            out.push(p);
                        }
                    }
                } else if ast::is_labeled_statement(stmt) && ast::is_arrow_function(fun) {
                    let label = stmt.as_labeled_statement();
                    if let Some(expr) = label.statement.expression() {
                        let object = t.new_object_literal_expression(
                            t.new_node_list(vec![t.new_property_assignment(
                                None,
                                label.label,
                                None,
                                None,
                                expr,
                            )]),
                            false,
                        );
                        let mut p = plan(
                            "returnValueCorrect",
                            "Wrap the following body with parentheses which should be an object literal",
                            vec![Edit::Replace(body, t.new_parenthesized_expression(object))],
                        );
                        p.id = "fixWrapTheBlockWithParen";
                        out.push(p);
                    }
                }
            }
        }
    }
    if fix.error_code == 2564
        && let Some(prop) = ancestor(n, &[Kind::PropertyDeclaration])
        && let Some(typ) = prop.type_node()
    {
        let name = prop.name().unwrap();

        let mut p = plan(
            "strictClassInitialization",
            format!("Add 'undefined' type to property '{}'", name.text()),
            vec![Edit::Raw(range(f, typ), format!("{}|undefined", text(f, typ)))],
        );
        p.id = "addMissingPropertyUndefinedType";
        out.push(p);
        let replacement = t.new_property_declaration(
            prop.modifiers(),
            name,
            Some(t.new_token(Kind::ExclamationToken)),
            Some(typ),
            prop.initializer(),
        );
        let mut p = plan(
            "strictClassInitialization",
            format!("Add definite assignment assertion to property '{}'", text(f, prop)),
            vec![Edit::Replace(prop, replacement)],
        );
        p.id = "addMissingPropertyDefiniteAssignmentAssertions";
        out.push(p);
    }
    if fix.error_code == 2689
        && let Some(heritage) = ancestor(n, &[Kind::HeritageClause])
        && let Some(keyword) = astnav::find_child_of_kind(heritage, Kind::ExtendsKeyword, f)
    {
        out.push(plan(
            "extendsInterfaceBecomesImplements",
            "Change 'extends' to 'implements'",
            vec![Edit::Raw(range(f, keyword), "implements".into())],
        ));
    }
    if fix.error_code == 2322
        && let Some(var) = ancestor(n, &[Kind::VariableDeclaration])
        && let (Some(target), Some(value)) = (var.type_node(), var.initializer())
        && let Some(decl) = c
            .get_type_at_location(value)
            .symbol()
            .and_then(|s| s.declarations().first().copied())
            .filter(|p| {
                p.kind() == Kind::TypeParameter
                    && p.as_type_parameter_declaration().constraint.is_none()
            })
    {
        let parameter = decl.as_type_parameter_declaration();
        let replacement = t.new_type_parameter_declaration(
            decl.modifiers(),
            decl.name().unwrap(),
            Some(target),
            parameter.expression,
            parameter.default_type,
        );
        out.push(plan(
            "addMissingConstraint",
            "Add `extends` constraint.",
            vec![Edit::Replace(decl, replacement)],
        ));
    }
    if matches!(fix.error_code, 2810 | 2794) {
        let call = ancestor(n, &[Kind::CallExpression]);
        if let Some(call) = call {
            let ctr = ast::find_ancestor(call, |p| {
                ast::is_new_expression(p)
                    && p.expression()
                        .is_some_and(|x| ast::is_identifier(x) && x.text() == "Promise")
            });
            if let Some(ctr) = ctr {
                if ast::is_in_js_file(ctr) {
                    out.push(plan(
                        "addVoidToPromise",
                        "Add 'void' to Promise resolved without a value",
                        vec![
                            Edit::Raw(
                                TextRange::new(
                                    range(f, ctr).pos().expect_source("constructor"),
                                    range(f, ctr).pos().expect_source("constructor"),
                                ),
                                "/** @type {Promise<void>} */(".into(),
                            ),
                            Edit::Raw(
                                TextRange::new(
                                    ctr.end().expect_source("constructor"),
                                    ctr.end().expect_source("constructor"),
                                ),
                                ")".into(),
                            ),
                        ],
                    ));
                } else {
                    let expression = ctr.expression().unwrap();
                    let at = expression.end().expect_source("constructor expression");
                    out.push(plan(
                        "addVoidToPromise",
                        "Add 'void' to Promise resolved without a value",
                        vec![Edit::Raw(TextRange::new(at, at), "<void>".into())],
                    ));
                }
            }
        }
    }
    if fix.error_code == 1337
        && let Some(signature) = ancestor(n, &[Kind::IndexSignature])
        && let Some(container) =
            ancestor(signature, &[Kind::TypeAliasDeclaration, Kind::InterfaceDeclaration])
        && let Some(parameter) = signature.parameters().first()
    {
        let key = t.new_type_parameter_declaration(
            None,
            parameter.name().unwrap(),
            parameter.type_node(),
            None,
            None,
        );
        let readonly = ast::has_syntactic_modifier(signature, ast::ModifierFlags::Readonly)
            .then(|| t.new_token(Kind::ReadonlyKeyword));
        let mapped = t.new_mapped_type_node(readonly, key, None, None, signature.type_node(), None);
        let mut parts: Vec<_> = ast::get_heritage_elements(container, Kind::ExtendsKeyword)
            .iter()
            .map(|base| {
                t.new_type_reference_node(base.expression().unwrap(), base.type_argument_list())
            })
            .collect();
        parts.push(mapped);
        let members = if container.kind() == Kind::InterfaceDeclaration {
            container.members()
        } else {
            container.type_node().unwrap().members()
        };
        let others: Vec<_> =
            members.iter().copied().filter(|n| n.kind() != Kind::IndexSignature).collect();
        if !others.is_empty() {
            parts.push(t.new_type_literal_node(t.new_node_list(others)));
        }
        let mapped = if parts.len() == 1 {
            parts[0]
        } else {
            t.new_intersection_type_node(t.new_node_list(parts))
        };
        let alias = t.new_type_alias_declaration(
            container.modifiers(),
            container.name().unwrap(),
            container.type_parameter_list(),
            Some(mapped),
        );
        out.push(plan(
            "fixConvertToMappedObjectType",
            format!("Convert '{}' to mapped object type", container.name().unwrap().text()),
            vec![Edit::Replace(container, alias)],
        ));
    }
    if fix.error_code == 2690
        && let Some(property) = ancestor(n, &[Kind::PropertySignature])
        && let Some(container) = property.parent().filter(|p| p.kind() == Kind::TypeLiteral)
        && ast::is_identifier(n)
    {
        let name = if n.text() == "K" { "P" } else { "K" };
        let key = t.new_type_parameter_declaration(
            None,
            t.new_identifier(name),
            Some(t.new_type_reference_node(n, None)),
            None,
            None,
        );
        let mapped = t.new_mapped_type_node(None, key, None, None, property.type_node(), None);
        out.push(plan(
            "convertLiteralTypeToMappedType",
            format!("Convert '{}' to '{name} in {}'", n.text(), n.text()),
            vec![Edit::Replace(container, mapped)],
        ));
    }
    if fix.error_code == 2375
        && let Some(decl) = ancestor(n, &[Kind::VariableDeclaration])
        && let (Some(target), Some(value)) = (decl.type_node(), decl.initializer())
    {
        let target = c.get_type_from_type_node(target);
        let value = c.get_type_at_location(value);
        let mut edits = Vec::new();
        let mut file = None;
        for &symbol in c.get_properties_of_type(target) {
            if let Some(property) = symbol.declarations().first().copied().filter(|p| {
                p.kind() == Kind::PropertySignature
                    && p.as_property_signature_declaration().postfix_token().is_some()
            }) {
                let source = ast::get_source_file_of_node(property).unwrap();
                if fix.ls.is_read_only_file(source) {
                    continue;
                }
                if let Some(actual) = c.get_property_of_type(value, symbol.name()) {
                    let actual = c.get_type_of_symbol_at_location(actual, Some(decl));
                    if actual
                        .is_some_and(|a| a.flags().intersects(tsrs_checker::TypeFlags::Undefined))
                        && let Some(typ) = property.type_node()
                    {
                        if file.is_some_and(|old| old != source) {
                            return out;
                        }
                        file = Some(source);
                        edits.push(Edit::Raw(
                            range(source, typ),
                            format!("{}|undefined", text(source, typ)),
                        ));
                    }
                }
            }
        }
        if !edits.is_empty() {
            let mut p = plan(
                "addOptionalPropertyUndefined",
                "Add 'undefined' to optional property type",
                edits,
            );
            p.file = file;
            p.id = "";
            out.push(p);
        }
    }
    if fix.error_code == 2515
        && let Some(class) = ancestor(n, &[Kind::ClassDeclaration])
    {
        let typ = c.get_type_at_location(class);
        let mut edits = Vec::new();
        for &base in c.get_base_types(typ) {
            for &symbol in c.get_properties_of_type(base) {
                if symbol
                    .declarations()
                    .iter()
                    .any(|&d| ast::has_syntactic_modifier(d, ast::ModifierFlags::Abstract))
                    && c.get_property_of_type(typ, symbol.name()).is_some_and(|s| s == symbol)
                {
                    let mut maker = crate::codeactions_missingmemberfixer::new_missing_member_fixer(
                        t,
                        fix.program,
                        &mut c,
                        fix.ls.user_preferences().clone(),
                        None,
                        tsrs_core::context::locale_from_context(ctx),
                    );
                    for member in maker.create_member_from_symbol(
                        symbol,
                        class,
                        f,
                        None,
                        crate::codeactions_missingmemberfixer::preserveOptionalFlags::All,
                        false,
                    ) {
                        edits.push(Edit::Member(class, member));
                    }
                }
            }
        }
        if !edits.is_empty() {
            out.push(plan(
                "fixClassDoesntImplementInheritedAbstractMember",
                "Implement inherited abstract class",
                edits,
            ));
        }
    }
    if matches!(fix.error_code, 2610 | 2611)
        && let Some(member) = ast::find_ancestor(n, ast::is_class_element)
    {
        let property = if ast::is_property_declaration(member) {
            Some(member)
        } else {
            member
                .parent()
                .and_then(ast::get_class_extends_heritage_element)
                .map(|base| c.get_type_at_location(base))
                .and_then(|base| c.get_property_of_type(base, member.name().unwrap().text()))
                .and_then(|s| s.value_declaration())
                .filter(|d| ast::is_property_declaration(*d))
        };
        if let Some(property) = property {
            let source = ast::get_source_file_of_node(property).unwrap();
            if !fix.ls.is_read_only_file(source) {
                let name = property.name().unwrap();
                if ast::is_identifier(name) {
                    let mut backing = format!("_{}", name.text());
                    let mut index = 1;
                    while property.parent().is_some_and(|class| {
                        class.members().iter().any(|m| {
                            m.name().is_some_and(|n| ast::is_identifier(n) && n.text() == backing)
                        })
                    }) {
                        backing = format!("_{}{index}", name.text());
                        index += 1;
                    }
                    let backing = t.new_identifier(t.alloc_text(&backing));
                    let field = t.new_property_declaration(
                        property.modifiers(),
                        backing,
                        None,
                        property.type_node(),
                        property.initializer(),
                    );
                    let access = t.new_property_access_expression(
                        t.new_token(Kind::ThisKeyword),
                        None,
                        backing,
                        ast::NodeFlags::None,
                    );
                    let get = t.new_get_accessor_declaration(
                        None,
                        name,
                        None,
                        Some(t.new_node_list(vec![])),
                        property.type_node(),
                        None,
                        Some(t.new_block(
                            t.new_node_list(vec![t.new_return_statement(Some(access))]),
                            true,
                        )),
                    );
                    let param = t.new_parameter_declaration(
                        None,
                        None,
                        t.new_identifier("value"),
                        None,
                        property.type_node(),
                        None,
                    );
                    let assignment = t.new_binary_expression(
                        None,
                        access,
                        None,
                        t.new_token(Kind::EqualsToken),
                        t.new_identifier("value"),
                    );
                    let set = t.new_set_accessor_declaration(
                        None,
                        name,
                        None,
                        Some(t.new_node_list(vec![param])),
                        None,
                        None,
                        Some(t.new_block(
                            t.new_node_list(vec![t.new_expression_statement(assignment)]),
                            true,
                        )),
                    );
                    let mut p = plan(
                        "fixPropertyOverrideAccessor",
                        "Generate 'get' and 'set' accessors",
                        vec![
                            Edit::Replace(property, field),
                            Edit::Insert(property.end().expect_source("property"), get),
                            Edit::Insert(property.end().expect_source("property"), set),
                        ],
                    );
                    p.file = Some(source);
                    out.push(p);
                }
            }
        }
    }
    out
}
