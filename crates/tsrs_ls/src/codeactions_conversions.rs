//! Suggestion migrations with binding-aware compiler transforms.
use crate::codeactions::CodeFixContext;
use crate::codeactions_syntax::{Edit, Plan, ancestor, plan};
use crate::{astnav, change};
use rustc_hash::FxHashSet;
use std::rc::Rc;
use tsrs_ast::{self as ast, Kind, Node};
use tsrs_core::P;
use tsrs_core::context::Context;

pub fn plans(ctx: &Context, fix: &CodeFixContext, t: &change::Tracker) -> Vec<Plan> {
    let f = fix.source_file;
    let n = astnav::get_token_at_position(f, fix.span.pos().expect_source("conversion diagnostic"));
    let mut c = fix.program.get_type_checker_for_file(ctx, f);
    if fix.error_code == 80002 {
        let Some(function) = ancestor(n, &[Kind::FunctionDeclaration]) else {
            return vec![];
        };
        if ast::has_syntactic_modifier(function, ast::ModifierFlags::Async)
            || function.as_function_declaration().asterisk_token().is_some()
        {
            return vec![];
        }
        let Some(symbol) = function.name().and_then(|name| c.get_symbol_at_location_exported(name))
        else {
            return vec![];
        };
        let mut members = Vec::new();
        let mut deletions = Vec::new();
        if let Some(body) = function.body() {
            members.push(t.new_constructor_declaration(
                None,
                None,
                function.parameter_list(),
                None,
                None,
                Some(body),
            ));
        }
        for &statement in f.statements().nodes() {
            let Some(binary) = Some(statement)
                .filter(|p| ast::is_expression_statement(*p))
                .and_then(|p| p.expression())
                .filter(|p| p.kind() == Kind::BinaryExpression)
            else {
                continue;
            };
            let binary = binary.as_binary_expression();
            if binary.operator_token.kind() != Kind::EqualsToken {
                continue;
            }
            let left = binary.left;
            let value = binary.right.get();
            if !ast::is_property_access_expression(left)
                || !(ast::is_function_expression(value) || ast::is_arrow_function(value))
            {
                continue;
            }
            let receiver = left.expression().unwrap();
            let (receiver, static_) = if ast::is_property_access_expression(receiver)
                && receiver.name().is_some_and(|n| n.text() == "prototype")
            {
                (receiver.expression().unwrap(), false)
            } else {
                (receiver, true)
            };
            if c.get_symbol_at_location_exported(receiver) != Some(symbol) {
                continue;
            }
            if let Some(body) = value.body() {
                let body = if body.kind() == Kind::Block {
                    body
                } else {
                    t.new_block(t.new_node_list(vec![t.new_return_statement(Some(body))]), false)
                };
                let mods = if static_ {
                    Some(t.new_modifier_list(vec![t.new_token(Kind::StaticKeyword)]))
                } else {
                    value.modifiers()
                };
                members.push(t.new_method_declaration(
                    mods,
                    None,
                    left.name().unwrap(),
                    None,
                    value.type_parameter_list(),
                    value.parameter_list(),
                    value.type_node(),
                    None,
                    Some(body),
                ));
                deletions.push(Edit::Delete(statement));
            }
        }
        if deletions.is_empty() && function.body().is_none_or(|b| !initializes_instance(b)) {
            return vec![];
        }
        let class = t.new_class_declaration(
            function.modifiers(),
            function.name(),
            function.type_parameter_list(),
            None,
            t.new_node_list(members),
        );
        let mut edits = vec![Edit::Replace(function, class)];
        edits.extend(deletions);
        return vec![plan(
            "convertFunctionToEs6Class",
            "Convert function to an ES2015 class",
            edits,
        )];
    }
    if fix.error_code == 80006 {
        let Some(function) = ast::find_ancestor(n, ast::is_function_like_declaration) else {
            return vec![];
        };
        let Some(body) = function.body().filter(|b| b.kind() == Kind::Block) else {
            return vec![];
        };
        if ast::has_syntactic_modifier(function, ast::ModifierFlags::Async) {
            return vec![];
        }
        let mut edits = vec![Edit::Before(function, Kind::AsyncKeyword)];
        for &statement in body.statements() {
            if statement.kind() != Kind::ReturnStatement {
                continue;
            }
            let Some(call) = statement.expression().filter(|p| ast::is_call_expression(*p)) else {
                continue;
            };
            let Some(access) = call.expression().filter(|p| {
                ast::is_property_access_expression(*p)
                    && p.name().is_some_and(|n| n.text() == "then")
            }) else {
                continue;
            };
            let receiver_type = c.get_type_at_location(access.expression().unwrap());
            if c.get_promised_type_of_promise(receiver_type).is_none() {
                continue;
            }
            let Some(callback) = call
                .arguments()
                .first()
                .copied()
                .filter(|p| ast::is_arrow_function(*p) || ast::is_function_expression(*p))
            else {
                continue;
            };
            if call.arguments().len() != 1 || callback.parameters().len() != 1 {
                continue;
            }
            let parameter = callback.parameters()[0];
            let Some(name) = parameter.name().filter(|p| ast::is_identifier(*p)) else {
                continue;
            };
            let Some(parameter_symbol) = c.get_symbol_at_location_exported(name) else {
                continue;
            };
            let mut refs = FxHashSet::default();
            let Some(callback_body) = callback.body() else {
                continue;
            };
            collect_callback_references(callback_body, &mut c, parameter_symbol, &mut refs);
            let unique = t.emit_context.factory.new_unique_name_ex(
                name.text(),
                tsrs_printer::AutoGenerateOptions {
                    flags: tsrs_printer::GeneratedIdentifierFlags::FileLevel
                        | tsrs_printer::GeneratedIdentifierFlags::ReservedInNestedScopes,
                    ..Default::default()
                },
            );
            let mut visitor = ast::new_node_visitor(
                Some(Rc::new(move |v, node| {
                    if refs.contains(&node) { Some(unique) } else { v.visit_each_child(Some(node)) }
                })),
                Some(t.node_factory.clone()),
                ast::NodeVisitorHooks::default(),
            );
            let transformed = visitor.visit_node(Some(callback_body)).unwrap();
            let declaration = t.new_variable_declaration(
                unique,
                None,
                None,
                Some(t.new_await_expression(access.expression().unwrap())),
            );
            let declaration = t.new_variable_statement(
                None,
                t.new_variable_declaration_list(
                    t.new_node_list(vec![declaration]),
                    ast::NodeFlags::Const,
                ),
            );
            let mut nodes = vec![declaration];
            if transformed.kind() == Kind::Block {
                nodes.extend_from_slice(transformed.statements());
            } else {
                nodes.push(t.new_return_statement(Some(transformed)));
            }
            edits.push(Edit::Nodes(statement, nodes));
        }
        if edits.len() > 1 {
            return vec![plan("convertToAsyncFunction", "Convert to async function", edits)];
        }
    }
    Vec::new()
}

fn initializes_instance(n: P<Node>) -> bool {
    if n.kind() == Kind::BinaryExpression {
        let b = n.as_binary_expression();
        if b.operator_token.kind() == Kind::EqualsToken
            && ast::is_property_access_expression(b.left)
            && b.left.expression().is_some_and(|n| n.kind() == Kind::ThisKeyword)
        {
            return true;
        }
    }
    if ast::is_function_like(n) {
        return false;
    }
    let mut found = false;
    n.for_each_child(&mut |child| {
        found = initializes_instance(child);
        found
    });
    found
}

fn collect_callback_references(
    n: P<Node>,
    checker: &mut tsrs_checker::Checker,
    symbol: P<ast::Symbol>,
    out: &mut FxHashSet<P<Node>>,
) {
    if ast::is_identifier(n) && checker.get_symbol_at_location_exported(n) == Some(symbol) {
        out.insert(n);
    }
    n.for_each_child(&mut |child| {
        collect_callback_references(child, checker, symbol, out);
        false
    });
}
