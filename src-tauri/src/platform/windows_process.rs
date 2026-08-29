use win32job::Job;

/// Desktop-lifetime process containment for all inherited descendants.
pub(crate) struct ProcessContainment {
    _job: Job,
}

impl ProcessContainment {
    pub(crate) fn establish() -> Result<Self, String> {
        let job = create_kill_on_close_job()?;
        job.assign_current_process().map_err(|error| {
            format!("could not assign Pi Wizard to its Windows Job Object: {error}")
        })?;
        Ok(Self { _job: job })
    }
}

fn create_kill_on_close_job() -> Result<Job, String> {
    let job =
        Job::create().map_err(|error| format!("could not create Windows Job Object: {error}"))?;
    let mut info = job
        .query_extended_limit_info()
        .map_err(|error| format!("could not query Windows Job Object limits: {error}"))?;
    info.limit_kill_on_job_close();
    job.set_extended_limit_info(&info)
        .map_err(|error| format!("could not enable kill-on-close process containment: {error}"))?;
    Ok(job)
}

#[cfg(test)]
mod tests {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    use super::*;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    #[test]
    fn kill_on_close_job_terminates_an_assigned_child() {
        let job = create_kill_on_close_job().expect("kill-on-close job");
        let mut child = Command::new("cmd.exe")
            .args(["/d", "/s", "/c", "ping -n 30 127.0.0.1 >nul"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .expect("long-lived child");
        job.assign_process(child.as_raw_handle() as isize)
            .expect("assign child to job");
        assert!(child.try_wait().expect("child state").is_none());

        drop(job);
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if child.try_wait().expect("child state").is_some() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("kill-on-close Job Object did not terminate the assigned child");
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}
