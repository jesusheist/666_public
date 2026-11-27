//! MCP over JSON-RPC 2.0: `initialize`, `ping`, `tools/list`, `tools/call`,
//! and the notifications a client sends.

use crate::badpix::{DEFAULT_MAX_DRAWS, MAX_DRAWS_LIMIT, ScanParams};
use crate::json::{self, Json};
use crate::{KINDS, Shared, memscan, remote_pad};

pub const PROTOCOL_VERSION: &str = "2025-06-18";

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// What a POST body gets back.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// 200 with this JSON-RPC response.
    Response(Json),
    /// 202 with no body (a notification, or a response from the client).
    Accepted,
    /// A status and the JSON-RPC error that goes with it.
    Error(u16, Json),
}

struct RpcError {
    code: i64,
    message: String,
}

fn rpc_error(code: i64, message: impl Into<String>) -> RpcError {
    RpcError { code, message: message.into() }
}

fn error_body(id: Json, code: i64, message: &str) -> Json {
    Json::obj([
        ("jsonrpc", Json::str("2.0")),
        ("id", id),
        ("error", Json::obj([("code", Json::Int(code)), ("message", Json::str(message))])),
    ])
}

/// Handles one POST body.
pub fn handle_body(shared: &Shared, body: &[u8]) -> Outcome {
    let message = match json::parse(body) {
        Ok(m) => m,
        Err(e) => return Outcome::Error(400, error_body(Json::Null, PARSE_ERROR, &format!("Parse error: {e}"))),
    };
    if !message.is_obj() {
        return Outcome::Error(
            400,
            error_body(Json::Null, INVALID_REQUEST, "Invalid Request: send one JSON-RPC object (batches are not served)"),
        );
    }
    let id = match message.get("id") {
        None => None,
        Some(id @ (Json::Int(_) | Json::Str(_) | Json::Num(_))) => Some(id.clone()),
        Some(_) => {
            return Outcome::Error(400, error_body(Json::Null, INVALID_REQUEST, "Invalid Request: id must be a number or a string"));
        }
    };
    if message.get("jsonrpc").and_then(Json::as_str) != Some("2.0") {
        return Outcome::Error(400, error_body(id.unwrap_or(Json::Null), INVALID_REQUEST, "Invalid Request: jsonrpc must be \"2.0\""));
    }
    let Some(method) = message.get("method").and_then(Json::as_str) else {
        // A response to something we never asked: nothing to do.
        if id.is_some() && (message.get("result").is_some() || message.get("error").is_some()) {
            return Outcome::Accepted;
        }
        return Outcome::Error(400, error_body(id.unwrap_or(Json::Null), INVALID_REQUEST, "Invalid Request: method is missing"));
    };
    let Some(id) = id else {
        // A notification (notifications/initialized, notifications/cancelled, ...).
        return Outcome::Accepted;
    };
    let params = message.get("params");
    match dispatch(shared, method, params) {
        Ok(result) => Outcome::Response(Json::obj([("jsonrpc", Json::str("2.0")), ("id", id), ("result", result)])),
        Err(e) => Outcome::Response(error_body(id, e.code, &e.message)),
    }
}

fn dispatch(shared: &Shared, method: &str, params: Option<&Json>) -> Result<Json, RpcError> {
    match method {
        "initialize" => initialize(params),
        "ping" => Ok(Json::Obj(Vec::new())),
        "tools/list" => Ok(Json::obj([("tools", tools())])),
        "tools/call" => call(shared, params),
        _ => Err(rpc_error(METHOD_NOT_FOUND, format!("Method not found: {method}"))),
    }
}

fn initialize(params: Option<&Json>) -> Result<Json, RpcError> {
    let params = params.filter(|p| p.is_obj()).ok_or_else(|| rpc_error(INVALID_PARAMS, "initialize needs params"))?;
    if params.get("protocolVersion").and_then(Json::as_str).is_none() {
        return Err(rpc_error(INVALID_PARAMS, "initialize needs params.protocolVersion (a string)"));
    }
    Ok(Json::obj([
        ("protocolVersion", Json::str(PROTOCOL_VERSION)),
        ("capabilities", Json::obj([("tools", Json::obj([("listChanged", Json::Bool(false))]))])),
        ("serverInfo", Json::obj([("name", Json::str("victory-debug")), ("version", Json::str(env!("CARGO_PKG_VERSION")))])),
        (
            "instructions",
            Json::str(
                "Read-only debug tools of a running VIctory game: frame meter values, recent log lines, shader and pipeline \
                 events, the device-lost report, a screenshot written to disk, and a one-frame scan for NaN and Inf pixels.",
            ),
        ),
    ]))
}

fn schema(properties: Vec<(&str, Json)>) -> Json {
    Json::obj([
        ("type", Json::str("object")),
        ("properties", Json::Obj(properties.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())),
        ("additionalProperties", Json::Bool(false)),
    ])
}

fn tool(name: &str, title: &str, description: &str, input: Json) -> Json {
    tool_with(name, title, description, input, true)
}

fn tool_with(name: &str, title: &str, description: &str, input: Json, read_only: bool) -> Json {
    Json::obj([
        ("name", Json::str(name)),
        ("title", Json::str(title)),
        ("description", Json::str(description)),
        ("inputSchema", input),
        ("annotations", Json::obj([("readOnlyHint", Json::Bool(read_only)), ("openWorldHint", Json::Bool(false))])),
    ])
}

fn count_schema(what: &str) -> Json {
    Json::obj([
        ("type", Json::str("integer")),
        ("minimum", Json::Int(1)),
        ("maximum", Json::Int(MAX_N as i64)),
        ("default", Json::Int(DEFAULT_N as i64)),
        ("description", Json::str(what)),
    ])
}

/// Largest pixel coordinate the scans take (the largest target is 16384).
const MAX_PIXEL: u32 = 16383;
const MAX_N: usize = 2000;
const DEFAULT_N: usize = 200;

