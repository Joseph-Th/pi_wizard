#[cfg(windows)]
mod windows_process;

#[cfg(windows)]
pub(crate) use windows_process::ProcessContainment;

#[cfg(not(windows))]
pub(crate) struct ProcessContainment;

#[cfg(not(windows))]
impl ProcessContainment {
    pub(crate) fn establish() -> Result<Self, String> {
        Ok(Self)
    }
}
