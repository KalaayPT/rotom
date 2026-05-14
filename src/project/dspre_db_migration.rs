//! DSPRE **v1** edited `scrcmd_database.json` migration: diff vs vanilla baseline and optional merge into
//! project **`_v2.json`** (**descriptions**, **`legacy_name`** / **params**; preserves v2 **`notes`**).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use uxie::GameFamily;

use super::config::RotomConfig;
use super::convert::ConvertOptions;
use super::error::{ProjectError, Result};
use super::scrcmd_baseline::{VanillaScrcmdV1, scrcmd_v1_repo_filename};

const DIFF_SUMMARY_MAX_LINES: usize = 40;
const STRUCTURAL_DELTA_PRINT_MAX: usize = 60;

fn index_scrcmd_by_id(map: &Map<String, Value>) -> BTreeMap<u16, (&str, &Value)> {
    let mut out = BTreeMap::new();
    for (k, v) in map {
        let h = k.strip_prefix("0x").or_else(|| k.strip_prefix("0X"));
        let Some(h) = h else {
            continue;
        };
        let Ok(id) = u16::from_str_radix(h, 16) else {
            continue;
        };
        out.insert(id, (k.as_str(), v));
    }
    out
}

fn without_description(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut c = Map::with_capacity(m.len());
            for (k, vv) in m {
                if k != "description" {
                    c.insert(k.clone(), vv.clone());
                }
            }
            Value::Object(c)
        }
        other => other.clone(),
    }
}

fn without_description_and_parameter_values(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut c = Map::with_capacity(m.len());
            for (k, vv) in m {
                if k != "description" && k != "parameter_values" {
                    c.insert(k.clone(), vv.clone());
                }
            }
            Value::Object(c)
        }
        other => other.clone(),
    }
}

fn description_merge_candidate(vanilla_cmd: Option<&Value>, user_cmd: &Value) -> Option<String> {
    let vanilla_clean = vanilla_cmd.map(without_description);
    let user_clean = without_description(user_cmd);
    match vanilla_clean {
        None => None,
        Some(ref v_clean) if v_clean != &user_clean => None,
        Some(_) => {
            let user_d = user_cmd.get("description").and_then(Value::as_str)?;
            let van_d = vanilla_cmd?
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("");
            (user_d != van_d).then(|| user_d.to_owned())
        }
    }
}

/// Path to DSPRE **`scrcmd_database.json`** under Roaming **`…/edited_databases/{project_folder_name}`** for a Wine drive layout (`wine_prefix` + **`drive_c/users/{wine_windows_username}/…`**).
pub fn dspre_roaming_edited_scrcmd_json(
    wine_prefix: &Path,
    wine_windows_username: &str,
    project_folder_name: &str,
) -> PathBuf {
    wine_prefix
        .join("drive_c")
        .join("users")
        .join(wine_windows_username)
        .join("AppData")
        .join("Roaming")
        .join("DSPRE")
        .join("databases")
        .join("edited_databases")
        .join(project_folder_name)
        .join("scrcmd_database.json")
}

/// `Rom_DSPRE_contents` → `Rom` for **`[project].name`** on **`rotom init`** and for locating DSPRE edited DB paths.
///
/// If the suffix is absent, or stripping would leave an empty string, returns `basename` trimmed.
pub fn dspre_workspace_dir_basename_for_rotom(basename: &str) -> String {
    let folder = basename.trim();
    if let Some(stripped) = folder.strip_suffix("_DSPRE_contents") {
        let stripped = stripped.trim();
        if !stripped.is_empty() {
            return stripped.to_owned();
        }
    }
    folder.to_owned()
}

fn push_dspre_edited_hint(names: &mut Vec<String>, label: &str) {
    let t = label.trim();
    if t.is_empty() || names.iter().any(|n| n.as_str() == t) {
        return;
    }
    names.push(t.to_owned());
}

/// Workspace folder names and DSPRE edited-database folder names usually differ for exports:
/// e.g. `PokemonPlatinumUSA_DSPRE_contents` vs `edited_databases` **`PokemonPlatinumUSA`** / `PokemonPlatinumUSA`.
fn dspre_edited_database_project_hints(root: &Path, config: &RotomConfig) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(n) = root
        .file_name()
        .and_then(|s| s.to_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        push_dspre_edited_hint(&mut names, n);
        push_dspre_edited_hint(&mut names, &dspre_workspace_dir_basename_for_rotom(n));
    }
    let cfg = config.project.name.trim();
    if !cfg.is_empty() {
        push_dspre_edited_hint(&mut names, cfg);
    }
    names
}

#[cfg(windows)]
fn native_windows_dspre_edited_scrcmd(project_folder_name: &str) -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|appdata| {
        PathBuf::from(appdata)
            .join("DSPRE")
            .join("databases")
            .join("edited_databases")
            .join(project_folder_name)
            .join("scrcmd_database.json")
    })
}

#[cfg(not(windows))]
fn native_windows_dspre_edited_scrcmd(_project_folder_name: &str) -> Option<PathBuf> {
    None
}

#[cfg(not(windows))]
fn wine_dspre_edited_scrcmd(project_folder_name: &str) -> Option<PathBuf> {
    let prefix = std::env::var_os("WINEPREFIX")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".wine")))?;

    let win_user = std::env::var("DSPRE_WINE_WINDOWS_USER")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| std::env::var("USER").ok().filter(|s| !s.trim().is_empty()))?;

    Some(dspre_roaming_edited_scrcmd_json(
        &prefix,
        win_user.trim(),
        project_folder_name,
    ))
}

