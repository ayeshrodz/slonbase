//! Plain-text excerpts of HTML, for the `fields` parameter's `:excerpt(max, withEllipsis?)`
//! modifier. A port of PocketBase's (`tools/picker/excerpt_modifier.go`): the HTML is parsed
//! with a spec-compliant HTML5 parser (html5ever, as Go's `x/net/html`), and the text is
//! gathered with the same rules, quirks included, so excerpts are the same as PocketBase's.

use std::borrow::Cow;
use std::cell::{Cell, RefCell};

use html5ever::interface::{ElemName, ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::{Attribute, LocalName, Namespace, QualName, local_name};

/// Elements whose content isn't text.
const EXCLUDED: &[&str] = &[
    "head", "style", "script", "iframe", "embed", "applet", "object", "svg", "img", "picture",
    "dialog", "template", "button", "form", "textarea", "input", "select", "option",
];

/// Elements that don't separate words; any other element is a block.
const INLINE: &[&str] = &[
    "a", "abbr", "acronym", "b", "bdo", "big", "br", "button", "cite", "code", "em", "i", "label",
    "q", "small", "span", "strong", "strike", "sub", "sup", "time",
];

/// Elements nested deeper than this make the document fail, as Go's `x/net/html` refuses
/// documents whose stack of open elements grows past 512. It also bounds the parsing: HTML5
/// tree construction takes time quadratic in the nesting.
const MAX_DEPTH: u32 = 512;

/// HTML nested deeper than [`MAX_DEPTH`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooDeep;

/// The plain text of `html`, cut to about `max` characters, with `...` when cut and
/// `with_ellipsis`. `max` is assumed positive (the modifier refuses others).
pub fn excerpt(html: &str, max: i64, with_ellipsis: bool) -> Result<String, TooDeep> {
    let dom = parse(html)?;
    let mut walk = Walk {
        dom: &dom,
        out: String::new(),
        prev_space: false,
        // PocketBase stops gathering past max+2 bytes (Go's int arithmetic, overflow included)
        limit: max.wrapping_add(2),
    };
    walk.strip(DOCUMENT);

    let mut result = walk.out.trim().to_string();
    if result.len() as i64 > max {
        // (counted in characters, once the length in bytes is over)
        let count = result.chars().count();
        if count as i64 > max {
            result = result
                .chars()
                .take(max as usize)
                .collect::<String>()
                .trim()
                .to_string();
            if with_ellipsis {
                result.push_str("...");
            }
        }
    }
    Ok(result)
}

/// Parses in chunks, stopping as soon as the nesting is too deep.
fn parse(html: &str) -> Result<Dom, TooDeep> {
    const CHUNK: usize = 16 * 1024;
    let too_deep = std::rc::Rc::new(Cell::new(false));
    let dom = Dom {
        too_deep: too_deep.clone(),
        ..Dom::default()
    };
    let mut parser = html5ever::parse_document(dom, Default::default());
    let mut rest = html;
    while !rest.is_empty() {
        let mut end = rest.len().min(CHUNK);
        while !rest.is_char_boundary(end) {
            end += 1;
        }
        let (chunk, tail) = rest.split_at(end);
        parser.process(StrTendril::from_slice(chunk));
        if too_deep.get() {
            return Err(TooDeep);
        }
        rest = tail;
    }
    let dom = parser.finish();
    if too_deep.get() {
        return Err(TooDeep);
    }
    Ok(dom)
}

struct Walk<'a> {
    dom: &'a Dom,
    out: String,
    prev_space: bool,
    limit: i64,
}

/// A step of the walk: PocketBase's recursive `stripTags`, with an explicit stack.
enum Step {
    Node(usize),
    /// The space around a block element, before and after its content.
    Space,
}

impl Walk<'_> {
    fn strip(&mut self, root: usize) {
        let mut stack = vec![Step::Node(root)];
        while let Some(step) = stack.pop() {
            let id = match step {
                Step::Space => {
                    if !self.prev_space {
                        self.out.push(' ');
                        self.prev_space = true;
                    }
                    continue;
                }
                Step::Node(id) => id,
            };
            let dom = self.dom;
            let nodes = dom.nodes.borrow();
            let node = &nodes[id];
            if let Kind::Text(text) = &node.kind {
                self.text(text);
            }
            if self.out.len() as i64 > self.limit {
                continue;
            }
            // children in order: pushed in reverse
            for &child in node.children.iter().rev() {
                let name = match &nodes[child].kind {
                    Kind::Element { name, .. } => Some(&*name.local),
                    _ => None,
                };
                if name.is_some_and(|n| EXCLUDED.contains(&n)) {
                    continue;
                }
                let block = name.is_some_and(|n| !INLINE.contains(&n));
                if block {
                    stack.push(Step::Space);
                }
                stack.push(Step::Node(child));
                if block {
                    stack.push(Step::Space);
                }
            }
        }
    }

    fn text(&mut self, text: &str) {
        // runs of Go's `\s` ([\t\n\f\r ]) become one space
        let mut collapsed = String::with_capacity(text.len());
        let mut in_space = false;
        for c in text.chars() {
            if matches!(c, '\t' | '\n' | '\x0C' | '\r' | ' ') {
                if !in_space {
                    collapsed.push(' ');
                }
                in_space = true;
            } else {
                collapsed.push(c);
                in_space = false;
            }
        }
        let text = if self.prev_space {
            collapsed.trim_start_matches(' ')
        } else {
            collapsed.as_str()
        };
        if !text.is_empty() {
            self.prev_space = text.ends_with(' ');
            self.out.push_str(text);
        }
    }
}