/// The tools, as `tools/list` gives them.
pub fn tools() -> Json {
    Json::Arr(vec![
        tool(
            "get_stats",
            "Frame meter",
            "The frame meter's values of the last second: fps, ms per frame, ms preparing submissions and its parts, ms waiting \
             for the GPU, copy-back, draws, dropped draws, submissions per frame, the frame number and uptime. Per-frame values \
             are means over that second.",
            schema(vec![]),
        ),
        tool(
            "get_recent_log",
            "Recent log lines",
            "The last log lines (what the runtime writes to stderr), oldest first, from an in-memory ring of the newest 4096.",
            schema(vec![
                ("n", count_schema("How many lines at most (default 200, up to 2000).")),
                ("filter", Json::obj([("type", Json::str("string")), ("description", Json::str("Only lines that contain this text (case-sensitive)."))])),
            ]),
        ),
        tool(
            "get_pipeline_events",
            "Shader and pipeline events",
            "Recent shader and pipeline events, oldest first: a shader translated (hash, stage, ms), a pipeline's first bind (shader \
             hash, stage, frame), a pipeline loaded from or made into the host pipeline library (name hash, ms), and the library \
             file loaded.",
            schema(vec![
                ("n", count_schema("How many events at most (default 200, up to 2000).")),
                (
                    "kind",
                    Json::obj([
                        ("type", Json::str("string")),
                        ("enum", Json::Arr(KINDS.iter().map(|k| Json::str(*k)).collect())),
                        ("description", Json::str("Only events of this kind.")),
                    ]),
                ),
                ("hash", Json::obj([("type", Json::str("string")), ("description", Json::str("Only events whose hash contains this text."))])),
            ]),
        ),
        tool(
            "get_device_lost_report",
            "Device-lost report",
            "The full text of the report written when the host D3D12 device was lost (what the submitting thread was doing and the \
             last things done, newest last), kept for the life of the process; lost=false and report=null while the device is fine.",
            schema(vec![]),
        ),
        tool(
            "screenshot",
            "Screenshot",
            "Copies the next presented frame (the swap chain's back buffer, as 8-bit sRGB; 16-bit float frames get the sRGB curve) and \
             writes it as a PNG under K:\\Leonida\\avatarmanz\\shots. Returns the path and size. Waits up to 10 s for a present.",
            schema(vec![(
                "downscale",
                Json::obj([
                    ("type", Json::str("integer")),
                    ("enum", Json::Arr(vec![Json::Int(1), Json::Int(2), Json::Int(4)])),
                    ("default", Json::Int(1)),
                    ("description", Json::str("Reduce the picture by 1, 2 or 4 in each direction.")),
                ]),
            )]),
        ),
        tool(
            "get_memory",
            "GPU memory",
            "The card's memory as the system accounts it to this process (budget and use, own memory and the PC's shared memory),              the bytes the runtime holds by kind (host copies of textures the title fills, tiles, render targets, card copies of              title buffers, free blocks of the texture pool), the texture pool setting, and the eviction totals and last reason.",
            schema(vec![]),
        ),
        tool(
            "get_dropped_draws",
            "Dropped draws",
            "The draws, mesh dispatches, dispatches and indirect executions the host did not run in the last finished 5 s window: the top 20 (reason, pipeline shader hashes, count, first frame) and the totals per reason, and per reason since the start. Needs VICTORY_DROP_REPORT=1.",
            schema(vec![]),
        ),
        tool(
            "get_settings",
            "Live settings",
            "The settings that change while the game runs (texture_pool_mb, room_percent, evict): value, what each may be, and the              file they are kept in.",
            schema(vec![]),
        ),
        tool_with(
            "set_setting",
            "Change a live setting",
            "Sets one live setting now and keeps it in the live settings file: texture_pool_mb (0 or auto: as the runtime decides;              else megabytes, 1024 or more), room_percent (50 to 97), evict (on or off; off for good when VICTORY_EVICT=0).              Returns the settings after the change.",
            schema(vec![
                (
                    "name",
                    Json::obj([
                        ("type", Json::str("string")),
                        ("enum", Json::Arr(["texture_pool_mb", "room_percent", "evict"].map(Json::str).to_vec())),
                    ]),
                ),
                (
                    "value",
                    Json::obj([
                        ("type", Json::Arr(vec![Json::str("string"), Json::str("integer"), Json::str("boolean")])),
                        ("description", Json::str("The new value: a number, \"auto\", \"on\" or \"off\", true or false.")),
                    ]),
                ),
            ]),
            false,
        ),
        tool(
            "list_tunables",
            "Live tunables",
            "Every live tunable (the VICTORY_* switches that can change while the game runs): name, env variable, type, value, default, \
             range or allowed values, live class (safe: applies now; next_frame: applies at the next frame; restart: kept in the \
             live file, needs a restart), description, whether it differs from the default, where its value came from, a pending \
             value and whether a restart is needed. The optional filter keeps those whose name, variable or description contains it.",
            schema(vec![(
                "filter",
                Json::obj([("type", Json::str("string")), ("description", Json::str("Keep the tunables whose name, variable or description contains this text."))]),
            )]),
        ),
        tool(
            "get_tunable",
            "One live tunable",
            "One tunable by name (view_guard, or the variable VICTORY_VIEW_GUARD), as list_tunables shows it.",
            schema(vec![("name", Json::obj([("type", Json::str("string"))]))]),
        ),
        tool_with(
            "set_tunable",
            "Change a live tunable",
            "Sets one tunable and keeps it in the [tunables] table of the live settings file. Safe ones apply now, next_frame ones at the \
             next frame (the result shows the pending value), restart ones after a restart (needs_restart). Booleans take true, false, on, off; \
             numbers must be inside the range; enums one of the allowed values. Returns the tunable after the change and the outcome.",
            schema(vec![
                ("name", Json::obj([("type", Json::str("string"))])),
                (
                    "value",
                    Json::obj([
                        ("type", Json::Arr(vec![Json::str("string"), Json::str("number"), Json::str("boolean")])),
                        ("description", Json::str("The new value: a number, a boolean, or text (an enum choice, on or off).")),
                    ]),
                ),
            ]),
            false,
        ),
        tool_with(
            "reset_tunable",
            "Put a live tunable back to its default",
            "Sets one tunable back to its default (by its live class) and removes its line from the live settings file.",
            schema(vec![("name", Json::obj([("type", Json::str("string"))]))]),
            false,
        ),
        tool_with(
            "press",
            "Press pad buttons",
            "Holds one or more buttons of a virtual pad for hold_ms (default 120), with no window focus needed; the game sees the pad connected even with no physical pad. Buttons: A B X Y LB RB LT RT View Menu LS RS DpadUp DpadDown DpadLeft DpadRight (any case); join several with + (A+RT).",
            schema(vec![
                ("button", Json::obj([("type", Json::str("string")), ("description", Json::str("A button name, or several joined by +, such as A or LB+A."))])),
                ("hold_ms", Json::obj([("type", Json::str("integer")), ("minimum", Json::Int(0)), ("maximum", Json::Int(remote_pad::MAX_HOLD_MS as i64)), ("default", Json::Int(remote_pad::DEFAULT_HOLD_MS as i64))])),
            ]),
            false,
        ),
        tool_with(
            "stick",
            "Push a pad stick",
            "Pushes the left or right stick of the virtual pad to x,y (each -1 to 1; y positive is up) for hold_ms (default 120), then lets it go back to centre.",
            schema(vec![
                ("stick", Json::obj([("type", Json::str("string")), ("enum", Json::Arr(["left", "right"].map(Json::str).to_vec()))])),
                ("x", Json::obj([("type", Json::str("number")), ("minimum", Json::Int(-1)), ("maximum", Json::Int(1))])),
                ("y", Json::obj([("type", Json::str("number")), ("minimum", Json::Int(-1)), ("maximum", Json::Int(1))])),
                ("hold_ms", Json::obj([("type", Json::str("integer")), ("minimum", Json::Int(0)), ("maximum", Json::Int(remote_pad::MAX_HOLD_MS as i64)), ("default", Json::Int(remote_pad::DEFAULT_HOLD_MS as i64))])),
            ]),
            false,
        ),
        tool_with(
            "release_all",
            "Release the virtual pad",
            "Lets go of every button and stick of the virtual pad now, and stops a running input_script.",
            schema(vec![]),
            false,
        ),
        tool_with(
            "input_script",
            "Run a pad script",
            "Runs steps in order on the virtual pad and returns at once (the script runs in the background; release_all or a new script stops it). Steps are split by commas, semicolons or new lines: \"<button or buttons> [ms]\" (default 120), a stick direction \"LeftUp [ms]\" (Left or Right, then Up, Down, Left, Right; full push), or \"wait <ms>\". Each step lasts its time, then the next starts. Example: \"A 120, wait 500, LeftUp 2000\". At most 64 steps and 120 s.",
            schema(vec![("script", Json::obj([("type", Json::str("string"))]))]),
            false,
        ),
        tool(
            "find_bad_pixels",
            "Find the first draw that writes NaN or Inf",
            "Captures one frame (the one after the next present) and, after every draw and dispatch recorded in it, scans the \
             render targets (and with include_uav the texture UAVs) that command wrote, before and after it, for NaN and Inf \
             (float formats only). Reports per command: its index, the order it ran in, pipeline and shader hashes (as the log \
             prints them), target format and size, NaN and Inf pixel counts, the largest finite value, the first bad pixel and \
             its value, and whether the target was already bad before the command, so the first writer is named. With pixel it \
             also follows that screen pixel's value through the targets of the back buffer's size. The frame runs slowly while \
             captured. Writes the full report as JSON under K:\\Leonida\\avatarmanz\\shots (badpixels-<time>-f<frame>.json) \
             and returns the summary and the path. Waits up to 120 s.",
            schema(vec![
                (
                    "max_draws",
                    Json::obj([
                        ("type", Json::str("integer")),
                        ("minimum", Json::Int(1)),
                        ("maximum", Json::Int(i64::from(MAX_DRAWS_LIMIT))),
                        ("default", Json::Int(i64::from(DEFAULT_MAX_DRAWS))),
                        ("description", Json::str("Scan at most this many draws and dispatches; the rest are counted, not scanned.")),
                    ]),
                ),
                (
                    "pixel",
                    Json::obj([
                        ("type", Json::str("object")),
                        (
                            "properties",
                            Json::obj([
                                ("x", Json::obj([("type", Json::str("integer")), ("minimum", Json::Int(0)), ("maximum", Json::Int(i64::from(MAX_PIXEL)))])),
                                ("y", Json::obj([("type", Json::str("integer")), ("minimum", Json::Int(0)), ("maximum", Json::Int(i64::from(MAX_PIXEL)))])),
                            ]),
                        ),
                        ("required", Json::Arr(vec![Json::str("x"), Json::str("y")])),
                        ("additionalProperties", Json::Bool(false)),
                        (
                            "description",
                            Json::str("A screen pixel (in the back buffer's size) whose value is reported before and after every command that changes it."),
                        ),
                    ]),
                ),
                (
                    "include_uav",
                    Json::obj([
                        ("type", Json::str("boolean")),
                        ("default", Json::Bool(false)),
                        ("description", Json::str("Also scan the float textures that shaders write through unordered access views.")),
                    ]),
                ),
            ]),
        ),
        tool(
            "mem_regions",
            "Title memory ranges",
            "The title's committed, readable and writable memory ranges (what a memory scan covers): base, end, size, protection and kind \
             (xmem: a reservation the title made through the memory API; image <section>: a writable section of its image), and the total. \
             Our own runtime and GPU allocations are not listed. Read-only.",
            schema(vec![(
                "limit",
                Json::obj([
                    ("type", Json::str("integer")),
                    ("minimum", Json::Int(1)),
                    ("maximum", Json::Int(MAX_REGIONS as i64)),
                    ("default", Json::Int(DEFAULT_REGIONS as i64)),
                    ("description", Json::str("How many ranges to list at most, by address (the totals cover all).")),
                ]),
            )]),
        ),
        tool_with(
            "mem_scan_start",
            "Start a memory scan",
            "Snapshots every range mem_regions lists (copied into our own buffers; unreadable pages are skipped) and makes every aligned \
             address a candidate. Holds about 1.1 bytes per scanned byte; refuses above max_mb. May take seconds per GB. Replaces a scan \
             already held. Never writes title memory. Returns the candidate count and time.",
            schema(vec![
                (
                    "width",
                    Json::obj([
                        ("type", Json::str("integer")),
                        ("enum", Json::Arr(vec![Json::Int(1), Json::Int(2), Json::Int(4)])),
                        ("default", Json::Int(1)),
                        ("description", Json::str("Bytes per value: 1, 2 or 4 (candidates are the aligned addresses of that width).")),
                    ]),
                ),
                (
                    "max_mb",
                    Json::obj([
                        ("type", Json::str("integer")),
                        ("minimum", Json::Int(1)),
                        ("default", Json::Int(memscan::DEFAULT_MAX_MB as i64)),
                        ("description", Json::str("Refuse when the ranges hold more than this many megabytes.")),
                    ]),
                ),
            ]),
            false,
        ),
        tool_with(
            "mem_scan_filter",
            "Narrow a memory scan",
            "Reads the title's memory again and keeps the candidates whose value, compared with the last snapshot, matches the mode: \
             changed, unchanged, increased, decreased (unsigned unless signed is true), equals or not_equals (these two take value). \
             The survivors' snapshot values are refreshed, so each call compares with the previous one. Returns survivors and time.",
            schema(vec![
                (
                    "mode",
                    Json::obj([("type", Json::str("string")), ("enum", Json::Arr(memscan::Mode::NAMES.iter().map(|m| Json::str(*m)).collect()))]),
                ),
                (
                    "value",
                    Json::obj([
                        ("type", Json::Arr(vec![Json::str("integer"), Json::str("string")])),
                        ("description", Json::str("For equals and not_equals: a number, or text (0x for hex).")),
                    ]),
                ),
                ("signed", Json::obj([("type", Json::str("boolean")), ("default", Json::Bool(false))])),
            ]),
            false,
        ),
        tool(
            "mem_scan_list",
            "List memory scan survivors",
            "The surviving candidates by address: address, value now, the range (base, kind, offset) it lies in, and for an address in the \
             title's image its offset from the image base (image_offset, img+<hex>), which tells a static address from a heap one.",
            schema(vec![(
                "limit",
                Json::obj([("type", Json::str("integer")), ("minimum", Json::Int(1)), ("maximum", Json::Int(MAX_N as i64)), ("default", Json::Int(50))]),
            )]),
        ),
        tool(
            "mem_read",
            "Read title memory",
            "A hex dump of up to 4096 bytes of the title's memory (a safe read: it stops at the first page that cannot be read).",
            schema(vec![
                (
                    "address",
                    Json::obj([
                        ("type", Json::Arr(vec![Json::str("string"), Json::str("integer")])),
                        ("description", Json::str("The address: text (0x for hex; hex without a prefix too), or a whole number.")),
                    ]),
                ),
                ("len", Json::obj([("type", Json::str("integer")), ("minimum", Json::Int(1)), ("maximum", Json::Int(MAX_READ as i64)), ("default", Json::Int(64))])),
            ]),
        ),
        tool_with("mem_scan_reset", "Drop the memory scan", "Frees the scan's snapshot and candidates.", schema(vec![]), false),
    ])
}

