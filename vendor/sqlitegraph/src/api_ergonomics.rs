use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeId(pub i64);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Label(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PropertyKey(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PropertyValue(pub String);

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl NodeId {
    pub fn as_i64(self) -> i64 {
        self.0
    }
}

impl From<i64> for NodeId {
    fn from(value: i64) -> Self {
        NodeId(value)
    }
}
