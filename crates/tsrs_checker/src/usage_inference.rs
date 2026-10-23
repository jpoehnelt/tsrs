//! Usage inference for language-service fixes, following TypeScript 5.9 inferFromUsage.ts.
//! This is invoked explicitly by editor operations and does not affect ordinary checking.
use crate::*;

#[derive(Clone, Copy)]
pub struct InferredParameter {
    pub declaration: P<Node>,
    pub type_: P<Type>,
    pub optional: bool,
}

#[derive(Default, Clone)]
struct Usage {
    number: bool,
    string: bool,
    number_or_string: bool,
    candidates: Vec<P<Type>>,
    properties: Vec<(String, Usage)>,
    calls: Vec<Call>,
    constructs: Vec<Call>,
    number_index: Option<Box<Usage>>,
    string_index: Option<Box<Usage>>,
    this_types: Vec<P<Type>>,
}
#[derive(Clone)]
struct Call {
    arguments: Vec<P<Type>>,
    result: Usage,
}
struct Inference<'a> {
    c: &'a mut Checker,
    files: &'a [P<SourceFile>],
    references: FxHashMap<P<Symbol>, Vec<P<Node>>>,
}

impl Checker {
    pub fn infer_parameters_from_usage(
        &mut self,
        function: P<Node>,
        files: &[P<SourceFile>],
    ) -> Vec<InferredParameter> {
        Inference { c: self, files, references: FxHashMap::default() }.parameters(function)
    }

    pub fn infer_this_from_usage(&mut self, function: P<Node>, files: &[P<SourceFile>]) -> P<Type> {
        let mut infer = Inference { c: self, files, references: FxHashMap::default() };
        let mut usage = Usage::default();
        if let Some(name) = function.name() {
            for node in infer.refs(name) {
                infer.calculate(node, &mut usage);
            }
        }
        infer.combine(usage.this_types)
    }
}