const MAX_REGIONS: usize = 5000;
const DEFAULT_REGIONS: usize = 200;
const MAX_READ: usize = 4096;

fn mem_hooks() -> Result<&'static memscan::MemHooks, ToolError> {
    memscan::mem_hooks().ok_or_else(|| ToolError::Failed("the title's memory is not mapped yet: no memory tools until the title image is loaded".into()))
}

fn int_arg(args: &Json, key: &str, default: i64, min: i64, max: i64) -> Result<i64, ToolError> {
    match args.get(key) {
        None | Some(Json::Null) => Ok(default),
        Some(v) => match v.as_i64() {
            Some(n) if (min..=max).contains(&n) => Ok(n),
            _ => Err(ToolError::Args(format!("{key} must be an integer from {min} to {max}"))),
        },
    }
}

/// A number given as an integer or as text (`0x` hex, else decimal; with
/// `bare_hex`, text without a prefix is hex).
fn number_arg(args: &Json, key: &str, bare_hex: bool) -> Result<Option<u64>, ToolError> {
    let bad = || ToolError::Args(format!("{key} must be a whole number or text such as 0x1f"));
    match args.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Str(s)) => {
            let t = s.trim();
            let t = if bare_hex && !t.starts_with("0x") && !t.starts_with("0X") { format!("0x{t}") } else { t.to_owned() };
            memscan::parse_u64(&t).map(Some).ok_or_else(bad)
        }
        Some(v) => v.as_i64().filter(|n| *n >= 0).map(|n| Some(n as u64)).ok_or_else(bad),
    }
}

fn mem_regions_tool(args: &Json) -> Result<Json, ToolError> {
    let limit = int_arg(args, "limit", DEFAULT_REGIONS as i64, 1, MAX_REGIONS as i64)? as usize;
    let hooks = mem_hooks()?;
    let regions = (hooks.regions)();
    let total: u64 = regions.iter().map(|r| r.size).sum();
    Ok(Json::obj([
        ("count", Json::uint(regions.len() as u64)),
        ("total_bytes", Json::uint(total)),
        ("total_mb", Json::num(total as f64 / 1_048_576.0)),
        ("image_base", memscan::hex((hooks.image_base)())),
        ("shown", Json::uint(regions.len().min(limit) as u64)),
        ("regions", Json::Arr(regions.iter().take(limit).map(memscan::region_json).collect())),
    ]))
}

fn mem_scan_start(args: &Json) -> Result<Json, ToolError> {
    let width = int_arg(args, "width", 1, 1, 4)? as usize;
    if width == 3 {
        return Err(ToolError::Args("width must be 1, 2 or 4".into()));
    }
    let max_mb = int_arg(args, "max_mb", memscan::DEFAULT_MAX_MB as i64, 1, i64::MAX >> 21)? as u64;
    let hooks = mem_hooks()?;
    let regions = (hooks.regions)();
    let mut scan = memscan::scanner();
    let info = scan.start(&memscan::HookSource(hooks.read), regions, width, max_mb << 20).map_err(ToolError::Failed)?;
    Ok(Json::obj([
        ("width", Json::uint(width as u64)),
        ("regions", Json::uint(info.regions as u64)),
        ("scanned_bytes", Json::uint(info.bytes)),
        ("scanned_mb", Json::num(info.bytes as f64 / 1_048_576.0)),
        ("candidates", Json::uint(info.candidates)),
        ("held_mb", Json::num(info.held_bytes as f64 / 1_048_576.0)),
        ("ms", Json::num(info.ms)),
    ]))
}

