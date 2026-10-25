//! Module repairs use resolved compiler import bindings and preserve source ownership.
use crate::codeactions::CodeFixContext;
use crate::codeactions_syntax::{Edit, Plan, ancestor, plan, range, text};
use crate::{astnav, change};
use tsrs_ast::{self as ast, Kind, Node};
use tsrs_core::context::Context;
use tsrs_core::{P, TextRange};

pub fn plans(ctx: &Context, fix: &CodeFixContext, t: &change::Tracker) -> Vec<Plan> {
    let f = fix.source_file;
    let n = astnav::get_token_at_position(f, fix.span.pos().expect_source("module diagnostic"));
    let mut c = fix.program.get_type_checker_for_file(ctx, f);
    let mut result = Vec::new();
    let import = ancestor(n, &[Kind::ImportDeclaration]).or_else(|| {
        if !ast::is_identifier(n) {
            return None;
        }
        let s = c.get_editor_symbol_at_location(n)?;
        s.declarations().iter().find_map(|&d| ancestor(d, &[Kind::ImportDeclaration]))
    });
    if let Some(import) = import {
        let data = import.as_import_declaration();
        let module = data.module_specifier;
        if let Some(clause) = data.import_clause {
            let ci = clause.as_import_clause();
            if fix.error_code == 80003
                && (fix.fix_id.is_none()
                    || c.get_editor_symbol_at_location(module).is_some_and(|s| {
                        s.declarations()
                            .iter()
                            .copied()
                            .filter_map(ast::get_source_file_of_node)
                            .any(|file| {
                                file.statements().nodes().iter().any(|n| {
                                    n.kind() == Kind::ExportAssignment
                                        && n.as_export_assignment().is_export_equals
                                })
                            })
                    }))
                && let Some(binding) =
                    ci.named_bindings().filter(|b| b.kind() == Kind::NamespaceImport)
            {
                let new = t.new_import_declaration(
                    import.modifiers(),
                    Some(t.new_import_clause(Kind::Unknown, binding.name(), None)),
                    module,
                    data.attributes(),
                );
                result.push(plan(
                    "useDefaultImport",
                    "Convert to default import",
                    vec![Edit::Replace(import, new)],
                ));
            }
            if fix.error_code == 1363
                && let (Some(name), Some(bindings)) = (ci.name(), ci.named_bindings())
            {
                let first = t.new_import_declaration(
                    import.modifiers(),
                    Some(t.new_import_clause(Kind::TypeKeyword, Some(name), None)),
                    module,
                    data.attributes(),
                );
                let second = t.new_import_declaration(
                    import.modifiers(),
                    Some(t.new_import_clause(Kind::TypeKeyword, None, Some(bindings))),
                    module,
                    data.attributes(),
                );
                result.push(plan(
                    "splitTypeOnlyImport",
                    "Split into two separate import declarations",
                    vec![Edit::Replace(import, first), Edit::After(import, second)],
                ));
            }
            if fix.error_code == 1484 {
                let new = t.new_import_declaration(
                    import.modifiers(),
                    Some(t.new_import_clause(Kind::TypeKeyword, ci.name(), ci.named_bindings())),
                    module,
                    data.attributes(),
                );
                result.push(plan(
                    "convertToTypeOnlyImport",
                    "Use 'import type'",
                    vec![Edit::Replace(import, new)],
                ));
                if fix.fix_id.is_none()
                    && let Some(spec) = ancestor(n, &[Kind::ImportSpecifier])
                {
                    let name = spec.name().unwrap();
                    let replacement = t.new_import_specifier(
                        true,
                        spec.as_import_specifier().property_name(),
                        name,
                    );
                    let mut p = plan(
                        "convertToTypeOnlyImport",
                        format!("Use 'type {}'", name.text()),
                        vec![Edit::Replace(spec, replacement)],
                    );
                    p.id = "";
                    result.push(p);
                }
            }
            if matches!(fix.error_code, 1541 | 1542) {
                let key = t.new_string_literal("resolution-mode", ast::TokenFlags::SingleQuote);
                let value = t.new_string_literal("import", ast::TokenFlags::SingleQuote);
                let attrs = t.new_import_attributes(
                    Kind::WithKeyword,
                    t.new_node_list(vec![t.new_import_attribute(Some(key), value)]),
                    false,
                );
                let new =
                    t.new_import_declaration(import.modifiers(), Some(clause), module, Some(attrs));
                result.push(plan(
                    "addMissingResolutionModeImportAttribute",
                    "Add 'resolution-mode' import attribute",
                    vec![Edit::Replace(import, new)],
                ));
            }
            if matches!(fix.error_code, 2349 | 2345)
                && fix.program.options().es_module_interop.is_true()
                && let Some(binding) =
                    ci.named_bindings().filter(|b| b.kind() == Kind::NamespaceImport)
            {
                let name = binding.name().unwrap();
                let new = t.new_import_declaration(
                    import.modifiers(),
                    Some(t.new_import_clause(Kind::Unknown, Some(name), None)),
                    module,
                    data.attributes(),
                );
                let printed =
                    crate::codeactions_fixmissingtypeannotation::type_to_string_for_diag(new, f, t);
                let mut p = plan(
                    "invalidImportSyntax",
                    format!("Replace import with '{printed}'."),
                    vec![Edit::Replace(import, new)],
                );
                p.id = "";
                result.push(p);
                let mut p = plan(
                    "invalidImportSyntax",
                    "Use synthetic 'default' member.",
                    vec![Edit::Replace(
                        n,
                        t.new_property_access_expression(
                            n,
                            None,
                            t.new_identifier("default"),
                            ast::NodeFlags::None,
                        ),
                    )],
                );
                p.id = "";
                result.push(p);
            }
            if fix.error_code == 2459
                && let Some(target) = c.get_editor_symbol_at_location(module).and_then(|s| {
                    s.declarations().iter().copied().find(|d| d.kind() == Kind::SourceFile)
                })
            {
                let target = ast::get_source_file_of_node(target).unwrap();
                if !fix.ls.is_read_only_file(target) {
                    let name = n.text();
                    let export = target.statements().nodes().iter().copied().find(|s| {
                        s.kind() == Kind::ExportDeclaration
                            && s.as_export_declaration()
                                .export_clause
                                .is_some_and(|p| p.kind() == Kind::NamedExports)
                    });
                    if let Some(export) = export {
                        let d = export.as_export_declaration();
                        let mut specs =
                            d.export_clause.unwrap().as_named_exports().elements.nodes().to_vec();
                        specs.push(t.new_export_specifier(false, None, t.new_identifier(name)));
                        let new = t.new_export_declaration(
                            export.modifiers(),
                            d.is_type_only,
                            Some(t.new_named_exports(t.new_node_list(specs))),
                            d.module_specifier,
                            d.attributes(),
                        );
                        let mut p = plan(
                            "fixImportNonExportedMember",
                            format!("Export '{name}' from module '{}'", module.text()),
                            vec![Edit::Replace(export, new)],
                        );
                        p.file = Some(target);
                        result.push(p);
                    }
                }
            }
            if fix.error_code == 1272
                && let Some(spec) = c.get_editor_symbol_at_location(n).and_then(|s| {
                    s.declarations().iter().copied().find(|d| d.kind() == Kind::ImportSpecifier)
                })
            {
                let path = module.text();
                let base =
                    path.rsplit('/').next().unwrap_or(path).split('.').next().unwrap_or("module");
                let unique = t.emit_context.factory.new_unique_name_ex(
                    base,
                    tsrs_printer::AutoGenerateOptions {
                        flags: tsrs_printer::GeneratedIdentifierFlags::Optimistic
                            | tsrs_printer::GeneratedIdentifierFlags::FileLevel,
                        ..Default::default()
                    },
                );
                let namespace = t.new_namespace_import(unique);
                let bindings = ci.named_bindings().unwrap();
                let mut edits = vec![Edit::Replace(bindings, namespace)];
                let mut refs = Vec::new();
                if let Some(symbol) = c.get_editor_symbol_at_location(spec.name().unwrap()) {
                    collect_import_references(f.as_node(), &mut c, symbol, &mut refs);
                }
                for reference in refs {
                    edits.push(Edit::Replace(reference, t.new_qualified_name(unique, reference)));
                }
                let mut p = plan(
                    "fixUnreferenceableDecoratorMetadata",
                    "Convert named imports to namespace import",
                    edits,
                );
                p.id = "";
                result.push(p);
                let at = range(f, clause).pos().expect_source("import clause");
                let mut p = plan(
                    "fixUnreferenceableDecoratorMetadata",
                    "Use 'import type'",
                    vec![Edit::Raw(TextRange::new(at, at), "type ".into())],
                );
                p.id = "";
                result.push(p);
            }
        }
    }
    if fix.error_code == 80005
        && let Some(var) = ancestor(n, &[Kind::VariableDeclaration])
        && let Some(call) = var.initializer().filter(|p| ast::is_call_expression(*p))
        && let Some(module) = call.arguments().first()
    {
        let name = var.name().unwrap();
        let statement = ancestor(var, &[Kind::VariableStatement]).unwrap();
        let reference = t.new_external_module_reference(*module);
        let new = t.new_import_equals_declaration(None, false, name, reference);
        // TS5 preserves its compact assignment spelling for this migration.
        result.push(plan(
            "requireInTs",
            "Convert 'require' to 'import'",
            vec![Edit::Raw(
                range(f, statement),
                format!("import {}=require({});", name.text(), text(f, *module)),
            )],
        ));
        let _ = new;
    }
    if fix.error_code == 80001
        && let Some(stmt) = ancestor(n, &[Kind::ExpressionStatement])
        && let Some(binary) = stmt.expression().filter(|p| p.kind() == Kind::BinaryExpression)
    {
        let b = binary.as_binary_expression();
        let right = b.right.get();
        if ast::is_object_literal_expression(right) {
            let left = b.left;
            if ast::is_property_access_expression(left)
                && left.expression().is_some_and(|e| ast::is_identifier(e) && e.text() == "module")
                && left.name().is_some_and(|name| name.text() == "exports")
            {
                let mut p = plan(
                    "convertToEsModule",
                    "Convert to ES module",
                    vec![Edit::Raw(
                        TextRange::new(
                            range(f, left).pos().expect_source("export"),
                            b.operator_token.end().expect_source("assignment"),
                        ),
                        "export default".into(),
                    )],
                );
                p.id = "";
                result.push(p);
            }
        }
    }
    result
}

fn collect_import_references(
    n: P<Node>,
    c: &mut tsrs_checker::Checker,
    symbol: P<ast::Symbol>,
    out: &mut Vec<P<Node>>,
) {
    if ast::is_identifier(n)
        && c.get_editor_symbol_at_location(n) == Some(symbol)
        && ast::find_ancestor(n, |p| p.kind() == Kind::ImportDeclaration).is_none()
    {
        out.push(n);
    }
    n.for_each_child(&mut |p| {
        collect_import_references(p, c, symbol, out);
        false
    });
}
