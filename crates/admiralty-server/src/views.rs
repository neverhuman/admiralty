//! Shared, latest-value observation feed. No source scans per viewer and no command authority.
use crate::{ApiError, AppState, ServerConfig, authorized, error};
use admiralty_core::{SchedulingState, views::*};
use admiralty_store::{Snapshot, Store};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{
        IntoResponse, Response, Sse,
        sse::{Event, KeepAlive},
    },
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    convert::Infallible,
    sync::{Arc, Weak},
    time::Duration,
};
use tokio::sync::{OnceCell, watch};

const MAX_FRAME: usize = 8 * 1024 * 1024;
#[derive(Clone)]
struct Cut {
    snapshot: ViewsSnapshot,
    wire: String,
}

pub(crate) struct ViewsHub {
    current: watch::Sender<Option<Arc<Cut>>>,
    started: OnceCell<()>,
}
impl ViewsHub {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            current: watch::channel(None).0,
            started: OnceCell::new(),
        })
    }
    async fn start(self: &Arc<Self>, store: Store, config: ServerConfig) {
        self.started
            .get_or_init(|| async {
                update(self, &store, &config).await;
                let weak = Arc::downgrade(self);
                tokio::spawn(async move {
                    serve_updates(weak, store, config).await;
                });
            })
            .await;
    }
}
async fn serve_updates(hub: Weak<ViewsHub>, store: Store, config: ServerConfig) {
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let Some(hub) = hub.upgrade() else { break };
        update(&hub, &store, &config).await;
    }
}
fn s(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().filter(|c| !c.is_control()).take(240).collect())
}
fn field(value: &Value, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| s(&value[name]))
}
fn items(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}
fn array(value: &Value) -> Vec<Value> {
    items(value).cloned().collect()
}
fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

async fn update(hub: &ViewsHub, store: &Store, config: &ServerConfig) {
    let store = store.clone();
    let config = config.clone();
    let result = tokio::task::spawn_blocking(move || {
        let native = store
            .snapshot()
            .map_err(|_| "Native projection unavailable")?;
        if config.simulated
            && native
                .projection
                .tasks
                .values()
                .any(|t| !t.contract.simulated)
        {
            return Err("Demo scope changed");
        }
        let legacy = config.legacy.as_ref().map(|c| {
            crate::legacy::read_bounded(&c.status_file, MAX_FRAME as u64)
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                .filter(|v| v["projects"].is_object() && v["generated_utc"].is_string())
        });
        Ok((native, legacy, config.simulated))
    })
    .await;
    let Ok(Ok((native, legacy, simulated))) = result else {
        // In particular, never retain private state after a demo scope violation.
        hub.current.send_replace(None);
        return;
    };
    let previous = hub.current.borrow().clone();
    let producer_failed = matches!(legacy, Some(None));
    let source = legacy.flatten();
    let fingerprint = admiralty_core::content_digest(
        &json!({"cursor":native.cursor,"source":source,"failed":producer_failed}),
    )
    .unwrap_or_default();
    if previous
        .as_ref()
        .is_some_and(|p| p.snapshot.revision == fingerprint)
    {
        return;
    }
    let mut snapshot = project(&native, source.as_ref(), simulated);
    snapshot.revision = fingerprint;
    if producer_failed {
        snapshot.sources.retain(|s| s.id != "producer");
        snapshot.sources.push(ViewSource {
            id: "producer".into(),
            label: "Producer feed unavailable".into(),
            observed_utc: None,
            state: "error".into(),
            coverage: "Last successful observations retained. Check observer status source.".into(),
        });
        if let Some(previous) = &previous {
            for prior in &previous.snapshot.projects {
                if let Some(current) = snapshot.projects.iter_mut().find(|p| p.id == prior.id) {
                    current.git = current.git.take().or_else(|| prior.git.clone());
                    current.name = prior.name.clone();
                    current.task_count = current.task_count.or(prior.task_count);
                } else {
                    snapshot.projects.push(prior.clone());
                }
            }
            snapshot.projects.sort_by_key(|p| {
                previous
                    .snapshot
                    .projects
                    .iter()
                    .position(|old| old.id == p.id)
                    .unwrap_or(usize::MAX)
            });
            for agent in &previous.snapshot.agents {
                if agent.role != ViewRole::Assignment {
                    retain_agent(&mut snapshot, agent);
                }
            }
        }
    } else if let (Some(source), Some(previous)) = (&source, &previous) {
        let failed = items(&source["global_resources"]["hosts"])
            .filter(|h| h["reachable"] == false || h["screen_inventory_available"] == false)
            .filter_map(|h| s(&h["host"]))
            .collect::<BTreeSet<_>>();
        for agent in &previous.snapshot.agents {
            if agent.node.as_ref().is_some_and(|n| failed.contains(n)) {
                retain_agent(&mut snapshot, agent);
            }
        }
    }
    if simulated {
        seed_demo(&mut snapshot);
    }
    let Ok(mut wire) = serde_json::to_string(&snapshot) else {
        return;
    };
    if wire.len() > MAX_FRAME {
        // Windowed Git history may shrink; agent membership may never be silently capped.
        for project in &mut snapshot.projects {
            if let Some(git) = &mut project.git
                && git.commits.len() > 64
            {
                git.commits.truncate(64);
                git.truncated = true;
            }
        }
        wire = serde_json::to_string(&snapshot).unwrap_or_default();
        if wire.len() > MAX_FRAME {
            let revision = snapshot.revision.clone();
            snapshot = previous.map_or(
                ViewsSnapshot {
                    schema_version: 1,
                    revision: revision.clone(),
                    simulated,
                    observed_utc: now(),
                    projects: vec![],
                    agents: vec![],
                    sources: vec![],
                },
                |p| p.snapshot.clone(),
            );
            snapshot.revision = revision;
            snapshot.sources.retain(|s| s.id != "capacity");
            snapshot.sources.push(ViewSource { id: "capacity".into(), label: "Inventory exceeds stream capacity".into(), observed_utc: None,
                state: "error".into(), coverage: "No new complete agent inventory was delivered. Last successful observations retained; narrow the configured source scope.".into() });
            wire = serde_json::to_string(&snapshot).unwrap_or_default();
        }
    }
    if !wire.is_empty() && wire.len() <= MAX_FRAME {
        hub.current
            .send_replace(Some(Arc::new(Cut { snapshot, wire })));
    }
}

