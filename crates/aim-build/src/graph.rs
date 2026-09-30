//! The task graph: nodes, their keys, and running what is stale in
//! dependency order, in parallel where the graph allows.
//!
//! A node's key hashes its recipe version, the versions of the tools it
//! runs, the keys of its upstream nodes and the content of its inputs: the
//! declared ones plus those its tools reported last time (cargo dep-info,
//! n2's deps log). A node whose key matches its stamp and whose outputs
//! exist is skipped.
//!
//! Downstream nodes see an upstream's output instead of its key where that
//! is cheap to name ([`nodes::output_key`]): a cargo artifact built again
//! the same, or a derived image found unchanged, leaves them fresh.

use crate::cargo::{self, Unit};
use crate::hash::{self, FileState, KeyHasher};
use crate::log::Log;
use crate::nodes;
use crate::stamp::{self, Stamp};
use crate::tools::{Tool, Tools};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Instant;

/// An upstream node. An order-only dependency must be built first but its
/// key is not part of this node's (a tool whose changes do not change this
/// node's output, such as linux-run for dex2oat's boot image).
#[derive(Clone, Debug)]
pub struct Dep {
    pub node: String,
    pub order_only: bool,
}

impl Dep {
    pub fn on(node: &str) -> Dep {
        Dep {
            node: node.into(),
            order_only: false,
        }
    }

    pub fn order_only(node: &str) -> Dep {
        Dep {
            node: node.into(),
            order_only: true,
        }
    }
}

pub enum Action {
    Cargo(Unit),
    Image,
    AidlGen,
    Xsdc,
    Art,
    BootImage,
    Angle,
    MoltenVk,
    SystemServer,
    DeviceServices,
    Oat,
    DerivedImage,
    TranslationCache,
    EmptyUserdata,
    UserdataTemplate,
}

pub struct Node {
    pub name: String,
    pub deps: Vec<Dep>,
    /// Declared input files.
    pub inputs: Vec<PathBuf>,
    /// Files or directories the node must leave behind.
    pub outputs: Vec<PathBuf>,
    pub tools: Vec<Tool>,
    /// Bumped when the recipe (the code that runs the node) changes what it
    /// produces.
    pub recipe: u32,
    pub action: Action,
    /// Part of `cargo aim build` without arguments: what a boot needs.
    pub boot: bool,
}

impl Node {
    /// Nodes run together in one job (one cargo invocation).
    fn batch(&self) -> Option<&str> {
        match &self.action {
            Action::Cargo(unit) => Some(unit.batch.as_str()),
            _ => None,
        }
    }
}

pub struct Ctx {
    pub tools: Tools,
    pub workspace: cargo::Workspace,
    pub verbose: bool,
}

pub struct Graph {
    pub nodes: Vec<Node>,
    index: HashMap<String, usize>,
}

/// A node's key as of now.
#[derive(Clone)]
pub struct Eval {
    pub key: String,
    tools: BTreeMap<String, String>,
    deps: BTreeMap<String, String>,
    inputs: BTreeMap<PathBuf, FileState>,
}

impl Graph {
    pub fn new(nodes: Vec<Node>) -> Result<Graph, String> {
        let index: HashMap<String, usize> = nodes
            .iter()
            .enumerate()
            .map(|(i, node)| (node.name.clone(), i))
            .collect();
        if index.len() != nodes.len() {
            return Err("duplicate node names".into());
        }
        for node in &nodes {
            for dep in &node.deps {
                if !index.contains_key(&dep.node) {
                    return Err(format!(
                        "{} depends on unknown node {}",
                        node.name, dep.node
                    ));
                }
            }
        }
        let graph = Graph { nodes, index };
        graph.order(
            &graph
                .nodes
                .iter()
                .map(|n| n.name.clone())
                .collect::<Vec<_>>(),
        )?;
        Ok(graph)
    }

    pub fn node(&self, name: &str) -> &Node {
        &self.nodes[self.index[name]]
    }