#[cfg(windows)]
fn wine_dspre_edited_scrcmd(_project_folder_name: &str) -> Option<PathBuf> {
    None
}

fn dspre_external_edited_scrcmd_candidates(root: &Path, config: &RotomConfig) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for name in dspre_edited_database_project_hints(root, config) {
        if let Some(p) = native_windows_dspre_edited_scrcmd(&name) {
            out.push(p);
        }
        if let Some(p) = wine_dspre_edited_scrcmd(&name) {
            out.push(p);
        }
    }
    out
}

/// First **`scrcmd_database.json`** whose root JSON has **`scrcmd`** (checks project dirs, **`edited_databases/**`**, then DSPRE Roaming: Windows **`%AppData%`**; Unix/Wine **`$WINEPREFIX` / `~/.wine`** and **`DSPRE_WINE_WINDOWS_USER`** vs **`USER`** for `drive_c/users/…`; folder guesses use **`dspre_workspace_dir_basename_for_rotom`** plus **`[project].name`**).
pub fn find_local_scrcmd_v1_path(
    root: &Path,
    family: GameFamily,
    config: &RotomConfig,
) -> Option<PathBuf> {
    let dbdir = config.database_dir(root);
    let mut cands: Vec<PathBuf> = vec![
        dbdir.join(scrcmd_v1_repo_filename(family)),
        dbdir.join("scrcmd_database.json"),
        root.join("scrcmd_database.json"),
    ];

    for p in dspre_external_edited_scrcmd_candidates(root, config) {
        cands.push(p);
    }

    // DSPRE stores edited databases at edited_databases/{project_name}/scrcmd_database.json
    for name in dspre_edited_database_project_hints(root, config) {
        cands.push(
            root.join("edited_databases")
                .join(&name)
                .join("scrcmd_database.json"),
        );
    }

    cands.into_iter().find(|p| {
        if !p.is_file() {
            return false;
        }
        let Ok(s) = std::fs::read_to_string(p) else {
            return false;
        };
        let Ok(v) = serde_json::from_str::<Value>(&s) else {
            return false;
        };
        v.get("scrcmd").and_then(Value::as_object).is_some()
    })
}

struct Row {
    id: u16,
    label: Option<String>,
    merged_description: Option<String>,
    structural_change: bool,
    user_only_entry: bool,
}

fn classify(vanilla_map: Option<&Map<String, Value>>, user_map: &Map<String, Value>) -> Vec<Row> {
    let v_index = vanilla_map.map(index_scrcmd_by_id).unwrap_or_default();
    let u_index = index_scrcmd_by_id(user_map);
    let mut ids: Vec<u16> = v_index.keys().chain(u_index.keys()).copied().collect();
    ids.sort_unstable();
    ids.dedup();

    ids.into_iter()
        .filter_map(|id| {
            let (_, user_v) = u_index.get(&id).copied()?;
            let vanilla_v_opt = v_index.get(&id).map(|(_, v)| *v);
            let label = user_v
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string);
            let user_only_entry = vanilla_v_opt.is_none();
            let merged = description_merge_candidate(vanilla_v_opt, user_v);
            let structural_change = without_description_and_parameter_values(user_v)
                != vanilla_v_opt.map_or_else(
                    || Value::Object(Map::new()),
                    without_description_and_parameter_values,
                );
            Some(Row {
                id,
                label,
                merged_description: merged,
                structural_change,
                user_only_entry,
            })
        })
        .collect()
}

fn print_summary(rows: &[Row]) {
    let mut d = 0usize;
    let mut s = 0usize;
    let mut u = 0usize;
    for r in rows {
        if r.user_only_entry {
            u += 1;
        } else if r.structural_change {
            s += 1;
        } else if r.merged_description.is_some() {
            d += 1;
        }
    }
    eprintln!("scrcmd v1 diff: {d} description-only, {s} structural, {u} user-only vs baseline");

    let mut n = 0usize;
    for r in rows {
        if !(r.structural_change || r.user_only_entry || r.merged_description.is_some()) {
            continue;
        }
        if n >= DIFF_SUMMARY_MAX_LINES {
            break;
        }
        let name = r.label.as_deref().unwrap_or("?");
        let tag = if r.user_only_entry {
            "user_only"
        } else if r.structural_change {
            "structural"
        } else {
            "description"
        };
        eprintln!("  0x{:04X}  {}  {}", r.id, tag, name);
        n += 1;
    }
    let total = rows
        .iter()
        .filter(|r| r.structural_change || r.user_only_entry || r.merged_description.is_some())
        .count();
    if total > n {
        eprintln!("  … and {} more", total - n);
    }
}

fn scrcmd_map_by_command_id(obj: &Map<String, Value>) -> BTreeMap<u16, &Value> {
    index_scrcmd_by_id(obj)
        .into_iter()
        .map(|(id, (_, v))| (id, v))
        .collect()
}

fn shape_fields_v1() -> [&'static str; 5] {
    [
        "name",
        "decomp_name",
        "parameters",
        "parameter_types",
        "parameter_values",
    ]
}

/// **`parameters`** / **`parameter_types`** lengths often disagree; when both arrays are populated,
/// we align on the **`min`** length — extra trailing `parameter_types` entries are DSPRE noise.
fn dspre_zip_arity(params: &[Value], types: &[Value]) -> usize {
    match (params.is_empty(), types.is_empty()) {
        (true, true) => 0,
        (true, false) => types.len(),
        (false, true) => params.len(),
        (false, false) => params.len().min(types.len()),
    }
}