fn retain_agent(snapshot: &mut ViewsSnapshot, agent: &ViewAgent) {
    let replacements = snapshot
        .agents
        .iter()
        .filter(|a| {
            a.id != agent.id
                && a.role == agent.role
                && a.project_id == agent.project_id
                && a.presence == ViewPresence::Unobserved
                && matches!(
                    a.role,
                    ViewRole::Owner | ViewRole::Watcher | ViewRole::Admiral | ViewRole::Commodore
                )
        })
        .map(|a| a.id.clone())
        .collect::<BTreeSet<_>>();
    snapshot
        .agents
        .retain(|a| a.id != agent.id && !replacements.contains(&a.id));
    for a in &mut snapshot.agents {
        if a.parent_id
            .as_ref()
            .is_some_and(|p| replacements.contains(p))
        {
            a.parent_id = Some(agent.id.clone());
        }
    }
    snapshot.agents.push(agent.clone());
}
enum Binding {
    Found(String),
    Missing,
    Ambiguous,
}
fn full_identity(node: &str, raw: &Value) -> Option<String> {
    let full = ["id", "screen"]
        .iter()
        .filter_map(|key| s(&raw[*key]))
        .find(|id| {
            id.starts_with("screen:")
                || id.split_once('.').is_some_and(|(pid, _)| {
                    !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit())
                })
        });
    full.and_then(|full| {
        if full.starts_with("screen:") {
            full.starts_with(&format!("screen:{node}:")).then_some(full)
        } else {
            Some(format!("screen:{node}:{full}"))
        }
    })
}
fn binding(agents: &BTreeMap<String, ViewAgent>, node: &str, raw: &Value) -> Binding {
    if let Some(canonical) = full_identity(node, raw) {
        return agents
            .get(&canonical)
            .filter(|a| a.node.as_deref() == Some(node))
            .map_or(Binding::Missing, |a| Binding::Found(a.id.clone()));
    }
    let Some(name) = field(raw, &["screen", "name", "id"]) else {
        return Binding::Missing;
    };
    let mut matches = agents
        .values()
        .filter(|a| a.node.as_deref() == Some(node) && a.name == name);
    let Some(first) = matches.next() else {
        return Binding::Missing;
    };
    if matches.next().is_some() {
        Binding::Ambiguous
    } else {
        Binding::Found(first.id.clone())
    }
}

