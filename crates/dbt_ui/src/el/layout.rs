//! The pipeline canvas layout: spec → positioned nodes and edges.
//! Auto-layout stacks streams on the left, Cast & Map in the middle, the
//! Snowflake target on the right; positions in the spec's `canvas:` block
//! override per node.

use el_engine::spec::{Connection, Connections, Pipeline, SourceObject};
use gpui::SharedString;

pub const NODE_WIDTH: f32 = 190.;
pub const NODE_HEIGHT: f32 = 54.;
pub const COL_GAP: f32 = 110.;
pub const ROW_GAP: f32 = 26.;
pub const PADDING: f32 = 40.;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum NodeId {
    Stream(String),
    Cast,
    Target,
}

impl NodeId {
    /// The key used in the YAML `canvas.nodes` map.
    pub fn spec_key(&self) -> String {
        match self {
            NodeId::Stream(name) => format!("stream:{name}"),
            NodeId::Cast => "cast".to_owned(),
            NodeId::Target => "target".to_owned(),
        }
    }
}

#[derive(Clone)]
pub enum ElNodeKind {
    Stream { stream_ix: usize },
    Cast,
    Target,
}

#[derive(Clone)]
pub struct ElNode {
    pub id: NodeId,
    pub kind: ElNodeKind,
    pub label: SharedString,
    pub sublabel: SharedString,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone)]
pub struct ElEdge {
    pub from: usize,
    pub to: usize,
    /// The stream this edge belongs to (stream→cast edges); None for
    /// cast→target.
    pub stream_ix: Option<usize>,
}

#[derive(Clone, Default)]
pub struct ElLayout {
    pub nodes: Vec<ElNode>,
    pub edges: Vec<ElEdge>,
    pub width: f32,
    pub height: f32,
}

pub fn build_layout(pipeline: &Pipeline, connections: Option<&Connections>) -> ElLayout {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    let streams = pipeline.streams.len().max(1);
    let column_height = streams as f32 * (NODE_HEIGHT + ROW_GAP) - ROW_GAP;
    let mid_y = PADDING + column_height / 2. - NODE_HEIGHT / 2.;

    let source_kind = connections
        .and_then(|connections| connections.connections.get(&pipeline.source))
        .map(Connection::kind);

    for (stream_ix, stream) in pipeline.streams.iter().enumerate() {
        let id = NodeId::Stream(stream.name.clone());
        let (x, y) = position(pipeline, &id).unwrap_or((
            PADDING,
            PADDING + stream_ix as f32 * (NODE_HEIGHT + ROW_GAP),
        ));
        let sublabel: SharedString = match &stream.source {
            SourceObject::Table { schema, table } => match schema {
                Some(schema) => format!("{}: {schema}.{table}", source_kind.unwrap_or("db")).into(),
                None => format!("{}: {table}", source_kind.unwrap_or("db")).into(),
            },
            SourceObject::Path { path, .. } => {
                let name = path.rsplit('/').next().unwrap_or(path);
                format!("file: {name}").into()
            }
        };
        nodes.push(ElNode {
            id,
            kind: ElNodeKind::Stream { stream_ix },
            label: stream.name.clone().into(),
            sublabel,
            x,
            y,
            width: NODE_WIDTH,
            height: NODE_HEIGHT,
        });
    }

    let cast_ix = nodes.len();
    let (cast_x, cast_y) = position(pipeline, &NodeId::Cast)
        .unwrap_or((PADDING + NODE_WIDTH + COL_GAP, mid_y));
    let rules: usize = pipeline
        .streams
        .iter()
        .map(|stream| stream.columns.len())
        .sum();
    nodes.push(ElNode {
        id: NodeId::Cast,
        kind: ElNodeKind::Cast,
        label: "Cast & Map".into(),
        sublabel: format!(
            "{} stream{} · {} column rule{}",
            pipeline.streams.len(),
            if pipeline.streams.len() == 1 { "" } else { "s" },
            rules,
            if rules == 1 { "" } else { "s" },
        )
        .into(),
        x: cast_x,
        y: cast_y,
        width: NODE_WIDTH,
        height: NODE_HEIGHT,
    });

    let target_ix = nodes.len();
    let (target_x, target_y) = position(pipeline, &NodeId::Target)
        .unwrap_or((PADDING + 2. * (NODE_WIDTH + COL_GAP), mid_y));
    let target_db = pipeline.target.database.as_deref().unwrap_or("");
    let target_kind = connections
        .and_then(|connections| connections.connections.get(&pipeline.target.connection))
        .map(Connection::kind)
        .unwrap_or("warehouse");
    nodes.push(ElNode {
        id: NodeId::Target,
        kind: ElNodeKind::Target,
        label: pipeline.target.connection.clone().into(),
        sublabel: if target_db.is_empty() {
            format!("{target_kind}: {}", pipeline.target.schema).into()
        } else {
            format!("{target_kind}: {target_db}.{}", pipeline.target.schema).into()
        },
        x: target_x,
        y: target_y,
        width: NODE_WIDTH,
        height: NODE_HEIGHT,
    });

    for stream_ix in 0..pipeline.streams.len() {
        edges.push(ElEdge {
            from: stream_ix,
            to: cast_ix,
            stream_ix: Some(stream_ix),
        });
    }
    edges.push(ElEdge {
        from: cast_ix,
        to: target_ix,
        stream_ix: None,
    });

    let width = nodes
        .iter()
        .map(|node| node.x + node.width)
        .fold(0., f32::max)
        + PADDING;
    let height = nodes
        .iter()
        .map(|node| node.y + node.height)
        .fold(0., f32::max)
        + PADDING;
    ElLayout {
        nodes,
        edges,
        width,
        height,
    }
}

fn position(pipeline: &Pipeline, id: &NodeId) -> Option<(f32, f32)> {
    let meta = pipeline.canvas.as_ref()?;
    let pos = meta.nodes.get(&id.spec_key())?;
    Some((pos.x, pos.y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_layout_and_overrides() {
        let yaml = r#"version: 1
pipeline: t
source: src
target: { connection: wh, schema: RAW }
streams:
- name: a
  source: { schema: s, table: a }
- name: b
  source: { path: x/b.csv, format: csv }
canvas:
  nodes:
    stream:b: { x: 7.0, y: 9.0 }
"#;
        let pipeline: el_engine::spec::Pipeline = serde_yaml_ng::from_str(yaml).unwrap();
        let layout = build_layout(&pipeline, None);
        assert_eq!(layout.nodes.len(), 4); // 2 streams + cast + target
        assert_eq!(layout.edges.len(), 3);
        // Auto for "a", override for "b".
        assert_eq!(layout.nodes[0].x, PADDING);
        assert_eq!((layout.nodes[1].x, layout.nodes[1].y), (7., 9.));
        assert!(layout.width > 0. && layout.height > 0.);
        assert_eq!(layout.nodes[1].sublabel.as_ref(), "file: b.csv");
    }
}