/// Subset of a v1 slot used before applying a structural v2 merge (**ignores** `parameter_values`
/// quirks and trims list length disagreement).
fn normalized_dspre_merge_shape(cmd: &Value) -> Value {
    let Value::Object(m) = cmd else {
        return Value::Object(Map::new());
    };
    let mut out = Map::new();
    for k in ["name", "decomp_name"] {
        if let Some(v) = m.get(k) {
            out.insert(k.to_string(), v.clone());
        }
    }
    let params = m
        .get("parameters")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let types = m
        .get("parameter_types")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let n = dspre_zip_arity(&params, &types);
    out.insert(
        "parameters".into(),
        Value::Array(params.iter().take(n).cloned().collect()),
    );
    out.insert(
        "parameter_types".into(),
        Value::Array(types.iter().take(n).cloned().collect()),
    );
    Value::Object(out)
}

fn dspre_merge_shape_differs(vanilla_cmd: Option<&Value>, user_cmd: &Value) -> bool {
    let u = normalized_dspre_merge_shape(user_cmd);
    let v = vanilla_cmd.map_or_else(|| Value::Object(Map::new()), normalized_dspre_merge_shape);
    u != v
}

/// Count of opcode ids present in **both** maps where normalized merge-shape differs — ignores ids only on one side (e.g. user-only additions).
pub fn dspre_merge_shape_diff_score(
    user_scrcmd: &Map<String, Value>,
    vanilla_scrcmd_obj: &Map<String, Value>,
) -> usize {
    let u_index = index_scrcmd_by_id(user_scrcmd);
    let v_index = index_scrcmd_by_id(vanilla_scrcmd_obj);
    let mut n = 0usize;
    for (id, (_, u_cmd)) in &u_index {
        let Some((_, v_cmd)) = v_index.get(id) else {
            continue;
        };
        if dspre_merge_shape_differs(Some(v_cmd), u_cmd) {
            n += 1;
        }
    }
    n
}

/// Heuristic: DSPRE **Following Platinum** exposes extended script commands (historically through opcode ~860 / `0x035C`).
pub fn dspre_edited_db_suggests_followplat(user_scrcmd: &Map<String, Value>) -> bool {
    index_scrcmd_by_id(user_scrcmd)
        .keys()
        .copied()
        .max()
        .unwrap_or(0)
        >= 860
}

fn json_delta_one_line(v: Option<&Value>) -> String {
    match v {
        None => "<missing>".to_string(),
        Some(val) => match serde_json::to_string(val) {
            Ok(s) if s.len() > 100 => format!("{}…", &s[..100]),
            Ok(s) => s,
            Err(_) => "<unprintable>".to_string(),
        },
    }
}

fn eprint_structural_detail(id: u16, label: &str, vanilla: Option<&Value>, user: &Value) {
    if !dspre_merge_shape_differs(vanilla, user) {
        return;
    }
    eprintln!(
        "  structural 0x{:04X} {} — deltas vs baseline (will merge)",
        id, label
    );
    for field in shape_fields_v1() {
        let a = vanilla.and_then(|o| o.get(field));
        let b = user.get(field);
        if a == b {
            continue;
        }
        eprintln!(
            "    {}: {} → {}",
            field,
            json_delta_one_line(a),
            json_delta_one_line(b)
        );
    }
}

/// Map DSPRE/scrcmd v1 `parameter_types` strings onto v2 **`type`** strings on **`params`** (e.g. `u16`).
fn dspre_v1_parameter_type_to_v2(t: &str) -> &'static str {
    match t.trim().to_ascii_uppercase().as_str() {
        "BYTE" | "U8" | "UINT8" => "u8",
        "SHORT" | "USHORT" | "U16" | "INTEGER" | "SHORTINT" | "USHORTINT" => "u16",
        "INT" | "UINT" | "U32" | "LONG" => "u32",
        "FLAG" => "flag",
        "VAR" | "VARIABLE" | "DESTVARID" | "DEST_VAR_ID" => "var",
        "POINTER" | "LABEL" => "label",
        "SCRIPT" | "SCRIPT_ID" => "script_id",
        "MSG" | "MSG_ID" | "MESSAGE" | "MSGID" => "msg_id",
        "MOVEMENT" | "MOVEMENT_ID" => "movement_id",
        _ => "unknown",
    }
}

fn dspre_v1_numeric_placeholder_cell(slot: Option<&Value>) -> bool {
    matches!(slot, Some(Value::Number(_)))
}

