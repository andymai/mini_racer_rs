#![allow(dead_code, reason = "each test binary uses a different part")]

use std::collections::HashMap;
use std::fmt::Write as _;

use mini_racer_wire::{Begun, Container, Scalar, Text, Visitor, text};

pub fn hex(bytes: impl IntoIterator<Item = u8>) -> String {
    bytes.into_iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Values,
    Dense,
    Sparse,
    Object,
    Map,
    Set,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Undefined,
    Null,
    Hole,
    Bool(bool),
    Int(i64),
    Number(f64),
    BigInt(bool, Vec<u8>),
    Text(String),
    Date(f64),
    RegExp(String, u32),
    Buffer(Vec<u8>),
    Container {
        kind: Kind,
        items: Vec<usize>,
        entries: Vec<(usize, usize)>,
    },
    Pending,
}

/// Builds an arena of nodes; a value is a node index, so a value reached
/// twice is the same index.
#[derive(Default)]
pub struct Tree {
    pub nodes: Vec<Node>,
    ids: HashMap<u32, usize>,
}

pub struct Frame {
    kind: Kind,
    slot: usize,
    items: Vec<usize>,
    entries: Vec<(usize, usize)>,
}

fn text_of(text: Text<'_>) -> String {
    match text {
        Text::Utf8(bytes) => String::from_utf8_lossy(bytes).into_owned(),
        Text::Latin1(bytes) => text::latin1_to_utf8(bytes),
        Text::Utf16Le(bytes) => text::utf16le_to_utf8(bytes).unwrap_or_else(|| {
            let units: Vec<String> = bytes
                .chunks(2)
                .map(|pair| format!("{:04x}", u16::from_le_bytes([pair[0], pair[1]])))
                .collect();
            format!("<utf16 {}>", units.join(" "))
        }),
    }
}

impl Tree {
    fn add(&mut self, node: Node) -> usize {
        self.nodes.push(node);
        self.nodes.len() - 1
    }
}

impl Visitor for Tree {
    type Value = usize;
    type Frame = Frame;
    type Error = String;

    fn scalar(&mut self, scalar: Scalar<'_>) -> Result<usize, String> {
        let node = match scalar {
            Scalar::Undefined => Node::Undefined,
            Scalar::Null => Node::Null,
            Scalar::Hole => Node::Hole,
            Scalar::Bool(b) => Node::Bool(b),
            Scalar::Int32(n) => Node::Int(n.into()),
            Scalar::Uint32(n) => Node::Int(n.into()),
            Scalar::Number(n) => Node::Number(n),
            Scalar::BigInt { negative, digits } => Node::BigInt(negative, digits.to_vec()),
            Scalar::Text(text) => Node::Text(text_of(text)),
            Scalar::Date(time) => Node::Date(time),
            Scalar::RegExp { pattern, flags } => Node::RegExp(text_of(pattern), flags),
            Scalar::ArrayBuffer(bytes) => Node::Buffer(bytes.to_vec()),
        };
        Ok(self.add(node))
    }

    fn begin(&mut self, container: Container) -> Result<Begun<Frame, usize>, String> {
        let kind = match container {
            Container::Values => Kind::Values,
            Container::DenseArray { .. } => Kind::Dense,
            Container::SparseArray { .. } => Kind::Sparse,
            Container::Object => Kind::Object,
            Container::Map => Kind::Map,
            Container::Set => Kind::Set,
            Container::Error => Kind::Error,
        };
        let slot = self.add(Node::Pending);
        Ok(Begun {
            frame: Frame {
                kind,
                slot,
                items: Vec::new(),
                entries: Vec::new(),
            },
            value: Some(slot),
        })
    }

    fn push(&mut self, frame: &mut Frame, value: usize) -> Result<(), String> {
        frame.items.push(value);
        Ok(())
    }

    fn insert(&mut self, frame: &mut Frame, key: usize, value: usize) -> Result<(), String> {
        frame.entries.push((key, value));
        Ok(())
    }

    fn end(&mut self, frame: Frame) -> Result<usize, String> {
        self.nodes[frame.slot] = Node::Container {
            kind: frame.kind,
            items: frame.items,
            entries: frame.entries,
        };
        Ok(frame.slot)
    }

