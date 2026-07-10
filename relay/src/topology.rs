//! Topology contribution merged into SPR's router topology view.
//!
//! The struct shapes and the root anchor node mirror the spr-tailscale /
//! spr-simplex contract: the SPR host attaches the plugin graph to the router
//! topology at the "root" node. `IP` and `ConnType` are omitted when empty
//! (Go's `json:",omitempty"`); `Name`/`Kind` always serialize.

use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TopoNode {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "Kind")]
    pub kind: String,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "IP", skip_serializing_if = "String::is_empty")]
    pub ip: String,
    #[serde(rename = "ConnType", skip_serializing_if = "String::is_empty")]
    pub conn_type: String,
    #[serde(rename = "Online")]
    pub online: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TopoEdge {
    #[serde(rename = "From")]
    pub from: String,
    #[serde(rename = "To")]
    pub to: String,
    #[serde(rename = "Layer")]
    pub layer: String,
    #[serde(rename = "Kind")]
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Topology {
    #[serde(rename = "Nodes")]
    pub nodes: Vec<TopoNode>,
    #[serde(rename = "Edges")]
    pub edges: Vec<TopoEdge>,
}

/// Build the topology: the root anchor plus one service node for the Nostr
/// relay. `online` reflects the live relay state; `ip` is the container IP on
/// the spr-nostr bridge (omitted when unknown). When the relay is down the
/// graph is still `{root, relay(offline)}` — the anchor is always online.
pub fn build_topology(running: bool, ip: &str) -> Topology {
    Topology {
        nodes: vec![
            // Root anchor carries ONLY ID/ConnType/Online (contract).
            TopoNode {
                id: "root".to_string(),
                kind: String::new(),
                name: String::new(),
                ip: String::new(),
                conn_type: "nostr".to_string(),
                online: true,
            },
            TopoNode {
                id: "nostr-relay".to_string(),
                kind: "service".to_string(),
                name: "Nostr relay".to_string(),
                ip: ip.to_string(),
                conn_type: "nostr".to_string(),
                online: running,
            },
        ],
        edges: vec![TopoEdge {
            from: "root".to_string(),
            to: "nostr-relay".to_string(),
            layer: "l1".to_string(),
            kind: "nostr".to_string(),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_anchor_shape() {
        let topo = build_topology(true, "172.18.0.5");
        let root = &topo.nodes[0];
        assert_eq!(root.id, "root");
        assert_eq!(root.conn_type, "nostr");
        assert!(root.online);
        // anchor must carry only ID/ConnType/Online
        assert!(root.kind.is_empty());
        assert!(root.name.is_empty());
        assert!(root.ip.is_empty());
    }

    #[test]
    fn service_node_reflects_live_state() {
        let topo = build_topology(true, "172.18.0.5");
        assert_eq!(topo.nodes.len(), 2);
        let svc = &topo.nodes[1];
        assert_eq!(svc.id, "nostr-relay");
        assert_eq!(svc.kind, "service");
        assert_eq!(svc.conn_type, "nostr");
        assert!(svc.online);
        assert_eq!(svc.ip, "172.18.0.5");
        assert_eq!(topo.edges.len(), 1);
        let e = &topo.edges[0];
        assert_eq!(e.from, "root");
        assert_eq!(e.to, "nostr-relay");
        assert_eq!(e.layer, "l1");
        assert_eq!(e.kind, "nostr");
    }

    #[test]
    fn daemon_down_omits_empty_ip_and_keeps_contract_keys() {
        let topo = build_topology(false, "");
        assert!(topo.nodes[0].online, "root anchor is always online");
        assert!(!topo.nodes[1].online, "service offline when relay is down");
        let data = serde_json::to_string(&topo).unwrap();
        assert!(!data.contains("\"IP\""), "empty IP must be omitted: {data}");
        assert!(data.contains("\"Nodes\""), "{data}");
        assert!(data.contains("\"Edges\""), "{data}");
        // ConnType on the root is non-empty and must be present
        assert!(data.contains("\"ConnType\":\"nostr\""), "{data}");
    }
}
