//! estate_access: the Region/Estate window's Access tab for an agent. `list`
//! reads the estate's managers, allowed residents and groups and banned
//! residents (the EstateAccess capability, asked when the window's panels
//! refresh); `ban` / `unban` send the same `estateaccessdelta` as the tab's
//! "Add…" / "Remove" and the resident menu's "Ban" (callbackEstateBan:
//! banned and taken off the allowed list), on this estate or on every estate
//! we own (APPLY_TO_ALL_ESTATES) or manage for its owner
//! (APPLY_TO_MANAGED_ESTATES). The estate's owner and its managers are never
//! banned (OwnerCanNotBeDenied, ProblemBanningEstateManager).

use super::*;
use crate::world::estate::{self, Role};
use aurora_net::estate::estate_access as ea;

impl App {
    fn mcp_estate_role(&self, handle: aurora_net::RegionHandle) -> (Role, Uuid) {
        let info = self.world.regions.get(&handle).and_then(|r| r.info.clone());
        let owner = self
            .world
            .estate
            .estate
            .as_ref()
            .map(|e| e.owner)
            .filter(|o| !o.is_nil())
            .or_else(|| info.as_ref().map(|i| i.owner))
            .unwrap_or_default();
        let manager = info.as_ref().is_some_and(|i| i.is_estate_manager);
        let role = if self.world.god_level > 0 {
            Role::Owner
        } else {
            estate::role(self.world.agent_id, owner, manager)
        };
        (role, owner)
    }

    pub(super) fn mcp_estate_access(&mut self, action: &str, agent_id: Option<Uuid>, all_estates: bool) -> Result<ViewerResponse, McpError> {
        let Some(handle) = self.world.main_region else {
            return Err(McpError::Internal("not in a region".into()));
        };
        // The lists come with the window's refresh; ask once for this region.
        if self.world.estate.open != Some(handle) {
            self.world.refresh_region_window(handle, true);
        }
        let (role, owner) = self.mcp_estate_role(handle);
        let role_name = match role {
            Role::Owner => "owner",
            Role::Manager => "manager",
            Role::Resident => "resident",
        };
        let estate_info = self.world.estate.estate.as_ref().map(|e| json!({ "name": e.name, "id": e.id, "owner_id": e.owner.to_string() }));
        let region = self.world.regions.get(&handle).and_then(|r| r.info.as_ref()).map(|i| i.name.clone());
        let access = self.world.estate.access.clone().flatten();
        match action {
            "list" => {
                let lists = access.as_ref().map(|a| {
                    json!({
                        "managers": a.managers.iter().map(Uuid::to_string).collect::<Vec<_>>(),
                        "allowed_agents": a.allowed_agents.iter().map(Uuid::to_string).collect::<Vec<_>>(),
                        "allowed_groups": a.allowed_groups.iter().map(Uuid::to_string).collect::<Vec<_>>(),
                        "banned": a.banned.iter().map(|b| json!({
                            "id": b.id.to_string(),
                            "banned_on": b.banned_on,
                            "banned_by": b.banned_by.map(|u| u.to_string()),
                            "last_login": b.last_login,
                        })).collect::<Vec<_>>(),
                    })
                });
                let banned_here = agent_id.map(|id| access.as_ref().map(|a| a.banned.iter().any(|b| b.id == id)));
                Ok(ViewerResponse::Json(json!({
                    "region": region,
                    "estate": estate_info,
                    "role": role_name,
                    "loaded": access.is_some(),
                    "hint": if access.is_none() { Some("the lists are on their way; ask again in a few seconds") } else { None },
                    "agent_banned": banned_here.flatten(),
                    "lists": lists,
                })))
            }
            "ban" | "unban" => {
                let id = agent_id.ok_or_else(|| McpError::InvalidParams("agent_id is required".into()))?;
                if role == Role::Resident {
                    return Err(McpError::InvalidParams("only the estate's owner or a manager can change its ban list".into()));
                }
                let ban = action == "ban";
                if ban && id == owner {
                    return Err(McpError::InvalidParams("the estate's owner cannot be banned".into()));
                }
                if ban && access.as_ref().is_some_and(|a| a.managers.contains(&id)) {
                    return Err(McpError::InvalidParams("an estate manager cannot be banned; remove them as manager first".into()));
                }
                let op = if ban { ea::BANNED_AGENT_ADD | ea::ALLOWED_AGENT_REMOVE } else { ea::BANNED_AGENT_REMOVE };
                let scope = match (all_estates, role) {
                    (false, _) => 0,
                    (true, Role::Owner) => ea::APPLY_TO_ALL_ESTATES,
                    (true, _) => ea::APPLY_TO_MANAGED_ESTATES,
                };
                self.world.estate.change_access(handle, op | scope, vec![id], false);
                log::info!("mcp estate_access: {action} {id} flags {:#x}", op | scope);
                Ok(ViewerResponse::Json(json!({
                    "sent": action,
                    "agent_id": id.to_string(),
                    "region": region,
                    "estate": estate_info,
                    "scope": if scope == ea::APPLY_TO_ALL_ESTATES { "all estates of this owner" } else if scope != 0 { "all estates managed for this owner" } else { "this estate" },
                    "verify": "call estate_access list with the same agent_id after a few seconds; agent_banned reads the simulator's list",
                })))
            }
            other => Err(McpError::InvalidParams(format!("unknown action {other:?}: list, ban or unban"))),
        }
    }
}
