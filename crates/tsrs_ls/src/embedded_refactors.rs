// tsrs-only embedded refactors. These in-place JavaScript operations are independent of the Go LSP
// capability table; unsupported selections produce no action, never a guessed textual rewrite.
use crate::{LanguageService, astnav};
use rustc_hash::FxHashSet;
use tsrs_ast::{self as ast, Kind, Node, SourceFile, SymbolFlags};
use tsrs_checker::{Checker, TypeFlags};
use tsrs_core::context::Context;
use tsrs_core::{P, TextRange};
use tsrs_lsproto as lsp;

pub struct EmbeddedRefactor {
    pub name: &'static str,
    pub description: String,
    pub kind: &'static str,
    pub changes: Vec<lsp::TextEdit>,
}

fn nodes(root: P<Node>) -> Vec<P<Node>> {
    fn visit(node: P<Node>, out: &mut Vec<P<Node>>) {
        out.push(node);
        node.for_each_child(&mut |child| {
            visit(child, out);
            false
        });
    }
    let mut result = Vec::new();
    visit(root, &mut result);
    result
}

fn bounds(file: P<SourceFile>, node: P<Node>) -> (u32, u32) {
    (astnav::get_start_of_node(node, file, false), node.end().expect_source("parsed refactor node"))
}

fn text(file: P<SourceFile>, node: P<Node>) -> &'static str {
    let (start, end) = bounds(file, node);
    &file.text()[start as usize..end as usize]
}

fn fresh_name(all: &[P<Node>], stem: &str, checker: &mut Checker, location: P<Node>) -> String {
    let names: FxHashSet<&str> =
        all.iter().filter(|node| node.kind() == Kind::Identifier).map(|node| node.text()).collect();
    if !names.contains(stem)
        && checker.resolve_name_exported(stem, location, SymbolFlags::Value, false).is_none()
    {
        return stem.to_string();
    }
    for index in 1usize.. {
        let candidate = format!("{stem}{index}");
        if !names.contains(candidate.as_str())
            && checker
                .resolve_name_exported(&candidate, location, SymbolFlags::Value, false)
                .is_none()
        {
            return candidate;
        }
    }
    unreachable!()
}

fn can_pass_free_names(expression: P<Node>, names: &[String]) -> bool {
    names.is_empty()
        || !nodes(expression).into_iter().any(|node| {
            matches!(
                node.kind(),
                Kind::CallExpression
                    | Kind::NewExpression
                    | Kind::PropertyAccessExpression
                    | Kind::ElementAccessExpression
                    | Kind::ConditionalExpression
            ) || (node.kind() == Kind::BinaryExpression
                && matches!(
                    node.as_binary_expression().operator_token().kind(),
                    Kind::AmpersandAmpersandToken | Kind::BarBarToken | Kind::QuestionQuestionToken
                ))
        })
}

fn captures_lexical_context(node: P<Node>) -> bool {
    nodes(node).into_iter().any(|child| {
        matches!(
            child.kind(),
            Kind::ThisKeyword
                | Kind::SuperKeyword
                | Kind::AwaitExpression
                | Kind::YieldExpression
                | Kind::MetaProperty
                | Kind::FunctionExpression
                | Kind::ArrowFunction
                | Kind::ClassExpression
                | Kind::PostfixUnaryExpression
        ) || ast::is_assignment_expression(child, false)
            || (child.kind() == Kind::PrefixUnaryExpression
                && matches!(
                    child.as_prefix_unary_expression().operator(),
                    Kind::PlusPlusToken | Kind::MinusMinusToken
                ))
            || (child.kind() == Kind::Identifier && matches!(child.text(), "arguments" | "eval"))
    })
}

fn is_reference(node: P<Node>) -> bool {
    if node.kind() != Kind::Identifier || ast::is_declaration_name(node) {
        return false;
    }
    let Some(parent) = node.parent() else {
        return false;
    };
    !matches!(
        parent.kind(),
        Kind::PropertyAccessExpression | Kind::PropertyAssignment | Kind::MethodDeclaration
    ) || parent.name() != Some(node)
}

fn free_names(file: P<SourceFile>, expression: P<Node>, checker: &mut Checker) -> Vec<String> {
    let mut seen = FxHashSet::default();
    nodes(expression)
        .into_iter()
        .filter(|node| is_reference(*node))
        .filter_map(|node| {
            let symbol = checker.get_editor_symbol_at_location(node)?;
            if !symbol
                .declarations()
                .iter()
                .any(|&declaration| ast::get_source_file_of_node(declaration) == Some(file))
                || !seen.insert(symbol)
            {
                return None;
            }
            Some(node.text().to_string())
        })
        .collect()
}

fn insertion_statement(expression: P<Node>) -> Option<P<Node>> {
    let parent = expression.parent()?;
    let statement =
        if parent.kind() == Kind::VariableDeclaration && parent.initializer() == Some(expression) {
            let list = parent.parent()?;
            if list.kind() != Kind::VariableDeclarationList
                || list.as_variable_declaration_list().declarations().nodes().len() != 1
            {
                return None;
            }
            list.parent()?
        } else if matches!(
            parent.kind(),
            Kind::ReturnStatement | Kind::ThrowStatement | Kind::ExpressionStatement
        ) {
            parent
        } else {
            return None;
        };
    statement
        .parent()
        .filter(|node| matches!(node.kind(), Kind::SourceFile | Kind::Block))
        .map(|_| statement)
}

