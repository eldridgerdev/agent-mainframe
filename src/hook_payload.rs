//! Building the IPC payload a hook script sends to the dashboard.
//!
//! Hook scripts used to do this themselves with `jq`: read a few fields out of
//! the harness's JSON on stdin, then reserialize a new object with AMF's
//! session metadata spliced in. That made `jq` a hard runtime dependency of
//! every hook — an undeclared one that failed *silently*, because each script
//! discards jq's stderr and exits 0, so a missing binary was indistinguishable
//! from "the agent had nothing to report".
//!
//! It was also redundant: the script piped its freshly-built JSON into
//! `amf notify`, which handed it to the dashboard, which parsed it again. The
//! parsing now happens once, here, and the scripts pass their stdin through
//! untouched.
//!
//! The metadata AMF used to splice in with `--arg` comes from the environment,
//! and `amf notify` runs as a child of the hook script, so it inherits the same
//! variables the script was reading. Nothing needs to be passed along the pipe.

use serde_json::{Map, Value};

/// The environment AMF exports into an agent session. Captured as a struct
/// rather than read inline so tests can build one without touching the real
/// process environment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookEnv {
    /// Tmux session name. Also the dashboard's key for a feature, which is why
    /// it takes precedence over the harness's own session id.
    pub amf_session: Option<String>,
    pub amf_feature_session_id: Option<String>,
    pub amf_tmux_session: Option<String>,
    pub amf_tmux_window: Option<String>,
}

impl HookEnv {
    /// Read the AMF session variables from the process environment. An empty
    /// variable is treated as unset — a hook script that expands `${FOO:-}`
    /// into an exported-but-empty value should not produce an empty field.
    pub fn from_process_env() -> Self {
        fn var(name: &str) -> Option<String> {
            std::env::var(name).ok().filter(|value| !value.is_empty())
        }
        Self {
            amf_session: var("AMF_SESSION"),
            amf_feature_session_id: var("AMF_FEATURE_SESSION_ID"),
            amf_tmux_session: var("AMF_TMUX_SESSION"),
            amf_tmux_window: var("AMF_TMUX_WINDOW"),
        }
    }
}

/// Per-invocation options, supplied by the hook script as flags.
#[derive(Debug, Clone, Default)]
pub struct NotifyOptions {
    /// Sets the payload's `type`, which is how the dashboard dispatches.
    /// `None` leaves whatever the harness sent — the Claude `Stop` hook
    /// forwards its raw JSON, which carries no `type` and must not gain one.
    pub event_type: Option<String>,
    pub source: Option<String>,
    /// Sets `amf_event_kind`, the attention layer's reason-for-stopping.
    pub event_kind: Option<String>,
    /// Fields that must be present and non-empty in the finished payload, or
    /// nothing is sent. Replaces the scripts' `[ -z "$X" ] && exit 0` guards.
    pub require: Vec<String>,
    /// Literal `key=value` string fields to set, for the fixed text a hook
    /// wants to attach (e.g. Codex's "finished and is waiting for input").
    pub fields: Vec<(String, String)>,
    /// `key=path` fields whose value is a file's contents. Keeps file bodies
    /// off the command line, where they would risk `ARG_MAX`. An unreadable
    /// path yields an empty string, matching the `jq --arg x ""` it replaces.
    pub file_fields: Vec<(String, String)>,
    /// `key=true|false` fields set as JSON booleans rather than strings, for
    /// the handful the dashboard deserializes into a `bool` (`is_new_file`).
    /// Anything other than `true` is `false`.
    pub bool_fields: Vec<(String, String)>,
    /// Fill in `prompt` from the harness's transcript shape when it is not
    /// already a top-level field. Codex reports the turn's messages rather
    /// than the prompt, so this recovers what the user actually typed.
    pub derive_prompt: bool,
}

