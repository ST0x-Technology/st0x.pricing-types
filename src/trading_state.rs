//! Authenticated HTTP schedule contract. A schedule is not broker execution permission.

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "i64", into = "i64")]
pub struct UnixMillis(i64);

impl UnixMillis {
    pub fn get(self) -> i64 {
        self.0
    }
}

impl TryFrom<i64> for UnixMillis {
    type Error = &'static str;
    fn try_from(value: i64) -> Result<Self, Self::Error> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err("timestamp must be positive")
        }
    }
}

impl From<UnixMillis> for i64 {
    fn from(value: UnixMillis) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ScopeId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Revision(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IntervalId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Environment {
    Staging,
    Production,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EligibleSessions {
    Regular,
    Extended,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TradingScope {
    pub id: ScopeId,
    pub profile_revision: Revision,
    pub sessions: EligibleSessions,
    pub assets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TradingInterval {
    pub id: IntervalId,
    pub opens_at: UnixMillis,
    /// Exclusive execution boundary in UTC Unix milliseconds.
    /// Consumers must require `now_ms < execution_cutoff`; equality forbids execution.
    pub execution_cutoff: UnixMillis,
    pub hedge_close: UnixMillis,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarEvidence {
    pub fetched_at: UnixMillis,
    pub coverage_start: UnixMillis,
    pub coverage_end: UnixMillis,
    pub revision: Revision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownReason {
    CalendarUnavailable,
    CalendarStale,
    InsufficientCoverage,
    InvalidCalendar,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Schedule {
    Open {
        current: TradingInterval,
        next: Option<TradingInterval>,
    },
    Draining {
        current: TradingInterval,
        next: Option<TradingInterval>,
    },
    Closed {
        previous: Option<TradingInterval>,
        next: Option<TradingInterval>,
    },
    Unknown {
        reason: UnknownReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuoteUnavailableReason {
    ScheduledClosure,
    CalendarUnavailable,
    PriceUnavailable,
    Maintenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum QuoteAvailability {
    Available,
    Unavailable { reason: QuoteUnavailableReason },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TradingState {
    #[serde(deserialize_with = "deserialize_schema_version")]
    pub schema_version: u16,
    pub environment: Environment,
    pub scope: TradingScope,
    pub policy_revision: Revision,
    pub observed_at: UnixMillis,
    pub valid_until: UnixMillis,
    pub calendar: Option<CalendarEvidence>,
    pub schedule: Schedule,
    pub quote_availability: QuoteAvailability,
}

fn deserialize_schema_version<'de, D>(deserializer: D) -> Result<u16, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let version = u16::deserialize(deserializer)?;
    if version == SCHEMA_VERSION {
        Ok(version)
    } else {
        Err(serde::de::Error::custom(
            "unsupported trading schedule schema version",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_schema_versions_are_rejected() {
        for version in [0, SCHEMA_VERSION + 1] {
            let error = serde_json::from_value::<TradingState>(serde_json::json!({
                "schema_version": version,
                "environment": "production",
                "scope": {
                    "id": "us-equities",
                    "profile_revision": "profile-1",
                    "sessions": "extended",
                    "assets": ["AAPL"]
                },
                "policy_revision": "policy-1",
                "observed_at": 150,
                "valid_until": 190,
                "calendar": null,
                "schedule": {"phase": "unknown", "reason": "calendar_unavailable"},
                "quote_availability": {"status": "unavailable", "reason": "calendar_unavailable"}
            }))
            .unwrap_err();
            assert!(error.is_data());
            assert!(error
                .to_string()
                .contains("unsupported trading schedule schema version"));
        }
    }

    #[test]
    fn timestamps_reject_nonpositive_json() {
        for value in ["0", "-1", "1.5", "\"100\""] {
            assert!(serde_json::from_str::<UnixMillis>(value).is_err());
        }
        assert_eq!(
            serde_json::from_str::<UnixMillis>("123").unwrap().get(),
            123
        );
    }

    #[test]
    fn trading_state_json_contract() {
        let state = TradingState {
            schema_version: SCHEMA_VERSION,
            environment: Environment::Production,
            scope: TradingScope {
                id: ScopeId("us-equities".into()),
                profile_revision: Revision("profile-1".into()),
                sessions: EligibleSessions::Extended,
                assets: vec!["AAPL".into()],
            },
            policy_revision: Revision("policy-1".into()),
            observed_at: 150.try_into().unwrap(),
            valid_until: 190.try_into().unwrap(),
            calendar: Some(CalendarEvidence {
                fetched_at: 90.try_into().unwrap(),
                coverage_start: 100.try_into().unwrap(),
                coverage_end: 600.try_into().unwrap(),
                revision: Revision("calendar-1".into()),
            }),
            schedule: Schedule::Open {
                current: TradingInterval {
                    id: IntervalId("alpaca:100".into()),
                    opens_at: 100.try_into().unwrap(),
                    execution_cutoff: 200.try_into().unwrap(),
                    hedge_close: 300.try_into().unwrap(),
                },
                next: None,
            },
            quote_availability: QuoteAvailability::Available,
        };
        let mut expected = serde_json::json!({
            "schema_version": 1,
            "environment": "production",
            "scope": {
                "id": "us-equities",
                "profile_revision": "profile-1",
                "sessions": "extended",
                "assets": ["AAPL"]
            },
            "policy_revision": "policy-1",
            "observed_at": 150,
            "valid_until": 190,
            "calendar": {
                "fetched_at": 90,
                "coverage_start": 100,
                "coverage_end": 600,
                "revision": "calendar-1"
            },
            "schedule": {
                "phase": "open",
                "current": {
                    "id": "alpaca:100",
                    "opens_at": 100,
                    "execution_cutoff": 200,
                    "hedge_close": 300
                },
                "next": null
            },
            "quote_availability": {"status": "available"}
        });
        assert_eq!(serde_json::to_value(&state).unwrap(), expected);
        assert_eq!(
            serde_json::from_value::<TradingState>(expected.clone()).unwrap(),
            state
        );

        let without_calendar = TradingState {
            calendar: None,
            ..state
        };
        expected["calendar"] = serde_json::Value::Null;
        assert_eq!(serde_json::to_value(&without_calendar).unwrap(), expected);
        assert_eq!(
            serde_json::from_value::<TradingState>(expected).unwrap(),
            without_calendar
        );
    }

    #[test]
    fn remaining_phase_and_availability_json_contracts() {
        let interval = TradingInterval {
            id: IntervalId("alpaca:100".into()),
            opens_at: 100.try_into().unwrap(),
            execution_cutoff: 200.try_into().unwrap(),
            hedge_close: 300.try_into().unwrap(),
        };
        let interval_json = serde_json::json!({
            "id": "alpaca:100",
            "opens_at": 100,
            "execution_cutoff": 200,
            "hedge_close": 300
        });
        for (schedule, expected) in [
            (
                Schedule::Draining {
                    current: interval.clone(),
                    next: None,
                },
                serde_json::json!({"phase": "draining", "current": interval_json, "next": null}),
            ),
            (
                Schedule::Closed {
                    previous: Some(interval),
                    next: Some(TradingInterval {
                        id: IntervalId("alpaca:400".into()),
                        opens_at: 400.try_into().unwrap(),
                        execution_cutoff: 500.try_into().unwrap(),
                        hedge_close: 600.try_into().unwrap(),
                    }),
                },
                serde_json::json!({"phase": "closed", "previous": interval_json, "next": {
                    "id": "alpaca:400", "opens_at": 400, "execution_cutoff": 500, "hedge_close": 600
                }}),
            ),
            (
                Schedule::Closed {
                    previous: None,
                    next: None,
                },
                serde_json::json!({"phase": "closed", "previous": null, "next": null}),
            ),
            (
                Schedule::Unknown {
                    reason: UnknownReason::CalendarStale,
                },
                serde_json::json!({"phase": "unknown", "reason": "calendar_stale"}),
            ),
        ] {
            assert_eq!(serde_json::to_value(&schedule).unwrap(), expected);
            assert_eq!(
                serde_json::from_value::<Schedule>(expected).unwrap(),
                schedule
            );
        }
        let unavailable = QuoteAvailability::Unavailable {
            reason: QuoteUnavailableReason::ScheduledClosure,
        };
        let expected = serde_json::json!({"status": "unavailable", "reason": "scheduled_closure"});
        assert_eq!(serde_json::to_value(&unavailable).unwrap(), expected);
        assert_eq!(
            serde_json::from_value::<QuoteAvailability>(expected).unwrap(),
            unavailable
        );
        assert!(serde_json::from_str::<Schedule>(r#"{"phase":"future"}"#).is_err());
    }
}