    fn register(&mut self, id: u32, value: usize) -> Result<(), String> {
        match self.ids.insert(id, value) {
            None => Ok(()),
            Some(_) => Err(format!("id {id} registered twice")),
        }
    }

    fn back_ref(&mut self, id: u32) -> Result<usize, String> {
        self.ids
            .get(&id)
            .copied()
            .ok_or_else(|| format!("no object {id}"))
    }
}

fn number(n: f64) -> String {
    if n == 0.0 && n.is_sign_negative() {
        "-0".to_owned()
    } else {
        format!("{n:?}")
    }
}

impl Tree {
    /// A stable text form: containers seen twice print as `@n` after their
    /// first appearance, which is labelled `#n`.
    pub fn render(&self, root: usize) -> String {
        let mut shared = HashMap::new();
        self.count(root, &mut shared, &mut Vec::new());
        let mut labels = HashMap::new();
        let mut out = String::new();
        self.write(root, &shared, &mut labels, &mut out);
        out
    }

    fn count(&self, node: usize, seen: &mut HashMap<usize, usize>, stack: &mut Vec<usize>) {
        let hits = seen.entry(node).or_insert(0);
        *hits += 1;
        if *hits > 1 || stack.contains(&node) {
            return;
        }
        if let Node::Container { items, entries, .. } = &self.nodes[node] {
            stack.push(node);
            for &item in items {
                self.count(item, seen, stack);
            }
            for &(key, value) in entries {
                self.count(key, seen, stack);
                self.count(value, seen, stack);
            }
            stack.pop();
        }
    }

    fn write(
        &self,
        node: usize,
        shared: &HashMap<usize, usize>,
        labels: &mut HashMap<usize, usize>,
        out: &mut String,
    ) {
        let container = matches!(self.nodes[node], Node::Container { .. } | Node::Buffer(_));
        if container && shared.get(&node).copied().unwrap_or(0) > 1 {
            if let Some(label) = labels.get(&node) {
                let _ = write!(out, "@{label}");
                return;
            }
            let label = labels.len();
            labels.insert(node, label);
            let _ = write!(out, "#{label}=");
        }
        match &self.nodes[node] {
            Node::Undefined => out.push_str("undefined"),
            Node::Null => out.push_str("null"),
            Node::Hole => out.push_str("hole"),
            Node::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Node::Int(n) => {
                let _ = write!(out, "{n}");
            }
            Node::Number(n) => {
                let _ = write!(out, "num({})", number(*n));
            }
            Node::BigInt(negative, digits) => {
                let sign = if *negative { "-" } else { "" };
                let _ = write!(out, "{sign}0x{}n", hex(digits.iter().rev().copied()));
            }
            Node::Text(text) => {
                let _ = write!(out, "{text:?}");
            }
            Node::Date(time) => {
                let _ = write!(out, "date({})", number(*time));
            }
            Node::RegExp(pattern, flags) => {
                let _ = write!(out, "regexp({pattern:?}, {flags})");
            }
            Node::Buffer(bytes) => {
                let _ = write!(out, "buffer({})", hex(bytes.iter().copied()));
            }
            Node::Pending => out.push_str("pending"),
            Node::Container {
                kind,
                items,
                entries,
            } => {
                let (open, close) = match kind {
                    Kind::Values => ("values(", ")"),
                    Kind::Dense => ("[", "]"),
                    Kind::Sparse => ("sparse[", "]"),
                    Kind::Object => ("{", "}"),
                    Kind::Map => ("map{", "}"),
                    Kind::Set => ("set{", "}"),
                    Kind::Error => ("error{", "}"),
                };
                out.push_str(open);
                let mut first = true;
                for &item in items {
                    if !first {
                        out.push_str(", ");
                    }
                    first = false;
                    self.write(item, shared, labels, out);
                }
                for &(key, value) in entries {
                    if !first {
                        out.push_str(", ");
                    }
                    first = false;
                    self.write(key, shared, labels, out);
                    out.push_str(": ");
                    self.write(value, shared, labels, out);
                }
                out.push_str(close);
            }
        }
    }
}
