//! # Radix Trie Node Representation

use aether_http::method::Method;

pub type HandlerId = u32;

#[derive(Clone, Debug)]
pub enum NodeType {
    Static,
    Param(String),
    CatchAll(String),
}

#[derive(Clone, Debug)]
pub struct RouteNode {
    pub prefix: String,
    pub node_type: NodeType,
    pub allowed_methods: u64,
    pub handlers: Vec<(Method, HandlerId)>,
    pub children: Vec<RouteNode>,
}

impl RouteNode {
    pub fn new(prefix: String, node_type: NodeType) -> Self {
        Self {
            prefix,
            node_type,
            allowed_methods: 0,
            handlers: Vec::new(),
            children: Vec::new(),
        }
    }

    #[inline(always)]
    pub fn supports_method(&self, method: Method) -> bool {
        (self.allowed_methods & method.bitmask()) != 0
    }

    pub fn get_handler(&self, method: Method) -> Option<HandlerId> {
        if !self.supports_method(method) {
            return None;
        }
        self.handlers.iter().find(|(m, _)| *m == method).map(|(_, h)| *h)
    }

    pub fn add_handler(&mut self, method: Method, handler: HandlerId) {
        self.allowed_methods |= method.bitmask();
        self.handlers.push((method, handler));
    }
}

