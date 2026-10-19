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

    pub fn create(&mut self, data: NodeData) -> NodeId {
        let id = NodeId(self.nodes.len());
        self.nodes.push(Node {
            data,
            parent: None,
            children: Vec::new(),
        });
        id
    }

    pub fn append_child(&mut self, parent: NodeId, child: NodeId) {
        if parent.0 >= self.nodes.len() || child.0 >= self.nodes.len() || parent == child {
            return;
        }
        if let Some(old) = self.nodes[child.0].parent {
            self.nodes[old.0].children.retain(|id| *id != child);
        }
        self.nodes[child.0].parent = Some(parent);
        self.nodes[parent.0].children.push(child);
    }

    pub fn clear_children(&mut self, id: NodeId) {
        let children = std::mem::take(&mut self.nodes[id.0].children);
        for child in children {
            self.nodes[child.0].parent = None;
        }
    }

    pub fn set_text(&mut self, id: NodeId, text: String) {
        self.clear_children(id);
        self.push(NodeData::Text(text), id);
    }

    pub fn text_content(&self, id: NodeId) -> String {
        match &self.nodes[id.0].data {
            NodeData::Text(text) => text.clone(),
            NodeData::Element { .. } | NodeData::Document => self.nodes[id.0]
                .children
                .iter()
                .map(|child| self.text_content(*child))
                .collect(),
        }
    }

    pub fn set_attr(&mut self, id: NodeId, name: &str, value: String) {
        let NodeData::Element { attrs, .. } = &mut self.nodes[id.0].data else {
            return;
        };
        if let Some((_, slot)) = attrs.iter_mut().find(|(key, _)| key.eq_ignore_ascii_case(name)) {
            *slot = value;
        } else {
            attrs.push((name.to_string(), value));
        }
    }

    pub fn attr(&self, id: NodeId, name: &str) -> Option<&str> {
        let NodeData::Element { attrs, .. } = &self.nodes[id.0].data else {
            return None;
        };
        attrs.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str())
    }
}