// ---------------------------------------------------------------------------
// a minimal DOM for html5ever: an arena of nodes, handles are indexes

const DOCUMENT: usize = 0;

enum Kind {
    Document,
    Element {
        name: QualName,
        template: Option<usize>,
    },
    Text(String),
    /// Comments, processing instructions, template contents
    Other,
}

struct Node {
    kind: Kind,
    parent: Option<usize>,
    children: Vec<usize>,
    /// Elements from the document down to this node, as inserted
    depth: u32,
}

struct Dom {
    nodes: RefCell<Vec<Node>>,
    /// Set once an element is nested deeper than [`MAX_DEPTH`]
    too_deep: std::rc::Rc<Cell<bool>>,
}

impl Default for Dom {
    fn default() -> Self {
        Self {
            nodes: RefCell::new(vec![Node {
                kind: Kind::Document,
                parent: None,
                children: Vec::new(),
                depth: 0,
            }]),
            too_deep: Default::default(),
        }
    }
}

impl Dom {
    fn add(&self, kind: Kind) -> usize {
        let mut nodes = self.nodes.borrow_mut();
        nodes.push(Node {
            kind,
            parent: None,
            children: Vec::new(),
            depth: 0,
        });
        nodes.len() - 1
    }

    fn detach(&self, id: usize) {
        let mut nodes = self.nodes.borrow_mut();
        if let Some(parent) = nodes[id].parent.take() {
            nodes[parent].children.retain(|&c| c != id);
        }
    }

    /// Inserts `child` into `parent` at `index`; text next to a text node joins it.
    fn insert(&self, parent: usize, index: usize, child: NodeOrText<usize>) {
        match child {
            NodeOrText::AppendText(text) => {
                let mut nodes = self.nodes.borrow_mut();
                if index > 0 {
                    let before = nodes[parent].children[index - 1];
                    if let Kind::Text(existing) = &mut nodes[before].kind {
                        existing.push_str(&text);
                        return;
                    }
                }
                drop(nodes);
                let id = self.add(Kind::Text(text.to_string()));
                let mut nodes = self.nodes.borrow_mut();
                nodes[id].parent = Some(parent);
                nodes[parent].children.insert(index, id);
            }
            NodeOrText::AppendNode(id) => {
                self.detach(id);
                let mut nodes = self.nodes.borrow_mut();
                let index = index.min(nodes[parent].children.len());
                nodes[id].parent = Some(parent);
                nodes[parent].children.insert(index, id);
                if matches!(nodes[id].kind, Kind::Element { .. }) {
                    let depth = nodes[parent].depth + 1;
                    nodes[id].depth = depth;
                    if depth > MAX_DEPTH {
                        self.too_deep.set(true);
                    }
                }
            }
        }
    }
}

/// An element's name, as html5ever asks for it.
#[derive(Debug)]
pub struct Name(QualName);

impl ElemName for Name {
    fn ns(&self) -> &Namespace {
        &self.0.ns
    }
    fn local_name(&self) -> &LocalName {
        &self.0.local
    }
}

impl TreeSink for Dom {
    type Handle = usize;
    type Output = Self;
    type ElemName<'a> = Name;

    fn finish(self) -> Self {
        self
    }

    fn parse_error(&self, _msg: Cow<'static, str>) {}

    fn get_document(&self) -> usize {
        DOCUMENT
    }

    fn elem_name<'a>(&'a self, target: &'a usize) -> Name {
        match &self.nodes.borrow()[*target].kind {
            Kind::Element { name, .. } => Name(name.clone()),
            // html5ever only asks for elements' names
            _ => Name(QualName::new(None, ns!(html), local_name!(""))),
        }
    }

    fn create_element(&self, name: QualName, _attrs: Vec<Attribute>, flags: ElementFlags) -> usize {
        let template = flags.template.then(|| self.add(Kind::Other));
        self.add(Kind::Element { name, template })
    }

    fn create_comment(&self, _text: StrTendril) -> usize {
        self.add(Kind::Other)
    }

    fn create_pi(&self, _target: StrTendril, _data: StrTendril) -> usize {
        self.add(Kind::Other)
    }

    fn append(&self, parent: &usize, child: NodeOrText<usize>) {
        let index = self.nodes.borrow()[*parent].children.len();
        self.insert(*parent, index, child);
    }