fn mem_scan_filter(args: &Json) -> Result<Json, ToolError> {
    let mode = optional_str(args, "mode")?
        .and_then(memscan::Mode::parse)
        .ok_or_else(|| ToolError::Args(format!("mode is required: one of {}", memscan::Mode::NAMES.join(", "))))?;
    let value = number_arg(args, "value", false)?;
    if mode.needs_value() && value.is_none() {
        return Err(ToolError::Args("value is required for equals and not_equals".into()));
    }
    let signed = match args.get("signed") {
        None | Some(Json::Null) => false,
        Some(Json::Bool(b)) => *b,
        Some(_) => return Err(ToolError::Args("signed must be a boolean".into())),
    };
    let hooks = mem_hooks()?;
    let mut scan = memscan::scanner();
    let info = scan.filter(&memscan::HookSource(hooks.read), mode, value, signed).map_err(ToolError::Failed)?;
    Ok(Json::obj([
        ("before", Json::uint(info.before)),
        ("survivors", Json::uint(info.survivors)),
        ("unreadable_dropped", Json::uint(info.unreadable)),
        ("stored_as", Json::str(if info.sparse { "list" } else { "snapshot" })),
        ("held_mb", Json::num(info.held_bytes as f64 / 1_048_576.0)),
        ("ms", Json::num(info.ms)),
    ]))
}

fn mem_scan_list(args: &Json) -> Result<Json, ToolError> {
    let limit = int_arg(args, "limit", 50, 1, MAX_N as i64)? as usize;
    let hooks = mem_hooks()?;
    let scan = memscan::scanner();
    if !scan.active() {
        return Err(ToolError::Failed("no scan is running: call mem_scan_start first".into()));
    }
    let hits = scan.list(&memscan::HookSource(hooks.read), limit);
    let (width, image) = (scan.width(), (hooks.image_base)());
    Ok(Json::obj([
        ("survivors", Json::uint(scan.candidates())),
        ("returned", Json::uint(hits.len() as u64)),
        ("width", Json::uint(width as u64)),
        ("image_base", memscan::hex(image)),
        ("hits", Json::Arr(hits.iter().map(|h| memscan::hit_json(h, width, image)).collect())),
    ]))
}

fn mem_read_tool(args: &Json) -> Result<Json, ToolError> {
    let address = number_arg(args, "address", true)?.ok_or_else(|| ToolError::Args("address is required".into()))?;
    let len = int_arg(args, "len", 64, 1, MAX_READ as i64)? as usize;
    let hooks = mem_hooks()?;
    let bytes = memscan::read_prefix(&memscan::HookSource(hooks.read), address, len);
    if bytes.is_empty() {
        return Err(ToolError::Failed(format!("the memory at {address:#x} cannot be read")));
    }
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(Json::obj([
        ("address", memscan::hex(address)),
        ("requested", Json::uint(len as u64)),
        ("read", Json::uint(bytes.len() as u64)),
        ("hex", Json::Str(hex)),
        ("dump", Json::Str(memscan::hex_dump(address, &bytes))),
    ]))
}

/// A tool call that went wrong.
enum ToolError {
    /// The arguments are wrong: a protocol error (-32602).
    Args(String),
    /// The tool ran and failed: a result with isError.
    Failed(String),
}

fn call(shared: &Shared, params: Option<&Json>) -> Result<Json, RpcError> {
    let params = params.filter(|p| p.is_obj()).ok_or_else(|| rpc_error(INVALID_PARAMS, "tools/call needs params"))?;
    let name = params
        .get("name")
        .and_then(Json::as_str)
        .ok_or_else(|| rpc_error(INVALID_PARAMS, "tools/call needs params.name (a string)"))?;
    let empty = Json::Obj(Vec::new());
    let args = match params.get("arguments") {
        None | Some(Json::Null) => &empty,
        Some(a) if a.is_obj() => a,
        Some(_) => return Err(rpc_error(INVALID_PARAMS, "params.arguments must be an object")),
    };
    let done = match name {
        "get_stats" => Ok(stats(shared)),
        "get_recent_log" => recent_log(shared, args),
        "get_pipeline_events" => pipeline_events(shared, args),
        "get_device_lost_report" => Ok(device_lost(shared)),
        "screenshot" => screenshot(shared, args),
        "get_memory" => memory_tool(|h| (h.memory)()),
        "get_dropped_draws" => crate::drops_hook()
            .map(|hook| hook())
            .ok_or_else(|| ToolError::Failed("the dropped-draw report is off: start the game with VICTORY_DROP_REPORT=1".into())),
        "get_settings" => memory_tool(|h| (h.settings)()),
        "set_setting" => set_setting(args),
        "list_tunables" => tunables_tool(|h| Ok((h.list)(optional_str(args, "filter")?.unwrap_or("")))),
        "get_tunable" => tunables_tool(|h| (h.get)(required_name(args)?).map_err(ToolError::Failed)),
        "set_tunable" => tunables_tool(|h| (h.set)(required_name(args)?, &value_text(args, false)?).map_err(ToolError::Failed)),
        "reset_tunable" => tunables_tool(|h| (h.reset)(required_name(args)?).map_err(ToolError::Failed)),
        "find_bad_pixels" => find_bad_pixels(shared, args),
        "mem_regions" => mem_regions_tool(args),
        "mem_scan_start" => mem_scan_start(args),
        "mem_scan_filter" => mem_scan_filter(args),
        "mem_scan_list" => mem_scan_list(args),
        "mem_read" => mem_read_tool(args),
        "mem_scan_reset" => {
            memscan::scanner().reset();
            Ok(Json::obj([("reset", Json::Bool(true))]))
        }
        "press" => press_tool(args),
        "stick" => stick_tool(args),
        "release_all" => {
            remote_pad::release_all();
            Ok(Json::obj([("released", Json::Bool(true))]))
        }
        "input_script" => script_tool(args),
        _ => return Err(rpc_error(INVALID_PARAMS, format!("Unknown tool: {name}"))),
    };
    match done {
        Ok(structured) => Ok(Json::obj([
            ("content", Json::Arr(vec![Json::obj([("type", Json::str("text")), ("text", Json::Str(structured.text()))])])),
            ("structuredContent", structured),
            ("isError", Json::Bool(false)),
        ])),
        Err(ToolError::Args(why)) => Err(rpc_error(INVALID_PARAMS, why)),
        Err(ToolError::Failed(why)) => Ok(Json::obj([
            ("content", Json::Arr(vec![Json::obj([("type", Json::str("text")), ("text", Json::Str(why))])])),
            ("isError", Json::Bool(true)),
        ])),
    }
}

fn count(args: &Json) -> Result<usize, ToolError> {
    match args.get("n") {
        None | Some(Json::Null) => Ok(DEFAULT_N),
        Some(v) => match v.as_i64() {
            Some(n) if (1..=MAX_N as i64).contains(&n) => Ok(n as usize),
            _ => Err(ToolError::Args(format!("n must be an integer from 1 to {MAX_N}"))),
        },
    }
}

fn optional_str<'a>(args: &'a Json, key: &str) -> Result<Option<&'a str>, ToolError> {
    match args.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Str(s)) => Ok(Some(s)),
        Some(_) => Err(ToolError::Args(format!("{key} must be a string"))),
    }
}