fn project(native: &Snapshot, source: Option<&Value>, simulated: bool) -> ViewsSnapshot {
    let empty = json!({});
    let source = source.unwrap_or(&empty);
    let observed = s(&source["generated_utc"]);
    let host = s(&source["host"]).unwrap_or_else(|| "unknown-node".into());
    let mut projects = BTreeMap::<String, ViewProject>::new();
    let mut order = items(&source["order"]).filter_map(s).collect::<Vec<_>>();
    for p in items(&source["project_catalog"]) {
        if let Some(id) = s(&p["id"]) {
            projects.insert(
                id.clone(),
                ViewProject {
                    name: s(&p["name"]).unwrap_or_else(|| id.clone()),
                    id,
                    git: None,
                    task_count: None,
                },
            );
        }
    }
    if let Some(values) = source["projects"].as_object() {
        for (id, p) in values {
            projects.entry(id.clone()).or_insert(ViewProject {
                id: id.clone(),
                name: s(&p["name"]).unwrap_or_else(|| id.clone()),
                git: None,
                task_count: p["total"].as_u64().and_then(|n| u32::try_from(n).ok()),
            });
        }
    }
    let rollout = &source["rollout"];
    if let Some(id) = s(&rollout["project"]["id"]) {
        projects.entry(id.clone()).or_insert(ViewProject {
            id: id.clone(),
            name: s(&rollout["project"]["name"]).unwrap_or(id),
            git: None,
            task_count: rollout["total"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok()),
        });
    }
    if let Some(repositories) = source["git"].as_object() {
        for (id, git) in repositories {
            if let Ok(git) = serde_json::from_value::<GitRepository>(git.clone()) {
                projects
                    .entry(id.clone())
                    .or_insert(ViewProject {
                        id: id.clone(),
                        name: id.clone(),
                        git: None,
                        task_count: None,
                    })
                    .git = Some(git);
            }
        }
    }
    let mut agents = BTreeMap::new();
    let mut insert =
        |raw: &Value, project_id: Option<String>, role: ViewRole, known_running: bool| {
            let node = field(raw, &["host", "node", "node_id"]).unwrap_or_else(|| host.clone());
            let Some(identity) = field(raw, &["id", "screen", "name"]) else {
                return;
            };
            let name = field(raw, &["screen", "name", "id"]).unwrap_or_else(|| identity.clone());
            let full = if identity
                .split_once('.')
                .is_some_and(|(pid, _)| !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()))
            {
                identity
            } else {
                name.clone()
            };
            let id = format!("screen:{node}:{full}");
            let state = field(raw, &["state", "status"]).unwrap_or_else(|| {
                if raw["agent_running"] == true {
                    "active".into()
                } else if raw["alive"] == true {
                    "screen alive; agent unknown".into()
                } else {
                    "unknown".into()
                }
            });
            let ended = matches!(
                state.as_str(),
                "finished" | "exited" | "gone" | "dead" | "cancelled"
            );
            let model = s(&raw["model"])
                .filter(|m| !matches!(m.to_lowercase().as_str(), "unknown" | "—" | "not observed"));
            agents.insert(
                id.clone(),
                ViewAgent {
                    id,
                    name,
                    role,
                    project_id,
                    parent_id: None,
                    node: Some(node),
                    presence: if ended {
                        ViewPresence::Ended
                    } else if known_running || raw["agent_running"] == true {
                        ViewPresence::Observed
                    } else {
                        ViewPresence::Unobserved
                    },
                    state,
                    model,
                    requested_model: field(raw, &["requested_model", "configured_model"]),
                    model_observed_utc: s(&raw["model_observed_utc"]),
                    model_source: s(&raw["model_source"]).unwrap_or_else(|| {
                        "producer report; provider confirmation unavailable".into()
                    }),
                    effort: s(&raw["effort"]),
                    task: field(raw, &["task", "packet_id"]),
                    branch: s(&raw["branch"]),
                    observed_utc: field(raw, &["observed_utc", "source_updated_utc"])
                        .or_else(|| observed.clone()),
                    source: "Producer observation".into(),
                },
            );
        };
    for raw in items(&source["agents"]) {
        insert(
            raw,
            s(&raw["project"]).filter(|s| s != "unassigned"),
            ViewRole::Worker,
            true,
        );
    }
    for raw in items(&source["remote_screens"]) {
        insert(
            raw,
            s(&raw["project"]).filter(|s| s != "unassigned"),
            ViewRole::Worker,
            false,
        );
    }
    let mut captains = array(&source["hierarchy"]["captains"]);
    for p in projects.values() {
        if !captains
            .iter()
            .any(|c| field(c, &["key", "project"]).as_deref() == Some(&p.id))
        {
            let mut captain = source["projects"][&p.id]["captain"].clone();
            if !captain.is_object() {
                captain = json!({});
            }
            captain["key"] = json!(p.id);
            captain["workers"] = source["projects"][&p.id]["workers"].clone();
            captains.push(captain);
        }
    }
    // Explicit lane mappings join observed screens by node/name; assignments do not add runs.
    for lane in items(&rollout["lanes"]) {
        let node = field(lane, &["node", "host"]).unwrap_or_else(|| host.clone());
        if let Binding::Found(id) = binding(&agents, &node, lane)
            && let Some(a) = agents.get_mut(&id)
        {
            a.project_id = s(&rollout["project"]["id"]);
            a.task = field(lane, &["packet_id", "task"]);
            a.branch = s(&lane["branch"]);
        }
    }
    let configure = |agents: &mut BTreeMap<String, ViewAgent>,
                     raw: &Value,
                     role: ViewRole,
                     project: Option<String>,
                     fallback: &str|
     -> String {
        let node = field(raw, &["host", "node"]).unwrap_or_else(|| host.clone());
        let screen = field(raw, &["screen", "id", "name"]);
        let joined = if matches!(role, ViewRole::Owner | ViewRole::Watcher)
            && field(raw, &["id", "screen"]).is_none()
        {
            Binding::Missing
        } else {
            binding(agents, &node, raw)
        };
        let ambiguous = matches!(joined, Binding::Ambiguous);
        let fallback_id = format!("role:{fallback}:{}", project.as_deref().unwrap_or("fleet"));
        let id = match joined {
            Binding::Found(id) => id,
            Binding::Missing if raw["agent_running"] == true => {
                full_identity(&node, raw).unwrap_or(fallback_id)
            }
            _ => fallback_id,
        };
        if let Some(agent) = agents.get_mut(&id) {
            agent.role = role;
            agent.project_id = project;
            if raw["agent_running"] == true && agent.model.is_none() {
                agent.model = s(&raw["model"]);
                agent.model_observed_utc = s(&raw["model_observed_utc"]);
                if agent.model.is_some() {
                    agent.model_source =
                        s(&raw["model_source"]).unwrap_or_else(|| "Producer model report".into());
                }
            }
        } else {
            let running = raw["agent_running"] == true && !ambiguous;
            agents.insert(
                id.clone(),
                ViewAgent {
                    id: id.clone(),
                    name: screen.unwrap_or_else(|| fallback.into()),
                    role,
                    project_id: project,
                    parent_id: None,
                    node: Some(node),
                    presence: if running {
                        ViewPresence::Observed
                    } else {
                        ViewPresence::Unobserved
                    },
                    state: if running {
                        "active".into()
                    } else {
                        "presence unknown".into()
                    },
                    model: s(&raw["model"]),
                    requested_model: s(&raw["requested_model"]),
                    model_observed_utc: s(&raw["model_observed_utc"]),
                    model_source: s(&raw["model_source"]).unwrap_or_else(|| {
                        "Configured role; model report may be unavailable".into()
                    }),
                    effort: s(&raw["effort"]),
                    task: None,
                    branch: None,
                    observed_utc: observed.clone(),
                    source: "Configured hierarchy / producer observation".into(),
                },
            );
        }
        id
    };
    let owner_name = field(&source["hierarchy"]["owner"], &["name"])
        .or_else(|| s(&source["hierarchy"]["owner"]))
        .unwrap_or_else(|| "Owner".into());
    let owner = configure(
        &mut agents,
        &json!({"name":owner_name}),
        ViewRole::Owner,
        None,
        "owner",
    );
    let mut watcher_raw = source["hierarchy"]["watcher"].as_object().map_or_else(
        || source["hierarchy"]["grokbot"].clone(),
        |v| Value::Object(v.clone()),
    );
    if !watcher_raw.is_object() {
        watcher_raw = json!({"name":"Grokbot"});
    }
    if field(&watcher_raw, &["name", "screen", "id"]).is_none() {
        watcher_raw["name"] = json!("Grokbot");
    }
    let watcher = configure(
        &mut agents,
        &watcher_raw,
        ViewRole::Watcher,
        None,
        "watcher",
    );
    let admiral_raw = if source["hierarchy"]["super_captain"].is_object() {
        &source["hierarchy"]["super_captain"]
    } else {
        &source["super_captain"]["screen"]
    };
    let admiral = configure(&mut agents, admiral_raw, ViewRole::Admiral, None, "admiral");
    let mut managers = BTreeMap::new();
    for captain in &captains {
        let Some(project_id) = field(captain, &["key", "project"]) else {
            continue;
        };
        let id = configure(
            &mut agents,
            captain,
            ViewRole::Commodore,
            Some(project_id.clone()),
            "commodore",
        );
        managers.insert(project_id.clone(), id);
        for raw in items(&captain["workers"]) {
            let node = field(raw, &["host", "node"]).unwrap_or_else(|| host.clone());
            let name = field(raw, &["screen", "name", "id"]);
            let existing = match binding(&agents, &node, raw) {
                Binding::Found(id) => Some(id),
                Binding::Missing => None,
                Binding::Ambiguous => continue,
            };
            if let Some(id) = existing {
                if let Some(a) = agents.get_mut(&id) {
                    a.project_id = Some(project_id.clone());
                    a.branch = a.branch.clone().or_else(|| s(&raw["branch"]));
                    a.task = a
                        .task
                        .clone()
                        .or_else(|| field(raw, &["task", "packet_id"]));
                }
            } else if let Some(name) = name {
                let identity = field(raw, &["id", "screen"]).unwrap_or_else(|| name.clone());
                let running = raw["agent_running"] == true;
                let id = format!("screen:{node}:{identity}");
                agents.insert(
                    id.clone(),
                    ViewAgent {
                        id,
                        name,
                        role: ViewRole::Worker,
                        project_id: Some(project_id.clone()),
                        parent_id: None,
                        node: Some(node),
                        presence: if running {
                            ViewPresence::Observed
                        } else {
                            ViewPresence::Unobserved
                        },
                        state: field(raw, &["state", "status"]).unwrap_or_else(|| "unknown".into()),
                        model: s(&raw["model"]),
                        requested_model: s(&raw["requested_model"]),
                        model_observed_utc: s(&raw["model_observed_utc"]),
                        model_source: s(&raw["model_source"])
                            .unwrap_or_else(|| "Producer report".into()),
                        effort: s(&raw["effort"]),
                        task: field(raw, &["task", "packet_id"]),
                        branch: s(&raw["branch"]),
                        observed_utc: field(raw, &["observed_utc", "source_updated_utc"])
                            .or_else(|| observed.clone()),
                        source: "Producer worker inventory".into(),
                    },
                );
            }
        }
    }
    for task in native.projection.tasks.values() {
        let project_id = task.contract.project_id.to_string();
        projects.entry(project_id.clone()).or_insert(ViewProject {
            id: project_id.clone(),
            name: project_id.clone(),
            git: None,
            task_count: None,
        });
        if let Some(principal) = &task.owner
            && !matches!(
                task.state,
                SchedulingState::Accepted | SchedulingState::Failed | SchedulingState::Cancelled
            )
        {
            let id = format!("assignment:{}", task.contract.task_id);
            agents.insert(
                id.clone(),
                ViewAgent {
                    id,
                    name: principal.to_string(),
                    role: ViewRole::Assignment,
                    project_id: Some(project_id),
                    parent_id: None,
                    node: None,
                    presence: ViewPresence::Unobserved,
                    state: format!("{:?}", task.state).to_lowercase(),
                    model: None,
                    requested_model: None,
                    model_source: "Not exposed by task projection".into(),
                    model_observed_utc: None,
                    effort: None,
                    task: Some(task.contract.task_id.to_string()),
                    branch: None,
                    observed_utc: None,
                    source: "Native task assignment; current process presence unobserved".into(),
                },
            );
        }
    }
    for a in agents.values() {
        if let Some(id) = &a.project_id {
            projects.entry(id.clone()).or_insert(ViewProject {
                id: id.clone(),
                name: id.clone(),
                git: None,
                task_count: None,
            });
        }
    }
    for p in projects.values() {
        if !managers.contains_key(&p.id) {
            let id = configure(
                &mut agents,
                &json!({}),
                ViewRole::Commodore,
                Some(p.id.clone()),
                "commodore",
            );
            managers.insert(p.id.clone(), id);
        }
    }
    for a in agents.values_mut() {
        a.parent_id = match a.role {
            ViewRole::Owner => None,
            ViewRole::Watcher => Some(owner.clone()),
            ViewRole::Admiral => Some(owner.clone()),
            ViewRole::Commodore => Some(admiral.clone()),
            _ => a
                .project_id
                .as_ref()
                .and_then(|p| managers.get(p))
                .cloned()
                .or_else(|| Some(admiral.clone())),
        };
    }
    let _ = watcher; // Watcher is an advisory peer, not invented command authority over Admiral.
    let mut sources=vec![ViewSource{id:"native".into(),label:"Native Store".into(),observed_utc:None,state:"current".into(),coverage:"Task assignments only; native provider/session inventory is not exposed by this projection.".into()}];
    if source.is_object() && source["generated_utc"].is_string() {
        sources.push(ViewSource{id:"producer".into(),label:"Producer fleet".into(),observed_utc:observed.clone(),state:"observed".into(),coverage:"Configured screen inventories. Provider child sessions and other unconfigured hosts are not enumerated.".into()});
        for node in items(&source["global_resources"]["hosts"]) {
            if let Some(id) = s(&node["host"]) {
                sources.push(ViewSource {
                    id: format!("node:{id}"),
                    label: id,
                    observed_utc: observed.clone(),
                    state: if node["reachable"] == false
                        || node["screen_inventory_available"] == false
                    {
                        "unavailable"
                    } else {
                        "observed"
                    }
                    .into(),
                    coverage: if node["screen_inventory_available"] == true {
                        "Screen inventory available; provider children may be opaque"
                    } else {
                        "Screen inventory incomplete or unavailable"
                    }
                    .into(),
                });
            }
        }
    } else {
        sources.push(ViewSource{id:"producer".into(),label:"Producer fleet".into(),observed_utc:None,state:"not_configured".into(),coverage:"Configure the Rust observer and --legacy-status-file for real fleet and Git observations.".into()});
    }
    order.extend(
        projects
            .keys()
            .filter(|id| !order.contains(id))
            .cloned()
            .collect::<Vec<_>>(),
    );
    ViewsSnapshot {
        schema_version: 1,
        revision: String::new(),
        simulated,
        observed_utc: observed.unwrap_or_else(now),
        projects: order.iter().filter_map(|id| projects.remove(id)).collect(),
        agents: agents.into_values().collect(),
        sources,
    }
}