    /// The nodes `patterns` name: a node name, or a prefix ending at `/`
    /// (`hal` is every `hal/...` node).
    pub fn select(&self, patterns: &[String]) -> Result<Vec<String>, String> {
        let mut selected = Vec::new();
        for pattern in patterns {
            let matches: Vec<String> = self
                .nodes
                .iter()
                .filter(|n| n.name == *pattern || n.name.starts_with(&format!("{pattern}/")))
                .map(|n| n.name.clone())
                .collect();
            if matches.is_empty() {
                return Err(format!(
                    "no node `{pattern}` (`cargo aim status` lists them)"
                ));
            }
            selected.extend(matches);
        }
        Ok(selected)
    }

    pub fn boot_set(&self) -> Vec<String> {
        self.nodes
            .iter()
            .filter(|n| n.boot)
            .map(|n| n.name.clone())
            .collect()
    }

    pub fn all(&self) -> Vec<String> {
        self.nodes.iter().map(|n| n.name.clone()).collect()
    }

    /// `targets` and everything upstream of them, upstream first.
    pub fn order(&self, targets: &[String]) -> Result<Vec<String>, String> {
        fn visit(
            graph: &Graph,
            name: &str,
            done: &mut BTreeSet<String>,
            active: &mut Vec<String>,
            out: &mut Vec<String>,
        ) -> Result<(), String> {
            if done.contains(name) {
                return Ok(());
            }
            if active.iter().any(|a| a == name) {
                return Err(format!(
                    "dependency cycle: {} -> {name}",
                    active.join(" -> ")
                ));
            }
            active.push(name.into());
            for dep in &graph.node(name).deps {
                visit(graph, &dep.node, done, active, out)?;
            }
            active.pop();
            done.insert(name.into());
            out.push(name.into());
            Ok(())
        }
        let (mut done, mut out) = (BTreeSet::new(), Vec::new());
        for target in targets {
            visit(self, target, &mut done, &mut Vec::new(), &mut out)?;
        }
        Ok(out)
    }

    /// The key of `node` given its upstream keys. `known` supplies file
    /// states to reuse: the stamp's (by size and mtime), or, after a run,
    /// the states read before it (so an edit during the run is caught next
    /// time).
    pub fn evaluate(
        &self,
        node: &Node,
        installed: &Tools,
        keys: &HashMap<String, String>,
        found: &[PathBuf],
        known: &BTreeMap<PathBuf, FileState>,
        trust_known: bool,
    ) -> Result<Eval, String> {
        let mut key = KeyHasher::default();
        key.part("recipe", &node.recipe.to_string());
        let mut tools = BTreeMap::new();
        for tool in &node.tools {
            let version = installed
                .version(*tool)
                .map_err(|e| format!("{}: {e}", tool.name()))?;
            tools.insert(tool.name().to_string(), hash::sha256(version.as_bytes()));
        }
        for (name, version) in &tools {
            key.part(&format!("tool {name}"), version);
        }
        let mut deps = BTreeMap::new();
        for dep in node.deps.iter().filter(|d| !d.order_only) {
            deps.insert(dep.node.clone(), keys[&dep.node].clone());
        }
        for (name, dep_key) in &deps {
            key.part(&format!("dep {name}"), dep_key);
        }
        let mut inputs = BTreeMap::new();
        for path in node.inputs.iter().chain(found) {
            let state = match known.get(path) {
                Some(state) if trust_known => state.clone(),
                known => hash::file_state(path, known),
            };
            inputs.insert(path.clone(), state);
        }
        for (path, state) in &inputs {
            key.part(
                &format!("input {}", stamp::display(path).display()),
                &state.hash,
            );
        }
        Ok(Eval {
            key: key.finish(),
            tools,
            deps,
            inputs,
        })
    }

    /// The first missing output of `node`.
    fn missing_output<'a>(&self, node: &'a Node) -> Option<&'a Path> {
        node.outputs
            .iter()
            .find(|p| std::fs::symlink_metadata(p).is_err())
            .map(PathBuf::as_path)
    }
}

