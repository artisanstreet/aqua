//! Semantic (accessibility) data. Appearance and meaning are independent:
//! a heading stays a heading regardless of font size, and a button keeps
//! activation behavior regardless of visuals.

use crate::geometry::Rect;
use crate::id::NodeId;

/// Accessibility role of a UI node. Setting a role alone does not implement
/// behavior — behaviors (activation, toggling, navigation) live in `aqua-ui`
/// and are verified by tests, not by role assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// Generic container.
    Group,
    /// Activatable control.
    Button,
    /// Toggleable control.
    Checkbox,
    /// Editable or readable text.
    TextField,
    /// Non-editable text with heading level 1..=6.
    Heading(u8),
    /// List container.
    List,
    /// Selectable list row.
    ListItem,
    /// Table with shared columns.
    Table,
    /// Table row.
    Row,
    /// Table column header.
    ColumnHeader,
    /// Table data cell.
    Cell,
    /// Value slider.
    Slider,
    /// Tab bar / tab.
    TabList,
    /// Tab bar / tab.
    Tab,
    /// Image with textual alternative.
    Image,
    /// Scrollable region.
    ScrollRegion,
    /// Overlay dialog.
    Dialog,
    /// Menu / menu item.
    Menu,
    /// Menu / menu item.
    MenuItem,
}

/// Logical collection coordinates, independent of which rows are mounted.
/// Table indices are zero-based; list positions are one-based.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CollectionSemantics {
    pub row_count: Option<usize>,
    pub column_count: Option<usize>,
    pub row_index: Option<usize>,
    pub column_index: Option<usize>,
    pub position_in_set: Option<usize>,
    pub set_size: Option<usize>,
}

/// Semantic record for one node: identity, role, label, and state flags.
/// The platform adapter translates these to native accessibility APIs.
/// `children` and `bounds` come from the UI tree at emission time.
#[derive(Debug, Clone, PartialEq)]
pub struct SemanticNode {
    /// UI node identity.
    pub node: NodeId,
    /// Accessibility role.
    pub role: Role,
    /// Human-readable label.
    pub label: String,
    /// Whether the node currently has keyboard focus.
    pub focused: bool,
    /// Whether the node is disabled (visible but non-interactive).
    pub disabled: bool,
    /// Whether the node is selected (lists, tabs).
    pub selected: bool,
    /// Current value text for sliders, numeric fields, checkboxes.
    pub value_text: Option<String>,
    /// Child node identities in paint order.
    pub children: Vec<NodeId>,
    /// Committed layout bounds in logical units (`None` before layout).
    pub bounds: Option<Rect>,
    /// Total collection size and logical positions for virtualized content.
    pub collection: Option<CollectionSemantics>,
}

impl SemanticNode {
    /// Minimal record: group role, no label, enabled, unfocused, no layout.
    /// Interactive nodes MUST be assigned their real role afterwards —
    /// leaving `Group` on a button is a contract violation, not a default.
    #[must_use]
    pub fn new(node: NodeId, role: Role) -> Self {
        Self {
            node,
            role,
            label: String::new(),
            focused: false,
            disabled: false,
            selected: false,
            value_text: None,
            children: Vec::new(),
            bounds: None,
            collection: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::RawId;

    #[test]
    fn heading_preserves_level() {
        let n = SemanticNode::new(
            NodeId::from_raw(RawId {
                index: 0,
                generation: 0,
            }),
            Role::Heading(2),
        );
        assert_eq!(n.role, Role::Heading(2));
        assert!(!n.focused && !n.disabled && !n.selected);
    }
}
