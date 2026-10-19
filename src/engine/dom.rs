#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeId(pub usize);

#[derive(Debug)]
pub enum NodeData {
    Document,
    Element {
        tag: String,
        #[allow(dead_code)]
        attrs: Vec<(String, String)>,
    },
    Text(String),
}

#[derive(Debug)]
pub struct Node {
    pub data: NodeData,
    /// 父节点。建树时写入，后续布局会用到。
    #[allow(dead_code)]
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
}

#[derive(Debug)]
pub struct Document {
    pub nodes: Vec<Node>,
}

impl Document {
    pub fn new() -> Self {
        Self {
            nodes: vec![Node {
                data: NodeData::Document,
                parent: None,
                children: Vec::new(),
            }],
        }
    }

    pub fn root(&self) -> NodeId {
        NodeId(0)
    }

    pub fn push(&mut self, data: NodeData, parent: NodeId) -> NodeId {
        let id = NodeId(self.nodes.len());
        self.nodes.push(Node {
            data,
            parent: Some(parent),
            children: Vec::new(),
        });
        self.nodes[parent.0].children.push(id);
        id
    }
}