/// Why a node is stale, or `None` when it is fresh.
fn staleness(graph: &Graph, node: &Node, eval: &Eval, stamp: Option<&Stamp>) -> Option<String> {
    let Some(stamp) = stamp else {
        return Some("never built".into());
    };
    if stamp.key == eval.key {
        return graph
            .missing_output(node)
            .map(|p| format!("output missing: {}", stamp::display(p).display()));
    }
    if stamp.recipe != node.recipe {
        return Some("recipe changed".into());
    }
    let mut reasons = Vec::new();
    for (name, version) in &eval.tools {
        if stamp.tools.get(name) != Some(version) {
            reasons.push(format!("{name} changed"));
        }
    }
    for (name, key) in &eval.deps {
        if stamp.deps.get(name) != Some(key) {
            reasons.push(format!("upstream {name}"));
        }
    }
    let changed: Vec<String> = eval
        .inputs
        .iter()
        .filter(|(path, state)| stamp.inputs.get(*path).map(|s| &s.hash) != Some(&state.hash))
        .map(|(path, _)| stamp::display(path).display().to_string())
        .collect();
    let removed = stamp
        .inputs
        .keys()
        .filter(|path| !eval.inputs.contains_key(*path))
        .count();
    if !changed.is_empty() {
        let shown: Vec<&str> = changed.iter().take(3).map(String::as_str).collect();
        let more = changed.len() - shown.len();
        reasons.push(if more > 0 {
            format!("changed {} (+{more})", shown.join(", "))
        } else {
            format!("changed {}", shown.join(", "))
        });
    }
    if removed > 0 {
        reasons.push(format!("{removed} input(s) no longer used"));
    }
    if reasons.is_empty() {
        reasons.push("inputs changed".into());
    }
    Some(reasons.join("; "))
}

pub struct Options {
    pub jobs: usize,
}

enum State {
    Waiting,
    Queued(Eval),
    Running(Eval),
    Done,
    Failed,
}

