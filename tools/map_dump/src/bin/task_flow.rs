//! Research: who drives each objective task. For every Kismet node that touches a task
//! (`DisSeqAct_SetTaskState`, `SetTaskHidden`, `UpdateTaskTarget`, `DisSeqCond_TaskState`), walk
//! the graph backwards to the events that trigger it, following remote events into their
//! activators (Kismet `SeqAct_ActivateRemoteEvent` and conversation
//! `DisConv_KismetActivateRemoteEvent`), dialog outputs into the conversation node that fires
//! them, and nested sequences into the links that activate them.
//!
//! `task_flow <package> [--depth N] [--task substring] [--from export]`
//! `--from` prints the backward tree of any node. Output is derived from retail data: keep it
//! under harness_out/.
use anyhow::Result;
use dishonored_assets::Content;
use dishonored_assets::kismet::{KismetGraph, KismetOp, read_kismet};
use std::collections::{BTreeMap, HashMap, HashSet};

struct Ctx<'g> {
    g: &'g KismetGraph,
    by_export: HashMap<usize, &'g KismetOp>,
    var_names: HashMap<usize, String>,
    flags: HashMap<String, String>,
    preds: HashMap<usize, Vec<(usize, String)>>,
    remote_acts: HashMap<String, Vec<usize>>,
    input_names: HashMap<usize, Vec<String>>,
    conv_remote: HashMap<String, Vec<usize>>,
    max_depth: usize,
}

fn prop<'a>(op: &'a KismetOp, name: &str) -> Option<&'a str> {
    op.props.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

fn unq(s: &str) -> String {
    s.trim_start_matches("Name(\"").trim_start_matches("Str(\"").trim_end_matches("\")").to_string()
}

fn tail(s: &str) -> &str {
    s.rsplit('.').next().unwrap_or(s)
}