/// Build the payload to send, or `None` when a required field is missing.
///
/// `stdin` is the harness's raw hook JSON. Anything it contains is preserved:
/// the dashboard reads harness-specific fields (`tool_name`, `notification_type`,
/// `message`, `prompt`, …) straight off the forwarded object, so this only ever
/// adds to what arrived.
pub fn build_payload(stdin: &str, env: &HookEnv, options: &NotifyOptions) -> Option<Value> {
    let mut payload: Map<String, Value> = match serde_json::from_str(stdin.trim()) {
        Ok(Value::Object(map)) => map,
        // A non-object or unparseable stdin still carries the environment's
        // session identity, which is enough for the dashboard to act on.
        _ => Map::new(),
    };

    // The harness's own session id, kept under a distinct key so it survives
    // `session_id` being replaced by the tmux session below.
    let provider_session_id = string_field(&payload, "session_id");
    if let Some(ref provider) = provider_session_id {
        payload.insert(
            "provider_session_id".to_string(),
            Value::String(provider.clone()),
        );
    }

    // The dashboard keys features by tmux session, so that wins when present;
    // the harness's id is the fallback for a hook running outside AMF's env.
    //
    // An explicit `--field session_id=…` outranks both. It is applied further
    // down like any other field, but it has to be consulted *here* too or a
    // caller that supplies the identity itself — the diff-review plugin, which
    // deliberately sends the provider's id — trips the "no identity" guard and
    // gets nothing sent.
    let explicit_session_id = options
        .fields
        .iter()
        .find(|(key, _)| key == "session_id")
        .map(|(_, value)| value.clone())
        .filter(|value| !value.is_empty());
    let session_id = explicit_session_id
        .or_else(|| env.amf_session.clone())
        .or(provider_session_id);
    match session_id {
        Some(id) => {
            payload.insert("session_id".to_string(), Value::String(id));
        }
        None => {
            // No identity at all: the dashboard could not attribute this to a
            // feature, so there is nothing worth sending.
            return None;
        }
    }

    if string_field(&payload, "cwd").is_none()
        && let Ok(cwd) = std::env::current_dir()
    {
        payload.insert(
            "cwd".to_string(),
            Value::String(cwd.to_string_lossy().into_owned()),
        );
    }

    if let Some(ref event_type) = options.event_type {
        payload.insert("type".to_string(), Value::String(event_type.clone()));
    }
    if let Some(ref source) = options.source {
        payload.insert("source".to_string(), Value::String(source.clone()));
    }
    if let Some(ref kind) = options.event_kind {
        payload.insert("amf_event_kind".to_string(), Value::String(kind.clone()));
    }

    if let Some(ref value) = env.amf_feature_session_id {
        payload.insert(
            "amf_feature_session_id".to_string(),
            Value::String(value.clone()),
        );
    }
    // Falls back to AMF_SESSION, matching what the scripts' `${A:-${B:-}}`
    // expansions did.
    if let Some(value) = env.amf_tmux_session.clone().or(env.amf_session.clone()) {
        payload.insert("amf_tmux_session".to_string(), Value::String(value));
    }
    if let Some(ref value) = env.amf_tmux_window {
        payload.insert("amf_tmux_window".to_string(), Value::String(value.clone()));
    }

    // Claude nests TodoWrite/Task details under `tool_input`; the dashboard
    // reads them as flat `task_*` fields, which is the shape `tool-start.sh`
    // used to build by hand.
    lift_task_fields(&mut payload);

    if options.derive_prompt
        && string_field(&payload, "prompt").is_none()
        && let Some(prompt) = derive_prompt(&payload)
    {
        payload.insert("prompt".to_string(), Value::String(prompt));
    }

    for (key, value) in &options.fields {
        payload.insert(key.clone(), Value::String(value.clone()));
    }
    for (key, path) in &options.file_fields {
        let contents = std::fs::read_to_string(path).unwrap_or_default();
        payload.insert(key.clone(), Value::String(contents));
    }
    for (key, value) in &options.bool_fields {
        payload.insert(key.clone(), Value::Bool(value == "true"));
    }

    for field in &options.require {
        string_field(&payload, field)?;
    }

    Some(Value::Object(payload))
}