/// Builds `targets` and what they need. Returns the number of nodes run.
pub fn build(
    graph: &Graph,
    targets: &[String],
    ctx: &Ctx,
    options: &Options,
) -> Result<usize, String> {
    let order = graph.order(targets)?;
    let started = Instant::now();
    let mut keys: HashMap<String, String> = HashMap::new();
    let mut states: HashMap<String, State> =
        order.iter().map(|n| (n.clone(), State::Waiting)).collect();
    let (results, received) =
        mpsc::channel::<(Vec<String>, Result<Vec<Vec<PathBuf>>, String>, PathBuf)>();
    let mut running = 0;
    let (mut built, mut fresh, mut failures) = (0, 0, Vec::<String>::new());

    std::thread::scope(|scope| -> Result<(), String> {
        loop {
            // Evaluate every node whose upstream is done.
            for name in &order {
                if !matches!(states[name], State::Waiting) {
                    continue;
                }
                let node = graph.node(name);
                let deps: Vec<&State> = node.deps.iter().map(|d| &states[&d.node]).collect();
                if deps.iter().any(|s| matches!(s, State::Failed)) {
                    states.insert(name.clone(), State::Failed);
                    failures.push(format!("{name}: not built (an upstream node failed)"));
                    continue;
                }
                if !deps.iter().all(|s| matches!(s, State::Done)) {
                    continue;
                }
                let stamp = Stamp::read(name);
                let (found, known) = stamp.as_ref().map_or((Vec::new(), BTreeMap::new()), |s| {
                    (s.found.clone(), s.inputs.clone())
                });
                let eval = match graph.evaluate(node, &ctx.tools, &keys, &found, &known, false) {
                    Ok(eval) => eval,
                    Err(error) => {
                        states.insert(name.clone(), State::Failed);
                        failures.push(format!("{name}: {error}"));
                        continue;
                    }
                };
                match staleness(graph, node, &eval, stamp.as_ref()) {
                    None => {
                        keys.insert(name.clone(), published(node, eval.key));
                        states.insert(name.clone(), State::Done);
                        fresh += 1;
                    }
                    Some(reason) => {
                        println!("  stale  {name}: {reason}");
                        states.insert(name.clone(), State::Queued(eval));
                    }
                }
            }
            // Start queued jobs, one cargo invocation per batch.
            let mut queued: Vec<&str> = order
                .iter()
                .filter(|n| matches!(states[*n], State::Queued(_)))
                .map(String::as_str)
                .collect();
            while running < options.jobs.max(1) && !queued.is_empty() {
                let first = graph.node(queued[0]);
                let group: Vec<&str> = match first.batch() {
                    Some(batch) => queued
                        .iter()
                        .copied()
                        .filter(|n| graph.node(n).batch() == Some(batch))
                        .collect(),
                    None => vec![queued[0]],
                };
                queued.retain(|n| !group.contains(n));
                for name in &group {
                    let State::Queued(eval) = states.remove(*name).unwrap() else {
                        unreachable!()
                    };
                    states.insert(name.to_string(), State::Running(eval));
                }
                let label = match first.batch() {
                    Some(batch) if group.len() > 1 => format!("{batch} ({} nodes)", group.len()),
                    _ => group[0].to_string(),
                };
                println!("  build  {label}");
                let log_path = aim_paths::cache().join("logs").join(format!(
                    "{}.log",
                    label.split(' ').next().unwrap().replace('/', "~")
                ));
                let names: Vec<String> = group.iter().map(|n| n.to_string()).collect();
                let results = results.clone();
                let verbose = ctx.verbose;
                running += 1;
                scope.spawn(move || {
                    let nodes: Vec<&Node> = names.iter().map(|n| graph.node(n)).collect();
                    let outcome = Log::create(log_path.clone(), verbose)
                        .and_then(|mut log| run(&nodes, ctx, &mut log));
                    let _ = results.send((names, outcome, log_path));
                });
            }
            if running == 0 {
                break;
            }
            // Record a finished job.
            let (names, outcome, log_path) = received.recv().unwrap();
            running -= 1;
            let found = match outcome {
                Ok(found) => found,
                Err(error) => {
                    for name in &names {
                        states.insert(name.clone(), State::Failed);
                    }
                    println!("  FAILED {}: {error}", names.join(", "));
                    println!("{}", indent(&Log::tail(&log_path, 25)));
                    println!("  (full log: {})", stamp::display(&log_path).display());
                    failures.push(format!("{}: {error}", names.join(", ")));
                    continue;
                }
            };
            for (name, found) in names.iter().zip(found) {
                let node = graph.node(name);
                let State::Running(before) = states.remove(name).unwrap() else {
                    unreachable!()
                };
                let result = graph
                    .evaluate(node, &ctx.tools, &keys, &found, &before.inputs, true)
                    .and_then(|eval| match graph.missing_output(node) {
                        Some(missing) => Err(format!("did not produce {}", missing.display())),
                        None => Ok(eval),
                    })
                    .and_then(|eval| {
                        Stamp {
                            key: eval.key.clone(),
                            recipe: node.recipe,
                            tools: eval.tools,
                            deps: eval.deps,
                            inputs: eval.inputs,
                            found,
                        }
                        .write(name)?;
                        Ok(published(node, eval.key))
                    });
                match result {
                    Ok(key) => {
                        keys.insert(name.clone(), key);
                        states.insert(name.clone(), State::Done);
                        built += 1;
                    }
                    Err(error) => {
                        println!("  FAILED {name}: {error}");
                        states.insert(name.clone(), State::Failed);
                        failures.push(format!("{name}: {error}"));
                    }
                }
            }
            println!(
                "  done   {} ({:.1} s since start)",
                names.join(", "),
                started.elapsed().as_secs_f64()
            );
        }
        Ok(())
    })?;
    println!(
        "{built} built, {fresh} fresh, {} failed in {:.1} s",
        failures.len(),
        started.elapsed().as_secs_f64()
    );
    if failures.is_empty() {
        Ok(built)
    } else {
        Err(failures.join("\n"))
    }
}

/// The key downstream nodes see for `node`: its output's, else its own.
fn published(node: &Node, key: String) -> String {
    nodes::output_key(node).unwrap_or(key)
}