fn stats(shared: &Shared) -> Json {
    let uptime = Json::num(shared.uptime_s());
    match shared.stats() {
        None => Json::obj([
            ("available", Json::Bool(false)),
            ("note", Json::str("no frame meter line yet: no frame was presented, or the second is not over")),
            ("uptime_s", uptime),
        ]),
        Some((s, age)) => Json::obj([
            ("available", Json::Bool(true)),
            ("frame", Json::uint(s.frame)),
            ("frames_in_window", Json::Int(i64::from(s.frames))),
            ("fps", Json::num(s.fps)),
            ("ms_per_frame", Json::num(s.ms_per_frame)),
            (
                "preparing",
                Json::obj([
                    ("ms", Json::num(s.preparing_ms)),
                    ("copies_ms", Json::num(s.copies_ms)),
                    ("copies_mb", Json::num(s.copies_mb)),
                    ("textures_ms", Json::num(s.textures_ms)),
                    ("textures_mb", Json::num(s.textures_mb)),
                ]),
            ),
            ("gpu_wait_ms", Json::num(s.gpu_ms)),
            ("copy_back_ms", Json::num(s.copy_back_ms)),
            ("copy_back_mb", Json::num(s.copy_back_mb)),
            ("draws_per_frame", Json::num(s.draws_per_frame)),
            ("dropped_per_frame", Json::num(s.dropped_per_frame)),
            ("indirect_per_frame", Json::num(s.indirect_per_frame)),
            ("submissions_per_frame", Json::num(s.submissions_per_frame)),
            ("uptime_s", uptime),
            ("age_ms", Json::num(age.as_secs_f64() * 1000.0)),
        ]),
    }
}

fn memory_tool(read: impl Fn(&crate::MemoryHooks) -> Json) -> Result<Json, ToolError> {
    crate::memory_hooks()
        .map(read)
        .ok_or_else(|| ToolError::Failed("the memory counters are not up yet: no frame was drawn".into()))
}

fn tunables_tool(run: impl Fn(&crate::TunableHooks) -> Result<Json, ToolError>) -> Result<Json, ToolError> {
    let hooks = crate::tunable_hooks().ok_or_else(|| ToolError::Failed("the tunables are not up yet: no frame was drawn".into()))?;
    run(hooks)
}

fn required_name(args: &Json) -> Result<&str, ToolError> {
    optional_str(args, "name")?.ok_or_else(|| ToolError::Args("name is required (a string)".into()))
}

/// The `value` argument as text. `whole`: numbers must be whole (the old settings take no fractions).
fn value_text(args: &Json, whole: bool) -> Result<String, ToolError> {
    Ok(match args.get("value") {
        Some(Json::Str(s)) => s.clone(),
        Some(Json::Int(i)) => i.to_string(),
        Some(Json::Bool(b)) => if *b { "on" } else { "off" }.to_owned(),
        Some(Json::Num(n)) if n.fract() == 0.0 && n.abs() < 1e15 => format!("{}", *n as i64),
        Some(Json::Num(n)) if !whole && n.is_finite() => format!("{n}"),
        _ => return Err(ToolError::Args("value is required (a number, a string or a boolean)".into())),
    })
}

fn hold_ms(args: &Json) -> Result<u64, ToolError> {
    match args.get("hold_ms") {
        None | Some(Json::Null) => Ok(remote_pad::DEFAULT_HOLD_MS),
        Some(v) => match v.as_i64() {
            Some(n) if (0..=remote_pad::MAX_HOLD_MS as i64).contains(&n) => Ok(n as u64),
            _ => Err(ToolError::Args(format!("hold_ms must be an integer from 0 to {}", remote_pad::MAX_HOLD_MS))),
        },
    }
}

fn axis(args: &Json, key: &str) -> Result<f32, ToolError> {
    match args.get(key) {
        None | Some(Json::Null) => Ok(0.0),
        Some(Json::Int(i)) if (-1..=1).contains(i) => Ok(*i as f32),
        Some(Json::Num(n)) if (-1.0..=1.0).contains(n) => Ok(*n as f32),
        Some(_) => Err(ToolError::Args(format!("{key} must be a number from -1 to 1"))),
    }
}

fn press_tool(args: &Json) -> Result<Json, ToolError> {
    let button = optional_str(args, "button")?.ok_or_else(|| ToolError::Args("button is required (a string)".into()))?;
    let ms = hold_ms(args)?;
    remote_pad::press(button, ms).map_err(ToolError::Args)?;
    Ok(Json::obj([("pressed", Json::str(button)), ("hold_ms", Json::uint(ms))]))
}

fn stick_tool(args: &Json) -> Result<Json, ToolError> {
    let side = match optional_str(args, "stick")? {
        Some(s) if s.eq_ignore_ascii_case("left") => remote_pad::Side::Left,
        Some(s) if s.eq_ignore_ascii_case("right") => remote_pad::Side::Right,
        _ => return Err(ToolError::Args("stick must be \"left\" or \"right\"".into())),
    };
    let (x, y, ms) = (axis(args, "x")?, axis(args, "y")?, hold_ms(args)?);
    remote_pad::stick(side, x, y, ms);
    Ok(Json::obj([("x", Json::num(f64::from(x))), ("y", Json::num(f64::from(y))), ("hold_ms", Json::uint(ms))]))
}

fn script_tool(args: &Json) -> Result<Json, ToolError> {
    let text = optional_str(args, "script")?.ok_or_else(|| ToolError::Args("script is required (a string)".into()))?;
    let steps = remote_pad::parse_script(text).map_err(ToolError::Args)?;
    let (count, ms) = (steps.len(), remote_pad::script_ms(&steps));
    remote_pad::run_script(steps);
    Ok(Json::obj([("started", Json::Bool(true)), ("steps", Json::uint(count as u64)), ("total_ms", Json::uint(ms))]))
}

fn set_setting(args: &Json) -> Result<Json, ToolError> {
    let name = required_name(args)?;
    let value = value_text(args, true)?;
    let hooks = crate::memory_hooks().ok_or_else(|| ToolError::Failed("the memory counters are not up yet: no frame was drawn".into()))?;
    (hooks.set)(name, &value).map_err(ToolError::Failed)
}

fn recent_log(shared: &Shared, args: &Json) -> Result<Json, ToolError> {
    let n = count(args)?;
    let filter = optional_str(args, "filter")?.filter(|f| !f.is_empty());
    let lines = shared.log.last_matching(n, |line| filter.is_none_or(|f| line.contains(f)));
    Ok(Json::obj([
        ("returned", Json::uint(lines.len() as u64)),
        ("logged_total", Json::uint(shared.log.total())),
        ("lines", Json::Arr(lines.iter().map(|l| Json::str(&**l)).collect())),
    ]))
}