/// Trimmed `parameter_types` entries from a v1 command object (for baseline-vs-edited comparisons).
fn dspre_v1_parameter_type_labels(cmd: Option<&Value>) -> Vec<String> {
    cmd.and_then(|c| c.get("parameter_types").and_then(Value::as_array))
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(|s| s.trim().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Builds v2 **`params`** from DSPRE **`parameters`** / **`parameter_types`**, zipped with **`dspre_zip_arity`**.
///
/// Numeric placeholders reuse **`existing_v2[i].name`** unless **`vanilla_v1`** shows that slot’s
/// **`parameter_types`** label changed vs the edited DB — then the edited label becomes the name
/// (DSPRE uses semantic labels like `OwMovementType` beside numeric cells).
fn build_v2_params_from_dspre_v1(
    cmd: &Value,
    existing_v2: &[Value],
    vanilla_v1: Option<&Value>,
) -> Vec<Value> {
    let cells = cmd
        .get("parameters")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let types = cmd
        .get("parameter_types")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let n = dspre_zip_arity(&cells, &types);
    let vanilla_type_labels = dspre_v1_parameter_type_labels(vanilla_v1);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let ty_mapped = types
            .get(i)
            .and_then(Value::as_str)
            .map_or("unknown", dspre_v1_parameter_type_to_v2);

        let ex = existing_v2.get(i).and_then(Value::as_object);
        let cell_opt = cells.get(i);
        let placeholder_or_missing_cell =
            dspre_v1_numeric_placeholder_cell(cell_opt) || cell_opt.is_none();

        let user_ty_label = types.get(i).and_then(Value::as_str).map_or("", str::trim);
        let rename_from_edited_type_label = vanilla_v1.is_some()
            && vanilla_type_labels.len() > i
            && placeholder_or_missing_cell
            && !user_ty_label.is_empty()
            && user_ty_label != vanilla_type_labels.get(i).map_or("", String::as_str);

        let name = if rename_from_edited_type_label {
            user_ty_label.to_string()
        } else {
            ex.and_then(|o| {
                if placeholder_or_missing_cell {
                    o.get("name")
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                } else {
                    cell_opt
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .or_else(|| {
                            o.get("name")
                                .and_then(Value::as_str)
                                .map(ToString::to_string)
                        })
                }
            })
            .or_else(|| {
                cell_opt.and_then(|v| match v {
                    Value::String(s) => {
                        let t = s.trim();
                        (!t.is_empty()).then(|| t.to_string())
                    }
                    _ => None,
                })
            })
            .unwrap_or_else(|| format!("arg_{}", i + 1))
        };

        let ty_json = if ty_mapped == "unknown" {
            ex.and_then(|o| o.get("type"))
                .cloned()
                .unwrap_or_else(|| Value::String("unknown".into()))
        } else {
            Value::String(ty_mapped.into())
        };

        let mut m = Map::with_capacity(4);
        m.insert("name".into(), Value::String(name));
        m.insert("type".into(), ty_json);

        if let Some(d) = ex.and_then(|o| o.get("default")) {
            m.insert("default".into(), d.clone());
        }
        if let Some(o) = ex
            .and_then(|e| e.get("optional"))
            .and_then(Value::as_bool)
            .filter(|&b| b)
        {
            m.insert("optional".into(), Value::Bool(o));
        }
        if let Some(c) = ex.and_then(|e| e.get("const")).filter(|c| !c.is_null()) {
            m.insert("const".into(), c.clone());
        }

        out.push(Value::Object(m));
    }
    out
}

fn find_script_cmd_bucket_key(doc: &Value, id: u16) -> Option<String> {
    let commands = doc.get("commands").and_then(Value::as_object)?;
    let mut matches: Vec<(&String, &str)> = Vec::new();
    for (k, cmd) in commands {
        let Some(ty) = cmd.get("type").and_then(Value::as_str) else {
            continue;
        };
        if ty != "script_cmd" && ty != "levelscript_cmd" {
            continue;
        }
        let Some(cid) = cmd.get("id").and_then(Value::as_u64) else {
            continue;
        };
        if cid == u64::from(id) {
            matches.push((k, ty));
        }
    }
    if matches.is_empty() {
        return None;
    }
    if matches.len() > 1
        && let Some((k, _)) = matches.iter().find(|(_, t)| *t == "script_cmd")
    {
        return Some((*k).clone());
    }
    Some(matches[0].0.clone())
}

/// Applies one edited v1 entry onto the matching **`script_cmd` / `levelscript_cmd`** in `doc`; keeps **`notes`** and object key intact.
///
/// **`vanilla_v1`** is the baseline command at the same opcode (from fetched vanilla JSON); used so placeholder slots adopt renamed **`parameter_types`** labels when only DSPRE’s type labels changed.
///
/// Drops **`variants`** and **`expansion`** so emitted shape matches DSPRE flat v1.
fn patch_script_cmd_from_v1_user_shape(
    doc: &mut Value,
    user_v1: &Value,
    id: u16,
    vanilla_v1: Option<&Value>,
) -> bool {
    let Some(key) = find_script_cmd_bucket_key(doc, id) else {
        return false;
    };
    let Some(entry) = doc
        .get_mut("commands")
        .and_then(|c| c.get_mut(key))
        .and_then(Value::as_object_mut)
    else {
        return false;
    };

    let name = user_v1
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_string);
    if let Some(n) = name {
        entry.insert("legacy_name".into(), Value::String(n));
    }

    let desc = user_v1
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_string);
    match desc {
        Some(d) => {
            entry.insert("description".into(), Value::String(d));
        }
        None => {
            entry.insert("description".into(), Value::String(String::new()));
        }
    }

    let existing_params_v2 = entry
        .get("params")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    entry.insert(
        "params".into(),
        Value::Array(build_v2_params_from_dspre_v1(
            user_v1,
            &existing_params_v2,
            vanilla_v1,
        )),
    );

    entry.remove("variants");
    entry.remove("expansion");
    true
}