fn indent(text: &str) -> String {
    text.lines()
        .map(|l| format!("    | {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Runs a job: one node, or a batch of cargo nodes. Returns each node's
/// found inputs.
fn run(nodes: &[&Node], ctx: &Ctx, log: &mut Log) -> Result<Vec<Vec<PathBuf>>, String> {
    if let Action::Cargo(_) = nodes[0].action {
        let units: Vec<&Unit> = nodes
            .iter()
            .map(|n| match &n.action {
                Action::Cargo(unit) => unit,
                _ => unreachable!(),
            })
            .collect();
        return cargo::build(&units, ctx, log);
    }
    let [node] = nodes else { unreachable!() };
    nodes::run(node, ctx, log).map(|found| vec![found])
}

/// Prints every node of `targets`' closure: fresh, or stale and why.
pub fn status(graph: &Graph, targets: &[String], ctx: &Ctx) -> Result<usize, String> {
    let order = graph.order(targets)?;
    let mut keys = HashMap::new();
    let mut stale = BTreeSet::new();
    let width = order.iter().map(String::len).max().unwrap_or(0);
    for name in &order {
        let node = graph.node(name);
        let stamp = Stamp::read(name);
        let (found, known) = stamp.as_ref().map_or((Vec::new(), BTreeMap::new()), |s| {
            (s.found.clone(), s.inputs.clone())
        });
        let reason = match graph.evaluate(node, &ctx.tools, &keys, &found, &known, false) {
            Ok(eval) => {
                let reason = staleness(graph, node, &eval, stamp.as_ref());
                keys.insert(name.clone(), published(node, eval.key));
                reason
            }
            Err(error) => {
                keys.insert(name.clone(), String::new());
                Some(format!("cannot check: {error}"))
            }
        };
        let upstream: Vec<&str> = node
            .deps
            .iter()
            .filter(|d| stale.contains(&d.node))
            .map(|d| d.node.as_str())
            .collect();
        match (reason, upstream.is_empty()) {
            (None, true) => println!("  {name:width$}  fresh"),
            (None, false) => {
                stale.insert(name.clone());
                println!("  {name:width$}  stale  after {}", upstream.join(", "));
            }
            (Some(reason), _) => {
                stale.insert(name.clone());
                println!("  {name:width$}  stale  {reason}");
            }
        }
    }
    println!("{} of {} nodes stale", stale.len(), order.len());
    Ok(stale.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, deps: &[&str], inputs: Vec<PathBuf>) -> Node {
        Node {
            name: name.into(),
            deps: deps.iter().map(|d| Dep::on(d)).collect(),
            inputs,
            outputs: Vec::new(),
            tools: Vec::new(),
            recipe: 1,
            action: Action::Image,
            boot: true,
        }
    }

    #[test]
    fn orders_upstream_first_and_rejects_cycles() {
        let graph = Graph::new(vec![
            node("c", &["b"], vec![]),
            node("b", &["a"], vec![]),
            node("a", &[], vec![]),
            node("hal/x", &[], vec![]),
        ])
        .unwrap();
        assert_eq!(graph.order(&["c".into()]).unwrap(), ["a", "b", "c"]);
        assert_eq!(graph.select(&["hal".into()]).unwrap(), ["hal/x"]);
        assert!(graph.select(&["nope".into()]).is_err());
        assert!(Graph::new(vec![node("a", &["b"], vec![]), node("b", &["a"], vec![])]).is_err());
        assert!(Graph::new(vec![node("a", &["missing"], vec![])]).is_err());
    }

    #[test]
    fn key_follows_inputs_and_upstream_keys() {
        let dir = std::env::temp_dir().join(format!("aim-graph-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("input");
        std::fs::write(&input, "one").unwrap();
        let graph = Graph::new(vec![
            node("a", &[], vec![]),
            node("b", &["a"], vec![input.clone()]),
        ])
        .unwrap();
        let tools = Tools::default();
        let none = BTreeMap::new();
        let key = |upstream: &str| {
            let keys = HashMap::from([("a".to_string(), upstream.to_string())]);
            graph
                .evaluate(graph.node("b"), &tools, &keys, &[], &none, false)
                .unwrap()
        };
        let first = key("k1");
        assert_eq!(first.key, key("k1").key);
        assert_ne!(first.key, key("k2").key);
        let stamp = Stamp {
            key: first.key.clone(),
            recipe: 1,
            deps: first.deps.clone(),
            inputs: first.inputs.clone(),
            ..Stamp::default()
        };
        assert_eq!(
            staleness(&graph, graph.node("b"), &first, Some(&stamp)),
            None
        );
        std::fs::write(&input, "two").unwrap();
        let changed = key("k1");
        let reason = staleness(&graph, graph.node("b"), &changed, Some(&stamp)).unwrap();
        assert!(
            reason.contains("changed") && reason.contains("input"),
            "{reason}"
        );
        let reason = staleness(&graph, graph.node("b"), &key("k2"), Some(&stamp)).unwrap();
        assert!(reason.contains("upstream a"), "{reason}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
