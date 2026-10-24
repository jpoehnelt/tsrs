//! JavaScript editor observations needed by TSServer-compatible clients. Ordinary
//! expression checking retains the native compiler rules; these entry points resolve
//! `CommonJS` assignment declarations and constructor-function instance members.
use crate::*;
impl Checker {
    pub(crate) fn editor_module_exports_symbol(&mut self, node: P<Node>) -> Option<P<Symbol>> {
        if !ast::is_in_js_file(node) {
            return None;
        }
        let access = if ast::is_identifier(node) { node.parent()? } else { node };
        if !ast::is_module_exports_access_expression(access)
            || (node != access && access.name() != Some(node))
        {
            return None;
        }
        let module = self.get_symbol_at_location(access.expression()?, true)?;
        if !module.flags().intersects(SymbolFlags::ModuleExports) {
            return None;
        }
        let file = ast::get_source_file_of_node(access)?;
        let export = file.symbol()?.exports()?.lookup(ast::INTERNAL_SYMBOL_NAME_EXPORT_EQUALS)?;
        let links = self.symbol_node_links.get(access);
        if let Some(symbol) = links
            .resolved_symbol
            .get()
            .filter(|s| s.value_declaration() == export.value_declaration())
        {
            return Some(symbol);
        }
        let typ = self.get_type_of_symbol(export);
        let symbol = self.new_symbol(SymbolFlags::Property, "exports");
        symbol.set_declarations_static(export.declarations());
        symbol.set_value_declaration(export.value_declaration());
        symbol.set_parent(Some(module));
        self.value_symbol_links.get(symbol).resolved_type.set(Some(typ));
        links.resolved_symbol.set(Some(symbol));
        Some(symbol)
    }
    pub(crate) fn editor_js_type(&mut self, node: P<Node>) -> Option<P<Type>> {
        if !ast::is_in_js_file(node) {
            return None;
        }
        if ast::is_identifier(node) && node.text() == "module" {
            let symbol = self.get_symbol_at_location(node, true)?;
            if symbol.flags().intersects(SymbolFlags::ModuleExports) {
                let file = ast::get_source_file_of_node(node)?;
                let export =
                    file.symbol()?.exports()?.lookup(ast::INTERNAL_SYMBOL_NAME_EXPORT_EQUALS)?;
                return Some(self.get_type_of_symbol(export));
            }
        }
        if node.kind() == Kind::ThisKeyword {
            let native = self.get_type_of_node(node);
            if is_this_type_parameter(native) {
                return None;
            }
            let container = Self::get_this_container(node, false, false)?;
            if !ast::is_function_declaration(container) && !ast::is_function_expression(container) {
                return None;
            }
            let signature = self.get_signature_from_declaration(container);
            if self.get_this_type_of_signature(signature).is_some()
                || self.get_contextual_this_parameter_type(container).is_some()
            {
                return None;
            }
            let symbol = container.symbol()?;
            let members = symbol.members().filter(|m| !m.is_empty())?;
            let links = self.declared_type_links.get(symbol);
            if let Some(typ) = links.declared_type.get() {
                return Some(typ);
            }
            let instance = self.new_anonymous_type(None, Some(members), &[], &[], &[]);
            let typ = self.new_type_parameter(Some(symbol));
            typ.as_type_parameter().is_this_type.set(true);
            typ.as_type_parameter().constraint.set(Some(instance));
            links.declared_type.set(Some(typ));
            return Some(typ);
        }
        None
    }
}