/// Follow a dotted path (`tool_input.file_path`) into `value`.
///
/// Only object traversal: the hook payloads this reads are objects all the way
/// down, and array indexing would invite paths that silently mean nothing.
pub fn lookup_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for segment in path.split('.') {
        current = current.as_object()?.get(segment)?;
    }
    Some(current)
}

/// The plain text a shell script wants for the first of `paths` that resolves,
/// or `None` when none do.
///
/// Matches `jq -r`: a string yields its contents verbatim — no quoting, no
/// escaping — because callers redirect it straight into a file that must hold
/// the literal bytes. Anything non-string yields its compact JSON, so a bool
/// reads as `true`/`false` and a nested object stays parseable.
///
/// Several paths are accepted so a caller can spell out the shapes it tolerates
/// instead of running one extraction per shape.
pub fn field_text(value: &Value, paths: &[String]) -> Option<String> {
    paths
        .iter()
        .find_map(|path| lookup_path(value, path))
        .filter(|found| !found.is_null())
        .map(|found| match found {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        })
}

/// Whether a diff-review reply says the user rejected the change.
///
/// The reply is flat — `handlers::diff_review` sends
/// `{type, decision, reason, skip, reject}` and nothing in the codebase nests
/// it under a `response` key. `decision` is the current field; the `reject`
/// boolean is checked too because both are sent and either alone is a complete
/// answer.
pub fn reply_is_reject(reply: &Value) -> bool {
    if lookup_path(reply, "decision").and_then(Value::as_str) == Some("reject") {
        return true;
    }
    lookup_path(reply, "reject")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// A field's value when it is a non-empty string, else `None`. Numbers and
/// nulls count as absent: every field consulted here is a string in the hook
/// contract, and a stray non-string is not a usable identity.
fn string_field(payload: &Map<String, Value>, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.is_empty())
}

/// Recover the user's prompt from a Codex-style payload.
///
/// Codex's notify hook reports the turn's messages rather than the prompt, so
/// the last user message is the closest thing to "what was asked". Both the
/// hyphenated and underscored spellings of the key are checked, because the
/// two have appeared across CLI versions.
///
/// A message's text may be a bare string, `{text: ...}`, or `{content: ...}`
/// where content is itself a string or a list of strings/`{text: ...}` parts.
fn derive_prompt(payload: &Map<String, Value>) -> Option<String> {
    if let Some(message) = string_field(payload, "message") {
        return Some(message);
    }

    let messages = ["input-messages", "input_messages"]
        .iter()
        .filter_map(|key| payload.get(*key))
        .filter_map(Value::as_array)
        .flatten();

    let mut last_user_text = None;
    for message in messages {
        let is_user = message
            .get("role")
            .and_then(Value::as_str)
            .is_some_and(|role| role == "user");
        if !is_user {
            continue;
        }
        let text = message_text(message);
        if !text.is_empty() {
            last_user_text = Some(text);
        }
    }
    last_user_text
}

/// Flatten one message into its text, across the shapes Codex has used.
fn message_text(message: &Value) -> String {
    fn part_text(value: &Value) -> String {
        match value {
            Value::String(text) => text.clone(),
            Value::Object(_) => value
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            _ => String::new(),
        }
    }

    match message {
        Value::String(text) => return text.clone(),
        Value::Object(_) => {}
        _ => return String::new(),
    }

    if let Some(text) = message.get("text").and_then(Value::as_str) {
        return text.to_string();
    }

    match message.get("content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts.iter().map(part_text).collect(),
        _ => String::new(),
    }
}

