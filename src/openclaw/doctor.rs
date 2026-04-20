use crate::openclaw::gateway;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DoctorRunOutcome {
    Ok,
    TimedOut { message: String, timeout_secs: u64 },
    Failed { message: String },
}

pub fn run_full_doctor() -> DoctorRunOutcome {
    match gateway::run_doctor() {
        Ok(()) => DoctorRunOutcome::Ok,
        Err(err) => {
            let message = format!("{err:#}");
            if message.contains("command timed out after ") {
                DoctorRunOutcome::TimedOut {
                    message,
                    timeout_secs: gateway::configured_doctor_timeout_secs(),
                }
            } else {
                DoctorRunOutcome::Failed { message }
            }
        }
    }
}