fn patch_v2_descriptions(doc: &mut Value, patches: &HashMap<u16, String>) -> usize {
    let Some(commands) = doc.get_mut("commands").and_then(Value::as_object_mut) else {
        return 0;
    };
    let mut applied = 0usize;
    for cmd in commands.values_mut() {
        let Some(obj) = cmd.as_object_mut() else {
            continue;
        };
        let Some(id64) = obj.get("id").and_then(Value::as_u64) else {
            continue;
        };
        if id64 > u64::from(u16::MAX) {
            continue;
        }
        let id = id64 as u16;
        if let Some(desc) = patches.get(&id) {
            obj.insert("description".into(), Value::String(desc.clone()));
            applied += 1;
        }
    }
    applied
}

/// Compares the DSPRE-edited **v1** **`scrcmd`** to **`vanilla_opt`** and after **`[Y/n]`** may patch **`legacy_name`**,
/// **`description`**, and **`params`** in **`v2_path`** (keeps **`notes`**); no write when **`dry_run`**,
/// **`non_interactive`**, or stdin is not a terminal. `user_path_hint` is a pre-resolved path from the
/// caller (avoids a repeated directory scan); falls back to [`find_local_scrcmd_v1_path`] when `None`.
#[allow(clippy::too_many_lines)] // Single interactive migration path; extract when a third caller appears.
pub fn maybe_reconcile_scrcmd_v1_into_v2(
    root: &Path,
    config: &RotomConfig,
    vanilla_opt: Option<&VanillaScrcmdV1>,
    family: GameFamily,
    v2_path: &Path,
    options: ConvertOptions,
    user_path_hint: Option<&Path>,
) -> Result<bool> {
    let Some(vanilla) = vanilla_opt else {
        return Ok(false);
    };
    let Some(user_path) = user_path_hint
        .map(Path::to_path_buf)
        .or_else(|| find_local_scrcmd_v1_path(root, family, config))
    else {
        return Ok(false);
    };

    let vanilla_root: Value = serde_json::from_str(&vanilla.json)
        .map_err(|e| ProjectError::ScrcmdBaseline(format!("vanilla JSON parse: {e}")))?;
    let Some(vanilla_scrcmd_obj) = vanilla_root
        .get("scrcmd")
        .and_then(Value::as_object)
        .cloned()
    else {
        return Err(ProjectError::ScrcmdBaseline(
            "vanilla JSON missing `scrcmd`".into(),
        ));
    };

    let user_s = match fs::read_to_string(&user_path) {
        Ok(text) => text,
        Err(source) => {
            return Err(ProjectError::Io {
                action: "read local scrcmd v1 JSON",
                path: user_path,
                source,
            });
        }
    };
    let user_raw: Value =
        serde_json::from_str(&user_s).map_err(|source| ProjectError::SerializeJson { source })?;
    let Some(user_scrcmd) = user_raw.get("scrcmd").and_then(Value::as_object).cloned() else {
        return Err(ProjectError::ScrcmdBaseline(format!(
            "local scrcmd v1 at '{}' is missing top-level `scrcmd` key",
            user_path.display()
        )));
    };

    let rows = classify(Some(&vanilla_scrcmd_obj), &user_scrcmd);
    if !rows
        .iter()
        .any(|r| r.structural_change || r.user_only_entry || r.merged_description.is_some())
    {
        return Ok(false);
    }

    eprintln!(
        "edited scrcmd v1 ({}) vs {} @{}",
        user_path.display(),
        vanilla.repo_path,
        &vanilla.commit_sha[..7.min(vanilla.commit_sha.len())],
    );
    print_summary(&rows);

    let v_by_id = scrcmd_map_by_command_id(&vanilla_scrcmd_obj);
    let u_by_id = scrcmd_map_by_command_id(&user_scrcmd);

    let structural_merge_ids: HashSet<u16> = rows
        .iter()
        .filter_map(|r| {
            if r.user_only_entry || !r.structural_change {
                return None;
            }
            let u_cmd = u_by_id.get(&r.id)?;
            dspre_merge_shape_differs(v_by_id.get(&r.id).copied(), u_cmd).then_some(r.id)
        })
        .collect();

    let structural_visual = rows
        .iter()
        .filter(|r| r.structural_change && !r.user_only_entry)
        .count();
    if structural_visual > structural_merge_ids.len() {
        let n = structural_visual - structural_merge_ids.len();
        eprintln!(
            "Note: {} structural row(s) only differ in display-only DSPRE noise (typically `parameter_values` or dangling `parameter_types`); skipping v2 patch for those.",
            n,
        );
    }

    let mut printed_struct_detail = false;
    let mut structural_detailed = 0usize;
    for r in &rows {
        if !r.structural_change
            || r.user_only_entry
            || structural_detailed >= STRUCTURAL_DELTA_PRINT_MAX
        {
            continue;
        }
        let label = r.label.as_deref().unwrap_or("?");
        let Some(shape) = u_by_id.get(&r.id) else {
            continue;
        };
        if !dspre_merge_shape_differs(v_by_id.get(&r.id).copied(), shape) {
            continue;
        }
        if !printed_struct_detail {
            printed_struct_detail = true;
            eprintln!(
                "Structural deltas (eligible for v2 merge; parameter_values quirks ignored for merge gates):",
            );
        }
        eprint_structural_detail(r.id, label, v_by_id.get(&r.id).copied(), shape);
        structural_detailed += 1;
    }
    if printed_struct_detail && structural_merge_ids.len() > structural_detailed {
        let rest = structural_merge_ids.len() - structural_detailed;
        eprintln!("  … {} more merged id(s) not summarized line-by-line", rest);
    }

    let n_desc_patch = rows
        .iter()
        .filter(|r| r.merged_description.is_some() && !structural_merge_ids.contains(&r.id))
        .count();
    let n_struct_patch = structural_merge_ids.len();

    if options.dry_run {
        if n_desc_patch + n_struct_patch > 0 {
            eprintln!(
                "dry-run: skipping {} description merge(s) and {} structural v2 patch(es) → {}",
                n_desc_patch,
                n_struct_patch,
                v2_path.display()
            );
        }
        return Ok(false);
    }

    if n_desc_patch == 0 && n_struct_patch == 0 {
        if rows.iter().any(|r| r.user_only_entry) {
            eprintln!(
                "Skipping v2 database merge: edited DB has {} id(s) that are absent from baseline (user-only additions).",
                rows.iter().filter(|r| r.user_only_entry).count(),
            );
        }
        return Ok(false);
    }

    if options.non_interactive || !std::io::stdin().is_terminal() {
        eprintln!(
            "non-interactive: skipping {} description merge(s) and {} structural v2 patch(es) → {}",
            n_desc_patch,
            n_struct_patch,
            v2_path.display()
        );
        return Ok(false);
    }

    print!(
        "Apply {n_desc_patch} description merge(s) and {n_struct_patch} structural patch(es) onto {}?\nStructural patches overwrite **legacy_name**, **description**, and **params** from edited scrcmd v1; Rotom preserves v2 extras such as **notes**. [Y/n] ",
        v2_path.display()
    );
    std::io::stdout()
        .flush()
        .map_err(|source| ProjectError::StdIo {
            action: "flush stdout",
            source,
        })?;

    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|source| ProjectError::StdIo {
            action: "read stdin",
            source,
        })?;
    let answer = line.trim().to_ascii_lowercase();
    if !(answer.is_empty() || answer == "y" || answer == "yes") {
        eprintln!("skipping v2 database merge");
        return Ok(false);
    }

    let patches: HashMap<u16, String> = rows
        .iter()
        .filter(|r| r.merged_description.is_some() && !structural_merge_ids.contains(&r.id))
        .filter_map(|r| r.merged_description.clone().map(|d| (r.id, d)))
        .collect();

    let raw = std::fs::read_to_string(v2_path).map_err(|source| ProjectError::Io {
        action: "read v2 database",
        path: v2_path.to_path_buf(),
        source,
    })?;
    let mut doc: Value =
        serde_json::from_str(&raw).map_err(|source| ProjectError::SerializeJson { source })?;
    let n_desc_done = patch_v2_descriptions(&mut doc, &patches);

    let mut n_struct_done = 0usize;
    let mut n_struct_miss = 0usize;
    for id in structural_merge_ids {
        let Some(user_shape) = u_by_id.get(&id).copied() else {
            continue;
        };
        if patch_script_cmd_from_v1_user_shape(&mut doc, user_shape, id, v_by_id.get(&id).copied())
        {
            n_struct_done += 1;
        } else {
            n_struct_miss += 1;
        }
    }
    if n_struct_miss > 0 {
        eprintln!(
            "{n_struct_miss} structural id(s) matched no {{script_cmd, levelscript_cmd}} entry in {}",
            v2_path.display()
        );
    }

    if n_desc_done == 0 && n_struct_done == 0 {
        return Ok(false);
    }
    let out =
        serde_json::to_vec_pretty(&doc).map_err(|source| ProjectError::SerializeJson { source })?;

    let dir = v2_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir).map_err(|source| ProjectError::Io {
        action: "create database directory",
        path: dir.to_path_buf(),
        source,
    })?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = dir.join(format!(
        "{}.{stamp}.tmp",
        v2_path.file_name().and_then(|s| s.to_str()).unwrap_or("db"),
    ));
    std::fs::write(&tmp, &out).map_err(|source| ProjectError::Io {
        action: "write v2 database temp file",
        path: tmp.clone(),
        source,
    })?;
    std::fs::rename(&tmp, v2_path).map_err(|source| ProjectError::Io {
        action: "commit v2 database",
        path: v2_path.to_path_buf(),
        source,
    })?;

    eprintln!(
        "wrote {} description merge(s), {} structural patch(es) → {}",
        n_desc_done,
        n_struct_done,
        v2_path.display()
    );
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const V_MSG: &str = r#"{"scrcmd":{"0x0002":{"name":"End","parameters":[],"parameter_types":[],"parameter_values":[],"description":"vanilla"}}}"#;

    #[test]
    fn detects_description_only_candidate() {
        let vanilla: Value = serde_json::from_str(V_MSG).unwrap();
        let mut user = vanilla.clone();
        user["scrcmd"]["0x0002"]["description"] = Value::String("user note".into());
        let vc = vanilla["scrcmd"].as_object().unwrap().get("0x0002");
        let u = user["scrcmd"].as_object().unwrap().get("0x0002").unwrap();
        assert_eq!(
            description_merge_candidate(vc, u).as_deref(),
            Some("user note")
        );
    }

    #[test]
    fn rejects_when_parameters_differ() {
        let vanilla: Value = serde_json::from_str(V_MSG).unwrap();
        let mut user = vanilla.clone();
        user["scrcmd"]["0x0002"]["parameters"] = serde_json::json!([1]);
        let vc = vanilla["scrcmd"].as_object().unwrap().get("0x0002");
        let u = user["scrcmd"].as_object().unwrap().get("0x0002").unwrap();
        assert!(description_merge_candidate(vc, u).is_none());
    }

    #[test]
    fn patches_v2_by_command_id_json() {
        let mut doc = serde_json::json!({
            "commands": {
                "End": { "type": "script_cmd", "id": 2, "description": "old" }
            }
        });
        let mut m = HashMap::new();
        m.insert(2, "fresh".into());
        m.insert(999, "no".into());
        patch_v2_descriptions(&mut doc, &m);
        assert_eq!(
            doc["commands"]["End"]["description"],
            serde_json::json!("fresh")
        );
    }

    #[test]
    fn dspre_workspace_dir_basename_for_rotom_strips_export_suffix() {
        assert_eq!(
            dspre_workspace_dir_basename_for_rotom("PokemonPlatinumUSA_DSPRE_contents"),
            "PokemonPlatinumUSA"
        );
        assert_eq!(
            dspre_workspace_dir_basename_for_rotom("JustAName"),
            "JustAName"
        );
        assert_eq!(
            dspre_workspace_dir_basename_for_rotom("_DSPRE_contents"),
            "_DSPRE_contents"
        );
    }

    #[test]
    fn dspre_hints_include_name_without_dspre_contents_suffix() {
        use crate::project::config::{
            PathsConfig, ProjectMetadata, ProjectTypeConfig, RotomConfig, WorkspaceConfig,
        };
        let config = RotomConfig {
            format_version: 1,
            project: ProjectMetadata {
                name: String::new(),
            },
            workspace: WorkspaceConfig {
                project_type: ProjectTypeConfig::Dspre,
                game_family: None,
            },
            paths: PathsConfig {
                database_dir: ".rotom/command_database".to_string(),
                cache_dir: ".rotom/cache".to_string(),
                status_dir: ".rotom/status".to_string(),
                source_roots: Vec::new(),
                include_roots: Vec::new(),
                binary_roots: Vec::new(),
            },
            database: None,
        };
        let root = Path::new("/tmp/PokemonPlatinumUSA_DSPRE_contents");
        let h = dspre_edited_database_project_hints(root, &config);
        assert_eq!(
            h,
            vec![
                "PokemonPlatinumUSA_DSPRE_contents".to_string(),
                "PokemonPlatinumUSA".to_string(),
            ]
        );
    }

    #[test]
    fn structural_patch_strips_params_keeps_notes() {
        let mut doc = serde_json::json!({
            "commands": {
                "Dummy088": {
                    "type": "script_cmd",
                    "id": 136,
                    "legacy_name": "DummyTakeTrap",
                    "description": "old",
                    "params": [{ "name": "x", "type": "u16" }],
                    "notes": "keep me",
                    "variants": []
                }
            }
        });
        let user_v1 = serde_json::json!({
            "name": "DummyTakeTrap",
            "parameters": [],
            "parameter_types": [],
            "parameter_values": [],
            "description": "Nothing",
        });
        assert!(patch_script_cmd_from_v1_user_shape(
            &mut doc, &user_v1, 136, None
        ));
        let dummy = doc["commands"]["Dummy088"].as_object().unwrap();
        assert_eq!(dummy["legacy_name"], "DummyTakeTrap");
        assert_eq!(dummy["description"], "Nothing");
        assert_eq!(dummy["params"], serde_json::json!([]));
        assert_eq!(dummy["notes"], "keep me");
        assert!(dummy.get("variants").is_none());
    }

    #[test]
    fn dspre_zip_length_mismatch_truncates_lists() {
        let placeholders: Vec<Value> = vec![serde_json::json!(2); 7];
        let t7 = vec![serde_json::json!("Integer"); 7];
        assert_eq!(dspre_zip_arity(&placeholders, &t7), 7);
        let t8 = vec![serde_json::json!("Integer"); 8];
        assert_eq!(dspre_zip_arity(&placeholders, &t8), 7);
    }

    #[test]
    #[allow(clippy::redundant_clone)] // `json!` needs owned `Vec` for both branches without duplicating the literal.
    fn dspre_extra_trailing_type_is_ignored_for_merge_gate() {
        let placeholders = vec![serde_json::json!(2); 7];
        let t7 = vec![serde_json::json!("Integer"); 7];
        let t8 = vec![serde_json::json!("Integer"); 8];
        let vanilla = serde_json::json!({
            "parameters": placeholders.clone(),
            "parameter_types": t7.clone(),
            "parameter_values": []
        });
        let user = serde_json::json!({
            "parameters": placeholders,
            "parameter_types": t8,
            "parameter_values": [],
        });
        assert!(!dspre_merge_shape_differs(Some(&vanilla), &user));
        let truncated = serde_json::json!({
            "parameters": [],
            "parameter_types": [],
            "parameter_values": [],
        });
        assert!(dspre_merge_shape_differs(Some(&vanilla), &truncated));
    }

    #[test]
    fn build_params_uses_edited_parameter_types_label_when_baseline_label_changed() {
        let existing = vec![
            serde_json::json!({"name": "event_id", "type": "u16"}),
            serde_json::json!({"name": "movement", "type": "u16"}),
        ];
        let vanilla_v1 = serde_json::json!({
            "parameters": [2, 2],
            "parameter_types": ["Overworld", "Action"],
            "parameter_values": [],
        });
        let user_v1 = serde_json::json!({
            "parameters": [2, 2],
            "parameter_types": ["Overworld", "OwMovementType"],
            "parameter_values": [],
        });
        let merged = build_v2_params_from_dspre_v1(&user_v1, &existing, Some(&vanilla_v1));
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0]["name"], serde_json::json!("event_id"));
        assert_eq!(merged[1]["name"], serde_json::json!("OwMovementType"));
    }

    #[test]
    fn patch_set_movement_type_renames_second_param_from_dspre_type_label() {
        let mut doc = serde_json::json!({
            "commands": {
                "SetMovementType": {
                    "type": "script_cmd",
                    "id": 109,
                    "legacy_name": "SetOWMovement",
                    "description": "x",
                    "params": [
                        {"name": "event_id", "type": "u16"},
                        {"name": "movement", "type": "u16"}
                    ],
                    "notes": "keep"
                }
            }
        });
        let vanilla_v1 = serde_json::json!({
            "name": "SetOWMovement",
            "parameters": [2, 2],
            "parameter_types": ["Overworld", "Action"],
            "parameter_values": [],
            "description": "",
        });
        let user_v1 = serde_json::json!({
            "name": "SetOWMovement",
            "parameters": [2, 2],
            "parameter_types": ["Overworld", "OwMovementType"],
            "parameter_values": [],
            "description": "",
        });
        assert!(patch_script_cmd_from_v1_user_shape(
            &mut doc,
            &user_v1,
            109,
            Some(&vanilla_v1),
        ));
        let p = doc["commands"]["SetMovementType"]["params"]
            .as_array()
            .unwrap();
        assert_eq!(p[0]["name"], "event_id");
        assert_eq!(p[1]["name"], "OwMovementType");
        assert_eq!(
            doc["commands"]["SetMovementType"]["notes"],
            serde_json::json!("keep")
        );
    }

    #[test]
    fn build_params_keeps_existing_names_for_numeric_dspre_cells() {
        let existing = vec![
            serde_json::json!({"name": "destVar", "type": "var", "default": "VAR_RESULT"}),
            serde_json::json!({"name": "arg_alpha", "type": "u16"}),
            serde_json::json!({"name": "last", "type": "unknown"}),
        ];
        let user = serde_json::json!({
            "parameters": [2, 2, 2],
            "parameter_types": ["Integer", "Integer", "Integer"],
            "parameter_values": [],
        });
        let merged = build_v2_params_from_dspre_v1(&user, &existing, None);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0]["name"], serde_json::json!("destVar"));
        assert_eq!(merged[1]["type"], serde_json::json!("u16"));
        assert_eq!(merged[1]["name"], serde_json::json!("arg_alpha"));
        assert_eq!(merged[0]["default"], serde_json::json!("VAR_RESULT"));
        assert_eq!(merged[2]["name"], serde_json::json!("last"));
    }

    #[test]
    fn classification_ignores_parameter_values_only_typo() {
        let van = serde_json::json!({
            "parameters": [1],
            "parameter_types": ["SHORT"],
            "parameter_values": ["a"],
        });
        let mut user = van.clone();
        user["parameter_values"] = serde_json::json!(["b"]);
        assert_eq!(
            without_description_and_parameter_values(&van),
            without_description_and_parameter_values(&user)
        );
    }

    #[test]
    #[allow(clippy::redundant_clone)] // Temporary copy to mutate one entry for the mismatch case.
    fn dspre_merge_shape_diff_score_counts_intersection_only() {
        let user = serde_json::json!({
            "0x0002": { "name": "A", "parameters": [], "parameter_types": [], "parameter_values": [] },
            "0x035C": { "name": "Extra", "parameters": [], "parameter_types": [], "parameter_values": [] },
        })
        .as_object()
        .unwrap()
        .clone();
        let stock_like = serde_json::json!({
            "0x0002": { "name": "A", "parameters": [], "parameter_types": [], "parameter_values": [] },
        })
        .as_object()
        .unwrap()
        .clone();
        assert_eq!(dspre_merge_shape_diff_score(&user, &stock_like), 0);

        let mut stock_diff = stock_like.clone();
        stock_diff.get_mut("0x0002").unwrap()["name"] = serde_json::json!("B");
        assert_eq!(dspre_merge_shape_diff_score(&user, &stock_diff), 1);
    }

    #[test]
    fn dspre_edited_db_suggests_followplat_max_opcode() {
        let small = serde_json::json!({
            "0x0100": { "name": "x", "parameters": [], "parameter_types": [], "parameter_values": [] },
        })
        .as_object()
        .unwrap()
        .clone();
        assert!(!dspre_edited_db_suggests_followplat(&small));

        let big = serde_json::json!({
            "0x035C": { "name": "x", "parameters": [], "parameter_types": [], "parameter_values": [] },
        })
        .as_object()
        .unwrap()
        .clone();
        assert!(dspre_edited_db_suggests_followplat(&big));
    }

    #[cfg(unix)]
    #[test]
    fn dspre_wine_roaming_scrcmd_path_shape() {
        let p = dspre_roaming_edited_scrcmd_json(
            Path::new("/tmp/wpfx"),
            "dspreuser",
            "MyRom_DSPRE_contents",
        );
        let s = p.to_string_lossy();
        assert!(
            s.ends_with(
                "/drive_c/users/dspreuser/AppData/Roaming/DSPRE/databases/edited_databases/MyRom_DSPRE_contents/scrcmd_database.json",
            ),
            "unexpected path: {}",
            p.display()
        );
    }
}