/// Copy `tool_input.{taskId,subject,description,activeForm,status}` up to
/// flat `task_*` keys. Absent nested fields are simply not copied.
fn lift_task_fields(payload: &mut Map<String, Value>) {
    const FIELDS: &[(&str, &str)] = &[
        ("taskId", "task_id"),
        ("subject", "task_subject"),
        ("description", "task_description"),
        ("activeForm", "task_active_form"),
        ("status", "task_status"),
    ];

    let Some(tool_input) = payload.get("tool_input").and_then(Value::as_object) else {
        return;
    };

    let lifted: Vec<(String, Value)> = FIELDS
        .iter()
        .filter_map(|(nested, flat)| {
            let value = tool_input
                .get(*nested)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())?;
            Some((flat.to_string(), Value::String(value.to_string())))
        })
        .collect();

    for (key, value) in lifted {
        payload.insert(key, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> HookEnv {
        HookEnv {
            amf_session: Some("amf-my-feat".to_string()),
            amf_feature_session_id: Some("fs-1".to_string()),
            amf_tmux_session: Some("amf-my-feat".to_string()),
            amf_tmux_window: Some("claude".to_string()),
        }
    }

    fn build(stdin: &str, options: &NotifyOptions) -> Option<Value> {
        build_payload(stdin, &env(), options)
    }

    fn opts(event_type: &str) -> NotifyOptions {
        NotifyOptions {
            event_type: Some(event_type.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn tmux_session_wins_over_the_harness_session_id() {
        let payload = build(
            r#"{"session_id":"uuid-from-claude","cwd":"/repo"}"#,
            &opts("thinking-start"),
        )
        .unwrap();

        // The dashboard keys features by tmux session...
        assert_eq!(payload["session_id"], "amf-my-feat");
        // ...but the harness's own id is still needed to match transcripts.
        assert_eq!(payload["provider_session_id"], "uuid-from-claude");
        assert_eq!(payload["type"], "thinking-start");
        assert_eq!(payload["cwd"], "/repo");
    }

    #[test]
    fn harness_session_id_is_used_when_amf_session_is_absent() {
        let bare = HookEnv::default();
        let payload = build_payload(
            r#"{"session_id":"uuid-from-claude","cwd":"/repo"}"#,
            &bare,
            &opts("thinking-start"),
        )
        .unwrap();

        assert_eq!(payload["session_id"], "uuid-from-claude");
        assert!(payload.get("amf_tmux_session").is_none());
    }

    #[test]
    fn no_identity_anywhere_sends_nothing() {
        let bare = HookEnv::default();
        assert!(build_payload(r#"{"cwd":"/repo"}"#, &bare, &opts("thinking-start")).is_none());
    }

    #[test]
    fn session_metadata_comes_from_the_environment() {
        let payload = build(r#"{"session_id":"u1","cwd":"/repo"}"#, &opts("clear")).unwrap();

        assert_eq!(payload["amf_feature_session_id"], "fs-1");
        assert_eq!(payload["amf_tmux_session"], "amf-my-feat");
        assert_eq!(payload["amf_tmux_window"], "claude");
    }

    #[test]
    fn tmux_session_falls_back_to_amf_session() {
        let env = HookEnv {
            amf_session: Some("amf-my-feat".to_string()),
            amf_tmux_session: None,
            ..HookEnv::default()
        };
        let payload =
            build_payload(r#"{"session_id":"u1","cwd":"/repo"}"#, &env, &opts("clear")).unwrap();

        assert_eq!(payload["amf_tmux_session"], "amf-my-feat");
    }

    #[test]
    fn harness_fields_are_preserved() {
        // The Stop hook forwards Claude's raw JSON; nothing may be dropped,
        // because the dashboard reads these fields directly.
        let payload = build(
            r#"{"session_id":"u1","cwd":"/repo","notification_type":"tool_permission_request","message":"needs permission","tool_name":"Bash","prompt":"hi"}"#,
            &NotifyOptions::default(),
        )
        .unwrap();

        assert_eq!(payload["notification_type"], "tool_permission_request");
        assert_eq!(payload["message"], "needs permission");
        assert_eq!(payload["tool_name"], "Bash");
        assert_eq!(payload["prompt"], "hi");
    }

    #[test]
    fn absent_event_type_is_not_invented() {
        // notify.sh forwards Claude's Stop payload untyped, and the dashboard
        // relies on `type` being missing to fall through to its generic path.
        let payload = build(
            r#"{"session_id":"u1","cwd":"/repo"}"#,
            &NotifyOptions::default(),
        )
        .unwrap();

        assert!(payload.get("type").is_none());
    }

    #[test]
    fn task_fields_are_lifted_out_of_tool_input() {
        let payload = build(
            r#"{"session_id":"u1","cwd":"/repo","tool_name":"TodoWrite","tool_input":{"taskId":"t1","subject":"Ship it","activeForm":"Shipping it","status":"in_progress"}}"#,
            &opts("tool-start"),
        )
        .unwrap();

        assert_eq!(payload["task_id"], "t1");
        assert_eq!(payload["task_subject"], "Ship it");
        assert_eq!(payload["task_active_form"], "Shipping it");
        assert_eq!(payload["task_status"], "in_progress");
        // Absent nested fields are simply not copied.
        assert!(payload.get("task_description").is_none());
    }

    #[test]
    fn tool_input_without_task_fields_lifts_nothing() {
        let payload = build(
            r#"{"session_id":"u1","cwd":"/repo","tool_input":{"command":"ls"}}"#,
            &opts("tool-start"),
        )
        .unwrap();

        assert!(payload.get("task_id").is_none());
    }

    #[test]
    fn required_fields_gate_the_send() {
        let options = NotifyOptions {
            event_type: Some("prompt-submit".to_string()),
            require: vec!["prompt".to_string()],
            ..Default::default()
        };

        assert!(build(r#"{"session_id":"u1","cwd":"/repo"}"#, &options).is_none());
        assert!(build(r#"{"session_id":"u1","cwd":"/repo","prompt":""}"#, &options).is_none());
        assert!(
            build(
                r#"{"session_id":"u1","cwd":"/repo","prompt":"hi"}"#,
                &options
            )
            .is_some()
        );
    }

    #[test]
    fn event_kind_and_source_are_set_from_options() {
        let options = NotifyOptions {
            event_type: Some("attention".to_string()),
            source: Some("attention-hook".to_string()),
            event_kind: Some("completed".to_string()),
            ..Default::default()
        };
        let payload = build(r#"{"session_id":"u1","cwd":"/repo"}"#, &options).unwrap();

        assert_eq!(payload["type"], "attention");
        assert_eq!(payload["source"], "attention-hook");
        assert_eq!(payload["amf_event_kind"], "completed");
    }

    #[test]
    fn unparseable_stdin_still_reports_the_session() {
        // A harness that writes nothing, or writes a log line instead of JSON,
        // must not cost us the event: the environment alone identifies the
        // feature.
        let payload = build("not json at all", &opts("thinking-stop")).unwrap();

        assert_eq!(payload["session_id"], "amf-my-feat");
        assert_eq!(payload["type"], "thinking-stop");
        assert!(payload.get("provider_session_id").is_none());
    }

    #[test]
    fn empty_stdin_still_reports_the_session() {
        let payload = build("", &opts("thinking-stop")).unwrap();
        assert_eq!(payload["session_id"], "amf-my-feat");
    }

    #[test]
    fn field_text_reads_nested_paths_verbatim() {
        let value: Value = serde_json::from_str(
            r#"{"tool_name":"Edit","tool_input":{"file_path":"/a/b.rs","old_string":"fn a() {\n  1\n}"}}"#,
        )
        .unwrap();

        assert_eq!(
            field_text(&value, &["tool_name".to_string()]).as_deref(),
            Some("Edit")
        );
        // Multi-line content comes back byte-for-byte: callers redirect it
        // straight into a file that has to hold exactly these bytes.
        assert_eq!(
            field_text(&value, &["tool_input.old_string".to_string()]).as_deref(),
            Some("fn a() {\n  1\n}")
        );
        assert_eq!(
            field_text(&value, &["tool_input.file_path".to_string()]).as_deref(),
            Some("/a/b.rs")
        );
    }

    #[test]
    fn field_text_is_absent_for_missing_and_null_paths() {
        let value: Value = serde_json::from_str(r#"{"a":{"b":1},"nulled":null}"#).unwrap();

        assert!(field_text(&value, &["missing".to_string()]).is_none());
        assert!(field_text(&value, &["a.missing".to_string()]).is_none());
        // Descending through a non-object must not panic.
        assert!(field_text(&value, &["a.b.c".to_string()]).is_none());
        // A JSON null is "no value", matching `// empty`.
        assert!(field_text(&value, &["nulled".to_string()]).is_none());
    }

    #[test]
    fn field_text_takes_the_first_path_that_resolves() {
        let value: Value = serde_json::from_str(r#"{"second":"hit"}"#).unwrap();
        let paths = ["first".to_string(), "second".to_string()];
        assert_eq!(field_text(&value, &paths).as_deref(), Some("hit"));
    }

    #[test]
    fn field_text_renders_non_strings_as_json() {
        let value: Value = serde_json::from_str(r#"{"flag":true,"n":3}"#).unwrap();
        assert_eq!(
            field_text(&value, &["flag".to_string()]).as_deref(),
            Some("true")
        );
        assert_eq!(field_text(&value, &["n".to_string()]).as_deref(), Some("3"));
    }

    #[test]
    fn reject_is_read_from_the_flat_reply_amf_actually_sends() {
        // `handlers::diff_review` sends this shape. The previous code looked
        // under a `response` key that nothing ever writes, so a rejected Codex
        // change was never reverted.
        let rejected: Value = serde_json::from_str(
            r#"{"type":"review-response","decision":"reject","reason":"no","skip":false,"reject":true}"#,
        )
        .unwrap();
        assert!(reply_is_reject(&rejected));

        for accepted in [
            r#"{"type":"review-response","decision":"proceed","skip":false,"reject":false}"#,
            r#"{"type":"review-response","decision":"cancel","skip":true,"reject":false}"#,
            r#"{}"#,
        ] {
            let value: Value = serde_json::from_str(accepted).unwrap();
            assert!(!reply_is_reject(&value), "should not reject: {accepted}");
        }

        // Either field alone is a complete answer.
        let bool_only: Value = serde_json::from_str(r#"{"reject":true}"#).unwrap();
        assert!(reply_is_reject(&bool_only));
    }

    #[test]
    fn explicit_session_id_field_satisfies_the_identity_guard() {
        // The diff-review plugin supplies the provider's id itself and carries
        // the AMF session alongside; without AMF_SESSION set this must still
        // build rather than trip the "no identity" guard.
        let options = NotifyOptions {
            event_type: Some("diff-review".to_string()),
            fields: vec![
                ("session_id".to_string(), "provider-uuid".to_string()),
                ("amf_session".to_string(), "amf-feat".to_string()),
            ],
            ..Default::default()
        };
        let payload = build_payload("", &HookEnv::default(), &options).unwrap();

        assert_eq!(payload["session_id"], "provider-uuid");
        assert_eq!(payload["amf_session"], "amf-feat");
    }

    #[test]
    fn explicit_session_id_outranks_the_tmux_session() {
        let options = NotifyOptions {
            fields: vec![("session_id".to_string(), "provider-uuid".to_string())],
            ..Default::default()
        };
        let payload = build_payload("", &env(), &options).unwrap();
        assert_eq!(payload["session_id"], "provider-uuid");
    }

    #[test]
    fn bool_fields_are_json_booleans_not_strings() {
        // `is_new_file` deserializes into an `Option<bool>`, so a quoted
        // "true" would silently drop the flag.
        let options = NotifyOptions {
            bool_fields: vec![
                ("is_new_file".to_string(), "true".to_string()),
                ("other".to_string(), "false".to_string()),
            ],
            ..Default::default()
        };
        let payload = build_payload(r#"{"session_id":"s1"}"#, &env(), &options).unwrap();

        assert_eq!(payload["is_new_file"], serde_json::json!(true));
        assert_eq!(payload["other"], serde_json::json!(false));
    }

    #[test]
    fn empty_environment_variables_count_as_unset() {
        let env = HookEnv {
            amf_session: Some("amf-my-feat".to_string()),
            amf_tmux_window: None,
            ..HookEnv::default()
        };
        let payload = build_payload(r#"{"session_id":"u1"}"#, &env, &opts("clear")).unwrap();

        // An exported-but-empty AMF_TMUX_WINDOW must not become `""`.
        assert!(payload.get("amf_tmux_window").is_none());
    }
}
