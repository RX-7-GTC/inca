//! Generic dependency nodes, attributes and plugs.
//!
//! DAG transforms/shapes stay in `Scene::nodes`; this graph holds non-DAG nodes such as
//! animation layers, display layers and shading-network nodes. Both are addressed through
//! `NodeRef`, so the UI and MEL can treat dependency nodes as first-class scene nodes.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::anim::AnimCurve;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AttrType {
    Float,
    Int,
    Bool,
    Enum(Vec<String>),
    Vector,
    String,
    Message,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AttrSpec {
    pub ty: AttrType,
    pub default: Value,
    #[serde(default)]
    pub keyable: bool,
    #[serde(default = "yes")]
    pub readable: bool,
    #[serde(default = "yes")]
    pub writable: bool,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
}

fn yes() -> bool {
    true
}

impl AttrSpec {
    fn base(ty: AttrType, default: Value) -> Self {
        Self { ty, default, keyable: true, readable: true, writable: true, min: None, max: None }
    }
    pub fn float(default: f64) -> Self {
        Self::base(AttrType::Float, Value::from(default))
    }
    pub fn int(default: i64) -> Self {
        Self::base(AttrType::Int, Value::from(default))
    }
    pub fn boolean(default: bool) -> Self {
        Self::base(AttrType::Bool, Value::from(default))
    }
    pub fn enumeration(labels: &[&str], default: &str) -> Self {
        Self::base(AttrType::Enum(labels.iter().map(|s| s.to_string()).collect()), Value::from(default))
    }
    pub fn vector(default: [f64; 3]) -> Self {
        Self::base(AttrType::Vector, Value::from(default))
    }
    pub fn string(default: &str) -> Self {
        Self::base(AttrType::String, Value::from(default))
    }
    pub fn message() -> Self {
        Self { ty: AttrType::Message, default: Value::Null, keyable: false, readable: false, writable: false, min: None, max: None }
    }
    pub fn keyable(mut self, on: bool) -> Self {
        self.keyable = on;
        self
    }
    pub fn read_only(mut self) -> Self {
        self.keyable = false;
        self.writable = false;
        self
    }
    pub fn range(mut self, min: f64, max: f64) -> Self {
        self.min = Some(min);
        self.max = Some(max);
        self
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DepNode {
    pub id: String,
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub attrs: IndexMap<String, Value>,
    #[serde(default)]
    pub specs: IndexMap<String, AttrSpec>,
    /// Key data for an `animCurve` node; None for every other node kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curve: Option<AnimCurve>,
}

impl DepNode {
    pub fn new(id: &str, name: &str, kind: &str) -> Self {
        Self { id: id.into(), name: name.into(), kind: kind.into(), attrs: IndexMap::new(), specs: IndexMap::new(), curve: None }
    }
    pub fn add_attr(&mut self, name: &str, spec: AttrSpec) -> &mut Self {
        self.attrs.entry(name.into()).or_insert_with(|| spec.default.clone());
        self.specs.insert(name.into(), spec);
        self
    }
    pub fn spec(&self, attr: &str) -> Option<&AttrSpec> {
        self.specs.get(attr)
    }
    pub fn get(&self, attr: &str) -> Option<Value> {
        self.attrs.get(attr).cloned().or_else(|| self.spec(attr).map(|s| s.default.clone()))
    }
    pub fn set(&mut self, attr: &str, value: Value) -> bool {
        let Some(spec) = self.spec(attr) else { return false };
        if !spec.writable {
            return false;
        }
        self.attrs.insert(attr.into(), value);
        true
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlugRef {
    pub node: String,
    pub attr: String,
}

impl PlugRef {
    pub fn new(node: &str, attr: &str) -> Self {
        Self { node: node.into(), attr: attr.into() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Connection {
    pub source: PlugRef,
    pub destination: PlugRef,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DepGraph {
    pub nodes: IndexMap<String, DepNode>,
    #[serde(default)]
    pub connections: Vec<Connection>,
}

impl DepGraph {
    pub fn node(&self, id: &str) -> Option<&DepNode> {
        self.nodes.get(id)
    }
    pub fn node_mut(&mut self, id: &str) -> Option<&mut DepNode> {
        self.nodes.get_mut(id)
    }
    pub fn find(&self, name: &str) -> Option<&DepNode> {
        self.nodes.values().find(|n| n.name == name)
    }
    pub fn insert(&mut self, node: DepNode) {
        self.nodes.insert(node.id.clone(), node);
    }
    pub fn remove(&mut self, id: &str) -> Option<DepNode> {
        self.connections.retain(|c| c.source.node != id && c.destination.node != id);
        self.nodes.shift_remove(id)
    }
    pub fn get(&self, plug: &PlugRef) -> Option<Value> {
        self.effective(plug).map(|(_, v)| v)
    }
    /// Follow a single incoming connection chain, returning the source plug and its value.
    pub fn effective(&self, plug: &PlugRef) -> Option<(PlugRef, Value)> {
        let mut cur = plug.clone();
        let mut seen = std::collections::HashSet::new();
        while seen.insert(cur.clone()) {
            if let Some(c) = self.connections.iter().rev().find(|c| c.destination == cur) {
                cur = c.source.clone();
            } else {
                let value = self.node(&cur.node)?.get(&cur.attr)?;
                return Some((cur, value));
            }
        }
        None
    }
    pub fn set(&mut self, plug: &PlugRef, value: Value) -> bool {
        self.node_mut(&plug.node).is_some_and(|n| n.set(&plug.attr, value))
    }
    pub fn connect(&mut self, source: PlugRef, destination: PlugRef) -> bool {
        if source == destination || source.node == destination.node && source.attr == destination.attr {
            return false;
        }
        if !self.plug_exists(&source) || !self.plug_exists(&destination) {
            return false;
        }
        if self.node(&destination.node).is_some_and(|n| n.spec(&destination.attr).is_some_and(|s| !s.writable && s.ty != AttrType::Message)) {
            return false;
        }
        // Reject cycles by walking backwards from the proposed source.
        let mut cur = source.clone();
        let mut seen = std::collections::HashSet::new();
        while seen.insert(cur.clone()) {
            if cur.node == destination.node && cur.attr == destination.attr {
                return false;
            }
            let Some(c) = self.connections.iter().rev().find(|c| c.destination == cur) else { break };
            cur = c.source.clone();
        }
        self.connections.retain(|c| c.destination != destination);
        self.connections.push(Connection { source, destination });
        true
    }
    pub fn disconnect(&mut self, destination: &PlugRef) -> bool {
        let before = self.connections.len();
        self.connections.retain(|c| &c.destination != destination);
        before != self.connections.len()
    }
    pub fn plug_exists(&self, plug: &PlugRef) -> bool {
        self.node(&plug.node).is_some_and(|n| n.spec(&plug.attr).is_some())
    }
    pub fn source_of(&self, plug: &PlugRef) -> Option<PlugRef> {
        self.connections.iter().rev().find(|c| &c.destination == plug).map(|c| c.source.clone())
    }
    pub fn destinations_of(&self, source: &PlugRef) -> Vec<PlugRef> {
        self.connections.iter().filter(|c| &c.source == source).map(|c| c.destination.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, name: &str) -> DepNode {
        let mut n = DepNode::new(id, name, "testNode");
        n.add_attr("input", AttrSpec::float(1.0));
        n.add_attr("output", AttrSpec::float(0.0));
        n
    }

    #[test]
    fn attrs_default_and_round_trip() {
        let mut g = DepGraph::default();
        g.insert(node("d1", "dep1"));
        let input = PlugRef::new("d1", "input");
        assert_eq!(g.get(&input), Some(Value::from(1.0)));
        assert!(g.set(&input, Value::from(4.0)));
        assert_eq!(g.get(&input), Some(Value::from(4.0)));
        let json = serde_json::to_string(&g).unwrap();
        let back: DepGraph = serde_json::from_str(&json).unwrap();
        assert_eq!(back, g);
    }

    #[test]
    fn connections_follow_and_reject_cycles() {
        let mut g = DepGraph::default();
        g.insert(node("a", "A"));
        g.insert(node("b", "B"));
        let in_a = PlugRef::new("a", "input");
        let out_a = PlugRef::new("a", "output");
        let in_b = PlugRef::new("b", "input");
        g.set(&in_a, Value::from(3.0));
        g.set(&out_a, Value::from(5.0));
        assert!(g.connect(out_a.clone(), in_a.clone()));
        assert_eq!(g.get(&in_a), Some(Value::from(5.0)));
        assert!(!g.connect(in_a.clone(), out_a), "cycle was accepted");
        assert!(!g.connect(in_a.clone(), in_a.clone()), "self-cycle was accepted");
        assert!(g.disconnect(&in_a));
        assert_eq!(g.get(&in_a), Some(Value::from(3.0)));
        assert!(g.connect(in_a, in_b.clone()));
        assert_eq!(g.get(&in_b), Some(Value::from(3.0)));
        assert!(g.disconnect(&in_b));
        assert_eq!(g.get(&in_b), Some(Value::from(1.0)));
    }
}
