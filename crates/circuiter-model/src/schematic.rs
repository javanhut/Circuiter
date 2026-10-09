//! Schematic view: instances of other cells connected by wires, net labels and the
//! cell's own ports. A schematic defines a cell's behaviour hierarchically.

use crate::cell::{CellRef, ViewKind};
use crate::geom::{Point, Transform};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Schematic {
    #[serde(default)]
    pub instances: Vec<Instance>,
    #[serde(default)]
    pub wires: Vec<Wire>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<NetLabel>,
    /// Where each of the cell's ports attaches inside this schematic.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<SchematicPort>,
}

impl Schematic {
    pub fn instance(&self, name: &str) -> Option<&Instance> {
        self.instances.iter().find(|i| i.name == name)
    }
}

/// A placed component.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Instance {
    /// Reference designator, unique within the schematic: `R1`, `M3`, `U12`.
    pub name: String,
    pub cell: CellRef,
    pub placement: Transform,
    /// Parameter overrides. Parameters not listed take the cell's defaults.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
    /// Forces a particular view for this instance (e.g. simulate one gate at transistor
    /// level while the rest use fast models). `None` uses the design's view order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<ViewKind>,
}

/// A wire drawn as a polyline. Connects to pins, ports, labels and other wires at its
/// vertices, and to other wires' end points that touch its segments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wire {
    pub points: Vec<Point>,
}

/// Names the net at a point. Labels with the same name in one schematic are connected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetLabel {
    pub net: String,
    pub at: Point,
}

/// The attachment point of one of the cell's ports inside its own schematic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchematicPort {
    pub port: String,
    pub at: Point,
}