fn pipeline_events(shared: &Shared, args: &Json) -> Result<Json, ToolError> {
    let n = count(args)?;
    let kind = optional_str(args, "kind")?;
    if let Some(k) = kind
        && !KINDS.contains(&k)
    {
        return Err(ToolError::Args(format!("kind must be one of {}", KINDS.join(", "))));
    }
    let hash = optional_str(args, "hash")?.filter(|h| !h.is_empty());
    let events = shared
        .events
        .last_matching(n, |e| kind.is_none_or(|k| e.kind == k) && hash.is_none_or(|h| e.hash.contains(h)));
    let opt = |v: &Option<String>| v.as_deref().map_or(Json::Null, Json::str);
    Ok(Json::obj([
        ("returned", Json::uint(events.len() as u64)),
        ("recorded_total", Json::uint(shared.events.total())),
        (
            "events",
            Json::Arr(
                events
                    .iter()
                    .map(|e| {
                        Json::obj([
                            ("kind", Json::str(e.kind)),
                            ("hash", Json::str(e.hash.as_str())),
                            ("stage", opt(&e.stage)),
                            ("detail", opt(&e.detail)),
                            ("frame", Json::uint(e.frame)),
                            ("at_ms", Json::num(e.at_ms)),
                            ("duration_ms", e.duration_ms.map_or(Json::Null, Json::num)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ]))
}

fn device_lost(shared: &Shared) -> Json {
    match shared.device_lost() {
        Some(text) => Json::obj([("lost", Json::Bool(true)), ("report", Json::str(&*text))]),
        None => Json::obj([("lost", Json::Bool(false)), ("report", Json::Null)]),
    }
}

fn screenshot(shared: &Shared, args: &Json) -> Result<Json, ToolError> {
    let scale = match args.get("downscale") {
        None | Some(Json::Null) => 1,
        Some(v) => match v.as_i64() {
            Some(s @ (1 | 2 | 4)) => s as u32,
            _ => return Err(ToolError::Args("downscale must be 1, 2 or 4".into())),
        },
    };
    let saved = shared.screenshot(scale).map_err(ToolError::Failed)?;
    Ok(Json::obj([
        ("path", Json::str(saved.path.display().to_string())),
        ("width", Json::Int(i64::from(saved.width))),
        ("height", Json::Int(i64::from(saved.height))),
        ("bytes", Json::uint(saved.bytes as u64)),
        ("downscale", Json::Int(i64::from(scale))),
    ]))
}

/// The arguments of `find_bad_pixels`.
fn scan_params(args: &Json) -> Result<ScanParams, ToolError> {
    let mut params = ScanParams::default();
    match args.get("max_draws") {
        None | Some(Json::Null) => {}
        Some(v) => match v.as_i64() {
            Some(n) if (1..=i64::from(MAX_DRAWS_LIMIT)).contains(&n) => params.max_draws = n as u32,
            _ => return Err(ToolError::Args(format!("max_draws must be an integer from 1 to {MAX_DRAWS_LIMIT}"))),
        },
    }
    match args.get("pixel") {
        None | Some(Json::Null) => {}
        Some(p) if p.is_obj() => {
            let coord = |key: &str| match p.get(key).and_then(Json::as_i64) {
                Some(v) if (0..=i64::from(MAX_PIXEL)).contains(&v) => Ok(v as u32),
                _ => Err(ToolError::Args(format!("pixel.{key} must be an integer from 0 to {MAX_PIXEL}"))),
            };
            params.pixel = Some((coord("x")?, coord("y")?));
        }
        Some(_) => return Err(ToolError::Args("pixel must be an object {\"x\": n, \"y\": n}".into())),
    }
    match args.get("include_uav") {
        None | Some(Json::Null) => {}
        Some(Json::Bool(b)) => params.include_uav = *b,
        Some(_) => return Err(ToolError::Args("include_uav must be a boolean".into())),
    }
    Ok(params)
}

fn find_bad_pixels(shared: &Shared, args: &Json) -> Result<Json, ToolError> {
    let params = scan_params(args)?;
    let (report, path, summary) = shared.find_bad_pixels(params, shared.config.scan_timeout).map_err(ToolError::Failed)?;
    let analysis = crate::badpix::analyse(&report);
    Ok(Json::obj([
        ("summary", Json::str(summary)),
        ("path", Json::str(path.display().to_string())),
        ("frame", Json::uint(report.frame)),
        ("draws_scanned", Json::uint(report.draws.len() as u64)),
        ("first_bad_writers", Json::uint(analysis.introducers.len() as u64)),
        ("bad_before_any_writer", Json::uint(analysis.inherited.len() as u64)),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;

    fn shared() -> Shared {
        Shared::new(Config::default())
    }

    fn rpc(shared: &Shared, text: &str) -> Outcome {
        handle_body(shared, text.as_bytes())
    }

    fn result(shared: &Shared, text: &str) -> Json {
        match rpc(shared, text) {
            Outcome::Response(r) => {
                assert!(r.get("error").is_none(), "{}", r.text());
                r.get("result").cloned().unwrap()
            }
            other => panic!("{other:?}"),
        }
    }

    fn error_code(shared: &Shared, text: &str) -> i64 {
        match rpc(shared, text) {
            Outcome::Response(r) | Outcome::Error(_, r) => r.get("error").and_then(|e| e.get("code")).and_then(Json::as_i64).unwrap(),
            other => panic!("{other:?}"),
        }
    }

    fn call_text(name: &str, args: &str) -> String {
        format!(r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"{name}","arguments":{args}}}}}"#)
    }

    #[test]
    fn initialize_answers_with_the_protocol_version() {
        let s = shared();
        let r = result(&s, r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#);
        assert_eq!(r.get("protocolVersion").and_then(Json::as_str), Some("2025-06-18"));
        assert!(r.get("capabilities").and_then(|c| c.get("tools")).is_some());
        assert_eq!(r.get("serverInfo").and_then(|i| i.get("name")).and_then(Json::as_str), Some("victory-debug"));
    }

    #[test]
    fn the_id_is_echoed_as_sent() {
        let s = shared();
        let Outcome::Response(r) = rpc(&s, r#"{"jsonrpc":"2.0","id":"abc","method":"ping"}"#) else { panic!() };
        assert_eq!(r.get("id"), Some(&Json::str("abc")));
        assert_eq!(r.get("result"), Some(&Json::Obj(vec![])));
        let Outcome::Response(r) = rpc(&s, r#"{"jsonrpc":"2.0","id":9007199254740993,"method":"ping"}"#) else { panic!() };
        assert_eq!(r.get("id"), Some(&Json::Int(9007199254740993)));
    }

    #[test]
    fn notifications_and_client_responses_are_accepted_silently() {
        let s = shared();
        assert_eq!(rpc(&s, r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#), Outcome::Accepted);
        assert_eq!(rpc(&s, r#"{"jsonrpc":"2.0","method":"anything"}"#), Outcome::Accepted);
        assert_eq!(rpc(&s, r#"{"jsonrpc":"2.0","id":3,"result":{}}"#), Outcome::Accepted);
    }

    #[test]
    fn pad_tools_check_their_arguments() {
        let s = shared();
        assert_eq!(error_code(&s, &call_text("press", r#"{}"#)), INVALID_PARAMS);
        assert_eq!(error_code(&s, &call_text("press", r#"{"button":"Start"}"#)), INVALID_PARAMS);
        assert_eq!(error_code(&s, &call_text("press", r#"{"button":"A","hold_ms":-1}"#)), INVALID_PARAMS);
        assert_eq!(error_code(&s, &call_text("stick", r#"{"stick":"middle"}"#)), INVALID_PARAMS);
        assert_eq!(error_code(&s, &call_text("stick", r#"{"stick":"left","x":2}"#)), INVALID_PARAMS);
        assert_eq!(error_code(&s, &call_text("input_script", r#"{"script":"A 1 2"}"#)), INVALID_PARAMS);
        let r = result(&s, &call_text("input_script", r#"{"script":"wait 1"}"#));
        assert_eq!(r.get("structuredContent").and_then(|c| c.get("steps")).and_then(Json::as_i64), Some(1));
        let r = result(&s, &call_text("release_all", r#"{}"#));
        assert_eq!(r.get("isError"), Some(&Json::Bool(false)));
    }

    #[test]
    fn tools_list_has_the_nine_tools_with_schemas() {
        let s = shared();
        let r = result(&s, r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#);
        let Some(Json::Arr(tools)) = r.get("tools") else { panic!() };
        let names: Vec<_> = tools.iter().filter_map(|t| t.get("name").and_then(Json::as_str)).collect();
        assert_eq!(
            names,
            ["get_stats", "get_recent_log", "get_pipeline_events", "get_device_lost_report", "screenshot", "get_memory", "get_dropped_draws", "get_settings", "set_setting", "list_tunables", "get_tunable", "set_tunable", "reset_tunable", "press", "stick", "release_all", "input_script", "find_bad_pixels", "mem_regions", "mem_scan_start", "mem_scan_filter", "mem_scan_list", "mem_read", "mem_scan_reset"]
        );
        for t in tools {
            let schema = t.get("inputSchema").unwrap();
            assert_eq!(schema.get("type").and_then(Json::as_str), Some("object"));
            assert!(schema.get("properties").is_some_and(Json::is_obj));
            let writes = matches!(t.get("name").and_then(Json::as_str), Some("set_setting" | "set_tunable" | "reset_tunable" | "press" | "stick" | "release_all" | "input_script" | "mem_scan_start" | "mem_scan_filter" | "mem_scan_reset"));
            assert_eq!(t.get("annotations").and_then(|a| a.get("readOnlyHint")), Some(&Json::Bool(!writes)));
            assert!(t.get("description").and_then(Json::as_str).is_some_and(|d| !d.is_empty()));
        }
        let shot = tools.iter().find(|t| t.get("name").and_then(Json::as_str) == Some("screenshot")).unwrap();
        let allowed = shot.get("inputSchema").and_then(|s| s.get("properties")).and_then(|p| p.get("downscale")).and_then(|d| d.get("enum"));
        assert_eq!(allowed, Some(&Json::Arr(vec![Json::Int(1), Json::Int(2), Json::Int(4)])));
        let scan = tools.iter().find(|t| t.get("name").and_then(Json::as_str) == Some("find_bad_pixels")).unwrap();
        let props = scan.get("inputSchema").and_then(|s| s.get("properties")).unwrap();
        for key in ["max_draws", "pixel", "include_uav"] {
            assert!(props.get(key).is_some(), "{key}");
        }
        assert_eq!(props.get("max_draws").and_then(|m| m.get("default")), Some(&Json::Int(i64::from(DEFAULT_MAX_DRAWS))));
        assert_eq!(props.get("max_draws").and_then(|m| m.get("maximum")), Some(&Json::Int(i64::from(MAX_DRAWS_LIMIT))));
        assert_eq!(props.get("include_uav").and_then(|m| m.get("type")).and_then(Json::as_str), Some("boolean"));
        let pixel = props.get("pixel").unwrap();
        assert_eq!(pixel.get("required"), Some(&Json::Arr(vec![Json::str("x"), Json::str("y")])));
        assert!(pixel.get("properties").and_then(|p| p.get("x")).is_some() && pixel.get("properties").and_then(|p| p.get("y")).is_some());
        assert_eq!(scan.get("inputSchema").and_then(|s| s.get("additionalProperties")), Some(&Json::Bool(false)));
    }

    #[test]
    fn memory_tool_arguments_are_checked() {
        let s = shared();
        for (tool, args) in [
            ("mem_scan_filter", r#"{}"#),
            ("mem_scan_filter", r#"{"mode":"bigger"}"#),
            ("mem_scan_filter", r#"{"mode":"equals"}"#),
            ("mem_scan_filter", r#"{"mode":"equals","value":"zz"}"#),
            ("mem_scan_filter", r#"{"mode":"changed","signed":1}"#),
            ("mem_scan_start", r#"{"width":3}"#),
            ("mem_scan_start", r#"{"width":8}"#),
            ("mem_scan_start", r#"{"max_mb":0}"#),
            ("mem_scan_list", r#"{"limit":0}"#),
            ("mem_read", r#"{}"#),
            ("mem_read", r#"{"address":"0x10","len":4097}"#),
            ("mem_read", r#"{"address":"nope"}"#),
            ("mem_regions", r#"{"limit":0}"#),
        ] {
            assert_eq!(error_code(&s, &call_text(tool, args)), INVALID_PARAMS, "{tool} {args}");
        }
        // Good arguments, but the title is not mapped in a test: a tool error, not a protocol one.
        for (tool, args) in [("mem_regions", "{}"), ("mem_scan_start", "{}"), ("mem_scan_filter", r#"{"mode":"changed"}"#), ("mem_read", r#"{"address":"1000"}"#)] {
            let r = result(&s, &call_text(tool, args));
            assert_eq!(r.get("isError"), Some(&Json::Bool(true)), "{tool}");
        }
        let r = result(&s, &call_text("mem_scan_reset", "{}"));
        assert_eq!(r.get("isError"), Some(&Json::Bool(false)));
    }

    #[test]
    fn find_bad_pixels_arguments_are_checked() {
        let s = shared();
        for args in [
            r#"{"max_draws":0}"#,
            r#"{"max_draws":100001}"#,
            r#"{"max_draws":"5"}"#,
            r#"{"pixel":5}"#,
            r#"{"pixel":{"x":1}}"#,
            r#"{"pixel":{"x":-1,"y":0}}"#,
            r#"{"pixel":{"x":0,"y":16384}}"#,
            r#"{"include_uav":1}"#,
        ] {
            assert_eq!(error_code(&s, &call_text("find_bad_pixels", args)), INVALID_PARAMS, "{args}");
        }
        let ok = scan_params(&crate::json::parse(br#"{"max_draws":50,"pixel":{"x":960,"y":540},"include_uav":true}"#).unwrap()).ok().unwrap();
        assert_eq!(ok, ScanParams { max_draws: 50, pixel: Some((960, 540)), include_uav: true });
        let none = scan_params(&Json::Obj(vec![])).ok().unwrap();
        assert_eq!(none, ScanParams::default());
    }

    #[test]
    fn find_bad_pixels_with_no_present_times_out_and_withdraws() {
        let _serial = crate::tests::SCAN_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let s = Shared::new(Config { scan_timeout: std::time::Duration::from_millis(30), ..Config::default() });
        let r = result(&s, &call_text("find_bad_pixels", "{}"));
        assert_eq!(r.get("isError"), Some(&Json::Bool(true)));
        assert!(!crate::scan_recording());
        assert!(s.scan_edge().is_none());
    }

    #[test]
    fn unknown_method_and_bad_requests_are_errors() {
        let s = shared();
        assert_eq!(error_code(&s, r#"{"jsonrpc":"2.0","id":1,"method":"nope"}"#), METHOD_NOT_FOUND);
        assert_eq!(error_code(&s, "{not json"), PARSE_ERROR);
        assert_eq!(error_code(&s, "[1,2]"), INVALID_REQUEST);
        assert_eq!(error_code(&s, r#"{"jsonrpc":"1.0","id":1,"method":"ping"}"#), INVALID_REQUEST);
        assert_eq!(error_code(&s, r#"{"jsonrpc":"2.0","id":1}"#), INVALID_REQUEST);
        assert_eq!(error_code(&s, r#"{"jsonrpc":"2.0","id":[1],"method":"ping"}"#), INVALID_REQUEST);
        assert!(matches!(rpc(&s, "{not json"), Outcome::Error(400, _)));
    }

    #[test]
    fn bad_params_are_invalid_params() {
        let s = shared();
        for body in [
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":[1]}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call"}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{}}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":5}}"#,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_stats","arguments":[]}}"#,
        ] {
            assert_eq!(error_code(&s, body), INVALID_PARAMS, "{body}");
        }
        assert_eq!(error_code(&s, &call_text("no_such_tool", "{}")), INVALID_PARAMS);
        for (name, args) in [
            ("get_recent_log", r#"{"n":0}"#),
            ("get_recent_log", r#"{"n":2001}"#),
            ("get_recent_log", r#"{"n":"5"}"#),
            ("get_recent_log", r#"{"n":1.5}"#),
            ("get_recent_log", r#"{"filter":3}"#),
            ("get_pipeline_events", r#"{"kind":"bogus"}"#),
            ("screenshot", r#"{"downscale":3}"#),
            ("screenshot", r#"{"downscale":"2"}"#),
        ] {
            assert_eq!(error_code(&s, &call_text(name, args)), INVALID_PARAMS, "{name} {args}");
        }
    }

    #[test]
    fn get_stats_before_and_after_a_meter_line() {
        let s = shared();
        let r = result(&s, &call_text("get_stats", "{}"));
        assert_eq!(r.get("isError"), Some(&Json::Bool(false)));
        assert_eq!(r.get("structuredContent").and_then(|c| c.get("available")), Some(&Json::Bool(false)));
        s.publish_stats(crate::Stats { frame: 120, frames: 13, fps: 12.7, ms_per_frame: 78.5, preparing_ms: 35.2, submissions_per_frame: 4.0, ..Default::default() });
        let r = result(&s, &call_text("get_stats", "{}"));
        let c = r.get("structuredContent").unwrap();
        assert_eq!(c.get("fps"), Some(&Json::Num(12.7)));
        assert_eq!(c.get("frame"), Some(&Json::Int(120)));
        assert_eq!(c.get("preparing").and_then(|p| p.get("ms")), Some(&Json::Num(35.2)));
        // The text content is the same JSON.
        let text = r.get("content").and_then(|c| if let Json::Arr(a) = c { a.first() } else { None }).and_then(|i| i.get("text")).and_then(Json::as_str).unwrap();
        assert_eq!(crate::json::parse(text.as_bytes()).unwrap().get("ms_per_frame"), Some(&Json::Num(78.5)));
    }

    #[test]
    fn get_recent_log_filters_and_bounds() {
        let s = shared();
        for i in 0..300 {
            s.push_log(&format!("line {i} {}", if i % 3 == 0 { "fizz" } else { "plain" }));
        }
        let lines = |args: &str| -> Vec<String> {
            let r = result(&s, &call_text("get_recent_log", args));
            let Some(Json::Arr(a)) = r.get("structuredContent").and_then(|c| c.get("lines")) else { panic!() };
            a.iter().map(|l| l.as_str().unwrap().to_owned()).collect()
        };
        let all = lines("{}");
        assert_eq!((all.len(), all[0].as_str(), all[199].as_str()), (200, "line 100 plain", "line 299 plain"));
        let few = lines(r#"{"n":3,"filter":"fizz"}"#);
        assert_eq!(few, ["line 291 fizz", "line 294 fizz", "line 297 fizz"].map(String::from));
        assert!(lines(r#"{"filter":"absent"}"#).is_empty());
    }

    #[test]
    fn get_pipeline_events_filters_by_kind_and_hash() {
        let s = shared();
        s.record_event(crate::KIND_SHADER_TRANSLATED, "aa11", Some("pixel"), None, 0, Some(3.5));
        s.record_event(crate::KIND_FIRST_BIND, "aa11", Some("pixel"), None, 42, None);
        s.record_event(crate::KIND_PIPELINE_MADE, "bb22", None, Some("[shaders]"), 43, Some(120.0));
        let events = |args: &str| -> Vec<Json> {
            let r = result(&s, &call_text("get_pipeline_events", args));
            let Some(Json::Arr(a)) = r.get("structuredContent").and_then(|c| c.get("events")) else { panic!() };
            a.clone()
        };
        assert_eq!(events("{}").len(), 3);
        let binds = events(r#"{"kind":"first_bind"}"#);
        assert_eq!(binds.len(), 1);
        assert_eq!(binds[0].get("frame"), Some(&Json::Int(42)));
        assert_eq!(binds[0].get("duration_ms"), Some(&Json::Null));
        assert_eq!(events(r#"{"hash":"bb"}"#)[0].get("duration_ms"), Some(&Json::Num(120.0)));
        assert_eq!(events(r#"{"n":1}"#)[0].get("kind").and_then(Json::as_str), Some("pipeline_made"));
    }

    #[test]
    fn device_lost_report_is_null_until_lost_then_kept() {
        let s = shared();
        let c = |s: &Shared| result(s, &call_text("get_device_lost_report", "{}")).get("structuredContent").cloned().unwrap();
        assert_eq!(c(&s).get("lost"), Some(&Json::Bool(false)));
        assert_eq!(c(&s).get("report"), Some(&Json::Null));
        s.set_device_lost("when the host device went: x\n  1 ms before".into());
        assert_eq!(c(&s).get("report").and_then(Json::as_str), Some("when the host device went: x\n  1 ms before"));
    }

    #[test]
    fn a_screenshot_that_finds_no_frame_is_a_tool_error() {
        let s = Shared::new(Config { shot_timeout: std::time::Duration::from_millis(30), ..Config::default() });
        let r = result(&s, &call_text("screenshot", r#"{"downscale":2}"#));
        assert_eq!(r.get("isError"), Some(&Json::Bool(true)));
    }

    #[test]
    fn dropped_draws_tool_uses_its_hook_and_says_when_it_is_off() {
        let s = shared();
        let r = result(&s, &call_text("get_dropped_draws", "{}"));
        if crate::drops_hook().is_none() {
            assert_eq!(r.get("isError"), Some(&Json::Bool(true)));
        }
        fn report() -> Json {
            Json::obj([("total", Json::Int(7))])
        }
        crate::set_drops_hook(report);
        let r = result(&s, &call_text("get_dropped_draws", "{}"));
        assert_eq!(r.get("structuredContent").and_then(|c| c.get("total")), Some(&Json::Int(7)));
    }

    #[test]
    fn tunable_tools_go_through_the_registered_functions() {
        fn list(filter: &str) -> Json {
            Json::obj([("count", Json::Int(1)), ("filter", Json::str(filter)), ("tunables", Json::Arr(vec![]))])
        }
        fn get(name: &str) -> Result<Json, String> {
            if name == "view_guard" { Ok(Json::obj([("name", Json::str(name))])) } else { Err(format!("unknown tunable {name:?}")) }
        }
        fn set(name: &str, value: &str) -> Result<Json, String> {
            get(name).map(|_| Json::obj([("name", Json::str(name)), ("value", Json::str(value))]))
        }
        fn reset(name: &str) -> Result<Json, String> {
            get(name).map(|_| Json::obj([("reset", Json::str(name))]))
        }
        let s = shared();
        // Before the hooks are registered a call says so (no panic).
        if crate::tunable_hooks().is_none() {
            assert_eq!(result(&s, &call_text("list_tunables", "{}")).get("isError"), Some(&Json::Bool(true)));
        }
        crate::set_tunable_hooks(crate::TunableHooks { list, get, set, reset });
        let sc = |r: &Json, key: &str| r.get("structuredContent").and_then(|c| c.get(key)).cloned();
        let r = result(&s, &call_text("list_tunables", r#"{"filter":"view"}"#));
        assert_eq!(r.get("isError"), Some(&Json::Bool(false)));
        assert_eq!((sc(&r, "filter"), sc(&r, "count")), (Some(Json::str("view")), Some(Json::Int(1))));
        let r = result(&s, &call_text("list_tunables", "{}"));
        assert_eq!(sc(&r, "filter"), Some(Json::str("")));
        let r = result(&s, &call_text("get_tunable", r#"{"name":"view_guard"}"#));
        assert_eq!(sc(&r, "name"), Some(Json::str("view_guard")));
        for (value, shown) in [("false", "off"), ("true", "on"), ("5", "5"), ("1.5", "1.5"), (r#""fast""#, "fast")] {
            let r = result(&s, &call_text("set_tunable", &format!(r#"{{"name":"view_guard","value":{value}}}"#)));
            assert_eq!(sc(&r, "value"), Some(Json::str(shown)), "{value}");
        }
        let r = result(&s, &call_text("set_tunable", r#"{"name":"nope","value":1}"#));
        assert_eq!(r.get("isError"), Some(&Json::Bool(true)));
        assert_eq!(error_code(&s, &call_text("set_tunable", r#"{"value":1}"#)), INVALID_PARAMS);
        assert_eq!(error_code(&s, &call_text("set_tunable", r#"{"name":"view_guard"}"#)), INVALID_PARAMS);
        assert_eq!(error_code(&s, &call_text("get_tunable", "{}")), INVALID_PARAMS);
        let r = result(&s, &call_text("reset_tunable", r#"{"name":"view_guard"}"#));
        assert_eq!(sc(&r, "reset"), Some(Json::str("view_guard")));
        let list = result(&s, r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#);
        let Some(Json::Arr(tools)) = list.get("tools") else { panic!() };
        let set_tool = tools.iter().find(|t| t.get("name").and_then(Json::as_str) == Some("set_tunable")).unwrap();
        let props = set_tool.get("inputSchema").and_then(|s| s.get("properties")).unwrap();
        assert!(props.get("name").is_some() && props.get("value").is_some());
    }

    #[test]
    fn memory_tools_go_through_the_registered_functions() {
        fn memory() -> Json {
            Json::obj([("local_mb", Json::Int(100))])
        }
        fn settings() -> Json {
            Json::obj([("evict", Json::Bool(true))])
        }
        fn set(name: &str, value: &str) -> Result<Json, String> {
            if name == "evict" { Ok(Json::obj([("evict", Json::str(value))])) } else { Err(format!("no setting {name}")) }
        }
        crate::set_memory_hooks(crate::MemoryHooks { memory, settings, set });
        let s = shared();
        let r = result(&s, &call_text("get_memory", "{}"));
        assert_eq!(r.get("structuredContent").and_then(|c| c.get("local_mb")), Some(&Json::Int(100)));
        let r = result(&s, &call_text("get_settings", "{}"));
        assert_eq!(r.get("isError"), Some(&Json::Bool(false)));
        for (value, shown) in [(r#""off""#, "off"), ("false", "off"), ("true", "on"), ("5", "5")] {
            let r = result(&s, &call_text("set_setting", &format!(r#"{{"name":"evict","value":{value}}}"#)));
            assert_eq!(r.get("structuredContent").and_then(|c| c.get("evict")), Some(&Json::str(shown)), "{value}");
        }
        let r = result(&s, &call_text("set_setting", r#"{"name":"nope","value":1}"#));
        assert_eq!(r.get("isError"), Some(&Json::Bool(true)));
        assert_eq!(error_code(&s, &call_text("set_setting", r#"{"value":1}"#)), INVALID_PARAMS);
        assert_eq!(error_code(&s, &call_text("set_setting", r#"{"name":"evict"}"#)), INVALID_PARAMS);
    }
}
