//! Pure scheduling windows. No clock, system timezone, priority, permit or issuer.
//!
//! New windows use the explicitly bundled TZif bytes. Historical replay uses the
//! retained UTC endpoints and an independently retained record digest; it never
//! re-resolves a civil date against an updated timezone database. Digests detect
//! changed data, but do not authenticate a profile, observation or override.
use crate::{ProjectId, RootId, content_digest, interfaces::DecimalU64, limits::bounded_json_size};
use jiff::{
    Timestamp,
    civil::{Date, DateTime, Time},
    tz::{AmbiguousOffset, Offset, TimeZone},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub const SCHEDULING_PROFILE_VERSION: u32 = 1;
pub const MAX_PROFILE_BYTES: usize = 65_536;
pub const MAX_WINDOWS: usize = 32;
pub const MAX_BLOCKED_DATES: usize = 366;
pub const MAX_TZIF_BYTES: usize = 65_536;
pub const MAX_WINDOW_SECONDS: u64 = 72 * 60 * 60;
pub const MAX_OVERRIDE_SECONDS: u64 = 7 * 24 * 60 * 60;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileScope {
    pub project_id: ProjectId,
    pub root_id: RootId,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SchedulingProfile {
    pub schema_version: u32,
    pub profile_id: String,
    pub scope: ProfileScope,
    pub revision: DecimalU64,
    pub mode: SchedulingMode,
    pub allow_window_override: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SchedulingMode {
    Immediate,
    Scheduled {
        timezone: String,
        windows: Vec<WeeklyWindow>,
        /// Holidays are explicit local START dates; an end date is not a shift.
        blocked_start_dates: Vec<String>,
        fold: FoldPolicy,
        gap: GapPolicy,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WeeklyWindow {
    pub window_id: String,
    /// ISO weekdays, Monday=1 through Sunday=7. Repetitions are rejected.
    pub weekdays: Vec<u8>,
    /// Canonical second-resolution HH:MM:SS, with no 24:00 or leap seconds.
    pub start: String,
    pub end: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FoldPolicy {
    Earlier,
    Later,
    Reject,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GapPolicy {
    SkipWindow,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimezoneIdentity {
    pub parser: String,
    pub bundle_version: String,
    pub iana_version: String,
    pub timezone: String,
    pub tzif_digest: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EndpointChoice {
    Unambiguous,
    FoldEarlier,
    FoldLater,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResolvedEndpoint {
    pub local_datetime: String,
    pub unix_seconds: DecimalU64,
    pub offset_seconds: i32,
    pub choice: EndpointChoice,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResolvedWindow {
    pub schema_version: u32,
    pub profile_digest: String,
    pub scope: ProfileScope,
    pub window_id: String,
    pub shift_start_date: String,
    pub timezone: TimezoneIdentity,
    pub start: ResolvedEndpoint,
    pub end: ResolvedEndpoint,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WindowResolution {
    Resolved { window: Box<ResolvedWindow> },
    NotScheduled,
    WeekdayNotApplicable,
    BlockedStartDate,
    GapSkipped,
    FoldRejected,
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum SchedulingProfileError {
    #[error("scheduling input exceeds a bound")]
    Limit,
    #[error("invalid scheduling data: {0}")]
    Invalid(&'static str),
    #[error("scheduling profile scope mismatch")]
    ScopeMismatch,
    #[error("unsupported scheduling profile version")]
    Version,
    #[error("unknown or noncanonical bundled timezone")]
    Timezone,
    #[error("instant or civil date is outside the supported range")]
    Range,
    #[error("recorded scheduling data does not match its independent binding")]
    Binding,
}
type Result<T> = std::result::Result<T, SchedulingProfileError>;

fn ident(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}
fn digest(s: &str) -> bool {
    s.len() == 71
        && s.starts_with("sha256:")
        && s.as_bytes()[7..]
            .iter()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
}
fn date(s: &str) -> Result<Date> {
    if s.len() != 10 || !s.is_ascii() {
        return Err(SchedulingProfileError::Invalid("date"));
    }
    let d: Date = s
        .parse()
        .map_err(|_| SchedulingProfileError::Invalid("date"))?;
    if d.year() < 1970 || d.to_string() != s {
        return Err(SchedulingProfileError::Range);
    }
    Ok(d)
}
fn time(s: &str) -> Result<Time> {
    if s.len() != 8 || !s.is_ascii() {
        return Err(SchedulingProfileError::Invalid("time"));
    }
    let t: Time = s
        .parse()
        .map_err(|_| SchedulingProfileError::Invalid("time"))?;
    if t.to_string() != s {
        return Err(SchedulingProfileError::Invalid("time"));
    }
    Ok(t)
}
fn instant(s: u64) -> Result<Timestamp> {
    Timestamp::from_second(i64::try_from(s).map_err(|_| SchedulingProfileError::Range)?)
        .map_err(|_| SchedulingProfileError::Range)
}
fn bound<T: Serialize>(value: &T) -> Result<()> {
    bounded_json_size(value, MAX_PROFILE_BYTES)
        .map(|_| ())
        .map_err(|_| SchedulingProfileError::Limit)
}

impl SchedulingProfile {
    /// Structural frozen-data checks, deliberately independent of the installed
    /// bundle. Historical profile identity and replay must remain usable after
    /// a zone is renamed or removed by a later tzdb release.
    pub fn validate_structure(&self, expected_scope: &ProfileScope) -> Result<()> {
        bound(self)?;
        if self.schema_version != SCHEDULING_PROFILE_VERSION {
            return Err(SchedulingProfileError::Version);
        }
        if &self.scope != expected_scope {
            return Err(SchedulingProfileError::ScopeMismatch);
        }
        if !ident(&self.profile_id) || self.revision.get() == 0 {
            return Err(SchedulingProfileError::Invalid("profile identity"));
        }
        if let SchedulingMode::Scheduled {
            timezone,
            windows,
            blocked_start_dates,
            ..
        } = &self.mode
        {
            if timezone.is_empty()
                || timezone.len() > 128
                || !timezone
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"/_+-".contains(&b))
            {
                return Err(SchedulingProfileError::Timezone);
            }
            if windows.is_empty()
                || windows.len() > MAX_WINDOWS
                || blocked_start_dates.len() > MAX_BLOCKED_DATES
            {
                return Err(SchedulingProfileError::Limit);
            }
            let mut ids = BTreeSet::new();
            for w in windows {
                if !ident(&w.window_id) || !ids.insert(&w.window_id) {
                    return Err(SchedulingProfileError::Invalid("window identity"));
                }
                if w.weekdays.is_empty() || w.weekdays.len() > 7 {
                    return Err(SchedulingProfileError::Invalid("weekdays"));
                }
                let mut days = BTreeSet::new();
                for d in &w.weekdays {
                    if !(1..=7).contains(d) || !days.insert(d) {
                        return Err(SchedulingProfileError::Invalid("weekdays"));
                    }
                }
                if time(&w.start)? == time(&w.end)? {
                    return Err(SchedulingProfileError::Invalid("zero-length window"));
                }
            }
            let mut dates = BTreeSet::new();
            for d in blocked_start_dates {
                date(d)?;
                if !dates.insert(d) {
                    return Err(SchedulingProfileError::Invalid("duplicate blocked date"));
                }
            }
        }
        Ok(())
    }

    /// New resolution/admission additionally requires an exact canonical zone
    /// in the pinned current bundle, even for a blocked or irrelevant weekday.
    pub fn validate_current(&self, expected_scope: &ProfileScope) -> Result<()> {
        self.validate_structure(expected_scope)?;
        if let SchedulingMode::Scheduled { timezone, .. } = &self.mode {
            bundled_zone(timezone)?;
        }
        Ok(())
    }
    pub fn identity_digest(&self, expected_scope: &ProfileScope) -> Result<String> {
        self.validate_structure(expected_scope)?;
        content_digest(self).map_err(|_| SchedulingProfileError::Invalid("profile encoding"))
    }
}

/// Bounded closed JSON. Parsing and a valid shape confer no scheduling authority.
pub fn parse_scheduling_profile(bytes: &[u8]) -> Result<SchedulingProfile> {
    if bytes.len() > MAX_PROFILE_BYTES {
        return Err(SchedulingProfileError::Limit);
    }
    serde_json::from_slice(bytes).map_err(|_| SchedulingProfileError::Invalid("profile JSON"))
}

fn bundled_zone(name: &str) -> Result<(TimeZone, TimezoneIdentity)> {
    let (canonical, bytes) = jiff_tzdb::get(name).ok_or(SchedulingProfileError::Timezone)?;
    if canonical != name {
        return Err(SchedulingProfileError::Timezone);
    }
    if bytes.len() > MAX_TZIF_BYTES {
        return Err(SchedulingProfileError::Limit);
    }
    let zone = TimeZone::tzif(canonical, bytes).map_err(|_| SchedulingProfileError::Timezone)?;
    Ok((
        zone,
        TimezoneIdentity {
            parser: "jiff/0.2.38".into(),
            bundle_version: "jiff-tzdb/0.1.9".into(),
            iana_version: jiff_tzdb::VERSION
                .ok_or(SchedulingProfileError::Timezone)?
                .into(),
            timezone: canonical.into(),
            tzif_digest: format!("sha256:{:x}", Sha256::digest(bytes)),
        },
    ))
}

fn endpoint(
    zone: &TimeZone,
    dt: DateTime,
    fold: FoldPolicy,
) -> Result<std::result::Result<ResolvedEndpoint, WindowResolution>> {
    let ambiguous = zone.to_ambiguous_timestamp(dt);
    let choice = match ambiguous.offset() {
        AmbiguousOffset::Unambiguous { .. } => EndpointChoice::Unambiguous,
        AmbiguousOffset::Gap { .. } => return Ok(Err(WindowResolution::GapSkipped)),
        AmbiguousOffset::Fold { .. } => match fold {
            FoldPolicy::Earlier => EndpointChoice::FoldEarlier,
            FoldPolicy::Later => EndpointChoice::FoldLater,
            FoldPolicy::Reject => return Ok(Err(WindowResolution::FoldRejected)),
        },
    };
    let ts = match choice {
        EndpointChoice::Unambiguous => ambiguous.unambiguous(),
        EndpointChoice::FoldEarlier => ambiguous.earlier(),
        EndpointChoice::FoldLater => ambiguous.later(),
    }
    .map_err(|_| SchedulingProfileError::Range)?;
    let seconds = u64::try_from(ts.as_second()).map_err(|_| SchedulingProfileError::Range)?;
    let offset = zone.to_offset(ts).seconds();
    Ok(Ok(ResolvedEndpoint {
        local_datetime: dt.to_string(),
        unix_seconds: DecimalU64::new(seconds),
        offset_seconds: offset,
        choice,
    }))
}

/// Resolve one exact local start date. End <= start is next-day, but equal times
/// have already been rejected. A gap skips the entire window; no silent shift.
pub fn resolve_window(
    profile: &SchedulingProfile,
    expected_scope: &ProfileScope,
    window_id: &str,
    shift_start_date: &str,
) -> Result<WindowResolution> {
    profile.validate_current(expected_scope)?;
    if window_id.len() > 128 {
        return Err(SchedulingProfileError::Limit);
    }
    let d = date(shift_start_date)?;
    let SchedulingMode::Scheduled {
        timezone,
        windows,
        blocked_start_dates,
        fold,
        ..
    } = &profile.mode
    else {
        return Ok(WindowResolution::NotScheduled);
    };
    let w = windows
        .iter()
        .find(|w| w.window_id == window_id)
        .ok_or(SchedulingProfileError::Invalid("unknown window"))?;
    if !w
        .weekdays
        .contains(&(d.weekday().to_monday_one_offset() as u8))
    {
        return Ok(WindowResolution::WeekdayNotApplicable);
    }
    if blocked_start_dates.iter().any(|v| v == shift_start_date) {
        return Ok(WindowResolution::BlockedStartDate);
    }
    let (zone, identity) = bundled_zone(timezone)?;
    let start_time = time(&w.start)?;
    let end_time = time(&w.end)?;
    let end_date = if end_time < start_time {
        d.tomorrow().map_err(|_| SchedulingProfileError::Range)?
    } else {
        d
    };
    let start = match endpoint(&zone, d.to_datetime(start_time), *fold)? {
        Ok(v) => v,
        Err(reason) => return Ok(reason),
    };
    let end = match endpoint(&zone, end_date.to_datetime(end_time), *fold)? {
        Ok(v) => v,
        Err(reason) => return Ok(reason),
    };
    let elapsed = end
        .unix_seconds
        .get()
        .checked_sub(start.unix_seconds.get())
        .ok_or(SchedulingProfileError::Invalid("window chronology"))?;
    if elapsed == 0 || elapsed > MAX_WINDOW_SECONDS {
        return Err(SchedulingProfileError::Invalid("window chronology"));
    }
    Ok(WindowResolution::Resolved {
        window: Box::new(ResolvedWindow {
            schema_version: SCHEDULING_PROFILE_VERSION,
            profile_digest: profile.identity_digest(expected_scope)?,
            scope: expected_scope.clone(),
            window_id: window_id.into(),
            shift_start_date: shift_start_date.into(),
            timezone: identity,
            start,
            end,
        }),
    })
}

impl ResolvedWindow {
    pub fn identity_digest(&self) -> Result<String> {
        bound(self)?;
        content_digest(self).map_err(|_| SchedulingProfileError::Invalid("window encoding"))
    }
}

/// Check original bindings and recorded arithmetic without any timezone lookup.
/// `expected_digest` must be independently retained, not copied from this record.
pub fn validate_recorded_window(
    profile: &SchedulingProfile,
    expected_scope: &ProfileScope,
    window: &ResolvedWindow,
    expected_digest: &str,
) -> Result<()> {
    profile.validate_structure(expected_scope)?;
    bound(window)?;
    if !digest(expected_digest)
        || window.identity_digest()? != expected_digest
        || window.scope != *expected_scope
        || window.schema_version != SCHEDULING_PROFILE_VERSION
        || window.profile_digest != profile.identity_digest(expected_scope)?
    {
        return Err(SchedulingProfileError::Binding);
    }
    let SchedulingMode::Scheduled {
        timezone,
        windows,
        blocked_start_dates,
        fold,
        ..
    } = &profile.mode
    else {
        return Err(SchedulingProfileError::Binding);
    };
    let w = windows
        .iter()
        .find(|w| w.window_id == window.window_id)
        .ok_or(SchedulingProfileError::Binding)?;
    let d = date(&window.shift_start_date)?;
    if !w
        .weekdays
        .contains(&(d.weekday().to_monday_one_offset() as u8))
        || blocked_start_dates.contains(&window.shift_start_date)
        || window.timezone.timezone != *timezone
        || !digest(&window.timezone.tzif_digest)
        || window.timezone.parser.is_empty()
        || window.timezone.parser.len() > 128
        || window.timezone.bundle_version.is_empty()
        || window.timezone.bundle_version.len() > 128
        || window.timezone.iana_version.is_empty()
        || window.timezone.iana_version.len() > 32
    {
        return Err(SchedulingProfileError::Binding);
    }
    let st = time(&w.start)?;
    let et = time(&w.end)?;
    let ed = if et < st {
        d.tomorrow().map_err(|_| SchedulingProfileError::Range)?
    } else {
        d
    };
    for (record, local) in [
        (&window.start, d.to_datetime(st)),
        (&window.end, ed.to_datetime(et)),
    ] {
        if record.local_datetime != local.to_string() {
            return Err(SchedulingProfileError::Binding);
        }
        let offset = Offset::from_seconds(record.offset_seconds)
            .map_err(|_| SchedulingProfileError::Binding)?;
        let ts = offset
            .to_timestamp(local)
            .map_err(|_| SchedulingProfileError::Binding)?;
        if u64::try_from(ts.as_second()).ok() != Some(record.unix_seconds.get()) {
            return Err(SchedulingProfileError::Binding);
        }
        match (record.choice, fold) {
            (EndpointChoice::Unambiguous, _)
            | (EndpointChoice::FoldEarlier, FoldPolicy::Earlier)
            | (EndpointChoice::FoldLater, FoldPolicy::Later) => {}
            _ => return Err(SchedulingProfileError::Binding),
        }
    }
    let elapsed = window
        .end
        .unix_seconds
        .get()
        .checked_sub(window.start.unix_seconds.get())
        .ok_or(SchedulingProfileError::Binding)?;
    if elapsed == 0 || elapsed > MAX_WINDOW_SECONDS {
        return Err(SchedulingProfileError::Binding);
    }
    Ok(())
}

/// Trusted service-local input only. Constructing this binding is an explicit
/// grant by that caller; this module neither authenticates it nor issues one
/// from serialized data. It can waive a window, never other admission checks.
#[derive(Debug)]
pub struct ExternallyGrantedOverride {
    scope: ProfileScope,
    profile_digest: String,
    authority_ref: String,
    reason: String,
    issued: u64,
    expires: u64,
}
impl ExternallyGrantedOverride {
    pub fn from_trusted_binding(
        scope: ProfileScope,
        profile_digest: String,
        authority_ref: String,
        reason: String,
        issued: u64,
        expires: u64,
    ) -> Result<Self> {
        if !digest(&profile_digest)
            || !ident(&authority_ref)
            || reason.is_empty()
            || reason.len() > 1024
            || reason.chars().any(char::is_control)
            || expires <= issued
            || expires - issued > MAX_OVERRIDE_SECONDS
        {
            return Err(SchedulingProfileError::Invalid("override binding"));
        }
        instant(issued)?;
        instant(expires)?;
        Ok(Self {
            scope,
            profile_digest,
            authority_ref,
            reason,
            issued,
            expires,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SetupObservation {
    Unknown,
    ToolchainAbsent,
    Checked {
        exit_code: i32,
        timed_out: bool,
        timeout_seconds: u32,
        noninteractive_environment_digest: String,
    },
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Available,
    Waiting,
    Unknown,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentObservations {
    pub setup: SetupObservation,
    pub memory: Availability,
    pub disk: Availability,
    pub quota: Availability,
    pub decisions: Availability,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EligibilityReason {
    ClockUnknown,
    WaitingWindow,
    ToolchainAbsent,
    SetupUnknown,
    SetupFailed,
    SetupTimeout,
    NoninteractiveEnvironmentUnknown,
    MemoryPressure,
    MemoryUnknown,
    DiskPressure,
    DiskUnknown,
    QuotaWait,
    QuotaUnknown,
    MissingDecision,
    DecisionUnknown,
    OverrideNotYetValid,
    OverrideExpired,
    OverrideNotAllowed,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EligibilityDecision {
    pub profile_digest: String,
    pub observed_unix_seconds: Option<DecimalU64>,
    pub window_eligible: bool,
    pub eligible: bool,
    pub reasons: Vec<EligibilityReason>,
    pub window: Option<Box<ResolvedWindow>>,
    pub override_provenance: Option<OverrideProvenance>,
    pub execution_granted: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OverrideProvenance {
    pub authority_ref: String,
    pub reason: String,
    pub issued: DecimalU64,
    pub expires: DecimalU64,
}

fn environment_reasons(observations: &EnvironmentObservations) -> Result<Vec<EligibilityReason>> {
    bound(observations)?;
    let mut reasons = Vec::new();
    match &observations.setup {
        SetupObservation::Unknown => reasons.push(EligibilityReason::SetupUnknown),
        SetupObservation::ToolchainAbsent => reasons.push(EligibilityReason::ToolchainAbsent),
        SetupObservation::Checked {
            exit_code,
            timed_out,
            timeout_seconds,
            noninteractive_environment_digest,
        } => {
            if *timeout_seconds == 0 || *timeout_seconds > 86_400 {
                return Err(SchedulingProfileError::Invalid("setup deadline"));
            }
            if !digest(noninteractive_environment_digest) {
                reasons.push(EligibilityReason::NoninteractiveEnvironmentUnknown);
            }
            if *timed_out {
                reasons.push(EligibilityReason::SetupTimeout);
            } else if *exit_code != 0 {
                reasons.push(EligibilityReason::SetupFailed);
            }
        }
    }
    for (observation, waiting, unknown) in [
        (
            observations.memory,
            EligibilityReason::MemoryPressure,
            EligibilityReason::MemoryUnknown,
        ),
        (
            observations.disk,
            EligibilityReason::DiskPressure,
            EligibilityReason::DiskUnknown,
        ),
        (
            observations.quota,
            EligibilityReason::QuotaWait,
            EligibilityReason::QuotaUnknown,
        ),
        (
            observations.decisions,
            EligibilityReason::MissingDecision,
            EligibilityReason::DecisionUnknown,
        ),
    ] {
        match observation {
            Availability::Available => {}
            Availability::Waiting => reasons.push(waiting),
            Availability::Unknown => reasons.push(unknown),
        }
    }
    Ok(reasons)
}

fn decide(
    profile: &SchedulingProfile,
    expected_scope: &ProfileScope,
    now: Option<u64>,
    window: Option<ResolvedWindow>,
    mut window_eligible: bool,
    environment: &EnvironmentObservations,
    granted_override: Option<&ExternallyGrantedOverride>,
) -> Result<EligibilityDecision> {
    let profile_digest = profile.identity_digest(expected_scope)?;
    let mut reasons = environment_reasons(environment)?;
    let mut provenance = None;
    if let Some(grant) = granted_override {
        if grant.scope != *expected_scope || grant.profile_digest != profile_digest {
            return Err(SchedulingProfileError::Binding);
        }
        if window_eligible { /* An unused override cannot block an already open window. */
        } else if !profile.allow_window_override {
            reasons.push(EligibilityReason::OverrideNotAllowed);
        } else if let Some(now) = now {
            if now < grant.issued {
                reasons.push(EligibilityReason::OverrideNotYetValid);
            } else if now >= grant.expires {
                reasons.push(EligibilityReason::OverrideExpired);
            } else {
                window_eligible = true;
                provenance = Some(OverrideProvenance {
                    authority_ref: grant.authority_ref.clone(),
                    reason: grant.reason.clone(),
                    issued: DecimalU64::new(grant.issued),
                    expires: DecimalU64::new(grant.expires),
                });
            }
        }
    }
    if now.is_none() {
        reasons.push(EligibilityReason::ClockUnknown);
        window_eligible = false;
    } else if !window_eligible {
        reasons.push(EligibilityReason::WaitingWindow);
    }
    Ok(EligibilityDecision {
        profile_digest,
        observed_unix_seconds: now.map(DecimalU64::new),
        window_eligible,
        eligible: window_eligible && reasons.is_empty(),
        reasons,
        window: window.map(Box::new),
        override_provenance: provenance,
        execution_granted: false,
    })
}

/// New observation against the pinned bundle. At most 5 dates x 32 windows are
/// considered, covering overnight and date-line transitions; lexical ID then
/// local start date breaks overlaps deterministically. This is not priority.
pub fn evaluate_eligibility(
    profile: &SchedulingProfile,
    expected_scope: &ProfileScope,
    observed_unix_seconds: Option<u64>,
    environment: &EnvironmentObservations,
    granted_override: Option<&ExternallyGrantedOverride>,
) -> Result<EligibilityDecision> {
    profile.validate_current(expected_scope)?;
    let mut selected = None;
    let immediate = matches!(profile.mode, SchedulingMode::Immediate);
    if let Some(now) = observed_unix_seconds {
        let ts = instant(now)?;
        if let SchedulingMode::Scheduled {
            timezone, windows, ..
        } = &profile.mode
        {
            let (zone, _) = bundled_zone(timezone)?;
            let local_date = zone.to_datetime(ts).date();
            let mut matches = Vec::new();
            for delta in -2..=2 {
                let Ok(d) = local_date.checked_add(jiff::Span::new().days(delta)) else {
                    continue;
                };
                if d.year() < 1970 {
                    continue;
                }
                for w in windows {
                    // Neighboring dates can have endpoints outside the declared
                    // Unix/Jiff domain even while this observation is valid.
                    // Only that range error is irrelevant to current eligibility;
                    // malformed profiles, zone failures and chronology still fail.
                    let candidate =
                        match resolve_window(profile, expected_scope, &w.window_id, &d.to_string())
                        {
                            Ok(candidate) => candidate,
                            Err(SchedulingProfileError::Range) => continue,
                            Err(error) => return Err(error),
                        };
                    if let WindowResolution::Resolved { window } = candidate
                        && now >= window.start.unix_seconds.get()
                        && now < window.end.unix_seconds.get()
                    {
                        matches.push(*window);
                    }
                }
            }
            matches.sort_by(|a, b| {
                (&a.window_id, &a.shift_start_date).cmp(&(&b.window_id, &b.shift_start_date))
            });
            selected = matches.into_iter().next();
        }
    }
    let open = immediate || selected.is_some();
    decide(
        profile,
        expected_scope,
        observed_unix_seconds,
        selected,
        open,
        environment,
        granted_override,
    )
}

/// Replay an independently bound saved resolution after restart. No TZif lookup
/// occurs here, even when its recorded bundle is older than the installed one.
pub fn evaluate_recorded_eligibility(
    profile: &SchedulingProfile,
    expected_scope: &ProfileScope,
    window: &ResolvedWindow,
    expected_digest: &str,
    observed_unix_seconds: Option<u64>,
    environment: &EnvironmentObservations,
    granted_override: Option<&ExternallyGrantedOverride>,
) -> Result<EligibilityDecision> {
    validate_recorded_window(profile, expected_scope, window, expected_digest)?;
    if let Some(now) = observed_unix_seconds {
        instant(now)?;
    }
    let open = observed_unix_seconds
        .is_some_and(|n| n >= window.start.unix_seconds.get() && n < window.end.unix_seconds.get());
    decide(
        profile,
        expected_scope,
        observed_unix_seconds,
        Some(window.clone()),
        open,
        environment,
        granted_override,
    )
}
