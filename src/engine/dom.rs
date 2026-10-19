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
    /// DOM 变更计数，供布局几何缓存失效。
    pub generation: u64,
}

impl Document {
    pub fn new() -> Self {
        Self {
            nodes: vec![Node {
                data: NodeData::Document,
                parent: None,
                children: Vec::new(),
            }],
            generation: 0,
        }
    }

    pub fn bump(&mut self) {
        self.generation = self.generation.wrapping_add(1);
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
        self.bump();
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
        if self.is_fragment(child) {
            let children = self.nodes[child.0].children.clone();
            for node in children {
                self.append_child(parent, node);
            }
            return;
        }
        if parent.0 >= self.nodes.len() || child.0 >= self.nodes.len() || parent == child {
            return;
        }
        if let Some(old) = self.nodes[child.0].parent {
            self.nodes[old.0].children.retain(|id| *id != child);
        }
        self.nodes[child.0].parent = Some(parent);
        self.nodes[parent.0].children.push(child);
        self.bump();
    }

    pub fn insert_before(&mut self, parent: NodeId, child: NodeId, before: Option<NodeId>) {
        if self.is_fragment(child) {
            let children = self.nodes[child.0].children.clone();
            for node in children {
                self.insert_before(parent, node, before);
            }
            return;
        }
        if parent.0 >= self.nodes.len() || child.0 >= self.nodes.len() || parent == child {
            return;
        }
        if let Some(old) = self.nodes[child.0].parent {
            self.nodes[old.0].children.retain(|id| *id != child);
        }
        self.nodes[child.0].parent = Some(parent);
        let inserted = {
            let children = &mut self.nodes[parent.0].children;
            if let Some(before) = before {
                if let Some(index) = children.iter().position(|id| *id == before) {
                    children.insert(index, child);
                    true
                } else {
                    children.push(child);
                    true
                }
            } else {
                children.push(child);
                true
            }
        };
        if inserted {
            self.bump();
        }
    }

    pub fn replace_child(&mut self, parent: NodeId, new_child: NodeId, old_child: NodeId) {
        self.insert_before(parent, new_child, Some(old_child));
        self.remove_child(parent, old_child);
    }

    pub fn is_fragment(&self, id: NodeId) -> bool {
        self.tag_name(id).is_some_and(|tag| tag == "#document-fragment")
    }

    pub fn remove_child(&mut self, parent: NodeId, child: NodeId) {
        if parent.0 >= self.nodes.len() || child.0 >= self.nodes.len() {
            return;
        }
        self.nodes[parent.0].children.retain(|id| *id != child);
        if self.nodes[child.0].parent == Some(parent) {
            self.nodes[child.0].parent = None;
        }
        self.bump();
    }

    pub fn remove(&mut self, id: NodeId) {
        if let Some(parent) = self.nodes.get(id.0).and_then(|node| node.parent) {
            self.remove_child(parent, id);
        }
    }

    pub fn clear_children(&mut self, id: NodeId) {
        let children = std::mem::take(&mut self.nodes[id.0].children);
        for child in children {
            self.nodes[child.0].parent = None;
        }
        self.bump();
    }

    pub fn clone_node(&mut self, id: NodeId, deep: bool) -> Option<NodeId> {
        if id.0 >= self.nodes.len() {
            return None;
        }
        let data = match &self.nodes[id.0].data {
            NodeData::Document => return None,
            NodeData::Text(text) => NodeData::Text(text.clone()),
            NodeData::Element { tag, attrs } => NodeData::Element {
                tag: tag.clone(),
                attrs: attrs.clone(),
            },
        };
        let created = self.create(data);
        if deep {
            let children = self.nodes[id.0].children.clone();
            for child in children {
                if let Some(cloned) = self.clone_node(child, true) {
                    self.append_child(created, cloned);
                }
            }
        }
        Some(created)
    }

    pub fn remove_attr(&mut self, id: NodeId, name: &str) {
        let NodeData::Element { attrs, .. } = &mut self.nodes[id.0].data else {
            return;
        };
        attrs.retain(|(key, _)| !key.eq_ignore_ascii_case(name));
        self.bump();
    }

    pub fn has_attr(&self, id: NodeId, name: &str) -> bool {
        self.attr(id, name).is_some()
    }

    pub fn tag_name(&self, id: NodeId) -> Option<&str> {
        match &self.nodes[id.0].data {
            NodeData::Element { tag, .. } => Some(tag.as_str()),
            _ => None,
        }
    }

    pub fn is_element(&self, id: NodeId) -> bool {
        matches!(self.nodes[id.0].data, NodeData::Element { .. })
    }

    pub fn element_children(&self, id: NodeId) -> Vec<NodeId> {
        self.nodes[id.0]
            .children
            .iter()
            .copied()
            .filter(|child| self.is_element(*child))
            .collect()
    }

    pub fn parent_of(&self, id: NodeId) -> Option<NodeId> {
        self.nodes.get(id.0).and_then(|node| node.parent)
    }

    pub fn is_connected(&self, id: NodeId) -> bool {
        let mut current = Some(id);
        while let Some(node) = current {
            if node == self.root() {
                return true;
            }
            current = self.parent_of(node);
        }
        false
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
        self.bump();
    }

    pub fn attr(&self, id: NodeId, name: &str) -> Option<&str> {
        let NodeData::Element { attrs, .. } = &self.nodes[id.0].data else {
            return None;
        };
        attrs.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str())
    }
}