impl Inference<'_> {
    fn refs(&mut self, name: P<Node>) -> Vec<P<Node>> {
        let Some(symbol) = self.c.get_symbol_at_location_exported(name) else {
            return Vec::new();
        };
        if let Some(references) = self.references.get(&symbol) {
            return references.clone();
        }
        let mut matches = Vec::new();
        fn visit(
            c: &mut Checker,
            node: P<Node>,
            name: &str,
            symbol: P<Symbol>,
            out: &mut Vec<P<Node>>,
        ) {
            if ast::is_identifier(node)
                && node.text() == name
                && c.get_symbol_at_location_exported(node) == Some(symbol)
            {
                out.push(node);
            }
            node.for_each_child(&mut |child| {
                visit(c, child, name, symbol, out);
                false
            });
        }
        for &file in self.files {
            visit(self.c, file.as_node(), name.text(), symbol, &mut matches);
        }
        self.references.insert(symbol, matches.clone());
        matches
    }

    fn parameters(&mut self, function: P<Node>) -> Vec<InferredParameter> {
        let name = function.name().or_else(|| {
            let p = function.parent().filter(|p| ast::is_variable_declaration(*p))?;
            p.name()
        });
        let mut usage = Usage::default();
        if let Some(name) = name {
            for node in self.refs(name) {
                self.calculate(node, &mut usage);
            }
        }
        let calls: Vec<Call> = usage.constructs.into_iter().chain(usage.calls).collect();
        function
            .parameters()
            .iter()
            .enumerate()
            .map(|(index, &declaration)| {
                let rest = is_rest_parameter(declaration);
                let mut types = Vec::new();
                let mut optional = false;
                for call in &calls {
                    if call.arguments.len() <= index {
                        optional = ast::is_in_js_file(function);
                        types.push(self.c.undefined_type);
                    } else if rest {
                        for &t in &call.arguments[index..] {
                            types.push(self.c.get_base_type_of_literal_type(t));
                        }
                    } else {
                        types.push(self.c.get_base_type_of_literal_type(call.arguments[index]));
                    }
                }
                if let Some(name) = declaration.name().filter(|n| ast::is_identifier(*n)) {
                    let mut usage = Usage::default();
                    for node in self.refs(name) {
                        self.calculate(node, &mut usage);
                    }
                    let inferred = self.types(&usage);
                    for t in inferred {
                        if rest {
                            if let Some(element) = self.c.get_element_type_of_array_type(t) {
                                types.push(element);
                            }
                        } else {
                            types.push(t);
                        }
                    }
                }
                let t = self.combine(types);
                InferredParameter {
                    declaration,
                    type_: if rest { self.c.create_array_type(t) } else { t },
                    optional: optional && !rest,
                }
            })
            .collect()
    }

    fn candidate(usage: &mut Usage, t: Option<P<Type>>) {
        if let Some(t) = t.filter(|t| !t.flags().intersects(TypeFlags::Any | TypeFlags::Never)) {
            usage.candidates.push(t);
        }
    }
    fn calculate(&mut self, mut node: P<Node>, usage: &mut Usage) {
        while node.parent().is_some_and(|p| {
            (ast::is_property_access_expression(p) || ast::is_qualified_name(p))
                && p.name() == Some(node)
        }) {
            node = node.parent().unwrap();
        }
        let Some(parent) = node.parent() else {
            return;
        };
        match parent.kind() {
            Kind::ExpressionStatement => {
                if ast::is_call_expression(node) {
                    usage.candidates.push(self.c.void_type);
                }
            }
            Kind::PostfixUnaryExpression => usage.number = true,
            Kind::PrefixUnaryExpression => match parent.as_prefix_unary_expression().operator {
                Kind::MinusToken
                | Kind::TildeToken
                | Kind::PlusPlusToken
                | Kind::MinusMinusToken => usage.number = true,
                Kind::PlusToken => usage.number_or_string = true,
                _ => {}
            },
            Kind::BinaryExpression => self.binary(node, parent, usage),
            Kind::CaseClause | Kind::DefaultClause => {
                let expression =
                    parent.parent().and_then(|p| p.parent()).and_then(|p| p.expression());
                let t = expression.map(|n| self.c.get_type_at_location(n));
                Self::candidate(usage, t);
            }
            Kind::CallExpression | Kind::NewExpression => {
                if parent.expression() == Some(node) {
                    let args = parent
                        .arguments()
                        .iter()
                        .map(|&n| self.c.get_type_at_location(n))
                        .collect();
                    let mut result = Usage::default();
                    self.calculate(parent, &mut result);
                    let call = Call { arguments: args, result };
                    if ast::is_call_expression(parent) {
                        usage.calls.push(call);
                    } else {
                        usage.constructs.push(call);
                    }
                } else {
                    let t = self.c.get_contextual_type(node, ContextFlags::None);
                    Self::candidate(usage, t);
                }
            }
            Kind::PropertyAccessExpression => {
                let name = parent.name().unwrap().text();
                let i = usage.properties.iter().position(|(n, _)| n == name).unwrap_or_else(|| {
                    usage.properties.push((name.to_string(), Usage::default()));
                    usage.properties.len() - 1
                });
                self.calculate(parent, &mut usage.properties[i].1);
            }
            Kind::ElementAccessExpression => {
                let arg = parent.as_element_access_expression().argument_expression;
                if arg == node {
                    usage.number_or_string = true;
                } else {
                    let t = self.c.get_type_at_location(arg);
                    let mut u = Usage::default();
                    self.calculate(parent, &mut u);
                    if t.flags().intersects(TypeFlags::NumberLike) {
                        usage.number_index = Some(Box::new(u));
                    } else {
                        usage.string_index = Some(Box::new(u));
                    }
                }
            }
            Kind::PropertyAssignment | Kind::ShorthandPropertyAssignment => {
                let object = parent.parent().unwrap();
                let declaration =
                    object.parent().filter(|p| ast::is_variable_declaration(*p)).unwrap_or(object);
                let t = self.c.get_type_at_location(declaration);
                if !t.flags().intersects(TypeFlags::Any | TypeFlags::Never) {
                    usage.this_types.push(t);
                }
            }
            Kind::PropertyDeclaration => {
                let t = self.c.get_type_at_location(parent.parent().unwrap());
                usage.this_types.push(t);
            }
            Kind::VariableDeclaration | Kind::Parameter => {
                if parent.name() == Some(node) {
                    let t = parent.initializer().map(|n| self.c.get_type_at_location(n));
                    Self::candidate(usage, t);
                } else {
                    let t = self.c.get_contextual_type(node, ContextFlags::None);
                    Self::candidate(usage, t);
                }
            }
            _ => {
                if ast::is_expression_node(node) {
                    let t = self.c.get_contextual_type(node, ContextFlags::None);
                    Self::candidate(usage, t);
                }
            }
        }
    }
    fn binary(&mut self, node: P<Node>, parent: P<Node>, usage: &mut Usage) {
        let expression = parent.as_binary_expression();
        let other = if expression.left == node { expression.right() } else { expression.left };
        match expression.operator_token.kind() {
            Kind::AsteriskAsteriskToken
            | Kind::AsteriskToken
            | Kind::SlashToken
            | Kind::PercentToken
            | Kind::LessThanLessThanToken
            | Kind::GreaterThanGreaterThanToken
            | Kind::GreaterThanGreaterThanGreaterThanToken
            | Kind::AmpersandToken
            | Kind::BarToken
            | Kind::CaretToken
            | Kind::MinusEqualsToken
            | Kind::AsteriskAsteriskEqualsToken
            | Kind::AsteriskEqualsToken
            | Kind::SlashEqualsToken
            | Kind::PercentEqualsToken
            | Kind::AmpersandEqualsToken
            | Kind::BarEqualsToken
            | Kind::CaretEqualsToken
            | Kind::LessThanLessThanEqualsToken
            | Kind::GreaterThanGreaterThanEqualsToken
            | Kind::GreaterThanGreaterThanGreaterThanEqualsToken
            | Kind::MinusToken
            | Kind::LessThanToken
            | Kind::LessThanEqualsToken
            | Kind::GreaterThanToken
            | Kind::GreaterThanEqualsToken => {
                let t = self.c.get_type_at_location(other);
                if t.flags().intersects(TypeFlags::EnumLike) {
                    Self::candidate(usage, Some(t));
                } else {
                    usage.number = true;
                }
            }
            Kind::PlusEqualsToken | Kind::PlusToken => {
                let t = self.c.get_type_at_location(other);
                if t.flags().intersects(TypeFlags::EnumLike) {
                    Self::candidate(usage, Some(t));
                } else if t.flags().intersects(TypeFlags::NumberLike) {
                    usage.number = true;
                } else if t.flags().intersects(TypeFlags::StringLike) {
                    usage.string = true;
                } else if !t.flags().intersects(TypeFlags::Any) {
                    usage.number_or_string = true;
                }
            }
            Kind::EqualsToken
            | Kind::EqualsEqualsToken
            | Kind::EqualsEqualsEqualsToken
            | Kind::ExclamationEqualsEqualsToken
            | Kind::ExclamationEqualsToken
            | Kind::AmpersandAmpersandEqualsToken
            | Kind::QuestionQuestionEqualsToken
            | Kind::BarBarEqualsToken => {
                let t = self.c.get_type_at_location(other);
                Self::candidate(usage, Some(t));
            }
            Kind::InKeyword => {
                if node == expression.left {
                    usage.string = true;
                }
            }
            Kind::BarBarToken | Kind::QuestionQuestionToken
                if node == expression.left
                    && parent.parent().is_some_and(|p| {
                        ast::is_variable_declaration(p) || ast::is_assignment_expression(p, true)
                    }) =>
            {
                let t = self.c.get_type_at_location(expression.right());
                Self::candidate(usage, Some(t));
            }
            _ => {}
        }
    }
    fn combine(&mut self, mut types: Vec<P<Type>>) -> P<Type> {
        if types.is_empty() {
            return self.c.any_type;
        }
        let string_number = self.c.get_union_type(&[self.c.string_type, self.c.number_type]);
        if types.contains(&self.c.string_type) || types.contains(&self.c.number_type) {
            types.retain(|&t| t != string_number);
        }
        if types.iter().any(|t| !t.flags().intersects(TypeFlags::Any | TypeFlags::Void)) {
            types.retain(|t| !t.flags().intersects(TypeFlags::Any | TypeFlags::Void));
        }
        if types.iter().any(|t| {
            !t.flags().intersects(TypeFlags::Nullable | TypeFlags::Any | TypeFlags::Void)
                && !t.object_flags().intersects(ObjectFlags::Anonymous)
        }) {
            types.retain(|t| !t.object_flags().intersects(ObjectFlags::Anonymous));
        }
        let anonymous: Vec<_> = types
            .iter()
            .copied()
            .filter(|t| t.object_flags().intersects(ObjectFlags::Anonymous))
            .collect();
        if anonymous.len() > 1 {
            types.retain(|t| !t.object_flags().intersects(ObjectFlags::Anonymous));
            types.push(self.merge_anonymous(&anonymous));
        }
        let widened: Vec<_> =
            types.into_iter().map(|t| self.c.get_base_type_of_literal_type(t)).collect();
        let union =
            self.c.get_union_type_ex(&widened, UnionReduction::Subtype, AliasArg::None, None);
        self.c.get_widened_type(union)
    }
    fn merge_anonymous(&mut self, types: &[P<Type>]) -> P<Type> {
        let mut properties: Vec<(&'static str, Vec<P<Type>>)> = Vec::new();
        let mut slots = FxHashMap::default();
        let mut calls = Vec::new();
        let mut constructs = Vec::new();
        let mut indices: [Vec<P<Type>>; 2] = [Vec::new(), Vec::new()];
        let mut readonly = [false, false];
        for &t in types {
            let props = self.c.get_properties_of_type(t).to_vec();
            for prop in props {
                let typ = prop
                    .value_declaration()
                    .and_then(|n| self.c.get_type_of_symbol_at_location(prop, Some(n)))
                    .unwrap_or(self.c.any_type);
                let i = *slots.entry(prop.name()).or_insert_with(|| {
                    properties.push((prop.name(), Vec::new()));
                    properties.len() - 1
                });
                properties[i].1.push(typ);
            }
            calls.extend(self.c.get_signatures_of_type(t, SignatureKind::Call));
            constructs.extend(self.c.get_signatures_of_type(t, SignatureKind::Construct));
            for (i, key) in [self.c.string_type, self.c.number_type].into_iter().enumerate() {
                if let Some(info) = self.c.get_index_info_of_type(t, key) {
                    indices[i].push(info.value_type());
                    readonly[i] |= info.is_readonly();
                }
            }
        }
        let members = SymbolTable::new();
        for (name, types_of_property) in properties {
            let mut flags = SymbolFlags::Property;
            if types_of_property.len() < types.len() {
                flags |= SymbolFlags::Optional;
            }
            let symbol = self.c.new_symbol(flags, name);
            let typ = self.c.get_union_type(&types_of_property);
            self.c.value_symbol_links.get(symbol).resolved_type.set(Some(typ));
            members.set(name, symbol);
        }
        let mut infos = Vec::new();
        for (i, key) in [self.c.string_type, self.c.number_type].into_iter().enumerate() {
            if !indices[i].is_empty() {
                let t = self.c.get_union_type(&indices[i]);
                infos.push(Checker::new_index_info(key, t, readonly[i], None, &[]));
            }
        }
        self.c.new_anonymous_type(types[0].symbol(), Some(members), &calls, &constructs, &infos)
    }
    fn types(&mut self, usage: &Usage) -> Vec<P<Type>> {
        let mut types = Vec::new();
        if usage.number {
            types.push(self.c.number_type);
        }
        if usage.string {
            types.push(self.c.string_type);
        }
        if usage.number_or_string {
            types.push(self.c.get_union_type(&[self.c.string_type, self.c.number_type]));
        }
        if let Some(index) = &usage.number_index {
            let t = self.type_from_usage(index);
            types.push(self.c.create_array_type(t));
        }
        if !usage.properties.is_empty()
            || !usage.constructs.is_empty()
            || usage.string_index.is_some()
        {
            types.push(self.structural(usage));
        }
        let mut candidates: Vec<_> =
            usage.candidates.iter().map(|&t| self.c.get_base_type_of_literal_type(t)).collect();
        if !usage.calls.is_empty() {
            candidates.insert(0, self.structural(usage));
            types.push(self.c.get_union_type_ex(
                &candidates,
                UnionReduction::Subtype,
                AliasArg::None,
                None,
            ));
        } else {
            types.extend(candidates);
        }
        types.extend(self.named(usage));
        types
    }
    fn type_from_usage(&mut self, usage: &Usage) -> P<Type> {
        let types = self.types(usage);
        self.combine(types)
    }
    fn structural(&mut self, usage: &Usage) -> P<Type> {
        let members = SymbolTable::new();
        for (name, u) in &usage.properties {
            let symbol = self.c.new_symbol(SymbolFlags::Property, alloc_str(name));
            let typ = self.type_from_usage(u);
            self.c.value_symbol_links.get(symbol).resolved_type.set(Some(typ));
            members.set(symbol.name(), symbol);
        }
        let calls =
            if usage.calls.is_empty() { Vec::new() } else { vec![self.signature(&usage.calls)] };
        let constructs = if usage.constructs.is_empty() {
            Vec::new()
        } else {
            vec![self.signature(&usage.constructs)]
        };
        let indices = usage
            .string_index
            .as_ref()
            .map(|u| {
                let t = self.type_from_usage(u);
                Checker::new_index_info(self.c.string_type, t, false, None, &[])
            })
            .into_iter()
            .collect::<Vec<_>>();
        self.c.new_anonymous_type(None, Some(members), &calls, &constructs, &indices)
    }
    fn signature(&mut self, calls: &[Call]) -> P<Signature> {
        let count = calls.iter().map(|call| call.arguments.len()).max().unwrap_or(0);
        let mut parameters = Vec::new();
        for i in 0..count {
            let mut flags = SymbolFlags::FunctionScopedVariable;
            if calls.iter().any(|c| c.arguments.len() <= i) {
                flags |= SymbolFlags::Optional;
            }
            let symbol = self.c.new_symbol(flags, alloc_str(&format!("arg{i}")));
            let types = calls
                .iter()
                .map(|call| call.arguments.get(i).copied().unwrap_or(self.c.undefined_type))
                .collect();
            let typ = self.combine(types);
            self.c.value_symbol_links.get(symbol).resolved_type.set(Some(typ));
            parameters.push(symbol);
        }
        let usage = Self::merge_usages(calls.iter().map(|c| &c.result));
        let return_type = self.type_from_usage(&usage);
        self.c.new_signature(
            SignatureFlags::None,
            None,
            &[],
            None,
            &parameters,
            Some(return_type),
            None,
            count,
        )
    }
    fn merge_usages<'a>(usages: impl Iterator<Item = &'a Usage>) -> Usage {
        let mut result = Usage::default();
        for u in usages {
            result.number |= u.number;
            result.string |= u.string;
            result.number_or_string |= u.number_or_string;
            result.candidates.extend(&u.candidates);
            result.calls.extend(u.calls.iter().cloned());
            result.constructs.extend(u.constructs.iter().cloned());
            result.this_types.extend(&u.this_types);
            if result.number_index.is_none() {
                result.number_index.clone_from(&u.number_index);
            }
            if result.string_index.is_none() {
                result.string_index.clone_from(&u.string_index);
            }
            for (name, usage) in &u.properties {
                if let Some((_, existing)) = result.properties.iter_mut().find(|(n, _)| n == name) {
                    *existing = Self::merge_usages([&*existing, usage].into_iter());
                } else {
                    result.properties.push((name.clone(), usage.clone()));
                }
            }
        }
        result
    }
    fn named(&mut self, usage: &Usage) -> Vec<P<Type>> {
        if usage.properties.is_empty() {
            return Vec::new();
        }
        let array = self.c.create_array_type(self.c.any_type);
        let promise = self.c.create_promise_type(self.c.any_type);
        let mut candidates = Vec::new();
        for t in [self.c.string_type, self.c.number_type, array, promise] {
            let mut valid = true;
            for (name, u) in &usage.properties {
                let Some(property_type) = self.c.get_type_of_property_of_type(t, name) else {
                    valid = false;
                    break;
                };
                let inferred = if u.calls.is_empty() {
                    self.type_from_usage(u)
                } else {
                    let sig = self.signature(&u.calls);
                    self.c.new_anonymous_type(None, None, &[sig], &[], &[])
                };
                if !self.c.is_type_assignable_to(property_type, inferred) {
                    valid = false;
                    break;
                }
            }
            if valid {
                candidates.push(t);
            }
        }
        if candidates.is_empty() || candidates.len() >= 3 {
            return Vec::new();
        }
        candidates.into_iter().map(|t| self.instantiate_named(t, usage)).collect()
    }
    fn instantiate_named(&mut self, t: P<Type>, usage: &Usage) -> P<Type> {
        if !t.object_flags().intersects(ObjectFlags::Reference) {
            return t;
        }
        let generic = t.target().unwrap();
        let parameters = generic.as_interface_type().type_parameters();
        if parameters.len() != 1 {
            return t;
        }
        let mut inferred = Vec::new();
        for (name, u) in &usage.properties {
            let property = self.c.get_type_of_property_of_type(generic, name).unwrap();
            let usage_type = self.type_from_usage(u);
            self.infer_type_parameters(property, usage_type, parameters[0], &mut inferred);
        }
        let arg = self.combine(inferred);
        if self.c.is_array_type(t) {
            self.c.create_array_type(arg)
        } else {
            self.c.create_promise_type(arg)
        }
    }
    fn infer_type_parameters(
        &mut self,
        generic: P<Type>,
        usage: P<Type>,
        parameter: P<Type>,
        out: &mut Vec<P<Type>>,
    ) {
        if generic == parameter {
            out.push(usage);
        } else if generic.flags().intersects(TypeFlags::UnionOrIntersection) {
            for &part in generic.types() {
                self.infer_type_parameters(part, usage, parameter, out);
            }
        } else if generic.object_flags().intersects(ObjectFlags::Reference)
            && usage.object_flags().intersects(ObjectFlags::Reference)
        {
            let left = self.c.get_type_arguments(generic);
            let right = self.c.get_type_arguments(usage);
            for (&left, &right) in left.iter().zip(right) {
                self.infer_type_parameters(left, right, parameter, out);
            }
        } else {
            let left = self.c.get_signatures_of_type(generic, SignatureKind::Call);
            let right = self.c.get_signatures_of_type(usage, SignatureKind::Call);
            if left.len() == 1 && right.len() == 1 {
                for (&left, &right) in left[0].parameters().iter().zip(right[0].parameters()) {
                    let mut l = self.c.get_type_of_symbol(left);
                    if left.value_declaration().is_some_and(is_rest_parameter) {
                        l = self.c.get_element_type_of_array_type(l).unwrap_or(l);
                    }
                    let r = self.c.get_type_of_symbol(right);
                    self.infer_type_parameters(l, r, parameter, out);
                }
                let l = self.c.get_return_type_of_signature(left[0]);
                let r = self.c.get_return_type_of_signature(right[0]);
                self.infer_type_parameters(l, r, parameter, out);
            }
        }
    }
}
