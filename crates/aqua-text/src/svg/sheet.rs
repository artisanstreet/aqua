use roxmltree::{Document, Node};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Compound {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Selector {
    chain: Vec<Compound>,
    specificity: u32,
}

#[derive(Clone, Debug)]
struct Rule {
    selector: Selector,
    order: usize,
    declarations: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default)]
pub struct Sheet {
    rules: Vec<Rule>,
}

fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        rest = match rest[start + 2..].find("*/") {
            Some(end) => &rest[start + 2 + end + 2..],
            None => "",
        };
    }
    out.push_str(rest);
    out
}

fn parse_compound(part: &str) -> Option<Compound> {
    let mut compound = Compound {
        tag: None,
        id: None,
        classes: Vec::new(),
    };
    let mut cursor = 0;
    let bytes = part.as_bytes();
    let mut marker = b'\0';
    for index in 0..=part.len() {
        let boundary = index == part.len() || matches!(bytes[index], b'.' | b'#');
        if !boundary {
            continue;
        }
        let token = &part[cursor..index];
        match (marker, token.is_empty()) {
            (_, true) => {}
            (b'.', false) => compound.classes.push(token.to_string()),
            (b'#', false) => compound.id = Some(token.to_string()),
            (_, false) if token == "*" => {}
            (_, false) if token.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') => {
                compound.tag = Some(token.to_string())
            }
            _ => return None,
        }
        if index < part.len() {
            marker = bytes[index];
            cursor = index + 1;
        }
    }
    Some(compound)
}

fn parse_selector(text: &str) -> Option<Selector> {
    let chain: Option<Vec<Compound>> = text.split_whitespace().map(parse_compound).collect();
    let chain = chain.filter(|chain| !chain.is_empty())?;
    let specificity = chain
        .iter()
        .map(|c| {
            c.id.iter().count() as u32 * 100 + c.classes.len() as u32 * 10 + c.tag.iter().count() as u32
        })
        .sum();
    Some(Selector { chain, specificity })
}

fn parse_declarations(block: &str) -> Vec<(String, String)> {
    block
        .split(';')
        .filter_map(|declaration| declaration.split_once(':'))
        .map(|(name, value)| {
            (
                name.trim().to_string(),
                value.replace("!important", "").trim().to_string(),
            )
        })
        .filter(|(name, value)| !name.is_empty() && !value.is_empty())
        .collect()
}

fn matches_compound(node: Node<'_, '_>, compound: &Compound) -> bool {
    node.is_element()
        && compound
            .tag
            .as_deref()
            .is_none_or(|tag| node.tag_name().name() == tag)
        && compound
            .id
            .as_deref()
            .is_none_or(|id| node.attribute("id") == Some(id))
        && compound.classes.iter().all(|class| {
            node.attribute("class")
                .is_some_and(|value| value.split_whitespace().any(|part| part == class))
        })
}

fn matches(node: Node<'_, '_>, chain: &[Compound]) -> bool {
    match chain.split_last() {
        None => true,
        Some((last, rest)) => {
            matches_compound(node, last)
                && match rest.is_empty() {
                    true => true,
                    false => node
                        .ancestors()
                        .skip(1)
                        .any(|ancestor| matches(ancestor, rest)),
                }
        }
    }
}

impl Sheet {
    pub fn parse_document(document: &Document<'_>) -> Self {
        let css: String = document
            .descendants()
            .filter(|node| node.is_element() && node.tag_name().name() == "style")
            .filter(|node| {
                node.attribute("type")
                    .is_none_or(|kind| kind.is_empty() || kind == "text/css")
            })
            .flat_map(|node| node.children())
            .filter_map(|child| child.text())
            .collect::<Vec<_>>()
            .join("\n");
        Self::parse(&css)
    }

    pub fn parse(css: &str) -> Self {
        let css = strip_comments(css);
        let mut rules = Vec::new();
        let mut rest = css.as_str();
        while let Some(open) = rest.find('{') {
            let head = rest[..open].trim();
            let body_start = open + 1;
            let Some(close) = rest[body_start..].find('}') else {
                break;
            };
            let body = &rest[body_start..body_start + close];
            rest = &rest[body_start + close + 1..];
            if head.starts_with('@') {
                if body.contains('{') {
                    rest = rest.split_once('}').map_or("", |(_, tail)| tail);
                }
                continue;
            }
            let declarations = parse_declarations(body);
            head.split(',')
                .filter_map(|text| parse_selector(text.trim()))
                .for_each(|selector| {
                    rules.push(Rule {
                        selector,
                        order: rules.len(),
                        declarations: declarations.clone(),
                    })
                });
        }
        Self { rules }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn declarations(&self, node: Node<'_, '_>) -> Vec<(String, String)> {
        let mut matched: Vec<&Rule> = self
            .rules
            .iter()
            .filter(|rule| matches(node, &rule.selector.chain))
            .collect();
        matched.sort_by_key(|rule| (rule.selector.specificity, rule.order));
        matched
            .into_iter()
            .flat_map(|rule| rule.declarations.iter().cloned())
            .collect()
    }
}
