//! Consultation persistence is independent of the full-replace ProjectStore.
//! Origin IDs are logical references; only consultation-owned rows cascade.
//! The app/UI consumers land in later prototype steps.
#![allow(dead_code)]

use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::path::PathBuf;

use super::AmfDb;
use crate::headless::job::{HeadlessJobLimits, HeadlessJobResult, HeadlessJobStatus};
use crate::headless::{HeadlessExecutionPolicy, HeadlessRunner, HeadlessUsage};
use crate::project::AgentKind;

pub mod evidence;

const SCHEMA_VERSION: i64 = 1;
const MAX_RECORD_BYTES: usize = 512 * 1024;
const MAX_HANDOFF_BYTES: usize = 48 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsultationOrigin {
    pub project_id: String,
    pub feature_id: String,
    pub session_id: String,
    pub launch_generation: String,
    pub provider_session_id: Option<String>,
    pub workdir: PathBuf,
    pub repository_identity: String,
    pub tmux_server_identity: String,
    pub tmux_session_id: String,
    pub tmux_window_id: String,
    pub tmux_pane_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpertProfile {
    pub harness: AgentKind,
    pub binary: String,
    pub model: String,
    pub policy: HeadlessExecutionPolicy,
    pub limits: HeadlessJobLimits,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsultationEvidence {
    pub id: String,
    pub source: String,
    pub excerpt: String,
    pub source_sha256: Option<String>,
    pub omitted: Option<String>,
    pub retrievable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsultationRequest {
    pub question: String,
    pub acceptance_criteria: Vec<String>,
    pub attempted_fixes: Vec<String>,
    pub evidence: Vec<ConsultationEvidence>,
    pub profile: ExpertProfile,
    pub prompt_id: String,
    pub template_source: String,
    pub rendered_prompt: String,
}

impl ConsultationRequest {
    fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            !self.question.trim().is_empty(),
            "consultation question is required"
        );
        anyhow::ensure!(
            !self.acceptance_criteria.is_empty()
                && self
                    .acceptance_criteria
                    .iter()
                    .all(|item| !item.trim().is_empty()),
            "acceptance criteria are required"
        );
        anyhow::ensure!(
            !self.profile.binary.trim().is_empty() && !self.profile.model.trim().is_empty(),
            "explicit expert executable and model are required"
        );
        anyhow::ensure!(
            self.profile.policy != HeadlessExecutionPolicy::Ordinary,
            "consultations require an explicit execution policy"
        );
        HeadlessRunner::capabilities(&self.profile.harness).access_for(self.profile.policy)?;
        self.profile.limits.validate()?;
        anyhow::ensure!(
            self.rendered_prompt.len() <= self.profile.limits.prompt_bytes,
            "rendered prompt exceeds the profile limit"
        );
        let mut ids = std::collections::HashSet::new();
        for evidence in &self.evidence {
            anyhow::ensure!(
                !evidence.id.is_empty() && ids.insert(&evidence.id),
                "evidence IDs must be unique and nonempty"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsultationOwner {
    pub instance_id: String,
    pub pid: u32,
    pub process_started_at: String,
}

impl ConsultationOwner {
    fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.pid > 0
                && self.pid <= i32::MAX as u32
                && !self.instance_id.is_empty()
                && !self.process_started_at.is_empty(),
            "valid process ownership is required"
        );
        Ok(())
    }
    /// Call once per AMF instance, not once per job. Unknown process identity
    /// prevents claiming durable work rather than fabricating an owner.
    pub fn current() -> Result<Self> {
        let pid = std::process::id();
        let process_started_at = crate::resources::procs::start_time_for_pid(pid as i64)
            .context("could not establish AMF process identity")?;
        Ok(Self {
            instance_id: uuid::Uuid::new_v4().to_string(),
            pid,
            process_started_at,
        })
    }

    fn liveness(&self) -> OwnerLiveness {
        if self.validate().is_err() {
            return OwnerLiveness::Unknown;
        }
        // SAFETY: signal 0 probes existence only. EPERM is unknown, not dead.
        if unsafe { libc::kill(self.pid as i32, 0) } != 0 {
            return if std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                OwnerLiveness::Dead
            } else {
                OwnerLiveness::Unknown
            };
        }
        match crate::resources::procs::start_time_for_pid(self.pid as i64) {
            Some(start) if start == self.process_started_at => OwnerLiveness::Alive,
            Some(_) => OwnerLiveness::Dead,
            None => OwnerLiveness::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OwnerLiveness {
    Alive,
    Dead,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct Consultation {
    pub id: String,
    pub schema_version: i64,
    pub origin: ConsultationOrigin,
    pub revision: i64,
    pub request_revision: i64,
    pub state: String,
    pub active_attempt_id: Option<String>,
    pub owner: Option<ConsultationOwner>,
    pub heartbeat_at: Option<i64>,
    pub handoff_state: String,
    pub handoff_revision: i64,
    pub delivery_id: Option<String>,
}

impl Consultation {
    fn supported(&self) -> Result<()> {
        anyhow::ensure!(
            self.schema_version == SCHEMA_VERSION,
            "unsupported consultation schema version {}",
            self.schema_version
        );
        anyhow::ensure!(
            [
                "draft",
                "running",
                "cancelling",
                "completed",
                "failed",
                "incomplete",
                "cancelled",
                "timed_out",
                "interrupted"
            ]
            .contains(&self.state.as_str()),
            "unsupported consultation state"
        );
        anyhow::ensure!(
            [
                "none",
                "ready",
                "sending",
                "submitted",
                "delivery_unknown",
                "dismissed"
            ]
            .contains(&self.handoff_state.as_str()),
            "unsupported handoff state"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsultationOutcome {
    pub status: HeadlessJobStatus,
    /// Bounded raw final response. The app validates the expert result schema
    /// before explicitly staging any handoff; storing it never sends it.
    pub response: Option<String>,
    pub error: Option<String>,
    pub usage: HeadlessUsage,
    pub usage_complete: bool,
    pub elapsed_millis: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpertAttemptUsageExport {
    pub attempt_id: String,
    pub status: String,
    pub usage: HeadlessUsage,
    pub usage_complete: bool,
    pub elapsed_millis: Option<u64>,
    pub cost: Option<f64>,
    pub billing_basis: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpertEvaluationExport {
    pub consultation_id: String,
    pub schema_version: i64,
    pub origin: ConsultationOrigin,
    pub request_revision: i64,
    pub profile: ExpertProfile,
    pub attempts: Vec<ExpertAttemptUsageExport>,
    pub handoff_state: String,
}

impl From<HeadlessJobResult> for ConsultationOutcome {
    fn from(result: HeadlessJobResult) -> Self {
        Self {
            status: result.status,
            response: result.response,
            error: result.error,
            usage: result.usage,
            usage_complete: result.usage_complete,
            elapsed_millis: result.elapsed.as_millis().min(u64::MAX as u128) as u64,
        }
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn encode(value: &impl Serialize) -> Result<String> {
    let json = serde_json::to_string(value)?;
    anyhow::ensure!(
        json.len() <= MAX_RECORD_BYTES,
        "consultation record exceeds its storage limit"
    );
    Ok(json)
}

fn decode<T: DeserializeOwned>(text: &str) -> Result<T> {
    anyhow::ensure!(
        text.len() <= MAX_RECORD_BYTES,
        "stored consultation record exceeds its size limit"
    );
    Ok(serde_json::from_str(text)?)
}

fn job_state(status: HeadlessJobStatus) -> &'static str {
    match status {
        HeadlessJobStatus::Completed => "completed",
        HeadlessJobStatus::Failed => "failed",
        HeadlessJobStatus::Incomplete => "incomplete",
        HeadlessJobStatus::Cancelled => "cancelled",
        HeadlessJobStatus::TimedOut => "timed_out",
    }
}

impl AmfDb {
    pub fn create_expert_consultation(
        &self,
        origin: &ConsultationOrigin,
        request: &ConsultationRequest,
    ) -> Result<String> {
        request.validate()?;
        anyhow::ensure!(
            [
                &origin.project_id,
                &origin.feature_id,
                &origin.session_id,
                &origin.launch_generation,
                &origin.repository_identity,
                &origin.tmux_server_identity,
                &origin.tmux_session_id,
                &origin.tmux_window_id,
                &origin.tmux_pane_id
            ]
            .iter()
            .all(|value| !value.trim().is_empty()),
            "stable consultation origin identity is required"
        );
        let origin_json = encode(origin)?;
        let request_json = encode(request)?;
        let id = uuid::Uuid::new_v4().to_string();
        let timestamp = now();
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("INSERT INTO expert_consultations(id,schema_version,project_id,feature_id,session_id,origin_json,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?7)", params![id, SCHEMA_VERSION, origin.project_id, origin.feature_id, origin.session_id, origin_json, timestamp])?;
        tx.execute("INSERT INTO expert_requests(consultation_id,revision,request_json,created_at) VALUES (?1,1,?2,?3)", params![id, request_json, timestamp])?;
        tx.commit()?;
        Ok(id)
    }

    pub fn expert_consultation(&self, id: &str) -> Result<Option<Consultation>> {
        let row = self.conn.query_row("SELECT schema_version,origin_json,revision,request_revision,state,active_attempt_id,owner_json,heartbeat_at,handoff_state,handoff_revision,delivery_id FROM expert_consultations WHERE id=?1", [id], |row| {
            Ok((row.get::<_,i64>(0)?, row.get::<_,String>(1)?, row.get::<_,i64>(2)?, row.get::<_,i64>(3)?, row.get::<_,String>(4)?, row.get::<_,Option<String>>(5)?, row.get::<_,Option<String>>(6)?, row.get::<_,Option<i64>>(7)?, row.get::<_,String>(8)?, row.get::<_,i64>(9)?, row.get::<_,Option<String>>(10)?))
        }).optional()?;
        let Some((
            schema_version,
            origin,
            revision,
            request_revision,
            state,
            active_attempt_id,
            owner,
            heartbeat_at,
            handoff_state,
            handoff_revision,
            delivery_id,
        )) = row
        else {
            return Ok(None);
        };
        anyhow::ensure!(
            schema_version == SCHEMA_VERSION,
            "unsupported consultation schema version {schema_version}"
        );
        let consultation = Consultation {
            id: id.into(),
            schema_version,
            origin: decode(&origin)?,
            revision,
            request_revision,
            state,
            active_attempt_id,
            owner: owner.as_deref().map(decode).transpose()?,
            heartbeat_at,
            handoff_state,
            handoff_revision,
            delivery_id,
        };
        consultation.supported()?;
        Ok(Some(consultation))
    }

    pub fn expert_consultations_for_session(&self, session_id: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM expert_consultations WHERE session_id=?1 ORDER BY created_at,id",
        )?;
        Ok(stmt
            .query_map([session_id], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn expert_request(&self, id: &str, revision: i64) -> Result<ConsultationRequest> {
        self.expert_consultation(id)?
            .context("consultation missing")?;
        let json: String = self.conn.query_row(
            "SELECT request_json FROM expert_requests WHERE consultation_id=?1 AND revision=?2",
            params![id, revision],
            |row| row.get(0),
        )?;
        decode(&json)
    }

    /// Append an immutable request revision; a stale editor cannot overwrite
    /// newer evidence, a running attempt, or a submitted/ambiguous handoff.
    pub fn revise_expert_request(
        &self,
        id: &str,
        expected_revision: i64,
        request: &ConsultationRequest,
    ) -> Result<bool> {
        request.validate()?;
        let json = encode(request)?;
        let timestamp = now();
        let tx = self.conn.unchecked_transaction()?;
        let updated = tx.execute("UPDATE expert_consultations SET request_revision=request_revision+1,revision=revision+1,state='draft',handoff_state='none',updated_at=?3 WHERE id=?1 AND revision=?2 AND schema_version=1 AND state IN ('draft','completed','failed','incomplete','cancelled','timed_out','interrupted') AND handoff_state IN ('none','ready','dismissed')", params![id, expected_revision, timestamp])?;
        if updated == 0 {
            return Ok(false);
        }
        tx.execute("INSERT INTO expert_requests(consultation_id,revision,request_json,created_at) SELECT id,request_revision,?2,?3 FROM expert_consultations WHERE id=?1", params![id,json,timestamp])?;
        tx.commit()?;
        Ok(true)
    }

    /// Called only after the app's explicit pre-call confirmation. Persist the
    /// attempt before launching; never overwrite failed/retried work.
    pub fn begin_expert_attempt(
        &self,
        id: &str,
        expected_revision: i64,
        owner: &ConsultationOwner,
    ) -> Result<Option<String>> {
        owner.validate()?;
        let owner_json = encode(owner)?;
        let attempt = uuid::Uuid::new_v4().to_string();
        let timestamp = now();
        let tx = self.conn.unchecked_transaction()?;
        let updated = tx.execute("UPDATE expert_consultations SET state='running',revision=revision+1,active_attempt_id=?3,owner_json=?4,heartbeat_at=?5,updated_at=?5 WHERE id=?1 AND revision=?2 AND schema_version=1 AND state IN ('draft','failed','incomplete','cancelled','timed_out','interrupted') AND handoff_state NOT IN ('sending','submitted','delivery_unknown')", params![id,expected_revision,attempt,owner_json,timestamp])?;
        if updated == 0 {
            return Ok(None);
        }
        tx.execute("INSERT INTO expert_attempts(id,consultation_id,request_revision,number,owner_json,state,started_at) SELECT ?2,id,request_revision,(SELECT COALESCE(MAX(number),0)+1 FROM expert_attempts WHERE consultation_id=?1),?3,'running',?4 FROM expert_consultations WHERE id=?1", params![id,attempt,owner_json,timestamp])?;
        tx.commit()?;
        Ok(Some(attempt))
    }

    pub fn heartbeat_expert_owner(&self, id: &str, owner: &ConsultationOwner) -> Result<bool> {
        Ok(self.conn.execute("UPDATE expert_consultations SET heartbeat_at=?3 WHERE id=?1 AND owner_json=?2 AND schema_version=1 AND (state IN ('running','cancelling') OR handoff_state='sending')", params![id,encode(owner)?,now()])? == 1)
    }

    pub fn cancel_expert_attempt(&self, id: &str, expected_revision: i64) -> Result<bool> {
        Ok(self.conn.execute("UPDATE expert_consultations SET state='cancelling',revision=revision+1,updated_at=?3 WHERE id=?1 AND revision=?2 AND schema_version=1 AND state='running'", params![id,expected_revision,now()])? == 1)
    }

    pub fn finish_expert_attempt(
        &self,
        id: &str,
        attempt: &str,
        owner: &ConsultationOwner,
        mut outcome: ConsultationOutcome,
    ) -> Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        let state: Option<String> = tx.query_row("SELECT state FROM expert_consultations WHERE id=?1 AND active_attempt_id=?2 AND owner_json=?3 AND schema_version=1 AND state IN ('running','cancelling')", params![id,attempt,encode(owner)?], |row| row.get(0)).optional()?;
        let Some(state) = state else {
            return Ok(false);
        };
        if state == "cancelling" {
            outcome.status = HeadlessJobStatus::Cancelled;
            outcome.response = None;
            outcome.error = Some("consultation cancelled".into());
        }
        if outcome.status == HeadlessJobStatus::Completed {
            anyhow::ensure!(
                outcome.error.is_none()
                    && outcome
                        .response
                        .as_ref()
                        .is_some_and(|text| !text.trim().is_empty()),
                "completed attempt requires an answer and no error"
            );
        } else {
            outcome.response = None;
        }
        let json = encode(&outcome)?;
        let status = job_state(outcome.status);
        let timestamp = now();
        tx.execute("UPDATE expert_attempts SET state=?2,outcome_json=?3,finished_at=?4 WHERE id=?1 AND state='running'", params![attempt,status,json,timestamp])?;
        tx.execute("UPDATE expert_consultations SET state=?2,revision=revision+1,active_attempt_id=NULL,owner_json=NULL,heartbeat_at=NULL,updated_at=?3 WHERE id=?1", params![id,status,timestamp])?;
        tx.commit()?;
        Ok(true)
    }

    pub fn expert_attempt_outcomes(
        &self,
        id: &str,
    ) -> Result<Vec<(String, Option<ConsultationOutcome>)>> {
        self.expert_consultation(id)?
            .context("consultation missing")?;
        let mut stmt = self.conn.prepare(
            "SELECT id,outcome_json FROM expert_attempts WHERE consultation_id=?1 ORDER BY number",
        )?;
        let rows = stmt
            .query_map([id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter()
            .map(|(id, json)| Ok((id, json.as_deref().map(decode).transpose()?)))
            .collect()
    }

    /// Export consultation-local usage without pretending optional counters
    /// are billable cost. Failed and incomplete attempts remain in the export.
    /// A concrete cost appears only after a future pricing snapshot is bound.
    pub fn export_expert_evaluation(&self, id: &str) -> Result<String> {
        let consultation = self
            .expert_consultation(id)?
            .context("consultation missing")?;
        let request = self.expert_request(id, consultation.request_revision)?;
        let mut stmt = self.conn.prepare(
            "SELECT id,state,outcome_json FROM expert_attempts WHERE consultation_id=?1 ORDER BY number",
        )?;
        let attempts = stmt
            .query_map([id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(|(attempt_id, status, outcome)| {
                let parsed: Option<ConsultationOutcome> =
                    outcome.as_deref().map(decode).transpose()?;
                Ok(ExpertAttemptUsageExport {
                    attempt_id,
                    status,
                    usage: parsed
                        .as_ref()
                        .map_or_else(HeadlessUsage::default, |item| item.usage.clone()),
                    usage_complete: parsed.as_ref().is_some_and(|item| item.usage_complete),
                    elapsed_millis: parsed.as_ref().map(|item| item.elapsed_millis),
                    cost: None,
                    billing_basis: None,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let export = ExpertEvaluationExport {
            consultation_id: id.into(),
            schema_version: SCHEMA_VERSION,
            origin: consultation.origin,
            request_revision: consultation.request_revision,
            profile: request.profile,
            attempts,
            handoff_state: consultation.handoff_state,
        };
        let json = serde_json::to_string_pretty(&export)?;
        anyhow::ensure!(
            json.len() <= MAX_RECORD_BYTES,
            "evaluation export exceeds its size limit"
        );
        Ok(json)
    }

    /// The caller must validate the expert schema and freshness first. Draft
    /// revisions are separate from ordinary composer text/images and immutable.
    pub fn stage_expert_handoff(
        &self,
        id: &str,
        expected_revision: i64,
        body: &str,
    ) -> Result<bool> {
        anyhow::ensure!(
            !body.trim().is_empty() && body.len() <= MAX_HANDOFF_BYTES,
            "handoff must be nonempty and within its size limit"
        );
        let tx = self.conn.unchecked_transaction()?;
        let timestamp = now();
        let updated = tx.execute("UPDATE expert_consultations SET handoff_state='ready',handoff_revision=handoff_revision+1,revision=revision+1,updated_at=?3 WHERE id=?1 AND revision=?2 AND schema_version=1 AND state='completed' AND handoff_state IN ('none','ready','dismissed')", params![id,expected_revision,timestamp])?;
        if updated == 0 {
            return Ok(false);
        }
        tx.execute("INSERT INTO expert_handoffs(consultation_id,revision,body,created_at) SELECT id,handoff_revision,?2,?3 FROM expert_consultations WHERE id=?1", params![id,body,timestamp])?;
        tx.commit()?;
        Ok(true)
    }

    pub fn expert_handoff_body(&self, id: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT h.body FROM expert_handoffs h JOIN expert_consultations c ON c.id=h.consultation_id AND c.handoff_revision=h.revision WHERE c.id=?1 AND c.schema_version=1", [id], |row| row.get(0)).optional()?)
    }

    /// Explicit user Send is required by the app. CAS + unique revision stops
    /// double keypresses and concurrent instances from claiming the same send.
    pub fn claim_expert_delivery(
        &self,
        id: &str,
        expected_revision: i64,
        owner: &ConsultationOwner,
    ) -> Result<Option<String>> {
        owner.validate()?;
        let tx = self.conn.unchecked_transaction()?;
        let delivery = uuid::Uuid::new_v4().to_string();
        let timestamp = now();
        let updated = tx.execute("UPDATE expert_consultations SET handoff_state='sending',revision=revision+1,delivery_id=?3,owner_json=?4,heartbeat_at=?5,updated_at=?5 WHERE id=?1 AND revision=?2 AND schema_version=1 AND state='completed' AND handoff_state='ready'", params![id,expected_revision,delivery,encode(owner)?,timestamp])?;
        if updated == 0 {
            return Ok(None);
        }
        tx.execute("INSERT INTO expert_deliveries(id,consultation_id,handoff_revision,state,created_at) SELECT ?2,id,handoff_revision,'sending',?3 FROM expert_consultations WHERE id=?1", params![id,delivery,timestamp])?;
        tx.commit()?;
        Ok(Some(delivery))
    }

    pub fn finish_expert_delivery(
        &self,
        id: &str,
        delivery: &str,
        owner: &ConsultationOwner,
        submitted: bool,
    ) -> Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        let state = if submitted {
            "submitted"
        } else {
            "delivery_unknown"
        };
        let timestamp = now();
        let updated = tx.execute("UPDATE expert_consultations SET handoff_state=?4,revision=revision+1,owner_json=NULL,heartbeat_at=NULL,updated_at=?5 WHERE id=?1 AND delivery_id=?2 AND owner_json=?3 AND schema_version=1 AND handoff_state='sending'", params![id,delivery,encode(owner)?,state,timestamp])?;
        if updated == 0 {
            return Ok(false);
        }
        tx.execute(
            "UPDATE expert_deliveries SET state=?2,finished_at=?3 WHERE id=?1",
            params![delivery, state, timestamp],
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub fn dismiss_expert_handoff(&self, id: &str, expected_revision: i64) -> Result<bool> {
        Ok(self.conn.execute("UPDATE expert_consultations SET handoff_state='dismissed',revision=revision+1,updated_at=?3 WHERE id=?1 AND revision=?2 AND schema_version=1 AND handoff_state='ready'", params![id,expected_revision,now()])? == 1)
    }

    /// Stale heartbeat alone is insufficient: preserve live or unidentifiable
    /// owners. This only reconciles durable state; it never kills a stored PID.
    pub fn recover_expert_consultations(&self, stale_before: i64) -> Result<usize> {
        self.recover_expert_with(stale_before, ConsultationOwner::liveness)
    }

    fn recover_expert_with(
        &self,
        stale_before: i64,
        probe: impl Fn(&ConsultationOwner) -> OwnerLiveness,
    ) -> Result<usize> {
        let ids = {
            let mut stmt = self.conn.prepare("SELECT id FROM expert_consultations WHERE schema_version=1 AND heartbeat_at < ?1 AND (state IN ('running','cancelling') OR handoff_state='sending')")?;
            stmt.query_map([stale_before], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut count = 0;
        for id in ids {
            let Some(row) = self.expert_consultation(&id)? else {
                continue;
            };
            let Some(owner) = &row.owner else {
                continue;
            };
            if probe(owner) != OwnerLiveness::Dead {
                continue;
            }
            let tx = self.conn.unchecked_transaction()?;
            let timestamp = now();
            let updated = tx.execute("UPDATE expert_consultations SET state=CASE WHEN state IN ('running','cancelling') THEN 'interrupted' ELSE state END,handoff_state=CASE WHEN handoff_state='sending' THEN 'delivery_unknown' ELSE handoff_state END,revision=revision+1,owner_json=NULL,heartbeat_at=NULL,active_attempt_id=NULL,updated_at=?5 WHERE id=?1 AND revision=?2 AND owner_json=?3 AND heartbeat_at=?4 AND schema_version=1", params![id,row.revision,encode(owner)?,row.heartbeat_at,timestamp])?;
            if updated == 0 {
                continue;
            }
            if let Some(attempt) = row.active_attempt_id {
                tx.execute("UPDATE expert_attempts SET state='interrupted',finished_at=?2 WHERE id=?1 AND state='running'", params![attempt,timestamp])?;
            }
            if let Some(delivery) = row.delivery_id {
                tx.execute("UPDATE expert_deliveries SET state='delivery_unknown',finished_at=?2 WHERE id=?1 AND state='sending'", params![delivery,timestamp])?;
            }
            tx.commit()?;
            count += 1;
        }
        Ok(count)
    }

    /// Called after explicit deletion/retention decisions, never during an
    /// ordinary store save. Active jobs/deliveries must be reconciled first.
    pub fn delete_expert_consultation(&self, id: &str) -> Result<bool> {
        Ok(self.conn.execute("DELETE FROM expert_consultations WHERE id=?1 AND schema_version=1 AND state NOT IN ('running','cancelling') AND handoff_state!='sending'", [id])? == 1)
    }
}

#[cfg(test)]
mod tests;