impl Ctx<'_> {
    fn var_desc(&self, v: usize) -> String {
        let Some(o) = self.by_export.get(&v) else { return v.to_string() };
        let vn = self.var_names.get(&v).map(|s| format!("\"{s}\"")).unwrap_or_default();
        let vals: Vec<String> = o
            .props
            .iter()
            .filter(|(k, _)| k != "m_TaskName" || true)
            .map(|(k, v)| {
                let v = v.trim_start_matches("-> ");
                format!("{}={}", k.trim_start_matches("m_p").trim_start_matches("m_"), tail(&unq(v)))
            })
            .collect();
        format!("{}{vn}({})", o.class.trim_start_matches("SeqVar_").trim_start_matches("DisSeqVar_"), vals.join(","))
    }

    fn label(&self, op: &KismetOp) -> String {
        let props: Vec<String> = op
            .props
            .iter()
            .filter(|(k, _)| !matches!(k.as_str(), "m_iSpeaker" | "m_pOwner" | "MaxTriggerCount" | "m_OutputLinks" | "bClientSideOnly"))
            .filter(|(k, _)| k != "m_pStoryFlagSet")
            .map(|(k, v)| match self.flags.get(v.as_str()) { Some(n) => format!("FLAG={n}"), None => format!("{k}={}", v.trim_start_matches("-> ")) })
            .collect();
        let vars: Vec<String> = op
            .variables
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(d, v)| format!("{d}:{}", v.iter().map(|x| self.var_desc(*x)).collect::<Vec<_>>().join("+")))
            .collect();
        let seq = tail(&op.sequence);
        format!(
            "{}[{}] {}{}{}{}",
            op.class,
            op.export,
            props.join(" "),
            if vars.is_empty() { String::new() } else { format!(" {{{}}}", vars.join("; ")) },
            op.comment.as_ref().map(|c| format!(" //{c}")).unwrap_or_default(),
            if seq.is_empty() { String::new() } else { format!(" @{seq}") }
        )
    }

    /// Leaf triggers (events, conversation nodes, unconnected starts) reachable backwards.
    fn roots(&self, export: usize, via: &str, depth: usize, seen: &mut HashSet<usize>, out: &mut Vec<String>) {
        let Some(op) = self.by_export.get(&export) else { return };
        if !seen.insert(export) || depth > 40 {
            return;
        }
        let is_event = op.kind() == "event";
        let mut expanded = false;
        if op.class == "SeqEvent_RemoteEvent" {
            let name = prop(op, "EventName").map(unq).unwrap_or_default();
            for a in self.remote_acts.get(&name).into_iter().flatten() {
                expanded = true;
                self.roots(*a, "remote", depth + 1, seen, out);
            }
            for c in self.conv_remote.get(&name).into_iter().flatten() {
                expanded = true;
                self.roots(*c, "conv", depth + 1, seen, out);
            }
        }
        if op.class == "DisSeqEvent_DialogOutputs" {
            let origin = prop(op, "Originator").map(|o| o.trim_start_matches("-> ").to_string()).unwrap_or_default();
            for c in self.conv_remote.get(via).into_iter().flatten() {
                let co = self.by_export[c];
                if prop(co, "m_pOwner").is_some_and(|o| o.contains(&format!("{origin}."))) {
                    expanded = true;
                    self.roots(*c, "dialog", depth + 1, seen, out);
                }
            }
        }
        if op.class == "SeqEvent_SequenceActivated" {
            if let Some(s) = self.g.ops.iter().find(|o| o.path == op.sequence) {
                expanded = true;
                self.roots(s.export, "seq-in", depth + 1, seen, out);
            }
        }
        let preds = self.preds.get(&export).cloned().unwrap_or_default();
        for (p, desc) in &preds {
            expanded = true;
            self.roots(*p, desc, depth + 1, seen, out);
        }
        if is_event || op.class == "DisConv_KismetActivateRemoteEvent" || !expanded {
            let mut l = format!("[{via}] {}", self.label(op));
            if is_event && expanded && op.class != "SeqEvent_RemoteEvent" {
                l.push_str(" (+preds)");
            }
            if op.class == "SeqEvent_RemoteEvent" && expanded {
                return;
            }
            out.push(l);
        }
    }
    fn effect(&self, op: &KismetOp, input: i32) -> Option<String> {
        let c = op.class.as_str();
        let keep = matches!(
            c,
            "DisSeqAct_SetTaskState" | "DisSeqAct_SetTaskHidden" | "DisSeqAct_UpdateTaskTarget" | "DisSeqAct_SetStoryFlag" | "DisSeqAct_AddAbstractItem"
                | "DisSeqAct_RemoveAbstractItem" | "DisSeqAct_ShowTargetNotification" | "DisSeqAct_DialogInputs" | "DisSeqAct_AddObjective" | "DisSeqAct_GameOver"
                | "DisSeqAct_StartSpawn" | "DisSeqAct_NPCMarkForVanish" | "DisSeqAct_SetDisposition" | "DisSeqAct_GivePickup" | "DisSeqAct_SetPlayerTravelDestination"
                | "DisSeqAct_SetObjectiveHidden" | "DisSeqAct_ShowHUDMessage" | "DisSeqAct_DialogScriptedChoice" | "DisSeqAct_IncrementPlayerStat"
                | "SeqAct_Interp" | "DisSeqAct_ActivateUsable" | "DisSeqAct_EquipItemType" | "DisSeqAct_NPCSetMaterials" | "SeqAct_Teleport" | "SeqAct_Destroy"
                | "DisSeqAct_ShowLocationDiscovery" | "DisSeqAct_AutoSave" | "DisSeqAct_AIRingAlarm" | "DisSeqAct_OverridePossess" | "DisSeqAct_LimitPawnMinHealth"
        );
        if !keep {
            return None;
        }
        let mut l = self.label(op);
        if c == "DisSeqAct_DialogInputs" {
            let n = self.input_names.get(&op.export).and_then(|v| v.get(input as usize)).cloned().unwrap_or_else(|| input.to_string());
            l = format!("DIALOG \"{n}\" {l}");
        }
        Some(l)
    }

    /// Forward from an event: effects reached, with the branch path (conditions/switches) that leads there.
    fn story(&self, export: usize, input: i32, path: &mut Vec<String>, depth: usize, seen: &mut HashSet<(usize, i32)>, out: &mut Vec<String>) {
        let Some(op) = self.by_export.get(&export) else { return };
        if !seen.insert((export, input)) || depth > 40 {
            return;
        }
        if let Some(e) = self.effect(op, input) {
            out.push(format!("{}{}", if path.is_empty() { String::new() } else { format!("[{}] ", path.join(" > ")) }, e));
        }
        if op.class == "SeqAct_ActivateRemoteEvent" {
            let name = prop(op, "EventName").map(unq).unwrap_or_default();
            for e in self.g.ops.iter().filter(|o| o.class == "SeqEvent_RemoteEvent" && prop(o, "EventName").map(unq).as_deref() == Some(name.as_str())) {
                path.push(format!("~{name}"));
                self.story(e.export, 0, path, depth + 1, seen, out);
                path.pop();
            }
        }
        let branchy = op.outputs.iter().filter(|o| !o.1.is_empty()).count() > 1 || op.kind() == "condition" || op.class.contains("Switch") || op.class == "SeqAct_Interp" || op.class == "DisSeqAct_AIGoToActor" || op.class == "DisSeqAct_DialogScriptedChoice";
        for (desc, targets) in &op.outputs {
            for (t, idx) in targets {
                if branchy {
                    let what = match op.variables.iter().find(|(_, v)| !v.is_empty()) {
                        Some((_, v)) if op.kind() == "condition" => self.var_desc(v[0]),
                        _ => op.comment.clone().unwrap_or_else(|| op.class.trim_start_matches("SeqAct_").trim_start_matches("SeqCond_").to_string()),
                    };
                    path.push(format!("{what}:{desc}"));
                    self.story(*t, *idx, path, depth + 1, seen, out);
                    path.pop();
                } else {
                    self.story(*t, *idx, path, depth + 1, seen, out);
                }
            }
        }
    }

    fn fwd(&self, export: usize, via: &str, depth: usize, seen: &mut HashSet<usize>) {
        let pad = "  ".repeat(depth + 1);
        let Some(op) = self.by_export.get(&export) else { return };
        let line = format!("{pad}->{} {}", if via.is_empty() { String::new() } else { format!("[{via}]") }, self.label(op));
        if !seen.insert(export) {
            println!("{line} (seen)");
            return;
        }
        println!("{line}");
        if depth >= self.max_depth {
            return;
        }
        if op.class == "SeqAct_ActivateRemoteEvent" {
            let name = prop(op, "EventName").map(unq).unwrap_or_default();
            for e in self.g.ops.iter().filter(|o| o.class == "SeqEvent_RemoteEvent" && prop(o, "EventName").map(unq).as_deref() == Some(name.as_str())) {
                self.fwd(e.export, "remote", depth + 1, seen);
            }
        }
        for (desc, targets) in &op.outputs {
            for (t, _) in targets {
                self.fwd(*t, desc, depth + 1, seen);
            }
        }
    }

    fn walk(&self, export: usize, via: &str, depth: usize, seen: &mut HashSet<usize>) {
        let pad = "  ".repeat(depth + 1);
        let Some(op) = self.by_export.get(&export) else {
            println!("{pad}<- ?{export}");
            return;
        };
        let line = format!("{pad}<-{} {}", if via.is_empty() { String::new() } else { format!("[{via}]") }, self.label(op));
        if !seen.insert(export) {
            println!("{line} (seen)");
            return;
        }
        println!("{line}");
        if depth >= self.max_depth {
            println!("{pad}  ...");
            return;
        }
        // Remote events: jump to activators.
        if op.class == "SeqEvent_RemoteEvent" {
            let name = prop(op, "EventName").map(unq).unwrap_or_default();
            for a in self.remote_acts.get(&name).into_iter().flatten() {
                self.walk(*a, "remote", depth + 1, seen);
            }
            for c in self.conv_remote.get(&name).into_iter().flatten() {
                self.walk(*c, "conv", depth + 1, seen);
            }
        }
        // Dialog outputs: the conversation node that fires the named output.
        if op.class == "DisSeqEvent_DialogOutputs" {
            let origin = prop(op, "Originator").map(|o| o.trim_start_matches("-> ").to_string()).unwrap_or_default();
            if !via.is_empty() {
                for c in self.conv_remote.get(via).into_iter().flatten() {
                    let co = self.by_export[c];
                    if prop(co, "m_pOwner").is_some_and(|o| o.contains(&format!("{origin}."))) {
                        self.walk(*c, "dialog", depth + 1, seen);
                    }
                }
            }
        }
        // Sequence entry: whoever activates the enclosing sequence.
        if op.class == "SeqEvent_SequenceActivated" {
            if let Some(s) = self.g.ops.iter().find(|o| o.path == op.sequence) {
                self.walk(s.export, "seq-in", depth + 1, seen);
            }
        }
        // Nested sequence used as a node: its finish actions.
        for (p, desc) in self.preds.get(&export).into_iter().flatten() {
            let po = self.by_export.get(p);
            if po.is_some_and(|po| po.class.starts_with("Sequence") && po.class != "SequenceFrame") {
                for f in self.g.ops.iter().filter(|o| o.class == "SeqAct_FinishSequence" && o.sequence == po.unwrap().path) {
                    self.walk(f.export, &format!("{desc} finish"), depth + 1, seen);
                }
            } else {
                self.walk(*p, desc, depth + 1, seen);
            }
        }
    }
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let c = Content::locate()?;
    let pkg = c.package(&a[0])?;
    let opt = |f: &str| a.iter().position(|x| x == f).and_then(|i| a.get(i + 1)).cloned();
    let g = read_kismet(&pkg);
    let by_export: HashMap<usize, &KismetOp> = g.ops.iter().map(|o| (o.export, o)).collect();
    let mut preds: HashMap<usize, Vec<(usize, String)>> = HashMap::new();
    for o in &g.ops {
        for (desc, targets) in &o.outputs {
            for (t, _) in targets {
                preds.entry(*t).or_default().push((o.export, desc.clone()));
            }
        }
    }
    let mut remote_acts: HashMap<String, Vec<usize>> = HashMap::new();
    let mut conv_remote: HashMap<String, Vec<usize>> = HashMap::new();
    for o in &g.ops {
        if o.class == "SeqAct_ActivateRemoteEvent" {
            if let Some(n) = prop(o, "EventName") {
                remote_acts.entry(unq(n)).or_default().push(o.export);
            }
        }
        if o.class == "DisConv_KismetActivateRemoteEvent" {
            if let Some(n) = prop(o, "m_EventName") {
                conv_remote.entry(unq(n)).or_default().push(o.export);
            }
        }
    }
    let mut var_names: HashMap<usize, String> = HashMap::new();
    for o in &g.ops {
        if o.kind() != "variable" {
            continue;
        }
        if let Ok(h) = pkg.read_object(o.export) {
            for key in ["FindVarName", "VarName"] {
                if let Some(ue3_package::properties::Value::Name(n)) = ue3_package::properties::find(&h.properties, key).map(|p| &p.value) {
                    if n != "None" {
                        var_names.insert(o.export, n.clone());
                    }
                }
            }
        }
    }
    let mut input_names: HashMap<usize, Vec<String>> = HashMap::new();
    for o in g.ops.iter().filter(|o| o.class == "DisSeqAct_DialogInputs") {
        if let Ok(h) = pkg.read_object(o.export) {
            if let Some(p) = ue3_package::properties::find(&h.properties, "InputLinks") {
                let names = p
                    .array_structs(&pkg)
                    .unwrap_or_default()
                    .iter()
                    .map(|l| match ue3_package::properties::find(l, "LinkDesc").map(|x| &x.value) {
                        Some(ue3_package::properties::Value::Str(s)) => s.clone(),
                        _ => String::new(),
                    })
                    .collect();
                input_names.insert(o.export, names);
            }
        }
    }
    let mut flags: HashMap<String, String> = HashMap::new();
    if let Ok(st) = c.package("Startup") {
        for i in 0..st.exports.len() {
            if st.export_class(i) != "DisStoryFlagSet" {
                continue;
            }
            let set = st.obj_name(ue3_package::ObjRef::Export(i));
            let Ok(h) = st.read_object(i) else { continue };
            let Some(p) = ue3_package::properties::find(&h.properties, "m_StoryFlags") else { continue };
            for e in p.array_structs(&st).unwrap_or_default() {
                let name = match ue3_package::properties::find(&e, "m_Name").map(|x| &x.value) {
                    Some(ue3_package::properties::Value::Name(n)) => n.clone(),
                    _ => continue,
                };
                if let Some(ue3_package::properties::Value::Guid(g)) = ue3_package::properties::find(&e, "m_GUID").map(|x| &x.value) {
                    flags.insert(format!("Guid([{}, {}, {}, {}])", g[0], g[1], g[2], g[3]), format!("{set}:{name}"));
                }
            }
        }
    }
    let ctx = Ctx { g: &g, by_export, var_names, input_names, flags, preds, remote_acts, conv_remote, max_depth: opt("--depth").and_then(|d| d.parse().ok()).unwrap_or(7) };
    if a.iter().any(|x| x == "--story") {
        let seqf = opt("--seq");
        for o in g.ops.iter().filter(|o| o.kind() == "event" && o.class != "SeqEvent_RemoteEvent" && seqf.as_ref().is_none_or(|s| o.sequence.ends_with(s.as_str()))) {
            let mut out = Vec::new();
            let mut seen = HashSet::new();
            ctx.story(o.export, 0, &mut Vec::new(), 0, &mut seen, &mut out);
            if out.is_empty() {
                continue;
            }
            println!("\n## {}", ctx.label(o));
            for l in out {
                println!("   {l}");
            }
        }
        return Ok(());
    }
    if let Some(from) = opt("--fwd") {
        for f in from.split(',') {
            let mut seen = HashSet::new();
            ctx.fwd(f.parse()?, "", 0, &mut seen);
        }
        return Ok(());
    }
    if let Some(from) = opt("--from") {
        for f in from.split(',') {
            let mut seen = HashSet::new();
            ctx.walk(f.parse()?, "", 0, &mut seen);
        }
        return Ok(());
    }
    let filter = opt("--task");
    // task key -> task ops
    let mut by_task: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for o in &g.ops {
        if !matches!(o.class.as_str(), "DisSeqAct_SetTaskState" | "DisSeqAct_SetTaskHidden" | "DisSeqAct_UpdateTaskTarget" | "DisSeqCond_TaskState") {
            continue;
        }
        let mut keys = Vec::new();
        for (_, vars) in &o.variables {
            for v in vars {
                if let Some(vo) = ctx.by_export.get(v) {
                    if vo.class == "DisSeqVar_Task" {
                        let obj = prop(vo, "m_pObjective").map(|s| tail(s.trim_start_matches("-> ")).to_string()).unwrap_or_default();
                        let t = prop(vo, "m_TaskName").map(unq).unwrap_or_default();
                        keys.push(format!("{obj}.{t}"));
                    }
                }
            }
        }
        if keys.is_empty() {
            keys.push("<no task var>".into());
        }
        for k in keys {
            by_task.entry(k).or_default().push(o.export);
        }
    }
    if a.iter().any(|x| x == "--roots") {
        for (task, ops) in &by_task {
            if filter.as_ref().is_some_and(|f| !task.contains(f.as_str())) {
                continue;
            }
            println!("\n######## TASK {task}");
            for o in ops {
                let op = ctx.by_export[o];
                let what: Vec<String> = op.props.iter().filter(|(k, _)| k != "m_bAlwaysOutOfBendTime").map(|(k, v)| format!("{k}={v}")).collect();
                let tgt: Vec<String> = op.variables.iter().filter(|(d, v)| d == "Target" && !v.is_empty()).flat_map(|(_, v)| v.iter().map(|x| ctx.var_desc(*x))).collect();
                println!("* {}[{}] {} {} @{}", op.class.trim_start_matches("DisSeqAct_"), op.export, what.join(" "), tgt.join(","), tail(&op.sequence));
                let mut seen = HashSet::new();
                let mut out = Vec::new();
                for (p, desc) in ctx.preds.get(o).into_iter().flatten() {
                    ctx.roots(*p, desc, 0, &mut seen, &mut out);
                }
                out.sort();
                out.dedup();
                for r in out {
                    println!("    <= {r}");
                }
            }
        }
        return Ok(());
    }
    for (task, ops) in &by_task {
        if filter.as_ref().is_some_and(|f| !task.contains(f.as_str())) {
            continue;
        }
        println!("\n######## TASK {task} ({} nodes)", ops.len());
        let mut seen = HashSet::new();
        for o in ops {
            println!("* {}", ctx.label(ctx.by_export[o]));
            for (p, desc) in ctx.preds.get(o).into_iter().flatten() {
                ctx.walk(*p, desc, 0, &mut seen);
            }
        }
    }
    Ok(())
}
