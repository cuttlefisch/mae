//! Auto-generated `module:<name>` KB nodes from active module data.
//!
//! Follows the same pattern as `install_command_nodes()` — one function
//! that generates nodes from registry data. No new KB infrastructure.

use mae_kb::{KnowledgeBase, Node, NodeKind};

/// Data about a module for KB node generation.
#[derive(Debug, Clone)]
pub struct ModuleKbData {
    pub name: String,
    pub version: String,
    pub category: String,
    pub description: String,
    pub status: String,
    pub flags: Vec<(String, String)>, // (flag_name, doc)
    pub commands: Vec<String>,
    pub options: Vec<String>,
    pub path: String,
}

/// Generate one `module:<name>` KB node per active module.
pub fn install_module_nodes(kb: &mut KnowledgeBase, modules: &[ModuleKbData]) {
    for m in modules {
        let mut body = String::new();

        body.push_str(&format!("* {}\n\n", m.name));

        if !m.description.is_empty() {
            body.push_str(&format!("{}\n\n", m.description));
        }

        body.push_str("| Field    | Value    |\n");
        body.push_str("|----------|----------|\n");
        body.push_str(&format!("| Version  | {}       |\n", m.version));
        if !m.category.is_empty() {
            body.push_str(&format!("| Category | {}       |\n", m.category));
        }
        body.push_str(&format!("| Status   | {}       |\n", m.status));
        body.push_str(&format!("| Path     | ={}=     |\n", m.path));
        body.push('\n');

        if !m.flags.is_empty() {
            body.push_str("** Flags\n\n");
            for (flag, doc) in &m.flags {
                body.push_str(&format!("- =+{}= — {}\n", flag, doc));
            }
            body.push('\n');
        }

        if !m.commands.is_empty() {
            body.push_str("** Commands\n\n");
            for cmd in &m.commands {
                body.push_str(&format!("- [[cmd:{}][{}]]\n", cmd, cmd));
            }
            body.push('\n');
        }

        if !m.options.is_empty() {
            body.push_str("** Options\n\n");
            for opt in &m.options {
                body.push_str(&format!("- [[option:{}][{}]]\n", opt, opt));
            }
            body.push('\n');
        }

        let mut tags = vec!["module".to_string()];
        if !m.category.is_empty() {
            tags.push(m.category.clone());
        }

        let node = Node::new(
            format!("module:{}", m.name),
            format!("Module: {}", m.name),
            NodeKind::Concept,
            body,
        )
        .with_tags(tags);

        kb.insert(node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_kb_node_generated() {
        let mut kb = KnowledgeBase::new();
        let modules = vec![ModuleKbData {
            name: "dashboard".to_string(),
            version: "0.1.0".to_string(),
            category: "ui".to_string(),
            description: "Splash screen module".to_string(),
            status: "loaded".to_string(),
            flags: vec![],
            commands: vec!["dashboard".to_string()],
            options: vec![],
            path: "modules/dashboard".to_string(),
        }];
        install_module_nodes(&mut kb, &modules);

        let node = kb.get("module:dashboard");
        assert!(node.is_some());
        assert_eq!(node.unwrap().title, "Module: dashboard");
    }

    #[test]
    fn module_kb_node_body_has_commands() {
        let mut kb = KnowledgeBase::new();
        let modules = vec![ModuleKbData {
            name: "surround".to_string(),
            version: "0.1.0".to_string(),
            category: "editor".to_string(),
            description: "Vim-surround bindings".to_string(),
            status: "loaded".to_string(),
            flags: vec![],
            commands: vec![
                "change-surround-await".to_string(),
                "delete-surround-await".to_string(),
            ],
            options: vec![],
            path: "modules/surround".to_string(),
        }];
        install_module_nodes(&mut kb, &modules);

        let node = kb.get("module:surround").unwrap();
        assert!(node.body.contains("change-surround-await"));
        assert!(node.body.contains("delete-surround-await"));
    }

    #[test]
    fn module_kb_node_has_flags() {
        let mut kb = KnowledgeBase::new();
        let modules = vec![ModuleKbData {
            name: "multicursor".to_string(),
            version: "0.1.0".to_string(),
            category: "editor".to_string(),
            description: "Multi-cursor editing".to_string(),
            status: "loaded".to_string(),
            flags: vec![("align".to_string(), "Enable alignment commands".to_string())],
            commands: vec![],
            options: vec![],
            path: "modules/multicursor".to_string(),
        }];
        install_module_nodes(&mut kb, &modules);

        let node = kb.get("module:multicursor").unwrap();
        assert!(node.body.contains("+align"));
    }
}
