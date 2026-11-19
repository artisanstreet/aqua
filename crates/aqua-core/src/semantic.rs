use crate::geometry::Rect;
use crate::id::NodeId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Text,
    PasswordField,
    ComboBox,
    Option,
    Separator,
    Tooltip,
    Group,
    Button,
    Checkbox,
    Switch,
    RadioButton,
    TextField,
    Heading(u8),
    List,
    ListBox,
    RadioGroup,
    ListItem,
    Tree,
    TreeItem,
    Table,
    Row,
    ColumnHeader,
    Cell,
    Slider,
    ProgressIndicator,
    TabList,
    Tab,
    Image,
    ScrollRegion,
    Dialog,
    Menu,
    MenuItem,
    Link,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LiveRegion {
    Polite,
    Assertive,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollSemantics {
    pub x: f64,
    pub y: f64,
    pub x_max: f64,
    pub y_max: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CollectionSemantics {
    pub row_count: Option<usize>,
    pub column_count: Option<usize>,
    pub row_index: Option<usize>,
    pub column_index: Option<usize>,
    pub position_in_set: Option<usize>,
    pub set_size: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NumericRangeSemantics {
    pub min: f64,
    pub max: f64,
    pub value: f64,
    pub step: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SemanticNode {
    pub node: NodeId,
    pub role: Role,
    pub label: String,
    pub description: Option<String>,
    pub focused: bool,
    pub disabled: bool,
    pub selected: bool,
    pub checked: Option<bool>,
    pub value_text: Option<String>,
    pub numeric_range: Option<NumericRangeSemantics>,
    pub expanded: Option<bool>,
    pub level: Option<usize>,
    pub children: Vec<NodeId>,
    pub bounds: Option<Rect>,
    pub collection: Option<CollectionSemantics>,
    pub focusable: bool,
    pub live: Option<LiveRegion>,
    pub busy: bool,
    pub modal: bool,
    pub language: Option<String>,
    pub scroll: Option<ScrollSemantics>,
    pub active_descendant: Option<NodeId>,
}

impl SemanticNode {
    #[must_use]
    pub fn new(node: NodeId, role: Role) -> Self {
        Self {
            node,
            role,
            label: String::new(),
            description: None,
            focused: false,
            disabled: false,
            selected: false,
            checked: None,
            value_text: None,
            numeric_range: None,
            expanded: None,
            level: None,
            children: Vec::new(),
            bounds: None,
            collection: None,
            focusable: false,
            live: None,
            busy: false,
            modal: false,
            language: None,
            scroll: None,
            active_descendant: None,
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
