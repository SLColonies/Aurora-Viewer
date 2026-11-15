//! object_land_impact: an in-world object's land impact and weights, asked of
//! the simulator as the build tool's Land Impact and Object Weights do
//! (GetObjectCost for every link, ResourceCostSelected for the root).

use super::*;
use crate::build::edits::CapPurpose;
use aurora_llsd::{Llsd, llsd_map};

const COST_DEADLINE: Duration = Duration::from_secs(15);

pub(super) struct PendingCost {
    root: Uuid,
    name: String,
    links: Vec<(u32, Uuid)>,
    cost_tag: u64,
    weights_tag: u64,
    started: Instant,
    reply_tx: oneshot::Sender<Result<ViewerResponse, McpError>>,
}

impl App {
    pub(super) fn start_mcp_land_impact(&mut self, local_id: u32, reply_tx: oneshot::Sender<Result<ViewerResponse, McpError>>) {
        let (idx, root) = match self.resolve_prim_by_local_id(local_id) {
            Ok(v) => v,
            Err(e) => {
                let _ = reply_tx.send(Err(e));
                return;
            }
        };
        let Some(o) = self.world.objects.get(idx) else {
            let _ = reply_tx.send(Err(McpError::InvalidParams(format!("no object {local_id}"))));
            return;
        };
        let handle = o.key.region;
        let root_local = if o.parent_id == 0 { o.key.local_id } else { o.parent_id };
        let mut links: Vec<(u32, Uuid)> = self
            .world
            .objects
            .iter()
            .filter(|(_, p)| !p.is_avatar() && p.key.region == handle && (p.key.local_id == root_local || p.parent_id == root_local))
            .map(|(_, p)| (p.key.local_id, p.full_id))
            .collect();
        links.sort_by_key(|(id, _)| (*id != root_local, *id));
        let name = self
            .world
            .objects
            .index_of_uuid(&root)
            .and_then(|i| self.world.objects.get(i))
            .and_then(|r| self.build.props.get(&r.full_id))
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let ids = Llsd::Array(links.iter().map(|(_, u)| Llsd::from(*u)).collect());
        let cost_tag = self
            .build
            .cap(handle, "GetObjectCost", false, llsd_map! { "object_ids" => ids }, CapPurpose::Mcp);
        let weights_tag = self.build.cap(
            handle,
            "ResourceCostSelected",
            false,
            llsd_map! { "selected_roots" => Llsd::Array(vec![Llsd::from(root)]) },
            CapPurpose::Mcp,
        );
        self.flush_build();
        self.mcp_bridge.pending_costs.push_back(PendingCost {
            root,
            name,
            links,
            cost_tag,
            weights_tag,
            started: Instant::now(),
            reply_tx,
        });
    }

    pub(super) fn poll_mcp_land_impact(&mut self) {
        for p in std::mem::take(&mut self.mcp_bridge.pending_costs) {
            let late = p.started.elapsed() > COST_DEADLINE;
            let cost_in = self.build.mcp_replies.contains_key(&p.cost_tag);
            let weights_in = self.build.mcp_replies.contains_key(&p.weights_tag);
            if !(cost_in && (weights_in || late)) && !late {
                self.mcp_bridge.pending_costs.push_back(p);
                continue;
            }
            let cost = self.build.mcp_replies.remove(&p.cost_tag);
            let weights = self.build.mcp_replies.remove(&p.weights_tag);
            self.build.caps.remove(&p.cost_tag);
            self.build.caps.remove(&p.weights_tag);
            let Some(Ok(cost)) = cost else {
                let why = match cost {
                    Some(Err(e)) => e,
                    _ => "no answer within 15 s".into(),
                };
                let _ = p.reply_tx.send(Err(McpError::Internal(format!("GetObjectCost failed: {why}"))));
                continue;
            };
            let root = &cost[p.root.to_string().as_str()];
            let num = |v: &Llsd, k: &str| v.has(k).then(|| (v[k].as_f64() * 1000.0).round() / 1000.0);
            let links: Vec<Value> = p
                .links
                .iter()
                .map(|(local_id, id)| {
                    let c = &cost[id.to_string().as_str()];
                    json!({
                        "local_id": local_id,
                        "resource_cost": num(c, "resource_cost"),
                        "physics_cost": num(c, "physics_cost"),
                    })
                })
                .collect();
            let weights = match weights {
                Some(Ok(w)) if w["selected"].is_map() => {
                    let s = &w["selected"];
                    json!({
                        "download": num(s, "streaming"),
                        "physics": num(s, "physics"),
                        "server": num(s, "simulation"),
                    })
                }
                Some(Ok(_)) => json!(null),
                Some(Err(e)) => json!({ "error": e }),
                None => json!({ "error": "no answer within 15 s" }),
            };
            let _ = p.reply_tx.send(Ok(ViewerResponse::Json(json!({
                "ok": true,
                "root_id": p.root.to_string(),
                "name": p.name,
                "link_count": p.links.len(),
                "land_impact": num(root, "linked_set_resource_cost"),
                "linkset_physics_cost": num(root, "linked_set_physics_cost"),
                "resource_limiting_type": root["resource_limiting_type"].as_str(),
                "weights": weights,
                "links": links,
            }))));
        }
    }
}
