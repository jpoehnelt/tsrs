//! Native `JSDoc` repairs and migrations from TypeScript 5.9 code fixes.
use crate::codeactions::CodeFixContext;
use crate::codeactions_syntax::{Edit, Plan, ancestor, plan, text};
use crate::{astnav, change};
use tsrs_ast::{self as ast, Kind, Node};

use tsrs_core::P;
use tsrs_core::context::Context;

fn host_tag(file: P<ast::SourceFile>, at: u32, kind: Kind) -> Option<(P<Node>, P<Node>, P<Node>)> {
    fn visit(n: P<Node>, at: u32, kind: Kind) -> Option<(P<Node>, P<Node>, P<Node>)> {
        for &doc in n.jsdoc(None) {
            if let Some(tags) = doc.as_jsdoc().tags {
                for &tag in tags.nodes() {
                    if tag.kind() == kind
                        && tag.pos().expect_source("tag") <= at
                        && tag.end().expect_source("tag") >= at
                    {
                        return Some((n, doc, tag));
                    }
                }
            }
        }
        let mut result = None;
        n.for_each_child(&mut |child| {
            result = visit(child, at, kind);
            result.is_some()
        });
        result
    }
    visit(file.as_node(), at, kind)
}
pub(crate) fn plans(_ctx: &Context, fix: &CodeFixContext, t: &change::Tracker) -> Vec<Plan> {
    let file = fix.source_file;
    let at = fix.span.pos().expect_source("JSDoc diagnostic");
    let token = astnav::get_token_at_position(file, at);
    if fix.error_code == 8024 {
        let Some((host, _, tag)) = host_tag(file, at, Kind::JSDocParameterTag) else {
            return vec![];
        };
        let tags: Vec<_> = host
            .jsdoc(None)
            .iter()
            .flat_map(|d| d.as_jsdoc().tags.map_or(&[][..], |l| l.nodes()))
            .copied()
            .collect();
        let name = tag.name().unwrap().text();
        let mut removed = plan(
            "deleteUnmatchedParameter",
            format!("Delete unused '@param' tag '{name}'"),
            vec![Edit::Jsdoc(host, tags.iter().copied().filter(|n| *n != tag).collect())],
        );
        removed.id = "deleteUnmatchedParameter";
        let mut result = vec![removed];
        if let Some(parameter) = host.parameters().iter().find(|p| {
            p.name().is_some_and(ast::is_identifier)
                && !tags.iter().any(|tag| {
                    tag.kind() == Kind::JSDocParameterTag
                        && tag.name().map(|n| n.text()) == p.name().map(|n| n.text())
                })
        }) {
            let new_name = parameter.name().unwrap();
            let old = tag.as_jsdoc_parameter_or_property_tag();
            let replacement = t.new_jsdoc_parameter_or_property_tag(
                Kind::JSDocParameterTag,
                old.tag_name(),
                t.new_identifier(new_name.text()),
                old.is_bracketed,
                old.type_expression,
                old.is_name_first,
                old.comment(),
            );
            let mut p = plan(
                "renameUnmatchedParameter",
                format!("Rename '@param' tag name '{name}' to '{}'", new_name.text()),
                vec![Edit::Jsdoc(
                    host,
                    tags.into_iter().map(|n| if n == tag { replacement } else { n }).collect(),
                )],
            );
            // Rename is intentionally a single action, as in TypeScript.
            p.id = "";
            result.push(p);
        }
        return result;
    }
    if fix.error_code == 80004 {
        let Some(function) = ast::find_ancestor(token, ast::is_function_like_declaration) else {
            return vec![];
        };
        let mut edits = Vec::new();
        for &doc in function.jsdoc(None) {
            for &tag in doc.as_jsdoc().tags.map_or(&[][..], |l| l.nodes()) {
                let Some(typ) = tag.type_expression().and_then(|e| e.type_node()) else {
                    continue;
                };
                if tag.kind() == Kind::JSDocReturnTag && function.type_node().is_none() {
                    edits.push(Edit::Type(function, typ));
                } else if tag.kind() == Kind::JSDocParameterTag {
                    if let Some(parameter) = function.parameters().iter().find(|p| {
                        p.type_node().is_none()
                            && p.name().map(|n| n.text()) == tag.name().map(|n| n.text())
                    }) {
                        let end = parameter.name().unwrap().end().expect_source("parameter");
                        edits.push(Edit::Raw(tsrs_core::TextRange::new(end,end),format!(": {}",crate::codeactions_fixmissingtypeannotation::type_to_string_for_diag(typ,file,t))));
                    }
                }
            }
        }
        return vec![plan("annotateWithTypeFromJSDoc", "Annotate with type from JSDoc", edits)];
    }
    if fix.error_code == 80009 {
        let Some((_, doc, tag)) = host_tag(file, at, Kind::JSDocTypedefTag) else {
            return vec![];
        };
        let Some(name) = tag.name() else {
            return vec![];
        };
        let Some(typ) = tag.type_expression().and_then(|e| {
            if e.kind() == Kind::JSDocTypeExpression { e.type_node() } else { Some(e) }
        }) else {
            return vec![];
        };
        let printed =
            crate::codeactions_fixmissingtypeannotation::type_to_string_for_diag(typ, file, t);
        return vec![plan(
            "convertTypedefToType",
            "Convert typedef to TypeScript type.",
            vec![Edit::Raw(
                crate::codeactions_syntax::range(file, doc),
                format!("type {}={printed};", name.text()),
            )],
        )];
    }
    if matches!(fix.error_code, 17019 | 17020 | 8020) {
        let Some(old) = ancestor(
            token,
            &[
                Kind::JSDocNullableType,
                Kind::JSDocNonNullableType,
                Kind::JSDocOptionalType,
                Kind::JSDocAllType,
                Kind::FunctionType,
            ],
        ) else {
            return vec![];
        };
        let mut typ = old.type_node().unwrap_or(old);
        if old.kind() == Kind::JSDocNullableType {
            typ = t.new_union_type_node(t.new_node_list(vec![
                typ,
                t.new_literal_type_node(t.new_token(Kind::NullKeyword)),
                t.new_keyword_type_node(Kind::UndefinedKeyword),
            ]));
        }
        let description = format!(
            "Change '{}' to '{}'",
            text(file, old),
            crate::codeactions_fixmissingtypeannotation::type_to_string_for_diag(typ, file, t)
        );
        let mut plain = plan("jdocTypes", description.clone(), vec![Edit::Replace(old, typ)]);
        plain.id = "fixJSDocTypes_plain";
        let mut result = vec![plain];
        if old.kind() == Kind::JSDocNullableType {
            let mut p = plan("jdocTypes", description, vec![Edit::Replace(old, typ)]);
            p.id = "fixJSDocTypes_nullable";
            result.push(p);
        }
        return result;
    }
    Vec::new()
}