fn seed_demo(snapshot: &mut ViewsSnapshot) {
    let stamp = snapshot.observed_utc.clone();
    snapshot.sources = vec![ViewSource {
        id: "demo".into(),
        label: "Synthetic fleet".into(),
        observed_utc: None,
        state: "simulated".into(),
        coverage:
            "Isolated demonstration. All agents, Git objects and relationships are synthetic."
                .into(),
    }];
    let models = [
        "gpt-6-luna",
        "gpt-6-sol",
        "claude-opus",
        "gemini-pro",
        "local/llama",
        "gpt-6-astra",
    ];
    for (index, p) in snapshot.projects.iter_mut().enumerate() {
        let oid = |n: usize| format!("{:040x}", (index + 1) * 1000 + n);
        p.git = Some(GitRepository {
            instance_id: format!("demo:{}", p.id),
            node: "demo-node".into(),
            observed_utc: stamp.clone(),
            error: None,
            shallow: false,
            truncated: false,
            commits: (1..=12)
                .map(|n| GitCommit {
                    oid: oid(n),
                    parents: match n {
                        1 => vec![],
                        5 => vec![oid(2)],
                        8 => vec![oid(7), oid(5)],
                        _ => vec![oid(n - 1)],
                    },
                    subject: [
                        "Initialize project",
                        "Define contracts",
                        "Build shared store",
                        "Add integration checks",
                        "Worker experiment",
                        "Refine interaction",
                        "Verify evidence",
                        "Merge worker branch",
                        "Polish controls",
                        "Preserve source facts",
                        "Review candidate",
                        "Ship read-only view",
                    ][n - 1]
                        .into(),
                    committed_utc: stamp.clone(),
                })
                .collect(),
            refs: vec![
                GitRef {
                    name: "refs/heads/main".into(),
                    oid: oid(12),
                    commit_oid: Some(oid(12)),
                    kind: "branch".into(),
                },
                GitRef {
                    name: "refs/heads/worker/ui".into(),
                    oid: oid(5),
                    commit_oid: Some(oid(5)),
                    kind: "branch".into(),
                },
            ],
            checkouts: vec![GitCheckout {
                id: format!("demo-checkout:{}", p.id),
                name: format!("{}-worker", p.id),
                kind: "linked".into(),
                head: Some(oid(5)),
                branch: Some("worker/ui".into()),
                detached: false,
                locked: false,
                prunable: false,
                status: Some(GitWorkingState {
                    staged: 1,
                    unstaged: 2,
                    untracked: 0,
                    conflicts: 0,
                }),
            }],
        });
        let manager = snapshot
            .agents
            .iter()
            .find(|a| a.project_id.as_ref() == Some(&p.id) && a.role == ViewRole::Commodore)
            .map(|a| a.id.clone());
        for n in 0..8 {
            snapshot.agents.push(ViewAgent {
                id: format!("demo:{}:{n}", p.id),
                name: format!("{}-worker-{:02}", p.id, n + 1),
                role: ViewRole::Worker,
                project_id: Some(p.id.clone()),
                parent_id: manager.clone(),
                node: Some(format!("demo-node-{}", n % 3 + 1)),
                presence: ViewPresence::Observed,
                state: [
                    "active",
                    "active",
                    "idle",
                    "waiting for input",
                    "blocked",
                    "active",
                    "active",
                    "quiet",
                ][n]
                    .into(),
                model: Some(models[(index + n) % models.len()].into()),
                requested_model: None,
                model_source: "Synthetic fixture".into(),
                model_observed_utc: None,
                effort: Some("high".into()),
                task: Some(format!("{}-task-{}", p.id, n + 1)),
                branch: (n == 0).then(|| "worker/ui".into()),
                observed_utc: None,
                source: "Synthetic demo".into(),
            });
        }
    }
    for a in &mut snapshot.agents {
        if matches!(a.role, ViewRole::Admiral | ViewRole::Commodore) {
            a.presence = ViewPresence::Observed;
            a.state = "active".into();
            a.model = Some("gpt-6-sol".into());
            a.source = "Synthetic demo".into();
        }
    }
}

