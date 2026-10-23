//! Configuration edits preserve the editor's source identity and original JSON text.
use crate::change;
use crate::codeactions::CodeFixContext;
use crate::codeactions_syntax::{Edit, Plan, plan};
use tsrs_ast as ast;
use tsrs_core::context::Context;
pub(crate) fn plans(_ctx: &Context, fix: &CodeFixContext, t: &change::Tracker) -> Vec<Plan> {
    if !matches!(fix.error_code, 1378 | 17004) {
        return Vec::new();
    }
    let name = &fix.ls.user_preferences().configuration_file_name;
    let source = if name.is_empty() {
        fix.program.command_line().config_file.map(|c| c.source_file)
    } else {
        fix.ls.host.read_file(name).map(|text| {
            tsrs_parser::parse_source_file_owned(
                ast::SourceFileParseOptions {
                    file_name: name.clone(),
                    path: fix.ls.to_path(name),
                    ..Default::default()
                },
                text,
                tsrs_core::ScriptKind::JSON,
            )
        })
    };
    let Some(source) = source else {
        return vec![];
    };
    if fix.ls.is_read_only_file(source) {
        return vec![];
    }
    let Some(root) = source
        .statements()
        .nodes()
        .first()
        .and_then(|n| n.expression())
        .filter(|n| ast::is_object_literal_expression(*n))
    else {
        return vec![];
    };
    let Some(options) = root
        .properties()
        .iter()
        .find(|p| p.name().is_some_and(|n| n.text() == "compilerOptions"))
        .map(|p| p.as_property_assignment().initializer.get())
        .filter(|n| ast::is_object_literal_expression(*n))
    else {
        return vec![];
    };
    let (key, value, id, description) = if fix.error_code == 17004 {
        ("jsx", "react", "fixEnableJsxFlag", "Enable the '--jsx' flag in your configuration file")
    } else {
        (
            "target",
            "es2017",
            "fixTargetOption",
            "Set the 'target' option in your configuration file to 'es2017'",
        )
    };
    let literal = t.new_string_literal(value, ast::TokenFlags::None);
    let edit = if let Some(property) =
        options.properties().iter().find(|p| p.name().is_some_and(|n| n.text() == key))
    {
        Edit::Replace(property.as_property_assignment().initializer.get(), literal)
    } else {
        Edit::Member(
            options,
            t.new_property_assignment(
                None,
                t.new_string_literal(key, ast::TokenFlags::None),
                None,
                None,
                literal,
            ),
        )
    };
    let mut p = plan(id, description, vec![edit]);
    p.file = Some(source);
    p.id = "";
    vec![p]
}
