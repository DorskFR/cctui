//! Worker handles bound to the run they were issued for.
//!
//! A Job's name derives from the dispatch's dedup key, so a later run of the
//! same key reuses the name. A handle names the session whose run it points at,
//! `jobs/<name>/<session_id>`; a Job under that name that runs another
//! session is not that handle's workload. A bare `jobs/<name>` handle matches
//! whatever Job holds the name.

use k8s_openapi::api::batch::v1::Job;

use super::ANNOTATION_SESSION_ID;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct JobHandle<'a> {
    pub(super) name: &'a str,
    pub(super) session: Option<&'a str>,
}

impl<'a> JobHandle<'a> {
    pub(super) fn parse(handle: &'a str) -> Self {
        let rest = handle.strip_prefix("jobs/").unwrap_or(handle);
        match rest.split_once('/') {
            Some((name, session)) if !session.is_empty() => Self { name, session: Some(session) },
            Some((name, _)) => Self { name, session: None },
            None => Self { name: rest, session: None },
        }
    }

    /// Whether `job` runs the session this handle was issued for.
    pub(super) fn owns(self, job: &Job) -> bool {
        let Some(session) = self.session else { return true };
        job_session(job).is_none_or(|owner| owner == session)
    }
}

/// The handle of `session`'s run in the Job called `name`.
pub(super) fn format(name: &str, session: Option<&str>) -> String {
    match session.filter(|s| !s.is_empty()) {
        Some(session) => format!("jobs/{name}/{session}"),
        None => format!("jobs/{name}"),
    }
}

/// The session a Job was created for.
pub(super) fn job_session(job: &Job) -> Option<&str> {
    job.metadata.annotations.as_ref()?.get(ANNOTATION_SESSION_ID).map(String::as_str)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn job_for(session: Option<&str>) -> Job {
        let annotations =
            session.map_or_else(|| json!({}), |s| json!({ ANNOTATION_SESSION_ID: s }));
        serde_json::from_value(json!({ "metadata": { "name": "w", "annotations": annotations } }))
            .unwrap()
    }

    #[test]
    fn a_handle_round_trips_its_name_and_session() {
        let h = format("cctui-worker-abc", Some("s-1"));
        assert_eq!(h, "jobs/cctui-worker-abc/s-1");
        assert_eq!(
            JobHandle::parse(&h),
            JobHandle { name: "cctui-worker-abc", session: Some("s-1") }
        );
        assert_eq!(format("w", None), "jobs/w");
        assert_eq!(format("w", Some("")), "jobs/w");
        assert_eq!(JobHandle::parse("jobs/w"), JobHandle { name: "w", session: None });
        assert_eq!(JobHandle::parse("w"), JobHandle { name: "w", session: None });
        assert_eq!(JobHandle::parse("jobs/w/"), JobHandle { name: "w", session: None });
    }

    #[test]
    fn a_handle_owns_only_the_run_of_its_own_session() {
        let later_run = job_for(Some("s-new"));
        assert!(JobHandle::parse("jobs/w/s-new").owns(&later_run));
        assert!(
            !JobHandle::parse("jobs/w/s-old").owns(&later_run),
            "a later run of the same dedup key is not the earlier session's workload"
        );
        assert!(JobHandle::parse("jobs/w").owns(&later_run), "a bare handle owns the name");
        assert!(JobHandle::parse("jobs/w/s-old").owns(&job_for(None)), "an unannotated Job");
    }
}