    fn append_based_on_parent_node(
        &self,
        element: &usize,
        prev_element: &usize,
        child: NodeOrText<usize>,
    ) {
        if self.nodes.borrow()[*element].parent.is_some() {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev_element, child);
        }
    }

    fn append_doctype_to_document(&self, _: StrTendril, _: StrTendril, _: StrTendril) {}

    fn get_template_contents(&self, target: &usize) -> usize {
        match &self.nodes.borrow()[*target].kind {
            Kind::Element {
                template: Some(contents),
                ..
            } => *contents,
            _ => *target,
        }
    }

    fn same_node(&self, x: &usize, y: &usize) -> bool {
        x == y
    }

    fn set_quirks_mode(&self, _mode: QuirksMode) {}

    fn append_before_sibling(&self, sibling: &usize, new_node: NodeOrText<usize>) {
        let Some(parent) = self.nodes.borrow()[*sibling].parent else {
            return;
        };
        let index = self.nodes.borrow()[parent]
            .children
            .iter()
            .position(|c| c == sibling)
            .unwrap_or(0);
        // a node moving within the same parent shifts the position
        if let NodeOrText::AppendNode(id) = &new_node {
            let nodes = self.nodes.borrow();
            if nodes[*id].parent == Some(parent) {
                let old = nodes[parent].children.iter().position(|c| c == id);
                drop(nodes);
                self.detach(*id);
                let index = match old {
                    Some(old) if old < index => index - 1,
                    _ => index,
                };
                self.insert(parent, index, new_node);
                return;
            }
        }
        self.insert(parent, index, new_node);
    }

    fn add_attrs_if_missing(&self, _target: &usize, _attrs: Vec<Attribute>) {}

    fn remove_from_parent(&self, target: &usize) {
        self.detach(*target);
    }

    fn reparent_children(&self, node: &usize, new_parent: &usize) {
        let children = std::mem::take(&mut self.nodes.borrow_mut()[*node].children);
        for child in children {
            self.nodes.borrow_mut()[child].parent = None;
            self.append(new_parent, NodeOrText::AppendNode(child));
        }
    }
}

use html5ever::ns;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_of_html() {
        let cases = [
            (
                "<p>Hello <b>world</b>!</p><p>Second   paragraph</p>",
                100,
                "Hello world! Second paragraph",
            ),
            ("<p>a<p>b<p>c", 100, "a b c"),
            (
                "<style>p{}</style><script>x()</script><p>visible</p>",
                100,
                "visible",
            ),
            ("<p>before<img src=x>after</p>", 100, "beforeafter"),
            (
                "<span>in</span><span>line</span><div>block</div>",
                100,
                "inline block",
            ),
            ("<p>&amp; &lt;tag&gt; &#x1F600;</p>", 100, "& <tag> 😀"),
            ("<title>head</title><p>body</p>", 100, "body"),
            ("<table>foster<tr><td>x</td></tr></table>", 100, "foster x"),
            ("<template><p>hidden</p></template>shown", 100, "shown"),
            ("", 10, ""),
        ];
        for (html, max, expected) in cases {
            assert_eq!(excerpt(html, max, false).unwrap(), expected, "{html}");
        }
    }

    #[test]
    fn cut_by_characters_with_an_optional_ellipsis() {
        assert_eq!(excerpt("<p>Hello world</p>", 5, false).unwrap(), "Hello");
        assert_eq!(excerpt("<p>Hello world</p>", 6, true).unwrap(), "Hello...");
        assert_eq!(excerpt("<p>Hello</p>", 5, true).unwrap(), "Hello");
        // the byte-based early stop, as in PocketBase: fewer characters for multi-byte text
        assert_eq!(excerpt("<p>Привет мир</p>", 4, true).unwrap(), "Прив...");
        // max+2 overflows in Go: nothing is gathered
        assert_eq!(excerpt("<p>Hello</p>", i64::MAX, true).unwrap(), "");
    }

    #[test]
    fn nesting_is_bounded_as_in_go() {
        // html and body, then 510 elements: as deep as Go's parser goes
        let ok = format!("{}x", "<div>".repeat(510));
        assert_eq!(excerpt(&ok, 5, false).unwrap(), "x");
        let deep = format!("{}x", "<div>".repeat(511));
        assert_eq!(excerpt(&deep, 5, false), Err(TooDeep));
        // implied and formatting elements count, auto-closed ones don't
        assert_eq!(excerpt(&"<b>".repeat(600), 5, false), Err(TooDeep));
        assert_eq!(
            excerpt(&"<table><tr><td>".repeat(300), 5, false),
            Err(TooDeep)
        );
        assert_eq!(excerpt(&"<p>a".repeat(2000), 5, false).unwrap(), "a a a");
        // and a hostile document is refused quickly
        let started = std::time::Instant::now();
        let huge = format!("{}x{}", "<div>".repeat(400_000), "</div>".repeat(400_000));
        assert_eq!(excerpt(&huge, 5, false), Err(TooDeep));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }
}