fn escape_template(value: &str) -> String {
    let mut result = String::new();
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\\' => result.push_str("\\\\"),
            '`' => result.push_str("\\`"),
            '$' if characters.peek() == Some(&'{') => result.push_str("\\$"),
            '\r' => result.push_str("\\r"),
            '\n' => result.push_str("\\n"),
            '\0' => result.push_str("\\x00"),
            _ => result.push(character),
        }
    }
    result
}

fn template_part(file: P<SourceFile>, node: P<Node>, checker: &mut Checker, result: &mut String) {
    let node = ast::skip_parentheses(node);
    if node.kind() == Kind::StringLiteral || node.kind() == Kind::NoSubstitutionTemplateLiteral {
        result.push_str(&escape_template(node.text()));
    } else if node.kind() == Kind::BinaryExpression
        && node.as_binary_expression().operator_token().kind() == Kind::PlusToken
        && checker.get_editor_type_at_location(node).flags().intersects(TypeFlags::StringLike)
    {
        template_part(file, node.as_binary_expression().left(), checker, result);
        template_part(file, node.as_binary_expression().right(), checker, result);
    } else {
        result.push_str("${");
        result.push_str(text(file, node));
        result.push('}');
    }
}

impl LanguageService {
    pub fn embedded_refactors(
        &self,
        ctx: &Context,
        uri: &lsp::DocumentUri,
        start: u32,
        end: u32,
    ) -> Result<Vec<EmbeddedRefactor>, lsp::Error> {
        let (program, file) = self.get_program_and_file(uri);
        let source = file.text();
        if start > end
            || end as usize > source.len()
            || !source.is_char_boundary(start as usize)
            || !source.is_char_boundary(end as usize)
        {
            return Err(lsp::Error::new("Invalid refactor range".to_string()));
        }
        let all = nodes(file.as_node());
        let newline = if source.contains("\r\n") { "\r\n" } else { "\n" };
        let edit = |start, end, new_text| {
            let (range, _) = self.converters.to_lsp_range(file.get(), TextRange::new(start, end));
            lsp::TextEdit { range, new_text }
        };
        let mut actions = Vec::new();
        let mut checker = program.get_type_checker_for_file(ctx, file);
        if start < end
            && let Some(&expression) = all
                .iter()
                .find(|&&node| ast::is_expression_node(node) && bounds(file, node) == (start, end))
            && !captures_lexical_context(expression)
        {
            let parameters = free_names(file, expression, &mut checker);
            let name = fresh_name(&all, "newFunction", &mut checker, file.as_node());
            let generated = format!(
                "{newline}function {name}({}) {{{newline}  return {};{newline}}}{newline}",
                parameters.join(", "),
                text(file, expression)
            );
            if can_pass_free_names(expression, &parameters) {
                actions.push(EmbeddedRefactor {
                    name: "extract_function",
                    description: "Extract to function in global scope".to_string(),
                    kind: "refactor.extract.function",
                    changes: vec![
                        edit(start, end, format!("{name}({})", parameters.join(", "))),
                        edit(source.len() as u32, source.len() as u32, generated),
                    ],
                });
            }
            if let Some(statement) = insertion_statement(expression) {
                let statement_start = bounds(file, statement).0;
                let line_start = source[..statement_start as usize]
                    .rfind(['\n', '\r'])
                    .map_or(0, |position| position + 1);
                let indent: String = source[line_start..statement_start as usize]
                    .chars()
                    .take_while(|&character| matches!(character, ' ' | '\t'))
                    .collect();
                let local = fresh_name(&all, "newLocal", &mut checker, expression);
                actions.push(EmbeddedRefactor {
                    name: "extract_constant",
                    description: "Extract to constant in enclosing block".to_string(),
                    kind: "refactor.extract.constant",
                    changes: vec![
                        edit(start, end, local.clone()),
                        edit(
                            statement_start,
                            statement_start,
                            format!("const {local} = {};{newline}{indent}", text(file, expression)),
                        ),
                    ],
                });
            }
        }
        let concatenation = all
            .iter()
            .copied()
            .filter(|node| {
                if node.kind() != Kind::BinaryExpression
                    || node.as_binary_expression().operator_token().kind() != Kind::PlusToken
                {
                    return false;
                }
                let (left, right) = bounds(file, *node);
                if start == end {
                    left <= start && start <= right
                } else {
                    left == start && right == end
                }
            })
            .max_by_key(|node| {
                let (left, right) = bounds(file, *node);
                right - left
            });
        if let Some(expression) = concatenation
            && checker
                .get_editor_type_at_location(expression)
                .flags()
                .intersects(TypeFlags::StringLike)
        {
            let mut replacement = "`".to_string();
            template_part(file, expression, &mut checker, &mut replacement);
            replacement.push('`');
            let (left, right) = bounds(file, expression);
            actions.push(EmbeddedRefactor {
                name: "convert_to_template",
                description: "Convert to template string".to_string(),
                kind: "refactor.rewrite.string",
                changes: vec![edit(left, right, replacement)],
            });
        }
        Ok(actions)
    }
}