pub(crate) async fn snapshot(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    authorized(&state, &headers)?;
    let _permit = state
        .reads
        .try_acquire()
        .map_err(|_| error(StatusCode::TOO_MANY_REQUESTS, "Visual read limit reached"))?;
    state
        .views
        .start(state.store.clone(), state.config.clone())
        .await;
    let cut = state.views.current.borrow().clone().ok_or_else(|| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Visual projection unavailable",
        )
    })?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(cut.snapshot.clone()),
    )
        .into_response())
}
pub(crate) async fn events(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    authorized(&state, &headers)?;
    let permit = state
        .streams
        .clone()
        .try_acquire_owned()
        .map_err(|_| error(StatusCode::TOO_MANY_REQUESTS, "Visual stream limit reached"))?;
    state
        .views
        .start(state.store.clone(), state.config.clone())
        .await;
    let receiver = state.views.current.subscribe();
    let stream = futures_util::stream::unfold(
        (receiver, true, permit),
        |(mut rx, first, permit)| async move {
            if !first && rx.changed().await.is_err() {
                return None;
            }
            let cut = rx.borrow_and_update().clone();
            let frame = match cut {
                Some(cut) => Event::default()
                    .event("views")
                    .id(&cut.snapshot.revision)
                    .data(&cut.wire),
                None => Event::default()
                    .event("unavailable")
                    .data("Visual projection unavailable"),
            };
            Some((Ok::<_, Infallible>(frame), (rx, false, permit)))
        },
    );
    Ok((
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ambiguous_short_screen_names_do_not_bind_lanes_or_managers() {
        let dir = tempfile::tempdir().expect("fixture");
        let store = Store::open(dir.path().join("db")).expect("store");
        let mut source = json!({"generated_utc":"2026-10-18T12:00:00Z","host":"n","projects":{"p":{}},
            "agents":[{"id":"1.same","screen":"same","host":"n","project":"unassigned","model":"model","model_source":"cached footer","model_observed_utc":"2026-10-18T10:00:00Z"},{"id":"2.same","screen":"same","host":"n","project":"unassigned"}],
            "hierarchy":{"captains":[{"key":"p","screen":"same","host":"n","agent_running":true,"workers":[{"screen":"same","host":"n"}]}]},
            "rollout":{"project":{"id":"p"},"lanes":[{"id":"lane","screen":"same","node":"n","branch":"branch","packet_id":"task"}]}});
        let observed = project(&store.snapshot().expect("snapshot"), Some(&source), false);
        let workers = observed
            .agents
            .iter()
            .filter(|a| a.role == ViewRole::Worker)
            .collect::<Vec<_>>();
        assert_eq!(workers.len(), 2);
        assert!(
            workers
                .iter()
                .all(|a| a.project_id.is_none() && a.task.is_none())
        );
        assert_eq!(
            workers
                .iter()
                .find(|a| a.id == "screen:n:1.same")
                .expect("worker")
                .model_observed_utc
                .as_deref(),
            Some("2026-10-18T10:00:00Z")
        );
        source["rollout"]["lanes"][0]["screen"] = json!("1.same");
        source["hierarchy"]["captains"][0]["screen"] = json!("2.same");
        let exact = project(&store.snapshot().expect("snapshot"), Some(&source), false);
        assert!(
            exact
                .agents
                .iter()
                .any(|a| a.id == "screen:n:1.same" && a.task.as_deref() == Some("task"))
        );
        assert!(
            exact
                .agents
                .iter()
                .any(|a| a.id == "screen:n:2.same" && a.role == ViewRole::Commodore)
        );
    }
    #[test]
    fn unknown_project_and_owner_named_worker_remain_discoverable() {
        let dir = tempfile::tempdir().expect("fixture");
        let store = Store::open(dir.path().join("store.sqlite")).expect("store");
        let source = json!({"generated_utc":"2026-10-18T12:00:00Z","host":"node","projects":{},"hierarchy":{"owner":"Owner"},
            "agents":[{"id":"12.Owner","screen":"Owner","host":"node","project":"orphan"}]});
        let result = project(&store.snapshot().expect("snapshot"), Some(&source), false);
        assert!(result.projects.iter().any(|p| p.id == "orphan"));
        assert!(result.agents.iter().any(|a| a.name == "Owner"
            && a.role == ViewRole::Worker
            && a.presence == ViewPresence::Observed));
        assert!(
            result
                .agents
                .iter()
                .any(|a| a.role == ViewRole::Owner && a.presence == ViewPresence::Unobserved)
        );
    }
    #[test]
    fn explicit_watcher_observation_retains_identity_and_reported_model() {
        let dir = tempfile::tempdir().expect("fixture");
        let store = Store::open(dir.path().join("store.sqlite")).expect("store");
        let source = json!({"generated_utc":"2026-10-18T12:00:00Z","host":"node","projects":{},
            "hierarchy":{"grokbot":{"id":"22.grokbot","host":"node","model":"grok-4","model_source":"heartbeat","agent_running":true}},
            "agents":[{"id":"22.grokbot","screen":"grokbot","host":"node"}]});
        let result = project(&store.snapshot().expect("snapshot"), Some(&source), false);
        let watcher = result
            .agents
            .iter()
            .find(|a| a.role == ViewRole::Watcher)
            .expect("watcher");
        assert_eq!(watcher.id, "screen:node:22.grokbot");
        assert_eq!(watcher.presence, ViewPresence::Observed);
        assert_eq!(watcher.model.as_deref(), Some("grok-4"));
        assert_eq!(
            result.agents.iter().filter(|a| a.id == watcher.id).count(),
            1
        );
    }
    #[test]
    fn projection_retains_taskless_projects_node_identity_and_honest_model_presence() {
        let dir = tempfile::tempdir().expect("fixture");
        let store = Store::open(dir.path().join("store.sqlite")).expect("store");
        let source = json!({"generated_utc":"2026-10-18T12:00:00Z","host":"a","projects":{},"project_catalog":[{"id":"empty","name":"Empty"}],"agents":[{"id":"1.same","screen":"same","host":"a","project":"empty","model":null},{"id":"1.same","screen":"same","host":"b","project":"unassigned","model":"model-b"}],"hierarchy":{"captains":[{"key":"empty","screen":"manager","workers":[{"screen":"same","host":"a"}]}]}});
        let result = project(&store.snapshot().expect("snapshot"), Some(&source), false);
        assert_eq!(result.projects.len(), 1);
        let workers = result
            .agents
            .iter()
            .filter(|a| a.role == ViewRole::Worker)
            .collect::<Vec<_>>();
        assert_eq!(workers.len(), 2);
        assert!(workers.iter().any(|a| a.node.as_deref() == Some("a")
            && a.model.is_none()
            && a.presence == ViewPresence::Observed));
        assert_eq!(
            result
                .agents
                .iter()
                .find(|a| a.role == ViewRole::Commodore)
                .expect("manager")
                .presence,
            ViewPresence::Unobserved
        );
        assert_eq!(
            result
                .agents
                .iter()
                .find(|a| a.role == ViewRole::Watcher)
                .expect("watcher")
                .presence,
            ViewPresence::Unobserved
        );
    }
}
